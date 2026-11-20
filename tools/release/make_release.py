"""Build the Windows release zip of Grainy RS.

    python tools/release/make_release.py [--skip-build] [--target-dir FOLDER] [--vcomp PATH] [--dist FOLDER]

1. `cargo build --release -p grainy-app -p grainy-cli` (unless --skip-build) into `<target-dir>` (default
   `target` in the repository).
2. THIRD_PARTY_NOTICES.txt (tools/release/notices.py), README.txt (tools/release/README.txt), LICENSE, and, when
   `updater::RS_ITEMS` names it, `vcomp140.dll`: Microsoft's OpenMP runtime, the only Visual C++ runtime file the
   programs need (the C runtime is linked statically, see `.cargo/config.toml`). It is taken from `--vcomp PATH`,
   else from the newest Visual Studio redistributable folder (`VCToolsRedistDir`, `vswhere`, the standard
   folders: `VC/Redist/MSVC/<version>/x64/Microsoft.VC143.OpenMP/vcomp140.dll`); the path and version are printed.
3. `<dist>/GrainyRS-<version>-windows-x64.zip` with exactly one top folder `GrainyRS/` holding exactly the items
   `updater::RS_ITEMS` names (`GrainyRS.exe` is the release build's `grainy-app.exe`, renamed), and
   `<zip>.sha256` beside it (`<hex> *<zip name>`, for people; the updater checks GitHub's own digest).
4. Prints the zip's size and SHA-256.

`<dist>` defaults to `grainy-rs-dist` beside the repository (tools/release/layout.py). The zip is rebuilt from
scratch on every run; entries carry the commit time of HEAD so the same build gives the same zip.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import layout  # noqa: E402
import notices  # noqa: E402

REPO = layout.REPO
FOLDER = 'GrainyRS'


def cargo_env():
    env = layout.release_env(windows=True)
    cargo_bin = Path.home() / '.cargo' / 'bin'
    env['PATH'] = str(cargo_bin) + os.pathsep + env.get('PATH', '')
    return env


def commit_time():
    try:
        out = subprocess.run(['git', 'log', '-1', '--format=%ct'], cwd=REPO, capture_output=True, text=True,
                             check=True).stdout.strip()
        t = time.gmtime(int(out))
    except (subprocess.CalledProcessError, ValueError, OSError):
        t = time.gmtime()
    return (max(t.tm_year, 1980), t.tm_mon, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec - t.tm_sec % 2)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def redist_version(path: Path) -> str:
    """The redistributable's version: `.../MSVC/<version>/x64/<package>/vcomp140.dll`."""
    return path.parents[2].name


def version_key(text: str):
    return tuple(int(n) if n.isdigit() else 0 for n in text.replace('-', '.').split('.'))


def redist_candidates() -> dict:
    """Every x64 `vcomp140.dll` (not onecore) in the Visual Studio redistributable folders of this PC."""
    roots = []
    redist = os.environ.get('VCToolsRedistDir')
    if redist:
        roots.append(Path(redist))
    for base in (os.environ.get('ProgramFiles(x86)'), os.environ.get('ProgramFiles')):
        vswhere = Path(base) / 'Microsoft Visual Studio' / 'Installer' / 'vswhere.exe' if base else None
        if vswhere and vswhere.is_file():
            try:
                out = subprocess.run([str(vswhere), '-products', '*', '-property', 'installationPath'],
                                     capture_output=True, text=True, check=True).stdout
            except (subprocess.CalledProcessError, OSError):
                continue
            for line in out.splitlines():
                if line.strip():
                    roots.extend((Path(line.strip()) / 'VC' / 'Redist' / 'MSVC').glob('*'))
            break
    for base in (os.environ.get('ProgramFiles'), os.environ.get('ProgramFiles(x86)')):
        if base:
            roots.extend((Path(base) / 'Microsoft Visual Studio').glob('*/*/VC/Redist/MSVC/*'))
    found = {}
    for root in roots:
        for path in root.glob('x64/Microsoft.VC14?.OpenMP/vcomp140.dll'):
            found[path.resolve()] = redist_version(path.resolve())
    return found


