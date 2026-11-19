"""Smoke test of the release zip, from a fresh temporary folder outside the repository.

    python tools/release/smoke.py [ZIP] [--keep] [--timeout SECONDS]

ZIP defaults to `<dist>/GrainyRS-<version>-windows-x64.zip` (tools/release/layout.py). Steps:

1. Notes whether `%LOCALAPPDATA%\\GrainyRS` exists and its modification time (the folder itself only; nothing
   inside it is opened).
2. Unzips into a new temporary folder (system temp, outside the repository); checks the entries are exactly
   `GrainyRS/<RS_ITEMS>`.
3. Lists the DLLs both programs import (PE import and delay-import tables) and checks each is found beside the
   program or in the Windows system folder: a fresh folder must start.
4. Writes a generated 8-bit PNG into the temporary folder.
5. Starts `GrainyRS.exe --offline --smoke <out> <image>` with `GRAINY_OFFLINE=1` and `GRAINY_RS_HOME=<temp data
   folder>`: the window opens on a new library in the temporary data folder, imports the image, shows it in
   Develop, exports it with the export dialog's defaults, writes `<out>/smoke.json` (lens database source and
   size, network switch, frame size, files written) and closes. Checks the exit code, the report (built-in lens
   database found, offline, the photo shown, one JPEG written, no failures) and the exported file.
6. Runs `grainy-cli.exe info` and `render` from the zip on the same image.
7. Checks `%LOCALAPPDATA%\\GrainyRS` did not appear or change, and that the app's data went to the temporary
   data folder.
8. Removes the temporary folder (unless --keep).

Exit code 0 when every check passes.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import zipfile
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import layout  # noqa: E402

REPO = layout.REPO


# ---- checks ------------------------------------------------------------------------------------------------------

class Checks:
    def __init__(self):
        self.failed = []

    def check(self, ok: bool, what: str, detail: str = ''):
        print(f'  [{"ok" if ok else "FAIL"}] {what}{(": " + detail) if detail else ""}', flush=True)
        if not ok:
            self.failed.append(what)
        return ok


def folder_state(path: Path):
    """(exists, modification time) of the folder itself; nothing inside is opened."""
    try:
        st = path.stat()
        return True, st.st_mtime_ns
    except FileNotFoundError:
        return False, None


# ---- PE imports --------------------------------------------------------------------------------------------------

def pe_imports(path: Path) -> list[str]:
    data = path.read_bytes()
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
        raise ValueError(f'RVA {rva:#x} outside the sections')

    def cstring(rva):
        start = offset(rva)
        return data[start:data.index(b'\0', start)].decode('ascii')

    names = []
    for index, size in ((1, 20), (13, 32)):  # import table, delay-load import table
        rva, length = struct.unpack_from('<II', data, directories + 8 * index)
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


def dll_found(name: str, folder: Path) -> str | None:
    low = name.lower()
    if low.startswith(('api-ms-win-', 'ext-ms-')):
        return 'API set'
    if (folder / name).exists():
        return 'beside the program'
    system = Path(os.environ.get('SystemRoot', r'C:\Windows')) / 'System32'
    if (system / name).exists():
        return 'Windows system folder'
    return None


# ---- a generated image -------------------------------------------------------------------------------------------

def write_png(path: Path, width: int = 640, height: int = 427):
    """An 8-bit RGB gradient with a few patches (pure Python, no third-party module)."""
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            r = x * 255 // (width - 1)
            g = y * 255 // (height - 1)
            b = 255 - (x + y) * 255 // (width + height - 2)
            if (x // 80 + y // 80) % 5 == 0:
                r, g, b = 200, 120, 60
            rows += bytes((r, g, b))

    def chunk(kind, body):
        return struct.pack('>I', len(body)) + kind + body + struct.pack('>I', zlib.crc32(kind + body))

    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
    png += chunk(b'IDAT', zlib.compress(bytes(rows), 9)) + chunk(b'IEND', b'')
    path.write_bytes(png)


def jpeg_size(path: Path):
    """(width, height) of a baseline/progressive JPEG, or None."""
    data = path.read_bytes()
    if data[:2] != b'\xff\xd8':
        return None
    pos = 2
    while pos + 4 <= len(data):
        if data[pos] != 0xFF:
            pos += 1
            continue
        marker = data[pos + 1]
        length = struct.unpack_from('>H', data, pos + 2)[0]
        if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
            h, w = struct.unpack_from('>HH', data, pos + 5)
            return w, h
        pos += 2 + length
    return None


# ---- the run -----------------------------------------------------------------------------------------------------

def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('zip', nargs='?', default=str(layout.dist_folder() / layout.zip_name()))
    parser.add_argument('--keep', action='store_true', help='keep the temporary folder (prints where)')
    parser.add_argument('--timeout', type=float, default=300.0)
    args = parser.parse_args(argv)
    archive_path = Path(args.zip)
    c = Checks()
    print(f'smoke test of {archive_path.name}')
    if not archive_path.is_file():
        print(f'no such zip: {archive_path}')
        return 2

    local = os.environ.get('LOCALAPPDATA')
    real_data = Path(local) / 'GrainyRS' if local else None
    before = folder_state(real_data) if real_data else (False, None)
    print(f'  the real data folder %LOCALAPPDATA%\\GrainyRS {"exists" if before[0] else "does not exist"} before')

    tmp = Path(tempfile.mkdtemp(prefix='grainy-smoke-'))
    try:
        c.check(REPO not in tmp.parents and tmp != REPO, 'temporary folder outside the repository')
        # 2. unzip
        items = layout.rs_items()
        with zipfile.ZipFile(archive_path) as archive:
            names = archive.namelist()
            c.check(sorted(names) == sorted(f'GrainyRS/{n}' for n in items), 'zip entries are GrainyRS/<RS_ITEMS>',
                    ', '.join(names))
            for n in names:
                if n.startswith('/') or '..' in Path(n).parts or ':' in n:
                    raise SystemExit(f'unsafe entry {n!r}')
            archive.extractall(tmp / 'unzipped')
        app_folder = tmp / 'unzipped' / 'GrainyRS'
        app, cli = app_folder / 'GrainyRS.exe', app_folder / 'grainy-cli.exe'

        # 3. DLLs
        for exe in (app, cli):
            dlls = pe_imports(exe)
            missing = [d for d in dlls if dll_found(d, app_folder) is None]
            outside = sorted({d for d in dlls if dll_found(d, app_folder) == 'Windows system folder'
                              and d.lower().startswith(('vcruntime', 'msvcp', 'vcomp', 'ucrtbase'))})
            c.check(not missing, f'{exe.name}: every imported DLL is found ({len(dlls)} DLLs)',
                    'missing ' + ', '.join(missing) if missing else '')
            if outside:
                print(f'       {exe.name} needs the Visual C++ runtime: {", ".join(outside)} '
                      '(found in the system folder here; a PC without the VC++ Redistributable lacks them)')

        # 4. image
        source = tmp / 'input' / 'smoke.png'
        source.parent.mkdir()
        write_png(source)
        source_bytes = source.read_bytes()

        # 5. the app
        home, out = tmp / 'data', tmp / 'out'
        env = {k: v for k, v in os.environ.items() if k not in ('GRAINY_RS_LENSFUN',)}
        env.update(GRAINY_RS_HOME=str(home), GRAINY_OFFLINE='1')
        started = time.monotonic()
        try:
            run = subprocess.run([str(app), '--offline', '--smoke', str(out), str(source)], cwd=tmp, env=env,
                                 capture_output=True, text=True, encoding='utf-8', errors='replace',
                                 timeout=args.timeout)
            code, log = run.returncode, (run.stderr or '') + (run.stdout or '')
        except subprocess.TimeoutExpired as e:
            code, log = None, f'timeout after {args.timeout:.0f} s: {e}'
        seconds = time.monotonic() - started
        c.check(code == 0, f'GrainyRS.exe started from the fresh folder and closed cleanly ({seconds:.1f} s)',
                f'exit code {code}')
        if code != 0:
            print('       ' + '\n       '.join(log.strip().splitlines()[-20:]))
        report_path = out / 'smoke.json'
        report = json.loads(report_path.read_text(encoding='utf-8')) if report_path.is_file() else {}
        c.check(bool(report), 'the app wrote its smoke report')
        if report:
            lens = report.get('lens_database', {})
            count = lens.get('lenses')
            c.check(lens.get('source') == 'builtin' and isinstance(count, int) and count > 0,
                    'the built-in lens database is found',
                    f"{lens.get('files')} XML files, {count} lenses, {lens.get('cameras')} cameras"
                    if lens.get('source') == 'builtin' and 'error' not in lens else str(lens))
            c.check(report.get('offline') is True, 'the network switch is off in the app')
            c.check('error' not in report, 'the photo was opened and shown', report.get('error') or
                    f"frame {report.get('shown')}, {report.get('photos')} photo(s) in the library")
            written = [Path(p) for p in report.get('written', [])]
            c.check(len(written) == 1 and not report.get('failures'), 'the app exported the photo',
                    ', '.join(p.name for p in written) + ('; failures: ' + '; '.join(report['failures'])
                                                          if report.get('failures') else ''))
            for p in written:
                size = jpeg_size(p) if p.is_file() else None
                c.check(size == (640, 427), 'the exported file is a 640x427 JPEG', str(size))
        c.check((home / 'Library').is_dir(), 'the app kept its library in the temporary data folder')
        c.check(source.read_bytes() == source_bytes, 'the original image is unchanged')

        # 6. the command-line program
        info = subprocess.run([str(cli), 'info', str(source)], cwd=tmp, env=env, capture_output=True, text=True,
                              encoding='utf-8', errors='replace', timeout=120)
        c.check(info.returncode == 0 and '640' in info.stdout, 'grainy-cli.exe info', f'exit code {info.returncode}')
        rendered = out / 'cli.jpg'
        render = subprocess.run([str(cli), 'render', str(source), '-o', str(rendered), '--longest', '320'], cwd=tmp,
                                env=env, capture_output=True, text=True, encoding='utf-8', errors='replace',
                                timeout=120)
        size = jpeg_size(rendered) if rendered.is_file() else None
        c.check(render.returncode == 0 and size == (320, 214), 'grainy-cli.exe render', f'{size}')
        c.check(source.read_bytes() == source_bytes, 'the original image is still unchanged')
    finally:
        # 7. the real data folder
        if real_data:
            after = folder_state(real_data)
            c.check(after == before, '%LOCALAPPDATA%\\GrainyRS did not appear or change',
                    'exists' if after[0] else 'absent')
        # 8. clean up
        if args.keep:
            print(f'  kept {tmp}')
        else:
            shutil.rmtree(tmp, ignore_errors=True)
            c.check(not tmp.exists(), 'temporary folder removed')
    print(f'\nRESULT: {"PASS" if not c.failed else "FAIL (" + ", ".join(c.failed) + ")"}')
    return 0 if not c.failed else 1


if __name__ == '__main__':
    sys.exit(main())
