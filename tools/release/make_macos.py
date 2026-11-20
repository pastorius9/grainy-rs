"""Build the macOS app bundle and disk image of Grainy RS (runs on macOS only; Python standard library only).

    python tools/release/make_macos.py [--skip-build] [--target-dir FOLDER] [--dist FOLDER] [--dry-run]

Inputs are the two release builds, `<target-dir>/aarch64-apple-darwin/release/` and
`<target-dir>/x86_64-apple-darwin/release/` (`grainy-app` and `grainy-cli` in each; built here with
`MACOSX_DEPLOYMENT_TARGET=11.0` unless --skip-build). `<target-dir>` defaults to `target` in the repository.

1. `lipo -create` makes universal `grainy-app` and `grainy-cli`; `lipo -archs` must name both architectures;
   `otool -L` of every slice may name only `/usr/lib/` and `/System/` libraries; `otool -l` must show a minimum
   system of 11.0 or older.
2. `Grainy RS.app`: `Contents/Info.plist`, `Contents/MacOS/GrainyRS` (the universal grainy-app),
   `Contents/MacOS/grainy-cli`, `Contents/Resources/GrainyRS.icns`, `LICENSE`, `THIRD_PARTY_NOTICES.txt`
   (tools/release/notices.py for both targets) and `README.txt` (tools/release/README-macos.txt).
   The icon `tools/release/macos/GrainyRS.icns` is a copy of the icon of the Python Grainy (same owner, MIT).
3. Signing as the Python Grainy's `tools/build_macos.py` does: identity `GRAINY_SIGN_IDENTITY` (with the keychain
   `GRAINY_SIGN_KEYCHAIN`), ad hoc (`-`) when it is not set; `codesign --force --deep --sign <identity>
   [--keychain <keychain>]`, no hardened runtime and no entitlements; `codesign --verify --deep --strict`; the
   designated requirement is printed. One certificate in every release keeps the permissions macOS gave the app.
4. A smoke run of the bundled program (the checks of tools/release/smoke.py: `--offline --smoke`, a temporary
   data folder, `grainy-cli info` and `render`), and a check that no file in the bundle names the checkout
   folder (on a CI runner a binary naming it only warns: the runner's folder is no secret).
5. `<dist>/GrainyRS-<version>-macos-universal.dmg` (volume `Grainy RS`: the app and a link `Applications`) and
   `<dmg>.sha256` beside it; prints the size and SHA-256.

--dry-run runs on any system: prints the Info.plist and the bundle's file list and checks the inputs that live in
the repository (no build, no macOS tools).
"""

from __future__ import annotations

import argparse
import hashlib
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import layout  # noqa: E402

