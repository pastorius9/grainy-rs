"""Goldens for grainy-platform: runs the oracle's plain-logic modules (user_paths, preferences, safe_start,
whats_new, updater, feedback) on fixed inputs and stores inputs and results as JSON.

    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_platform.py

Everything that touches files runs in temporary folders; nothing reads the real settings or library, and nothing
uses the network. The notes files of the oracle are copied into crates/grainy-platform/assets/root/ (the same
layout as the oracle's root folder) so the Rust tests read the very files the goldens were made from."""
import contextlib
import hashlib
import io
import json
import os
import re
import shutil
import sys
import tempfile
import zipfile
from datetime import date
from pathlib import Path

from common import Case, ORACLE, ROOT

from luma import __version__, feedback, safe_start, updater, user_paths, whats_new
from luma.preferences import Preferences

ASSETS = ROOT / 'crates' / 'grainy-platform' / 'assets' / 'root'
MODULE = 'platform'


def sha(text):
    return hashlib.sha256(text.encode('utf-8')).hexdigest()


def copy_notes():
    (ASSETS / 'assets').mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ORACLE / 'CHANGELOG.md', ASSETS / 'CHANGELOG.md')
    for name in ('whats-new-en.md', 'feedback-done.json'):
        shutil.copyfile(ORACLE / 'assets' / name, ASSETS / 'assets' / name)


@contextlib.contextmanager
def temp():
    folder = tempfile.mkdtemp(prefix='grainy-golden-')
    try:
        yield Path(folder)
    finally:
        shutil.rmtree(folder, ignore_errors=True)


@contextlib.contextmanager
def patched(module, **values):
    old = {name: getattr(module, name) for name in values}
    for name, value in values.items():
        setattr(module, name, value)
    try:
        yield
    finally:
        for name, value in old.items():
            setattr(module, name, value)


def entry_summary(entries):
    return [dict(version=v, title=t, chars=len(b), sha256=sha(b)) for v, t, b in entries]


# ---------------------------------------------------------------------------------------------------- user_paths
def gen_user_paths():
    cases = []
    homes = ['C:/Users/someone', '/home/someone']
    for mac in (False, True):
        for local in (None, 'D:/Apps/Local', ''):
            for fallback in (None, 'E:/fallback'):
                for folder in ('Grainy', 'Luma'):
                    home = homes[0]
                    with patched(user_paths, MAC=mac), patched(Path, home=classmethod(lambda cls, h=home: Path(h))):
                        saved = os.environ.get('LOCALAPPDATA')
                        if local is None:
                            os.environ.pop('LOCALAPPDATA', None)
                        else:
                            os.environ['LOCALAPPDATA'] = local
                        try:
                            result = str(user_paths.local_data(folder, Path(fallback) if fallback else None))
                        finally:
                            if saved is None:
                                os.environ.pop('LOCALAPPDATA', None)
                            else:
                                os.environ['LOCALAPPDATA'] = saved
                    cases.append(dict(mac=mac, local_app_data=local, home=home, folder=folder, fallback=fallback,
                                      expected=result.replace('\\', '/')))
    with Case(MODULE, 'user_paths') as case:
        case.json(cases=cases)


# ---------------------------------------------------------------------------------------------------- versions
def gen_versions():
    pairs = [('0.5.80', '0.5.79'), ('0.6', '0.5.99'), ('0.5.100', '0.5.99'), ('0.5.9', '0.5.10'), (__version__, __version__),
             ('1.0', '1.0.0'), ('1.0.0', '1.0'), ('v2.0.1-rc3', '2.0.1'), ('', '0'), ('abc', ''), ('10.0', '9.9.9.9'),
             ('007.01', '7.1'), ('1.2.3.4.5', '1.2.3.4'), ('99999999999999999999.1', '99999999999999999998.9')]
    texts = ['0.6.8', '0.6', 'v1.2.3-rc4', '1.2.3.4.5', '007.01', 'abc', '', '12', '3.14.15.92.65', '0.0.0']
    case_pairs = [dict(a=a, b=b, newer=updater.newer(a, b)) for a, b in pairs]
    keys = [dict(text=t, updater=[str(x) for x in updater.version_key(t)],
                 whats_new=[str(x) for x in whats_new.version_key(t)]) for t in texts]
    keys.append(dict(text=None, updater=[str(x) for x in updater.version_key(None)],
                     whats_new=[str(x) for x in whats_new.version_key(None)]))
    with Case(MODULE, 'versions') as case:
        case.json(pairs=case_pairs, keys=keys)


# ---------------------------------------------------------------------------------------------------- updater
SYSTEMS = [dict(name='windows', mac=False, machine='AMD64'), dict(name='mac_arm64', mac=True, machine='arm64'),
           dict(name='mac_x64', mac=True, machine='x86_64'), dict(name='mac_other', mac=True, machine='ppc')]


@contextlib.contextmanager
def system(spec):
    with patched(updater, MAC=spec['mac']), patched(updater.platform, machine=lambda: spec['machine']):
        yield


