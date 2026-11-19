"""Check which DLLs the Windows release programs need: no Visual C++ / C runtime except `vcomp140.dll`.

    python tools/release/check_imports.py [ZIP | FOLDER]

ZIP defaults to `<dist>/GrainyRS-<version>-windows-x64.zip` (tools/release/layout.py); a FOLDER must hold
`GrainyRS.exe`, `grainy-cli.exe` and `vcomp140.dll` side by side (the unzipped `GrainyRS` folder). Pure Python:
the PE import and delay-import tables are read directly (no dumpbin needed). The import lists are printed.

Fails (exit code 1) when
- `GrainyRS.exe` or `grainy-cli.exe` imports a Visual C++ / C runtime DLL other than `vcomp140.dll`
  (`vcruntime*`, `msvcp*`, `concrt*`, `vccorlib*`, `ucrtbase*`, `api-ms-win-crt-*`, `msvcr*`, `mfc*`, `atl*`),
- either program imports a DLL that is neither beside it nor a Windows system DLL,
- `vcomp140.dll` imports anything but Windows system DLLs (and nothing of the C runtime).

The Windows system folder is looked up on this machine; "API set" names (`api-ms-win-*`, `ext-ms-*`) belong to
Windows itself. The C runtime ones (`api-ms-win-crt-*`) do not count as system DLLs here: they come with the
Universal CRT, which a Windows 10 PC has but an older one may not, and a static runtime does not import them.
"""

from __future__ import annotations

import argparse
import os
import shutil
import struct
import sys
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import layout  # noqa: E402

PROGRAMS = ('GrainyRS.exe', 'grainy-cli.exe')
OPENMP = 'vcomp140.dll'
RUNTIME_PREFIXES = ('vcruntime', 'msvcp', 'concrt', 'vccorlib', 'ucrtbase', 'api-ms-win-crt-', 'msvcr', 'mfc', 'atl')


def pe_imports(path: Path) -> list[str]:
    """The DLL names in the PE import table and the delay-load import table of `path`, in file order."""
    data = path.read_bytes()
    if data[:2] != b'MZ':
        raise ValueError(f'{path.name} is not a PE file')
    pe = struct.unpack_from('<I', data, 0x3C)[0]
    if data[pe:pe + 4] != b'PE\0\0':
        raise ValueError(f'{path.name} is not a PE file')
    sections = struct.unpack_from('<H', data, pe + 6)[0]
    optional_size = struct.unpack_from('<H', data, pe + 20)[0]
    optional = pe + 24
    magic = struct.unpack_from('<H', data, optional)[0]
    directories = optional + (112 if magic == 0x20B else 96)
    table = optional + optional_size

    def offset(rva):
        for i in range(sections):
            s = table + 40 * i
            vsize, vaddr, rsize, raddr = struct.unpack_from('<IIII', data, s + 8)
            if vaddr <= rva < vaddr + max(vsize, rsize):
                return rva - vaddr + raddr
        raise ValueError(f'RVA {rva:#x} outside the sections of {path.name}')

    def cstring(rva):
        start = offset(rva)
        return data[start:data.index(b'\0', start)].decode('ascii')

    names = []
    for index, size in ((1, 20), (13, 32)):  # import table, delay-load import table
        rva, _length = struct.unpack_from('<II', data, directories + 8 * index)
        if not rva:
            continue
        pos = offset(rva)
        while True:
            entry = data[pos:pos + size]
            if len(entry) < size or not any(entry):
                break
            name_rva = struct.unpack_from('<I', entry, 12 if index == 1 else 4)[0]
            names.append(cstring(name_rva))
            pos += size
    return names


def is_runtime(name: str) -> bool:
    """A Visual C++ or C runtime DLL (the app must not need one, except `vcomp140.dll`, which is not listed)."""
    return name.lower().startswith(RUNTIME_PREFIXES)


def system_folder() -> Path | None:
    root = os.environ.get('SystemRoot')
    if sys.platform == 'win32' and root and (Path(root) / 'System32').is_dir():
        return Path(root) / 'System32'
    return None


def classify(name: str, folder: Path, system: Path | None) -> str:
    """'runtime' (VC/C runtime), 'beside' (in the folder), 'system' (Windows) or 'missing'."""
    low = name.lower()
    if low != OPENMP and is_runtime(name):
        return 'runtime'
    if (folder / name).exists():
        return 'beside'
    if low.startswith(('api-ms-win-', 'ext-ms-')):
        return 'system'
    if system is not None:
        return 'system' if (system / name).exists() else 'missing'
    return 'system'  # cannot look on a non-Windows machine: only the runtime rule is checked there


def check(folder: Path) -> list[str]:
    """Prints the import lists of the three files in `folder`; returns the problems found."""
    problems = []
    system = system_folder()
    if system is None:
        print('  note: not on Windows: DLLs are not looked up in the system folder, only the runtime rule applies')
    for name in (*PROGRAMS, OPENMP):
        path = folder / name
        if not path.is_file():
            problems.append(f'{name} is missing from {folder}')
            continue
        imports = pe_imports(path)
        print(f'{name}: {len(imports)} imported DLLs')
        for dll in sorted({d.lower(): d for d in imports}.values(), key=str.lower):
            kind = classify(dll, folder, system)
            print(f'    {dll:<40} {kind}')
            if name == OPENMP:
                if kind != 'system':
                    problems.append(f'{name} imports {dll} ({kind}); only Windows system DLLs are allowed')
            elif kind == 'runtime':
                problems.append(f'{name} imports the C/C++ runtime DLL {dll}')
            elif kind == 'missing':
                problems.append(f'{name} imports {dll}, which is neither beside it nor a Windows system DLL')
    return problems


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('source', nargs='?', default=str(layout.dist_folder() / layout.zip_name()))
    args = parser.parse_args(argv)
    source = Path(args.source)
    tmp = None
    try:
        if source.is_dir():
            folder = source
        elif source.is_file() and source.suffix.lower() == '.zip':
            tmp = Path(tempfile.mkdtemp(prefix='grainy-imports-'))
            with zipfile.ZipFile(source) as archive:
                for member in archive.namelist():
                    if member.startswith('/') or '..' in Path(member).parts or ':' in member:
                        raise SystemExit(f'unsafe entry {member!r}')
                archive.extractall(tmp)
            folder = tmp / 'GrainyRS'
        else:
            print(f'no such zip or folder: {source}')
            return 2
        print(f'import check of {source.name}')
        problems = check(folder)
    finally:
        if tmp is not None:
            shutil.rmtree(tmp, ignore_errors=True)
    if problems:
        print('\nFAIL')
        for p in problems:
            print(f'  {p}')
        return 1
    print('\nPASS: no C/C++ runtime DLL is imported except vcomp140.dll (which imports only system DLLs)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
