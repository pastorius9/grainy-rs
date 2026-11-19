"""Golden scenarios for grainy-library, computed by the Python oracle on generated temporary folders only.

Every scenario builds a small tree of files with known bytes under a temporary root, a catalogue next to it, runs
scripted calls of the oracle's functions and records the return values (or exception type and message), the file
tree (relative paths, sizes, SHA-256) and the changed catalogue rows. Paths in the goldens use the placeholder
`$ROOT`; the Rust tests substitute their own temporary root.

    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_library.py

Never touches the owner's library, the oracle's data folder or real photos.
"""
import sys
sys.dont_write_bytecode = True
from common import ROOT, ORACLE, GOLDEN, Case, write_json, np
import base64
import hashlib
import json
import os
import shutil
import sqlite3
import stat
import tempfile
import threading
import xml.etree.ElementTree as ET
from datetime import datetime
from pathlib import Path
from unittest.mock import patch
from luma import (library, file_access, folder_locations, folder_sync, import_copy, quick_export, catalog_maintenance,
                  workflow_data, folder_recovery, folders, preview_store)
from luma.catalog import Catalog
from luma.engine import normalized

OUT = GOLDEN / 'library'
ET_ERRORS = (ET.ParseError,)


class Clock:
    """Fake time module for folder_sync: monotonic() and time() come from this object."""
    def __init__(self):
        self.mono = 1000.0
        self.epoch = 1_800_000_000.0

    def monotonic(self):
        return self.mono

    def time(self):
        return self.epoch


def error_info(error):
    kind = 'OSError' if isinstance(error, OSError) else 'ParseError' if isinstance(error, ET.ParseError) else \
        'sqlite3.Error' if isinstance(error, sqlite3.Error) else type(error).__name__
    if kind == 'JSONDecodeError' or kind == 'UnicodeDecodeError':
        kind = 'ValueError'
    keep = kind in ('ValueError', 'DataError')
    return {'type': kind, 'message': str(error) if keep else None}