REPO = layout.REPO
HERE = Path(__file__).resolve().parent
APP_NAME = 'Grainy RS.app'
VOLUME = 'Grainy RS'
IDENTIFIER = 'io.github.pastorius9.grainyrs'  # never the Python Grainy's io.github.pastorius9.grainy
TARGETS = ('aarch64-apple-darwin', 'x86_64-apple-darwin')
ARCHS = {'arm64', 'x86_64'}
MINIMUM_SYSTEM = (11, 0)
ICON = HERE / 'macos' / 'GrainyRS.icns'
README = HERE / 'README-macos.txt'
# bundle path -> source (relative to the stage folder or the repository, resolved in `bundle_files`)
RESOURCES = ('GrainyRS.icns', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.txt')


def run(*command, **options):
    print('+', ' '.join(str(c) for c in command), flush=True)
    return subprocess.run([str(c) for c in command], check=True, **options)


def output(*command) -> str:
    result = subprocess.run([str(c) for c in command], capture_output=True, text=True, encoding='utf-8',
                            errors='replace')
    if result.returncode != 0:
        raise SystemExit(f'{" ".join(str(c) for c in command)} failed ({result.returncode}): {result.stderr.strip()}')
    return result.stdout


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


# ---- the bundle's content (runs anywhere) ------------------------------------------------------------------------

def info_plist(version: str) -> dict:
    return {
        'CFBundleIdentifier': IDENTIFIER,
        'CFBundleName': 'Grainy RS',
        'CFBundleDisplayName': 'Grainy RS',
        'CFBundleExecutable': 'GrainyRS',
        'CFBundleIconFile': 'GrainyRS',
        'CFBundlePackageType': 'APPL',
        'CFBundleInfoDictionaryVersion': '6.0',
        'CFBundleShortVersionString': version,
        'CFBundleVersion': version,
        'LSMinimumSystemVersion': '.'.join(str(n) for n in MINIMUM_SYSTEM),
        'NSHighResolutionCapable': True,
        'LSApplicationCategoryType': 'public.app-category.photography',
        'NSHumanReadableCopyright': 'MIT License. Third-party notices: Contents/Resources/THIRD_PARTY_NOTICES.txt',
    }


def bundle_files() -> list[str]:
    """The files of the bundle, relative to `Grainy RS.app` (Info.plist and the two programs included)."""
    return ['Contents/Info.plist', 'Contents/MacOS/GrainyRS', 'Contents/MacOS/grainy-cli',
            *(f'Contents/Resources/{name}' for name in RESOURCES)]


def readme_text(version: str) -> str:
    return README.read_text(encoding='utf-8').replace('{version}', version).replace('\r\n', '\n')


def check_repository_inputs(version: str) -> list[str]:
    problems = []
    for path in (ICON, README, REPO / 'LICENSE'):
        if not path.is_file():
            problems.append(f'{path} is missing')
    if ICON.is_file() and ICON.read_bytes()[:4] != b'icns':
        problems.append(f'{ICON} is not an icns file')
    if README.is_file() and '{version}' not in README.read_text(encoding='utf-8'):
        problems.append(f'{README.name} has no {{version}} placeholder')
    plist = plistlib.loads(plistlib.dumps(info_plist(version)))
    if not plist['CFBundleIdentifier'].endswith('.grainyrs'):
        problems.append('CFBundleIdentifier must end in .grainyrs')
    return problems


# ---- the build (macOS only) --------------------------------------------------------------------------------------

def slices_of(path: Path) -> set[str]:
    return set(output('lipo', '-archs', path).split())


def check_libraries(path: Path):
    """`otool -L`: only /usr/lib and /System libraries. Header lines name the file itself (`<path>:`, or
    `<path> (architecture arm64):` once per slice of a universal file) and are skipped; libraries are indented."""
    lines = output('otool', '-L', path).splitlines()
    names = sorted({line.strip().split(' (')[0] for line in lines if line.strip() and line[:1].isspace()})
    bad = [n for n in names if not n.startswith(('/usr/lib/', '/System/'))]
    if bad:
        raise SystemExit(f'{path} loads libraries outside /usr/lib and /System: {bad}')
    return names


def minimum_system(path: Path):
    """The newest `minos` / `version` of the macOS load commands of a Mach-O file (every slice), or None."""
    found, command, platform = [], None, None
    for line in output('otool', '-l', path).splitlines():
        words = line.split()
        if len(words) == 2 and words[0] == 'cmd':
            command, platform = words[1], None
        elif len(words) == 2 and words[0] == 'platform':
            platform = words[1]
        elif command == 'LC_BUILD_VERSION' and platform in ('1', 'MACOS') and words[:1] == ['minos']:
            found.append(words[1])
        elif command == 'LC_VERSION_MIN_MACOSX' and words[:1] == ['version']:
            found.append(words[1])
    versions = [tuple(int(n) for n in v.split('.')[:2]) for v in found]
    return max(versions) if versions else None


def build_universal(target_dir: Path, work: Path, skip_build: bool) -> dict[str, Path]:
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET='.'.join(str(n) for n in MINIMUM_SYSTEM))
    env['PATH'] = str(Path.home() / '.cargo' / 'bin') + os.pathsep + env.get('PATH', '')
    for target in TARGETS:
        if not skip_build:
            run('cargo', 'build', '--release', '--target', target, '--target-dir', target_dir, '-p', 'grainy-app',
                '-p', 'grainy-cli', cwd=REPO, env=env)
    universal = {}
    for program in ('grainy-app', 'grainy-cli'):
        inputs = [target_dir / target / 'release' / program for target in TARGETS]
        for path in inputs:
            if not path.is_file():
                raise SystemExit(f'{path} is missing: build it first (without --skip-build)')
            libraries = check_libraries(path)
            minos = minimum_system(path)
            print(f'  {path.parent.parent.name}/{program}: {len(libraries)} system libraries; minimum macOS '
                  f'{".".join(str(n) for n in minos) if minos else "(none declared)"}')
            if minos is None or minos > MINIMUM_SYSTEM:
                raise SystemExit(f'{path} needs macOS {minos}, newer than {MINIMUM_SYSTEM}: build with '
                                 'MACOSX_DEPLOYMENT_TARGET=11.0')
        target = work / program
        run('lipo', '-create', *inputs, '-output', target)
        archs = slices_of(target)
        print(f'  {program}: universal, slices {sorted(archs)}')
        if archs != ARCHS:
            raise SystemExit(f'{target} holds {sorted(archs)}, expected {sorted(ARCHS)}')
        check_libraries(target)
        universal[program] = target
    return universal