def find_vcomp(explicit):
    """(path, version) of the `vcomp140.dll` to ship: `--vcomp`, else the newest redistributable."""
    if explicit:
        path = Path(explicit).resolve()
        if not path.is_file():
            raise SystemExit(f'--vcomp {path}: no such file')
        return path, (redist_version(path) if len(path.parents) > 2 else 'unknown')
    found = redist_candidates()
    if not found:
        raise SystemExit('vcomp140.dll not found in a Visual Studio redistributable folder '
                         '(VC/Redist/MSVC/<version>/x64/Microsoft.VC143.OpenMP): install the Visual C++ build '
                         'tools or pass --vcomp PATH')
    path = max(found, key=lambda p: (version_key(found[p]), str(p)))
    return path, found[path]


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--skip-build', action='store_true', help='use the existing release executables')
    parser.add_argument('--dist', default=str(layout.dist_folder()))
    parser.add_argument('--target-dir', default=str(REPO / 'target'),
                        help='the cargo target folder (default: target in the repository)')
    parser.add_argument('--vcomp', help='the vcomp140.dll to ship (default: newest Visual Studio redistributable)')
    args = parser.parse_args(argv)
    os.environ['PATH'] = cargo_env()['PATH']  # notices.py runs cargo too

    version = layout.version()
    items = layout.rs_items()
    known = ['GrainyRS.exe', 'grainy-cli.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.txt', 'vcomp140.dll']
    if not set(known[:5]) <= set(items) or not set(items) <= set(known):
        raise SystemExit(f'updater::RS_ITEMS is {items}; this script makes {known} (vcomp140.dll optional): '
                         'update the script')
    vcomp = find_vcomp(args.vcomp) if 'vcomp140.dll' in items else None
    if vcomp:
        print(f'vcomp140.dll: {vcomp[0]}\n  Visual Studio redistributable version {vcomp[1]}', flush=True)

    if not args.skip_build:
        print('cargo build --release -p grainy-app -p grainy-cli', flush=True)
        subprocess.run(['cargo', 'build', '--release', '--target-dir', args.target_dir, '-p', 'grainy-app', '-p',
                        'grainy-cli'], cwd=REPO, env=cargo_env(), check=True)
    release = Path(args.target_dir) / 'release'
    for exe in ('grainy-app.exe', 'grainy-cli.exe'):
        if not (release / exe).is_file():
            raise SystemExit(f'{release / exe} is missing: build first (without --skip-build)')

    for exe in ('grainy-app.exe', 'grainy-cli.exe'):
        layout.check_binary_paths(release / exe, [REPO, Path.home()])

    with tempfile.TemporaryDirectory(prefix='grainy-release-') as tmp:
        stage = Path(tmp)
        text, rows, flags = notices.build('x86_64-pc-windows-msvc', vcomp_version=vcomp[1] if vcomp else None)
        notices.require_reviewed(flags)
        (stage / 'THIRD_PARTY_NOTICES.txt').write_bytes(text.replace('\n', '\r\n').encode('utf-8'))
        readme = (Path(__file__).parent / 'README.txt').read_text(encoding='utf-8').replace('{version}', version)
        (stage / 'README.txt').write_bytes(readme.replace('\r\n', '\n').replace('\n', '\r\n').encode('utf-8'))
        sources = {
            'GrainyRS.exe': release / 'grainy-app.exe',
            'grainy-cli.exe': release / 'grainy-cli.exe',
            'LICENSE': REPO / 'LICENSE',
            'THIRD_PARTY_NOTICES.txt': stage / 'THIRD_PARTY_NOTICES.txt',
            'README.txt': stage / 'README.txt',
        }
        if vcomp:
            sources['vcomp140.dll'] = vcomp[0]
        dist = Path(args.dist)
        dist.mkdir(parents=True, exist_ok=True)
        target = dist / layout.zip_name(version)
        partial = target.with_name(target.name + '.partial')
        stamp = commit_time()
        with zipfile.ZipFile(partial, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name in items:
                info = zipfile.ZipInfo(f'{FOLDER}/{name}', date_time=stamp)
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o644 << 16
                archive.writestr(info, sources[name].read_bytes(), compresslevel=9)
        os.replace(partial, target)

    with zipfile.ZipFile(target) as archive:
        names = archive.namelist()
        bad = archive.testzip()
    if bad or sorted(names) != sorted(f'{FOLDER}/{n}' for n in items):
        raise SystemExit(f'the zip does not hold exactly {FOLDER}/{items}: {names} (bad member: {bad})')
    digest = sha256(target)
    (target.parent / (target.name + '.sha256')).write_text(f'{digest} *{target.name}\n', encoding='ascii',
                                                             newline='\n')
    size = target.stat().st_size
    print(f'\n{target}')
    print(f'  entries: {", ".join(names)}')
    print(f'  size:    {size} bytes ({size / 1e6:.1f} MB)')
    print(f'  SHA-256: {digest}')
    print(f'  Rust crates in the notices: {len(rows)}; licence flags: {len(flags)}')
    for label, why in flags:
        print(f'    FLAG {label}: {why}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