class Runner:
    def __init__(self, spec):
        self.spec = spec
        self.base = Path(tempfile.mkdtemp(prefix='grainy-lib-')).resolve()
        self.root = self.base / 'files'
        self.root.mkdir()
        self.catdir = self.base / 'catalog'
        self.cat = None
        self.vars = {}
        self.fs = None
        self.fake_clock = Clock()
        self.host = None
        self.jobs = []
        self.patchers = []

    # ---- placeholders -------------------------------------------------
    def p(self, value):
        """Argument text: `$ROOT/a/b` becomes the real path."""
        if isinstance(value, str) and value.startswith('$KEY:'):
            return folders.path_key(self.p(value[5:]))
        if isinstance(value, str) and value == '$ANCHOR':
            return self.root.anchor
        if isinstance(value, str) and value.startswith('$ROOT'):
            return str(self.root) + value[5:].replace('/', os.sep)
        return value

    def arg(self, value):
        """A SQL argument: text with placeholders or {'blob_hex': ...}."""
        if isinstance(value, dict):
            return bytes.fromhex(value['blob_hex'])
        return self.p(value)

    def deep_in(self, value):
        if isinstance(value, str):
            return self.p(value)
        if isinstance(value, list):
            return [self.deep_in(v) for v in value]
        if isinstance(value, dict):
            return {self.p(k): self.deep_in(v) for k, v in value.items()}
        return value

    def out(self, value):
        """JSON-ready copy with the root replaced by the placeholder."""
        if isinstance(value, str):
            doubled = str(self.root).replace('\\', '\\\\')
            low = str(self.root).lower()
            return (value.replace(doubled, '$ROOT').replace(str(self.root), '$ROOT').replace(low.replace('\\', '\\\\'), '$root')
                    .replace(low, '$root'))
        if isinstance(value, Path):
            return self.out(str(value))
        if isinstance(value, (set, frozenset)):
            return sorted((self.out(v) for v in value), key=lambda v: json.dumps(v, sort_keys=True))
        if isinstance(value, (list, tuple)):
            return [self.out(v) for v in value]
        if isinstance(value, dict):
            return {str(k) if not isinstance(k, str) else self.out(k): self.out(v) for k, v in value.items()}
        if isinstance(value, bytes):
            return value.hex()
        if isinstance(value, np.generic):
            return value.item()
        if isinstance(value, datetime):
            return value.strftime('%Y-%m-%d %H:%M:%S')
        return value

    # ---- setup ---------------------------------------------------------
    def build(self):
        s = self.spec.get('setup', {})
        if self.spec.get('fake_images'):
            self.fake_images()
        for rel, item in s.get('files', {}).items():
            path = self.root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            if 'text' in item:
                path.write_bytes(item['text'].encode('utf-8'))
            elif 'bytes_hex' in item:
                path.write_bytes(bytes.fromhex(item['bytes_hex']))
            elif 'text16' in item:
                path.write_bytes(b'\xff\xfe' + item['text16'].encode('utf-16-le'))
            else:
                byte, size = item['fill']
                path.write_bytes(bytes([byte]) * size)
        for rel in s.get('dirs', []):
            (self.root / rel).mkdir(parents=True, exist_ok=True)
        for rel, ns in s.get('mtimes', {}).items():
            path = self.root / rel
            os.utime(path, ns=(ns, ns))
        self.cat = Catalog(self.catdir)
        for photo in s.get('photos', []):
            if photo.get('virtual_of'):
                self.cat.virtual_copy(photo['virtual_of'])
            else:
                self.cat.add(self.p(photo['path']), photo.get('settings'), photo.get('metadata'))
        for sql, args in s.get('db', []):
            self.cat.db.execute(sql, [self.arg(a) for a in args])
        self.cat.db.commit()
        for key, value in s.get('prefs', {}).items():
            self.cat.save_preference(key, self.deep_in(value))
        for name, other in s.get('other', {}).items():
            directory = self.base / ('other_' + name)
            c = Catalog(directory)
            for photo in other.get('photos', []):
                if photo.get('virtual_of'):
                    c.virtual_copy(photo['virtual_of'])
                else:
                    c.add(self.p(photo['path']), photo.get('settings'), photo.get('metadata'))
            for sql, args in other.get('db', []):
                c.db.execute(sql, [self.arg(a) for a in args])
            c.db.commit()
            for key, value in other.get('prefs', {}).items():
                c.save_preference(key, self.deep_in(value))
            c.backup(self.base / 'backups_in' / f'{name}.sqlite')
            c.close()

    # ---- observation ---------------------------------------------------
    def tree(self):
        files, dirs = {}, []
        for path in sorted(self.root.rglob('*')):
            rel = path.relative_to(self.root).as_posix()
            if path.is_dir():
                dirs.append(rel)
            else:
                data = path.read_bytes()
                files[rel] = {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
        return {'files': files, 'dirs': dirs}

    def database(self):
        if self.cat is None:
            return {}
        db = self.cat.db
        db.commit()
        photos = [dict(id=r[0], path=r[1], name=r[2], virtual_source=r[3]) for r in
                  db.execute('SELECT id,path,name,virtual_source FROM photos ORDER BY id')]
        result = dict(photos=self.out(photos),
                      folder_roots=self.out(sorted(r[0] for r in db.execute('SELECT path FROM folder_roots'))),
                      preferences=self.out({r[0]: r[1] for r in db.execute('SELECT key,value FROM preferences ORDER BY key')}))
        for table in self.spec.get('dump_tables', []):
            columns = [r[1] for r in db.execute(f'PRAGMA table_info({table})')]
            result[table] = self.out([list(r) for r in db.execute(f'SELECT {",".join(columns)} FROM {table} ORDER BY 1')])
        backups = self.catdir / 'backups'
        if backups.exists():
            import re
            result['backup_names'] = sorted(re.sub(r'-[0-9a-f]{8}\.sqlite$', '-<id>.sqlite', re.sub(r'\d{8}-\d{6}', '<stamp>', p.name)) for p in backups.iterdir())
        return result

    # ---- calls ---------------------------------------------------------
    def run(self):
        self.build()
        results = []
        for step in self.spec['calls']:
            try:
                value = self.call(step['fn'], step.get('args', {}))
                results.append({'ok': self.out(value)})
            except Exception as error:
                results.append({'err': error_info(error)})
        return dict(spec=self.spec, results=results, tree=self.tree(), db=self.database())

    def cleanup(self):
        try:
            if self.cat is not None:
                self.cat.close()
        except Exception:
            pass
        for patcher in self.patchers:
            patcher.stop()
        shutil.rmtree(self.base, ignore_errors=True)

    def fake_images(self):
        import gen_library_arrays as arrays
        seeds = arrays.seed_images()

        class Fake:
            def __init__(f, payload):
                d = json.loads(payload)
                f.seed, f.size = d['seed'], (d['w'], d['h'])

            def __enter__(f):
                return f

            def __exit__(f, *a):
                return False

        def describe(image):
            return seeds[image.seed].astype(np.float32) / 255, image.size[0] / image.size[1]

        def candidate(path, row, evidence):
            d = json.loads(Path(path).read_bytes())
            return seeds[d['seed']].astype(np.float32) / 255, d['w'] / d['h']
        for patcher in (patch.object(folder_recovery.Image, 'open', lambda f, *a, **k: Fake(f.read())),
                        patch.object(folder_recovery, 'descriptor', describe),
                        patch.object(folder_recovery, 'candidate_descriptor', candidate)):
            patcher.start()
            self.patchers.append(patcher)

    def call(self, fn, a):
        m = getattr(self, 'c_' + fn.replace('.', '_'))
        return m(a)

    # filesystem steps of the harness (not under test)
    def c_fs_write(self, a):
        path = Path(self.p(a['path']))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(a['text'].encode('utf-8'))

    def c_fs_rename(self, a):
        os.rename(self.p(a['from']), self.p(a['to']))

    def c_fs_mkdir(self, a):
        Path(self.p(a['path'])).mkdir(parents=True, exist_ok=True)

    def c_fs_rmdir(self, a):
        shutil.rmtree(self.p(a['path']))

    def c_fs_utime(self, a):
        os.utime(self.p(a['path']), ns=(a['ns'], a['ns']))

    def c_pref(self, a):
        self.cat.save_preference(a['key'], self.deep_in(a['value']))

    def c_fs_snapshot_identities(self, a):
        self.vars[a['name']] = {folders.path_key(self.p(p)): folder_locations.directory_identity(self.p(p)) for p in a['paths']}

    def c_fs_sql(self, a):
        self.cat.db.execute(a['sql'], [self.p(x) for x in a.get('args', [])])
        self.cat.db.commit()

    # library.py
    def c_file_digest(self, a):
        return library.file_digest(self.p(a['path']))

    def c_duplicate_groups(self, a):
        return [[r['id'] for r in group] for group in library.duplicate_groups(self.cat.photos())]

    def c_move_files(self, a):
        return library.move_files(self.cat, {self.p(s): self.p(d) for s, d in a['mapping']})

    def c_relink(self, a):
        return library.relink(self.cat, a['id'], self.p(a['path']))

    def c_move_folder(self, a):
        return library.move_folder(self.cat, self.p(a['source']), self.p(a['destination']))

    def c_relink_folder(self, a):
        return library.relink_folder(self.cat, self.p(a['source']), self.p(a['destination']))

    def c_restore_backup(self, a):
        return library.restore_backup(self.cat, self.base / 'backups_in' / a['name'] if 'name' in a else self.p(a['path']))

    def c_write_sidecar(self, a):
        return library.write_sidecar(a['record'], self.p(a['path']))

    def c_read_sidecar(self, a):
        r = library.read_sidecar(self.p(a['path']))
        return {k: (dict(v) if k == 'settings' else v) for k, v in r.items()}

    def c_read_file(self, a):
        return Path(self.p(a['path'])).read_bytes()

    # file_access.py
    def c_access(self, a):
        return file_access.access(self.p(a['path']), a.get('folder', False))

    def c_present(self, a):
        return file_access.present(self.p(a['path']))

    def c_unreadable_folders(self, a):
        return file_access.unreadable_folders([self.p(x) for x in a['roots']])

    # folder_locations.py
    def c_directory_identity_shape(self, a):
        value = folder_locations.directory_identity(self.p(a['path']))
        return None if value is None else len(value)

    def c_rebase(self, a):
        return folder_locations.rebase(self.p(a['path']), self.p(a['source']), self.p(a['destination']))

    def c_scan_folders(self, a):
        cancel = threading.Event()
        if a.get('cancel'):
            cancel.set()
        identities = {k: {'path': k, 'id': v} for k, v in self.vars.get(a.get('identities', ''), {}).items() if v}
        calls = {'n': 0}

        def monotonic():
            calls['n'] += 1
            return 0.0 if calls['n'] == 1 or a.get('clock') != 'expire' else 100.0
        expanded = {folders.path_key(self.p(x)) for x in a.get('expanded', [])}
        with patch.object(folder_locations, 'monotonic', monotonic):
            r = folder_locations.scan_folders([self.p(x) for x in a['paths']], [self.p(x) for x in a['roots']],
                                              expanded, identities, cancel)
        under = lambda keys: sorted(k for k in keys if k.startswith(folders.path_key(self.root)))
        return {'identities': under(r['identities']), 'missing': under(r['missing']),
                'moves': sorted([list(m) for m in r['moves']]), 'children': r['children'],
                'scanned': sorted(r['scanned']), 'incomplete': r['incomplete']}

    def c_relink_plan(self, a):
        return folder_locations.relink_plan(self.cat, self.p(a['source']), self.p(a['destination']))

    def c_apply_relinks(self, a):
        plans = [folder_locations.relink_plan(self.cat, self.p(s), self.p(d)) for s, d in a['pairs']]
        return folder_locations.apply_relinks(self.cat, plans)

    # folder_sync.py (module functions)
    def c_new_files(self, a):
        known = {folders.path_key(self.p(x)) for x in a.get('known', [])}
        skipped = {}
        for x in a.get('skipped', []):
            info = os.stat(self.p(x))
            skipped[folders.path_key(str(Path(self.p(x)).resolve()))] = (info.st_mtime, info.st_size)
        cancel = threading.Event()
        if a.get('cancel'):
            cancel.set()
        result = folder_sync.new_files([self.p(x) for x in a['roots']], known, skipped, cancel, a.get('settle', 5),
                                       a['now'], [self.p(x) for x in a.get('removed', [])])
        return result

    # FolderSync class with a fake window
    def c_fs_init(self, a):
        from PySide6.QtCore import QObject
        from PySide6.QtWidgets import QApplication
        QApplication.instance() or QApplication([])
        runner = self

        class W(QObject):
            def __init__(w, catalog):
                super().__init__()
                w.catalog = catalog
                w.closing = False
                w.import_busy = False
                w.import_scans = 0
                w.export_running = False
                w.maintenance_running = False
                w.command_running = False
                w.import_cancel = threading.Event()
                w.import_queue = []
                w.import_pending_paths = set()
                w.import_errors = []
                w.messages = []
                w.started = 0

            def spawn(w, job, done, failed, pool=None):
                runner.jobs.append((job, done, failed))

            def statusBar(w):
                return w

            def showMessage(w, text, *rest):
                w.messages.append(text)

            @property
            def import_cancel_button(w):
                class B:
                    def show(self):
                        pass
                return B()

            def import_next(w):
                w.started += 1
        self.host = W(self.cat)
        self.fake_clock.mono = a.get('mono', 1000.0)
        self.fake_clock.epoch = a.get('epoch', 1_800_000_000.0)
        self.patcher = patch.object(folder_sync, 'time', self.fake_clock)
        self.patcher.start()
        folder_sync.OUTPUTS.clear()
        self.fs = folder_sync.FolderSync(self.host)
        return self.fs_state()

    def fs_state(self):
        fs, w = self.fs, self.host
        return {'running': fs.running, 'revision': fs.revision, 'auto': sorted(fs.auto),
                'skipped': {k: list(v) for k, v in sorted(fs.skipped.items())}, 'excluded': sorted(fs.excluded),
                'queue': list(w.import_queue), 'pending': sorted(w.import_pending_paths), 'errors': list(w.import_errors),
                'messages': list(w.messages), 'started': w.started, 'last': fs.last,
                'excluded_pref': self.cat.preference('sync_excluded', None)}

    def c_fs_host(self, a):
        for k, v in a.items():
            if k == 'import_cancelled':
                self.host.import_cancel.set() if v else self.host.import_cancel.clear()
            elif k == 'pending':
                self.host.import_pending_paths = set(self.p(x) for x in v)
            else:
                setattr(self.host, k, v)

    def c_fs_time(self, a):
        if 'mono' in a:
            self.fake_clock.mono = a['mono']
        if 'epoch' in a:
            self.fake_clock.epoch = a['epoch']

    def c_fs_scan(self, a):
        before = len(self.jobs)
        r = self.fs.scan()
        return {'started': r, 'spawned': len(self.jobs) - before, 'state': self.fs_state()}

    def c_fs_run_job(self, a):
        job, done, failed = self.jobs[-1]
        files = job()
        self.vars['files'] = files
        return files

    def c_fs_deliver(self, a):
        job, done, failed = self.jobs[-1]
        done(self.vars['files'] if 'files' not in a else [self.p(x) for x in a['files']])
        return self.fs_state()

    def c_fs_failed(self, a):
        self.jobs[-1][2](Exception('x'))
        return self.fs_state()

    def c_fs_activated(self, a):
        from PySide6.QtCore import Qt
        state = Qt.ApplicationState.ApplicationActive if a['active'] else Qt.ApplicationState.ApplicationInactive
        before = len(self.jobs)
        self.fs.activated(state)
        return {'spawned': len(self.jobs) - before, 'state': self.fs_state()}

    def c_fs_set_enabled(self, a):
        before = len(self.jobs)
        self.fs.set_enabled(a['value'])
        return {'spawned': len(self.jobs) - before, 'enabled': self.fs.enabled(), 'state': self.fs_state()}

    def c_fs_import_failed(self, a):
        return {'result': self.fs.import_failed(self.p(a['path'])), 'state': self.fs_state()}

    def c_fs_imported(self, a):
        self.fs.imported(self.p(a['path']), resolved=a.get('resolved', False))
        return self.fs_state()

    def c_fs_exclude(self, a):
        self.fs.exclude([self.p(x) for x in a['paths']])
        return self.fs_state()

    def c_fs_folder_removed(self, a):
        self.fs.folder_removed()
        return self.fs_state()

    def c_fs_release(self, a):
        self.fs.release()
        return self.fs_state()

    def c_fs_note_output(self, a):
        folder_sync.note_output(self.p(a['path']))

    def c_fs_keep_outputs(self, a):
        self.fs.keep_outputs()
        return self.fs_state()

    def c_fs_shutdown(self, a):
        self.fs.shutdown()
        return {'cancelled': self.fs.cancel.is_set(), 'state': self.fs_state()}

    # import_copy.py
    def c_capture_time(self, a):
        return import_copy.capture_time(self.p(a['path']), a.get('metadata'))

    def c_check_template(self, a):
        return import_copy.check_template(a['template'])

    def c_duplicate_key(self, a):
        return list(import_copy.duplicate_key(self.p(a['path']), a['metadata']))

    def c_plan(self, a):
        table = a.get('meta', {})
        known = {tuple(k) for k in a.get('known', [])}
        read = lambda p: dict(table.get(Path(p).name, {}))
        planned, skipped = import_copy.plan([self.p(x) for x in a['sources']], self.p('$ROOT/dest'),
                                            a.get('subfolders', 'year/date'), a.get('template', '{stem}'), known, read)
        return {'planned': [[s, r.as_posix()] for s, r in planned], 'skipped': skipped}

    def c_free(self, a):
        return import_copy._free(self.p(a['path']))

    def c_execute(self, a):
        planned = [(self.p(s), Path(r)) for s, r in a['planned']]
        cancel = threading.Event()
        seen = []

        def progress(n):
            seen.append(n)
            if a.get('cancel_after') == n:
                cancel.set()
        copied, errors = import_copy.execute(planned, self.p(a['destination']), self.p(a['backup']) if a.get('backup') else None,
                                             cancel, progress)
        return {'copied': copied, 'errors': errors, 'progress': seen}

    def c_known_keys(self, a):
        return sorted([list(k) for k in import_copy.known_keys(self.cat)], key=json.dumps)

    # quick_export.py
    def c_preset(self, a):
        return quick_export.preset(a['kind'])

    def c_previous(self, a):
        with patch('luma.engine.HDR_FEATURE', a.get('hdr', True)):
            return quick_export.previous(self.cat)

    def c_copy_original(self, a):
        r = quick_export.copy_original(self.deep_in(a['record']), self.p(a['destination']))
        return list(r)

    def c_export_concurrency(self, a):
        import luma.render_cache as rc
        with patch('os.cpu_count', lambda: a['cpus']), patch.object(rc, 'physical_memory', lambda: a['memory']):
            return quick_export.export_concurrency(a['records'])

    # catalog_maintenance.py
    def c_optimize(self, a):
        stages = []
        cancel = threading.Event()

        def progress(value):
            stages.append([value['stage'], value['examined'], value['total']])
            if a.get('cancel_at_stage') is not None and value['stage'] == a['cancel_at_stage']:
                cancel.set()
        if a.get('cancel_before'):
            cancel.set()
        self.cat.db.commit()
        before = self.snapshot_tables()
        r = catalog_maintenance.optimize(self.catdir, cancel, progress, batch_size=a.get('batch_size', 16))
        after = self.snapshot_tables()
        warnings = [w.split(': ', 1)[0] + ('|' + w.split(': ', 1)[1] if w.split(': ', 1)[1][:1] >= '\uac00' else '') for w in r['warnings']]
        return {'converted': r['converted'], 'examined': r['examined'], 'total': r['total'], 'removed_assets': r['removed_assets'],
                'profiles': r['profiles'], 'cancelled': r['cancelled'], 'vacuumed': r['vacuumed'], 'warnings': warnings,
                'warning_count': r.get('warning_count', 0), 'has_backup': bool(r['backup']) and Path(r['backup']).exists(),
                'integrity': r['integrity'], 'stages': stages, 'checkpoint_len': len(r['checkpoint']),
                'tables_equal': before == after, 'tables': after}

    def snapshot_tables(self):
        with sqlite3.connect(self.catdir / 'catalog.sqlite') as db:
            out = {}
            for table in ('photos', 'history', 'presets', 'snapshots', 'preferences', 'profile_assets'):
                try:
                    columns = [r[1] for r in db.execute(f'PRAGMA table_info({table})')]
                    if not columns:
                        continue
                    rows = [[v.hex() if isinstance(v, bytes) else v for v in r] for r in
                            db.execute(f'SELECT {",".join(columns)} FROM {table} ORDER BY 1')]
                    if table == 'photos':
                        rows = [[v for k, v in zip(columns, r) if k not in ('imported',)] for r in rows]
                    out[table] = self.out(rows)
                except sqlite3.Error:
                    pass
            return out

    # workflow_data.py
    def c_capture_exposure(self, a):
        return workflow_data.capture_exposure(a['info'])

    def c_match_exposure(self, a):
        return workflow_data.match_exposure(a['reference'], a['target'])

    # folder_recovery.py
    def c_missing_directory(self, a):
        return folder_recovery.missing_directory(self.p(a['path']))

    def c_write_previews(self, a):
        thumbs = self.catdir / 'thumbnails'
        thumbs.mkdir(exist_ok=True)
        for item in a['items']:
            row = self.cat.db.execute('SELECT path FROM photos WHERE id=?', (item['id'],)).fetchone()
            token = self.cat.db.execute('SELECT token FROM preview_state WHERE photo_id=?', (item['id'],)).fetchone()
            payload = json.dumps({'seed': item['seed'], 'w': item.get('w', 96), 'h': item.get('h', 64)}).encode()
            (thumbs / f"{item['id']}.jpg").write_bytes(payload)
            if not item.get('legacy'):
                stamp = {'version': preview_store.VERSION, 'path': row[0], 'token': token[0] if token else None,
                         'sha256': hashlib.sha256(payload).hexdigest(), 'color_space': 'sRGB'}
                (thumbs / f"{item['id']}.json").write_text(json.dumps(stamp), encoding='utf-8')

    def c_recover(self, a):
        self.cat.db.commit()
        cancel = threading.Event()
        if a.get('cancel'):
            cancel.set()
        sources = [self.p(x) for x in a['sources']] if a.get('sources') else None
        r = folder_recovery.recover_folders(self.catdir, cancel, sources=sources, timeout=a.get('timeout', 120))
        self.vars['plans'] = r['plans']
        plans = []
        for p in r['plans']:
            parent = Path(p['destination']).parent
            plans.append({'source': p['source'], 'destination': p['destination'], 'updates': [list(u) for u in p['updates']],
                          'found': p['found'], 'missing': p['missing'], 'physical': p['physical'], 'proof': p['proof'],
                          'anchors': p['anchors'], 'collision': p['collision'], 'expected_paths': p['expected_paths'],
                          'guards': [[g[0], g[1]] for g in p['guards']],
                          'identity_ok': p['identity'] == folder_locations.directory_identity(p['destination']),
                          'parent_mtime_ok': p['parent_mtime_ns'] == parent.stat().st_mtime_ns})
        return {'plans': plans, 'unresolved': r['unresolved'], 'total': r['total'], 'incomplete': r['incomplete'],
                'cancelled': r['cancelled']}

    def c_validate(self, a):
        self.cat.db.commit()
        return folder_recovery.validate_recovery(self.cat, self.vars['plans'])


def scenario(name, spec):
    spec.setdefault('name', name)
    runner = Runner(spec)
    try:
        record = runner.run()
    finally:
        runner.cleanup()
        folder_sync.OUTPUTS.clear()
        if getattr(runner, 'patcher', None):
            runner.patcher.stop()
    folder = OUT / name
    folder.mkdir(parents=True, exist_ok=True)
    # insertion order matters (stored JSON text depends on it): no sort_keys here
    (folder / 'case.json').write_text(json.dumps(record, ensure_ascii=False, indent=1) + chr(10), encoding='utf-8', newline=chr(10))
    return record


def bad_json_edit():
    return 'not json'


from gen_library_cases import all_scenarios  # noqa: E402

if __name__ == '__main__':
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    count = 0
    for name, spec in all_scenarios():
        scenario(name, spec)
        count += 1
    import gen_library_arrays
    gen_library_arrays.main()
    print('wrote', count, 'scenarios to', OUT)