def gen_assets():
    names = ['Grainy-0.6.8-windows-x64.zip', 'Grainy-1.0-windows-x64.zip', 'Grainy-1-windows-x64.zip', 'Grainy-0.6.8-windows-x64.zip\n',
             'Grainy-0.6.8-windows-x64.zip\n\n', 'grainy-0.6.8-windows-x64.zip', 'Grainy-0.6.8-windows-x86.zip', 'Grainy-0.6.8-windows-x64.zip.sig',
             'Grainy-0.6.8-macos-arm64.zip', 'Grainy-0.6.8-macos-x64.zip', 'Grainy-0.6.8-macos-unknown.zip',
             'Grainy-0.6.8-macos-arm64.dmg', 'Grainy-0.6.8.1-macos-arm64.zip', 'Grainy-0.6.8-mac.zip', 'Grainy-.6-windows-x64.zip',
             'Grainy-0..6-windows-x64.zip', 'Grainy-0.6.-windows-x64.zip', 'XGrainy-0.6.8-windows-x64.zip', '', 'Grainy-12.34.56-macos-x64.zip']
    out = []
    for spec in SYSTEMS:
        with system(spec):
            pattern = updater.asset_pattern()
            out.append(dict(system=spec, results=[dict(name=n, version=(m.group(1) if (m := pattern.match(n)) else None)) for n in names]))
    with Case(MODULE, 'asset_names') as case:
        case.json(names=names, systems=out)


