"""Build the Windows release zip of Grainy RS.

    python tools/release/make_release.py [--skip-build] [--dist FOLDER]

1. `cargo build --release -p grainy-app -p grainy-cli` (unless --skip-build).
2. THIRD_PARTY_NOTICES.txt (tools/release/notices.py), README.txt (tools/release/README.txt), LICENSE.
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
    env = dict(os.environ)
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


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--skip-build', action='store_true', help='use the existing target/release executables')
    parser.add_argument('--dist', default=str(layout.dist_folder()))
    args = parser.parse_args(argv)
    os.environ['PATH'] = cargo_env()['PATH']  # notices.py runs cargo too

    version = layout.version()
    items = layout.rs_items()
    expected = ['GrainyRS.exe', 'grainy-cli.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.txt']
    if sorted(items) != sorted(expected):
        raise SystemExit(f'updater::RS_ITEMS is {items}; this script makes {expected}: update the script')

    if not args.skip_build:
        print('cargo build --release -p grainy-app -p grainy-cli', flush=True)
        subprocess.run(['cargo', 'build', '--release', '-p', 'grainy-app', '-p', 'grainy-cli'], cwd=REPO,
                       env=cargo_env(), check=True)
    release = REPO / 'target' / 'release'
    for exe in ('grainy-app.exe', 'grainy-cli.exe'):
        if not (release / exe).is_file():
            raise SystemExit(f'{release / exe} is missing: build first (without --skip-build)')

    with tempfile.TemporaryDirectory(prefix='grainy-release-') as tmp:
        stage = Path(tmp)
        text, rows, flags = notices.build('x86_64-pc-windows-msvc')
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