def make_bundle(root: Path, version: str, programs: dict[str, Path]) -> Path:
    import notices  # noqa: E402  (runs cargo)

    app = root / APP_NAME
    macos, resources = app / 'Contents' / 'MacOS', app / 'Contents' / 'Resources'
    macos.mkdir(parents=True)
    resources.mkdir(parents=True)
    shutil.copy2(programs['grainy-app'], macos / 'GrainyRS')
    shutil.copy2(programs['grainy-cli'], macos / 'grainy-cli')
    for path in (macos / 'GrainyRS', macos / 'grainy-cli'):
        path.chmod(0o755)
    shutil.copy2(ICON, resources / 'GrainyRS.icns')
    shutil.copy2(REPO / 'LICENSE', resources / 'LICENSE')
    text, rows, flags = notices.build(list(TARGETS))
    (resources / 'THIRD_PARTY_NOTICES.txt').write_bytes(text.encode('utf-8'))
    (resources / 'README.txt').write_bytes(readme_text(version).encode('utf-8'))
    with (app / 'Contents' / 'Info.plist').open('wb') as handle:
        plistlib.dump(info_plist(version), handle, fmt=plistlib.FMT_XML)
    print(f'  notices: {len(rows)} Rust crates, {len(flags)} licence flags')
    for label, why in flags:
        print(f'    FLAG {label}: {why}')
    found = sorted(p.relative_to(app).as_posix() for p in app.rglob('*') if p.is_file())
    if found != sorted(bundle_files()):
        raise SystemExit(f'the bundle holds {found}, expected {sorted(bundle_files())}')
    return app


def sign(app: Path):
    """As the Python Grainy's build_macos.py: one self-signed identity in releases, ad hoc otherwise."""
    identity = os.environ.get('GRAINY_SIGN_IDENTITY') or '-'
    if identity == '-':
        print('NOTICE: GRAINY_SIGN_IDENTITY is not set: the app is signed ad hoc. macOS then forgets the folder '
              'and disk permissions it gave the app with every new build; releases use one certificate.')
    keychain = os.environ.get('GRAINY_SIGN_KEYCHAIN')
    keychain_args = ['--keychain', keychain] if identity != '-' and keychain else []
    run('codesign', '--force', '--deep', '--sign', identity, *keychain_args, app)
    run('codesign', '--verify', '--deep', '--strict', app)
    run('codesign', '--verify', '--strict', app / 'Contents' / 'MacOS' / 'grainy-cli')
    result = subprocess.run(['codesign', '-d', '-r-', str(app)], capture_output=True, text=True)
    lines = (result.stdout + result.stderr).splitlines()
    print('Designated requirement:', next((line for line in lines if 'designated' in line), '(none)'))