def digest(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


def asset(version='9.9.9', **changes):
    value = dict(name=f'Grainy-{version}-windows-x64.zip', size=1234, digest=digest(b'x'),
                 browser_download_url=updater.DOWNLOADS + f'v{version}/Grainy-{version}-windows-x64.zip')
    value.update(changes)
    return {k: v for k, v in value.items() if v is not ...}


def release(*assets, **changes):
    value = dict(tag_name='v9.9.9', draft=False, prerelease=False, body='notes', assets=list(assets))
    value.update(changes)
    return {k: v for k, v in value.items() if v is not ...}


def gen_release_parse():
    good = asset()
    mac_assets = [asset(name='Grainy-9.9.9-macos-arm64.zip'), asset(name='Grainy-9.9.9-macos-x64.zip'), good]
    payloads = {
        'good': release(good),
        'no_body': release(good, body=None),
        'number_body': release(good, body=12),
        'list_body': release(good, body=['a', 'b']),
        'empty_body': release(good, body=''),
        'unicode_body': release(good, body='새 버전 \u2014 notes\n'),
        'draft': release(good, draft=True),
        'prerelease': release(good, prerelease=True),
        'draft_string': release(good, draft='no'),
        'draft_zero': release(good, draft=0),
        'foreign_host': release(asset(browser_download_url='https://example.com/Grainy-9.9.9-windows-x64.zip')),
        'prefix_without_slash': release(asset(browser_download_url='https://github.com/pastorius9/grainy/releases/download')),
        'lookalike_host': release(asset(browser_download_url='https://github.com.evil.example/pastorius9/grainy/releases/download/x.zip')),
        'http_not_https': release(asset(browser_download_url='http://github.com/pastorius9/grainy/releases/download/v9/Grainy-9.9.9-windows-x64.zip')),
        'no_url': release(asset(browser_download_url=...)),
        'digest_none': release(asset(digest=None)),
        'digest_missing': release(asset(digest=...)),
        'digest_uppercase': release(asset(digest='sha256:' + 'A' * 64)),
        'digest_short': release(asset(digest='sha256:' + 'a' * 63)),
        'digest_long': release(asset(digest='sha256:' + 'a' * 65)),
        'digest_md5': release(asset(digest='md5:' + 'a' * 64)),
        'digest_number': release(asset(digest=5)),
        'size_zero': release(asset(size=0)),
        'size_negative': release(asset(size=-5)),
        'size_float': release(asset(size=10.0)),
        'size_string': release(asset(size='10')),
        'size_true': release(asset(size=True)),
        'size_missing': release(asset(size=...)),
        'size_big': release(asset(size=2 ** 40)),
        'wrong_name': release(asset(name='Grainy-9.9.9-mac.zip')),
        'name_newline': release(asset(name='Grainy-9.9.9-windows-x64.zip\n')),
        'name_number': release(asset(name=5)),
        'bad_asset_first': release('text', None, 5, [], asset(size=0), asset(digest=None), good),
        'first_valid_wins': release(asset('1.0'), asset('2.0')),
        'second_after_untrusted': release(asset('1.0', browser_download_url='https://x.example/a.zip'), asset('2.0')),
        'no_assets': release(),
        'assets_null': release(assets=None),
        'assets_dict': release(assets={'name': 'x'}),
        'assets_string': release(assets='abc'),
        'assets_empty_string': release(assets=''),
        'mac_assets': release(*mac_assets),
        'windows_only_for_mac': release(good),
        'no_draft_keys': {'assets': [good]},
        'empty_object': {},
        'a_list': [],
        'a_string': 'x',
        'null': None,
    }
    cases = []
    for name, payload in payloads.items():
        for trusted in (updater.DOWNLOADS, None):
            for spec in SYSTEMS:
                with system(spec):
                    try:
                        result = updater.parse_release(payload, trusted)
                    except Exception as error:   # not reached for the payloads above
                        raise SystemExit(f'{name}: {error!r}')
                cases.append(dict(name=name, payload=payload, trusted=trusted, system=spec, expected=result))
    with Case(MODULE, 'release_parse') as case:
        case.json(cases=cases)


def zip_bytes(path, entries):
    with zipfile.ZipFile(path, 'w') as bundle:
        for name, data in entries:
            bundle.writestr(name, data)
    return path


def release_entries(version='9.9.9', extra=()):
    return [('Grainy/Grainy.exe', f'exe {version}'), ('Grainy/_internal/lib.dll', f'lib {version}'), ('Grainy/LICENSE.txt', 'MIT'), *extra]


def normal(text):
    text = re.sub(r'previous-\d{8}-\d{6}', 'previous-STAMP', text)
    text = re.sub(r'failed-\d{8}-\d{6}', 'failed-STAMP', text)
    return re.sub(r'extract-[0-9a-f]{32}', 'extract-ID', text)


def listing(root, work=None):
    """{relative path: text or None for a folder}, stamps normalised, the work folder written as <work>."""
    out = {}
    for path in sorted(Path(root).rglob('*')):
        rel = path.relative_to(root).as_posix()
        if work is not None:
            prefix = Path(work).relative_to(root).as_posix()
            if rel == prefix or rel.startswith(prefix + '/'):
                rel = '<work>' + rel[len(prefix):]
        out[normal(rel)] = path.read_text(encoding='utf-8') if path.is_file() else None
    return dict(sorted(out.items()))


def put(root, files):
    for rel, text in files.items():
        path = Path(root) / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8')


def run(call):
    try:
        value = call()
        return dict(ok=True, value=None if value is None else str(value))
    except Exception as error:
        return dict(ok=False, error=type(error).__name__, message=str(error))


def gen_stage():
    cases = []
    scenarios = {
        'ok': release_entries(),
        'with_directory_entries': [('Grainy/', ''), ('Grainy/_internal/', ''), *release_entries()],
        'backslash_names': [('Grainy\\Grainy.exe', 'x'), ('Grainy/_internal/lib.dll', 'y')],
        'parent_escape': release_entries(extra=[('../evil.txt', 'x')]),
        'other_top_folder': release_entries(extra=[('Other/x.txt', 'x')]),
        'nested_escape': release_entries(extra=[('Grainy/../../evil.txt', 'x')]),
        'backslash_escape': release_entries(extra=[('Grainy\\..\\..\\evil.txt', 'x')]),
        'absolute': release_entries(extra=[('/abs.txt', 'x')]),
        'backslash_absolute': release_entries(extra=[('\\abs.txt', 'x')]),
        'drive': release_entries(extra=[('C:/evil.txt', 'x')]),
        'colon_in_name': release_entries(extra=[('Grainy/a:b.txt', 'x')]),
        'dot_part_ok': [('Grainy/./Grainy.exe', 'x'), ('Grainy/_internal/lib.dll', 'y'), ('Grainy/Grainy.exe', 'z')],
        'no_program': [('Grainy/readme.txt', 'x')],
        'no_internal': [('Grainy/Grainy.exe', 'x')],
        'internal_as_file': [('Grainy/Grainy.exe', 'x'), ('Grainy/_internal', 'y')],
        'empty': [],
        'illegal_characters': release_entries(extra=[('Grainy/we?ird*name.txt', 'x')]),
        'symlink_like_text': release_entries(extra=[('Grainy/link', '../../outside')]),
    }
    for name, entries in scenarios.items():
        with temp() as root:
            archive = zip_bytes(root / 'new.zip', entries)
            work = root / 'work'
            with patched(updater, MAC=False):
                result = run(lambda: updater.stage(archive, work))
            if result['ok']:
                result['value'] = Path(result['value']).relative_to(root).as_posix()
            cases.append(dict(name=name, entries=entries, result=result, listing=listing(work) if work.exists() else {}))
    # a second stage into a folder holding an unfinished update, and an extraction folder left by the first
    with temp() as root:
        archive = zip_bytes(root / 'new.zip', release_entries())
        with patched(updater, MAC=False):
            updater.stage(archive, root / 'work')
            second = run(lambda: updater.stage(archive, root / 'work'))
        cases.append(dict(name='second_stage', entries=release_entries(), result=second, listing=listing(root / 'work')))
    with Case(MODULE, 'stage') as case:
        case.json(cases=cases)


def gen_bundle_names():
    lists = {
        'ok': ['Grainy.app/Contents/MacOS/Grainy', 'Grainy.app/Contents/Info.plist'],
        'escape': ['Grainy.app/Contents/MacOS/Grainy', 'Grainy.app/Contents/Info.plist', '../evil.txt'],
        'other_app': ['Grainy.app/Contents/MacOS/Grainy', 'Grainy.app/Contents/Info.plist', 'Other.app/x'],
        'no_program': ['Grainy.app/Contents/Info.plist'],
        'windows_zip': ['Grainy/Grainy.exe', 'Grainy/_internal/lib.dll'],
        'absolute': ['/Grainy.app/Contents/MacOS/Grainy', 'Grainy.app/Contents/Info.plist'],
        'dot_parts': ['Grainy.app/./Contents/MacOS/Grainy', 'Grainy.app/Contents/MacOS/Grainy', 'Grainy.app/Contents/Info.plist'],
        'nested_escape': ['Grainy.app/Contents/MacOS/Grainy', 'Grainy.app/Contents/Info.plist', 'Grainy.app/../x'],
        'colon_allowed': ['Grainy.app/Contents/MacOS/Grainy', 'Grainy.app/Contents/Info.plist', 'Grainy.app/a:b'],
    }
    cases = []
    for name, names in lists.items():
        with temp() as root:
            archive = zip_bytes(root / 'new.zip', [(n, 'x') for n in names])
            # The name checks run before ditto; a failing check never reaches it (also on Windows).
            with patched(updater, MAC=True):
                result = run(lambda: updater.stage(archive, root / 'w'))
        # an accepted list would need ditto: record only whether the check raised ValueError
        cases.append(dict(name=name, names=names, rejected=result.get('error') == 'ValueError'))
    with Case(MODULE, 'bundle_names') as case:
        case.json(cases=cases)


def installed(root, version='1.0'):
    put(root, {'Grainy.exe': f'exe {version}', '_internal/lib.dll': f'lib {version}', 'my-notes.txt': 'mine'})


def gen_finish():
    cases = []
    full = release_entries(extra=[('Grainy/THIRD_PARTY_LICENSES/a.txt', 'a'), ('Grainy/skipped.txt', 'not an item')])
    # success
    for name, entries in (('success', release_entries()), ('success_all_items', full)):
        with temp() as root:
            app = root / 'app'; installed(app)
            with patched(updater, MAC=False):
                staged = updater.stage(zip_bytes(root / 'new.zip', entries), app / '.update')
                log = io.StringIO()
                result = updater.finish(app, staged, log=log)
            lines = [re.sub(r'^\d\d:\d\d:\d\d ', '', normal(line)) for line in log.getvalue().splitlines()]
            cases.append(dict(name=name, kind='windows', entries=entries, installed={'Grainy.exe': 'exe 1.0', '_internal/lib.dll': 'lib 1.0', 'my-notes.txt': 'mine'},
                              result=result, log=lines, listing=listing(app)))
    # the copy of the program folder fails half way
    with temp() as root:
        app = root / 'app'; installed(app)
        with patched(updater, MAC=False):
            staged = updater.stage(zip_bytes(root / 'new.zip', release_entries()), app / '.update')

            def broken(source, target, *a, **k):
                Path(target).mkdir(); (Path(target) / 'half.dll').write_text('partial'); raise OSError('disk full')
            with patched(updater.shutil, copytree=broken):
                result = updater.finish(app, staged)
        cases.append(dict(name='copy_fails', kind='windows', entries=release_entries(), installed={'Grainy.exe': 'exe 1.0', '_internal/lib.dll': 'lib 1.0', 'my-notes.txt': 'mine'},
                          result=result, log=[], listing=listing(app)))
    # staged folder incomplete
    with temp() as root:
        app = root / 'app'; installed(app); (root / 'staged').mkdir()
        with patched(updater, MAC=False):
            result = updater.finish(app, root / 'staged')
        cases.append(dict(name='incomplete', kind='windows', entries=[], installed={'Grainy.exe': 'exe 1.0', '_internal/lib.dll': 'lib 1.0', 'my-notes.txt': 'mine'},
                          result=result, log=[], listing=listing(app)))
    # macOS bundle swap (plain files stand for the bundle; the folder moves are the same calls)
    bundle = {'Contents/MacOS/Grainy': 'program 1.0', 'Contents/Info.plist': 'plist 1.0', 'Contents/Resources/data.txt': 'data 1.0'}
    new_bundle = {'Contents/MacOS/Grainy': 'program 9.9.9', 'Contents/Info.plist': 'plist 9.9.9', 'Contents/Resources/data.txt': 'data 9.9.9'}
    for name, fail_second in (('mac_success', False), ('mac_place_fails', True), ('mac_incomplete_staged', None)):
        with temp() as root:
            work = root / 'Support' / 'Update'
            app = root / 'Applications' / 'Grainy.app'
            put(app, bundle); (root / 'Applications' / 'Other.app').mkdir()
            staged = work / 'new' / 'Grainy.app'
            put(staged, new_bundle if fail_second is not None else {'Contents/Info.plist': 'plist'})
            log = io.StringIO()
            real = updater._place; calls = []

            def failing(source, target):
                calls.append(target)
                if len(calls) == 2:
                    raise OSError('disk full')
                real(source, target)
            with patched(updater, MAC=True, work_folder=lambda a, w=work: w, _place=failing if fail_second else real):
                result = updater.finish(app, staged, log=log)
            lines = [re.sub(r'^\d\d:\d\d:\d\d ', '', normal(line)) for line in log.getvalue().splitlines()]
            cases.append(dict(name=name, kind='mac', bundle=bundle, new_bundle=new_bundle if fail_second is not None else {'Contents/Info.plist': 'plist'},
                              fail_second_place=bool(fail_second), result=result, log=lines, listing=listing(root, work)))
    with Case(MODULE, 'finish') as case:
        case.json(cases=cases)


def gen_cleanup():
    cases = []
    names = ['new', 'download', 'failed-1', 'failed-20260101-000000', 'extract-abc', 'previous-20260101-000000', 'previous-20260201-000000',
             'previous-20260301-000000', 'unrelated']
    for subset in (names, names[:3], ['previous-20260201-000000'], ['previous-20260101-000000', 'previous-20260201-000000'], [], ['unrelated']):
        with temp() as root:
            work = root / 'app' / '.update'
            work.mkdir(parents=True)
            for name in subset:
                (work / name).mkdir()
            trashed = []
            import send2trash
            with patched(send2trash, send2trash=lambda p: trashed.append(Path(p).name)), patched(updater, MAC=False):
                updater.cleanup(root / 'app')
        cases.append(dict(present=subset, trashed=sorted(trashed)))
    with Case(MODULE, 'cleanup') as case:
        case.json(cases=cases)


def gen_blocked():
    cases = []
    Usage = type('Usage', (), {})
    for name, mac, exists, free, size in (('no_app', False, None, 0, 0), ('missing_folder', False, False, 10 ** 12, 0),
                                          ('ok', False, True, 10 ** 12, 0), ('mac_missing_parent', True, False, 10 ** 12, 0),
                                          ('mac_ok', True, True, 10 ** 12, 0), ('low_free', False, True, 10 ** 9 - 1, 0),
                                          ('exact_free', False, True, 10 ** 9, 0), ('gigabyte_free', False, True, 1024 ** 3, 0),
                                          ('below_gigabyte', False, True, 1024 ** 3 - 1, 0), ('big_release', False, True, 2 * 1024 ** 3, 400 * 1024 ** 2),
                                          ('big_release_fits', False, True, 3 * 1024 ** 3, 400 * 1024 ** 2)):
        with temp() as root:
            if exists is None:
                app = None
            elif mac:
                app = (root / 'Apps' / 'Grainy.app') if exists else (root / 'nowhere' / 'Grainy.app')
                if exists:
                    app.mkdir(parents=True)
            else:
                app = root / 'app' if exists else root / 'nowhere'
                if exists:
                    app.mkdir()

            def usage(path, free=free):
                result = Usage(); result.free = free; return result
            with patched(updater, MAC=mac), patched(updater.shutil, disk_usage=usage):
                result = updater.blocked(app, size)
        cases.append(dict(name=name, mac=mac, app_kind=None if exists is None else ('exists' if exists else 'missing'), free=free, size=size, expected=result))
    with Case(MODULE, 'blocked') as case:
        case.json(cases=cases)


def gen_download():
    cases = []
    data = bytes(range(256)) * 5000  # 1.28 MB: more than one block
    for name, mutate in (('ok', {}), ('wrong_digest', {'sha256': '0' * 64}), ('size_too_small', {'size': len(data) - 1}),
                         ('size_too_big', {'size': len(data) + 1}), ('empty_file', {'empty': True})):
        with temp() as root:
            source = root / 'source.zip'
            source.write_bytes(b'' if mutate.get('empty') else data)
            blob = source.read_bytes()
            rel = dict(version='9.9.9', name='Grainy-9.9.9-windows-x64.zip', url=str(source), size=len(blob),
                       sha256=hashlib.sha256(blob).hexdigest(), notes='')
            rel.update({k: v for k, v in mutate.items() if k != 'empty'})
            seen = []
            result = run(lambda: updater.download(rel, root / 'work' / 'download', lambda d, t: seen.append((d, t))))
            if result['ok'] and result['value']:
                result['value'] = Path(result['value']).relative_to(root).as_posix()
            files = sorted(p.relative_to(root / 'work').as_posix() for p in (root / 'work').rglob('*') if p.is_file()) if (root / 'work').exists() else []
        cases.append(dict(name=name, length=len(blob), release_size=rel['size'], release_sha256_is_real=rel['sha256'] == hashlib.sha256(blob).hexdigest(),
                          result=result, files=files, progress=seen))
    with Case(MODULE, 'download') as case:
        case.json(cases=cases, pattern_length=len(data))


# ---------------------------------------------------------------------------------------------------- preferences
def gen_preferences():
    scripts = {
        'fresh': dict(initial=None, ops=[('save', 'language', '"ko"'), ('save', 'zeta', '1'), ('save', 'alpha', '{"z": 1, "a": [1, 2.5, null, true], "m": {}}'),
                                         ('save', 'language', '"en"')]),
        'unicode_and_floats': dict(initial=None, ops=[('save', 'title', '"한글 \\u00e9 \\ud83d\\ude00 \\"q\\" \\\\ \\n \\t \\u0001"'),
                                                      ('save', 'f', '[0.1, 1e-05, 1e16, 100.0, -0.0, 12345678.9, 1.5e300, 9223372036854775807, 18446744073709551615]'),
                                                      ('save', 'empty_list', '[]'), ('save', 'nested', '[[], [[]], {}, {"a": {}}]')]),
        'language_and_gpu': dict(initial=None, ops=[('language', 'en', None), ('language', 'xx', None), ('language', '', None), ('gpu', False, None), ('gpu', True, None), ('language', 'ko', None)]),
        'existing_order_kept': dict(initial='{"z": 1, "a": 2, "language": "en", "list": [1, {"k": "v", "b": 1}]}\n',
                                    ops=[('save', 'a', '5'), ('save', 'new', '"x"')]),
        'duplicate_keys': dict(initial='{"a": 1, "b": 2, "a": 3}', ops=[('save', 'c', '1')]),
        'not_an_object': dict(initial='[1, 2, 3]', ops=[('save', 'a', '1')]),
        'damaged_file': dict(initial='{"a": 1', ops=[('save', 'a', '1')]),
        'empty_file': dict(initial='', ops=[('save', 'a', '1')]),
        'bom_file': dict(initial='\ufeff{"a": 1}', ops=[('save', 'b', '2')]),
        'other_writer_between': dict(initial='{"a": 1}', ops=[('save', 'b', '2'), ('external', None, '{"a": 1, "b": 2, "from_other": true, "feedback_sent": [{"ref": "AAAAA", "version": "0.6.8", "sent": "2026-11-01"}]}'),
                                                              ('save', 'c', '3'), ('external', None, 'garbage'), ('save', 'd', '4')]),
        'gpu_false_variants': dict(initial='{"use_gpu": 0}', ops=[('probe', None, None), ('external', None, '{"use_gpu": false}'), ('probe', None, None),
                                                                 ('external', None, '{"use_gpu": null}'), ('probe', None, None), ('external', None, '{"language": 5}'), ('probe', None, None)]),
    }
    cases = []
    for name, script in scripts.items():
        with temp() as root:
            path = root / 'settings' / 'settings.json'
            if script['initial'] is not None:
                path.parent.mkdir(parents=True)
                path.write_text(script['initial'], encoding='utf-8', newline='')
            prefs = Preferences(path)
            steps = []
            for kind, key, arg in script['ops']:
                outcome = 'ok'
                if kind == 'save':
                    prefs.save(key, json.loads(arg))
                elif kind == 'language':
                    try:
                        prefs.save_language(key)
                    except ValueError:
                        outcome = 'error'
                elif kind == 'gpu':
                    prefs.save_use_gpu(key)
                elif kind == 'external':
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(arg, encoding='utf-8', newline='')
                    prefs_other = None
                elif kind == 'probe':
                    pass
                text = path.read_text(encoding='utf-8', newline='') if path.exists() else None
                steps.append(dict(op=[kind, key, arg], outcome=outcome, file=text, language=prefs.language, use_gpu=prefs.use_gpu,
                                  folder=sorted(p.name for p in path.parent.iterdir()) if path.parent.exists() else []))
            reread = Preferences(path)
            cases.append(dict(name=name, initial=script['initial'], steps=steps, reread_language=reread.language, reread_use_gpu=reread.use_gpu))
    with Case(MODULE, 'preferences') as case:
        case.json(cases=cases)


# ---------------------------------------------------------------------------------------------------- safe_start
def gen_safe_start():
    cases = []
    markers = {'none': None, 'dead_process': '{"pid": 111, "version": "0.6.6", "started": 1.5}', 'alive_process': '{"pid": 4242, "version": "0.6.6"}',
               'no_pid': '{"version": "0.6.6"}', 'string_pid': '{"pid": "111"}', 'float_pid': '{"pid": 111.0}', 'negative_pid': '{"pid": -5}',
               'not_json': 'not json', 'a_list': '[1]', 'empty': '', 'null_pid': '{"pid": null, "version": "x"}'}
    for name, text in markers.items():
        with temp() as root:
            if text is not None:
                (root / safe_start.NAME).write_text(text, encoding='utf-8')
            with patched(safe_start, _alive=lambda pid: pid == 4242):
                previous = safe_start.begin(root)
            written = json.loads((root / safe_start.NAME).read_text(encoding='utf-8'))
            raw = (root / safe_start.NAME).read_text(encoding='utf-8')
        cases.append(dict(name=name, marker=text, previous=previous, written_keys=list(written), written_version=written['version'],
                          written_template=re.sub(r'"started": [^}]+', '"started": <t>', re.sub(r'"pid": \d+', '"pid": <pid>', raw))))
    with Case(MODULE, 'safe_start') as case:
        case.json(cases=cases)


# ---------------------------------------------------------------------------------------------------- whats_new
LOG = '''# Log

## 1.2.10 — Ten

- ten

## 1.2.9 — Nine

- nine

## 1.2.8

- eight
'''
ODD = ('# x\n\n## 3.0 \u2014  \tSpaced title \t \n\nbody 3\n\n\n## 2.1.1\u2014no spaces\nline\n\n##  1.0.0 not a heading\n## 1.0 no dash\n'
       '## 1.0.0.0 \u2014 four parts\n\u00a0\u2003 nbsp body \u00a0\n### 0.9 deeper\n## 0.8.\n## 0.7.1 — title — more — dashes\ntail text\n')
CRLF = '# Log\r\n\r\n## 2.0.0 \u2014 Two\r\n\r\n- two\r\n\r\n## 1.0.0\r\n\r\n- one\r\n\r\n## 0.9.0 \u2014 Nine\r\n\r\n- nine\r\n'


def notes_root(root, korean, english=None, done=None):
    (root / 'assets').mkdir(parents=True, exist_ok=True)
    (root / 'CHANGELOG.md').write_text(korean, encoding='utf-8', newline='')
    if english is not None:
        (root / 'assets' / 'whats-new-en.md').write_text(english, encoding='utf-8', newline='')
    if done is not None:
        (root / 'assets' / 'feedback-done.json').write_text(done, encoding='utf-8')
    return root


def gen_whats_new():
    texts = {'log': LOG, 'odd': ODD, 'crlf': CRLF, 'empty': '', 'no_headings': 'just text\n', 'duplicate': '## 1.0 — a\n\nx\n\n## 1.0 — b\n\ny\n'}
    sec = {name: [list(entry) for entry in whats_new.sections(text)] for name, text in texts.items()}
    with Case(MODULE, 'sections') as case:
        case.json(texts=texts, expected=sec)
    # real files
    changelog = (ORACLE / 'CHANGELOG.md').read_text(encoding='utf-8')
    english = (ORACLE / 'assets' / 'whats-new-en.md').read_text(encoding='utf-8')
    real = dict(changelog=entry_summary(whats_new.sections(changelog)), english=entry_summary(whats_new.sections(english)))
    previous_values = [None, '', '0.6.7', '0.6.5', '0.6.0', '0.5.90', '0.5.1', '0.1', '0.6.8', '0.6.9', '999', 'garbage', '0.6', 'v0.6.7-beta', '0.6.7.2.1']
    currents = [__version__, '0.6.0', '0.6.6', '0.5.96', '1.0', '0.0.1', '0']
    note_cases = []
    for lang in ('ko', 'en'):
        for current in currents:
            for previous in previous_values:
                entries = whats_new.notes(previous, current, lang=lang, root=ORACLE)
                note_cases.append(dict(previous=previous, current=current, lang=lang, entries=entry_summary(entries), markdown_sha256=sha(whats_new.markdown(entries)),
                                       markdown_chars=len(whats_new.markdown(entries))))
    with Case(MODULE, 'notes_real') as case:
        case.json(real=real, cases=note_cases)
    # synthetic roots
    syn = []
    setups = {'ten': (LOG, '## 1.2.10 — Ten in English\n\n- ten!\n'), 'no_english': (LOG, None), 'crlf': (CRLF, '## 2.0.0 — Two EN\r\n\r\n- two en\r\n'),
              'odd': (ODD, '## 3.0 — Third\n\nthree\n\n## 3.0 — Third again\n\nthree b\n'), 'missing': (None, None)}
    previous_syn = [None, '1.2.8', '1.2.9', '1.2.10', '1.0', '1', '2', '1.3', '0.9.0', '1.0.0', '2.0.0']
    for setup, (korean, eng) in setups.items():
        with temp() as root:
            if korean is not None:
                notes_root(root, korean, eng)
            for lang in ('ko', 'en'):
                for current in ('1.2.10', '1.2.9', '2.0.0', '3.0', '9', '1.2.8'):
                    for previous in previous_syn:
                        entries = whats_new.notes(previous, current, lang=lang, root=root)
                        syn.append(dict(setup=setup, lang=lang, current=current, previous=previous, entries=[list(e) for e in entries],
                                        markdown=whats_new.markdown(entries)))
    with Case(MODULE, 'notes_synthetic') as case:
        case.json(setups={k: dict(changelog=v[0], english=v[1]) for k, v in setups.items()}, cases=syn)


def sent(*refs):
    return [{'ref': ref, 'version': '0.5.94', 'sent': '2026-10-16'} for ref in refs]


def gen_whats_new_state():
    # pending / remember / shipped / thank over the real notes, with preference files written by the oracle's own class
    done_data = json.loads((ORACLE / 'assets' / 'feedback-done.json').read_text(encoding='utf-8'))
    real_refs = [r for refs in done_data.values() for r in refs]
    pend = []
    seens = [None, '', '0.6.7', '0.6.8', '0.6.9', '999.0.0', '0.5.90', 5, 0, True, False, ['0.6.7'], {'a': 1}, '0.6.7-beta', ' ', 'x', '0.6.8.0']
    for seen in seens:
        for first_run in (False, True):
            for lang in ('ko', 'en'):
                with temp() as root:
                    prefs = Preferences(root / 's.json')
                    if seen is not None:
                        prefs.save(whats_new.KEY, seen)
                    from luma import i18n
                    old = i18n.language()
                    i18n.set_language(lang)
                    try:
                        entries = whats_new.pending(prefs, first_run=first_run)
                    finally:
                        i18n.set_language(old)
                    pend.append(dict(seen=seen, seen_present=seen is not None, first_run=first_run, lang=lang, entries=entry_summary(entries)))
    remembered = []
    for seen in (None, '0.6.7', '0.6.8', 5):
        with temp() as root:
            prefs = Preferences(root / 's.json')
            if seen is not None:
                prefs.save(whats_new.KEY, seen)
            whats_new.remember(prefs)
            remembered.append(dict(seen=seen, seen_present=seen is not None, file=(root / 's.json').read_text(encoding='utf-8'), value=prefs.values.get(whats_new.KEY)))
    with Case(MODULE, 'pending') as case:
        case.json(pending=pend, remember=remembered)
    # shipped / thank over the real done file and over the synthetic test one
    synthetic_done = {'1.2.8': ['AAAAA'], '1.2.9': ['BBBBB', '#ccccc'], '1.2.10': ['DDDDD', 'EEEEE'], '1.3': ['FFFFF'], 'notes': 'x', '2.0': 'not a list',
                      '1.4': [5, None, '##gg']}
    ship = []
    for label, root_kind, done_text in (('synthetic', 'synthetic', json.dumps(synthetic_done)), ('real', 'real', None), ('damaged', 'synthetic', '['),
                                        ('list', 'synthetic', '[1]'), ('missing', 'missing', None)):
        with temp() as root:
            if root_kind == 'synthetic':
                notes_root(root, LOG, None, done_text)
                use_root = root
            elif root_kind == 'real':
                use_root = ORACLE
            else:
                use_root = root / 'missing'
            sent_value = (sent('AAAAA', 'BBBBB', 'CCCCC', 'DDDDD', 'EEEEE', 'GGGGG', 'GGGGG') + ['damaged', {'ref': 5}] if root_kind != 'real'
                          else sent(*real_refs, 'ZZZZZ') + [{'ref': 'NOPE1'}])
            for thanked in (None, ['ZZZZZ', 'EEEEE'], [], 'not a list'):
                prefs_path = root / 'prefs' / 's.json'
                prefs = Preferences(prefs_path)
                prefs.save(feedback.SENT, sent_value)
                if thanked is not None:
                    prefs.save(whats_new.THANKED, thanked)
                ranges = [('1.2.8', '1.2.10'), ('1.2.9', '1.2.10'), ('1.0', '1.2.8'), ('1.2.10', '1.2.10'), ('1.2.10', '1.2.9'), (None, '1.2.9'),
                          ('1.2.10', '1.3'), ('1.0', '2.0'), (None, '1.4'), ('1.3', '1.4'), (None, '1.0'),
                          ('0.5.90', '0.6.8'), ('0.6.5', '0.6.6'), (None, '0.6.6'), ('0.6.6', '0.6.8'), ('0.5.96', '0.6.0'), ('', '0.6.0')]
                results = [dict(previous=p, current=c, shipped=whats_new.shipped(prefs, p, c, root=use_root)) for p, c in ranges]
                thank_steps = []
                for refs in (['BBBBB', 'DDDDD'], ['CCCCC'], [], ['AAAAA']):
                    whats_new.thank(prefs, refs)
                    thank_steps.append(dict(refs=refs, file=prefs_path.read_text(encoding='utf-8'),
                                            shipped_after=whats_new.shipped(prefs, '1.2.8', '1.2.10', root=use_root)))
                ship.append(dict(label=label, root_kind=root_kind, done_text=done_text, thanked=thanked, thanked_present=thanked is not None,
                                 sent=sent_value, sent_text=json.dumps(sent_value), thanked_text=json.dumps(thanked), results=results, thank_steps=thank_steps,
                                 done=whats_new.done(use_root)))
    thanks = {}
    from luma import i18n
    for lang in ('ko', 'en'):
        old = i18n.language(); i18n.set_language(lang)
        try:
            thanks[lang] = [dict(refs=refs, text=whats_new.thanks(refs)) for refs in (['K7Q2M'], ['K7Q2M', 'A3BC4'], [])]
        finally:
            i18n.set_language(old)
    with Case(MODULE, 'shipped') as case:
        case.json(cases=ship, thanks=thanks, real_refs=real_refs)


# ---------------------------------------------------------------------------------------------------- feedback
class Response:
    def __init__(self, status): self.status = status
    def __enter__(self): return self
    def __exit__(self, *a): return False


def gen_feedback():
    import urllib.error
    long_text = 'a' * 3999 + '\U0001F600' + 'tail'
    messages = ['hello', '  padded with spaces \n', '', '   ', '한글 메시지 & more = stuff + plus/slash ?q#h', 'x' * 4000, 'x' * 4001, long_text, 'a\tb\nc\r\nd',
                '\u00a0nbsp trimmed\u00a0', '~-_. unreserved', "quote ' \" back\\slash", '%41 percent']
    systems = ['Windows 11 (10.0.26200) · AMD64 · GPU NVIDIA', 'macOS 15.1 · arm64']
    posts = []
    for message in messages:
        for system_line in systems:
            seen = {}

            def opener(request, timeout=None):
                seen.update(url=request.full_url, data=request.data.decode('ascii'), headers=dict(request.header_items()), timeout=timeout)
                return Response(200)
            result = feedback.send(message, system_line, opener)
            posts.append(dict(message=message, system=system_line, result=result, **seen))
    outcomes = []
    for name, behaviour in (('ok', lambda: Response(200)), ('status_204', lambda: Response(204)), ('status_500', lambda: Response(500)),
                            ('http_404', lambda: (_ for _ in ()).throw(urllib.error.HTTPError('u', 404, 'nf', {}, None))),
                            ('os_error', lambda: (_ for _ in ()).throw(OSError('network down'))),
                            ('url_error', lambda: (_ for _ in ()).throw(urllib.error.URLError('no route'))),
                            ('empty_os_error', lambda: (_ for _ in ()).throw(OSError()))):
        outcomes.append(dict(name=name, result=feedback.send('m', 's', lambda request, timeout=None, b=behaviour: b())))
    info = []
    from luma import native_gpu
    for mac, release, version, mac_ver, machine, adapter, enabled in ((False, '11', '10.0.26200', '', 'AMD64', 'NVIDIA GeForce', True), (False, '10', '10.0.19045', '', 'AMD64', 'Intel UHD', False),
                                                                      (False, '11', '10.0.26200', '', 'AMD64', '', False), (True, '', '', '15.1', 'arm64', 'Apple M2', True),
                                                                      (True, '', '', '14.7.1', 'x86_64', None, False)):
        import platform as plat
        with patched(feedback.sys, platform='darwin' if mac else 'win32'), patched(plat, release=lambda r=release: r, version=lambda v=version: v,
                                                                                     mac_ver=lambda m=mac_ver: (m, ('', '', ''), ''), machine=lambda m=machine: m), \
                patched(native_gpu, adapter=adapter, enabled=lambda e=enabled: e):
            info.append(dict(mac=mac, release=release, version=version, mac_ver=mac_ver, machine=machine, adapter=adapter, enabled=enabled, expected=feedback.system_info()))
    records = []
    for name, initial in (('fresh', None), ('with_damaged', [1, 'x', {'ref': 'OLD01', 'version': '0.5', 'sent': '2026-01-01'}, None]), ('not_a_list', {'a': 1}), ('full', sent(*[f'R{i:04d}'[:5] for i in range(50)]))):
        with temp() as root:
            prefs = Preferences(root / 's.json')
            if initial is not None:
                prefs.save(feedback.SENT, initial)
            steps = []
            for index in range(3 if name != 'full' else 3):
                feedback.record(prefs, f'AB{index}CD', '0.6.8', date(2026, 11, 1 + index))
                steps.append((root / 's.json').read_text(encoding='utf-8'))
            records.append(dict(name=name, initial_text=json.dumps(initial), initial_present=initial is not None, files=steps))
    many = None
    with temp() as root:
        prefs = Preferences(root / 's.json')
        for index in range(55):
            feedback.record(prefs, f'M{index:04d}'[:5], '0.6.8', date(2026, 11, 1))
        many = (root / 's.json').read_text(encoding='utf-8')
    with Case(MODULE, 'feedback') as case:
        case.json(version=__version__, form_id=feedback.FORM_ID, fields=feedback.FIELDS, limit=feedback.LIMIT, alphabet=feedback.ALPHABET, keep=feedback.KEEP,
                  tagged=feedback.tagged('sys', 'K7Q2M'), posts=posts, outcomes=outcomes, system_info=info, records=records, many=many)


def main():
    copy_notes()
    gen_user_paths(); gen_versions(); gen_assets(); gen_release_parse(); gen_stage(); gen_bundle_names(); gen_finish(); gen_cleanup(); gen_blocked()
    gen_download(); gen_preferences(); gen_safe_start(); gen_whats_new(); gen_whats_new_state(); gen_feedback()
    print('platform goldens written to', ROOT / 'golden' / MODULE)


if __name__ == '__main__':
    main()