def no_checkout_paths(app: Path):
    """No file in the bundle may name the folder this was built in (a CI runner's folder is no secret)."""
    private = [str(REPO).encode()]
    on_ci = bool(os.environ.get('CI'))
    if not on_ci:
        private.append(str(Path.home()).encode())
    leaked = [p.relative_to(app).as_posix() for p in app.rglob('*')
              if p.is_file() and not p.is_symlink() and any(text in p.read_bytes() for text in private)]
    if not leaked:
        print('  no file in the bundle names the checkout folder')
        return
    binaries = {'Contents/MacOS/GrainyRS', 'Contents/MacOS/grainy-cli'}
    if on_ci and set(leaked) <= binaries:
        print(f'  WARNING: {leaked} name the checkout folder (a CI runner\'s folder: not secret; '
              'a build on a person\'s Mac must not do this)')
        return
    raise SystemExit(f'files in the app name the folder it was built in: {leaked[:10]}')


def smoke_run(app: Path):
    import smoke  # noqa: E402

    real = Path.home() / 'Library' / 'Application Support' / 'GrainyRS'
    before = smoke.folder_state(real)
    c = smoke.Checks()
    tmp = Path(tempfile.mkdtemp(prefix='grainy-smoke-'))
    try:
        smoke.exercise(c, app / 'Contents' / 'MacOS' / 'GrainyRS', app / 'Contents' / 'MacOS' / 'grainy-cli', tmp,
                       300.0)
    finally:
        after = smoke.folder_state(real)
        c.check(after == before, '~/Library/Application Support/GrainyRS did not appear or change')
        shutil.rmtree(tmp, ignore_errors=True)
    if c.failed:
        raise SystemExit(f'smoke run failed: {", ".join(c.failed)}')


def make_dmg(app: Path, dist: Path, version: str) -> Path:
    dist.mkdir(parents=True, exist_ok=True)
    image = dist / layout.dmg_name(version)
    image.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='grainy-dmg-') as folder:
        stage = Path(folder) / VOLUME
        stage.mkdir()
        run('ditto', app, stage / APP_NAME)
        os.symlink('/Applications', stage / 'Applications')
        run('hdiutil', 'create', '-volname', VOLUME, '-srcfolder', stage, '-ov', '-format', 'UDZO', image)
    digest = sha256(image)
    (dist / (image.name + '.sha256')).write_text(f'{digest} *{image.name}\n', encoding='ascii', newline='\n')
    return image


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--skip-build', action='store_true', help='use the existing release builds')
    parser.add_argument('--target-dir', default=str(REPO / 'target'),
                        help='the cargo target folder (default: target in the repository)')
    parser.add_argument('--dist', default=str(layout.dist_folder()))
    parser.add_argument('--dry-run', action='store_true',
                        help='print the Info.plist and the file list and check the repository inputs; any system')
    args = parser.parse_args(argv)
    version = layout.version()

    problems = check_repository_inputs(version)
    if args.dry_run:
        print(plistlib.dumps(info_plist(version)).decode('utf-8'))
        print(f'{APP_NAME}:')
        for name in bundle_files():
            print(f'  {name}')
        print(f'volume "{VOLUME}": {APP_NAME}, Applications -> /Applications')
        print(f'disk image: {layout.dmg_name(version)} (+ .sha256)')
        print(f'README.txt ({len(readme_text(version).splitlines())} lines):')
        print('  ' + '\n  '.join(readme_text(version).splitlines()[:3]))
        for p in problems:
            print(f'PROBLEM: {p}')
        return 1 if problems else 0
    if problems:
        raise SystemExit('; '.join(problems))
    if sys.platform != 'darwin':
        raise SystemExit('make_macos.py builds the macOS app and runs on macOS only (use --dry-run elsewhere).')

    target_dir = Path(args.target_dir).resolve()
    with tempfile.TemporaryDirectory(prefix='grainy-macos-') as folder:
        work = Path(folder)
        print(f'universal binaries ({" + ".join(TARGETS)})')
        programs = build_universal(target_dir, work, args.skip_build)
        print('bundle')
        app = make_bundle(work / 'bundle', version, programs)
        print('signing')
        sign(app)
        print('checks')
        no_checkout_paths(app)
        smoke_run(app)
        print('disk image')
        image = make_dmg(app, Path(args.dist), version)
    size = image.stat().st_size
    print(f'\n{image}')
    print(f'  size:    {size} bytes ({size / 1e6:.1f} MB)')
    print(f'  SHA-256: {sha256(image)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
