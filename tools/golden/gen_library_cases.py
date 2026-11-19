"""The scripted scenarios of gen_library.py (data only)."""
import base64
import hashlib
import json

NS = 1_000_000_000
BASE = 1_800_000_000 * NS           # 2027-01-15 08:00:00 UTC
NOON = 1_800_014_400 * NS           # 2027-01-15 12:00:00 UTC


def text(s):
    return {'text': s}


def fill(byte, size):
    return {'fill': [byte, size]}


def photos(*paths):
    return [{'path': p} for p in paths]


SETTINGS = {'exposure': 0.5, 'film_look': 'Portra 400', 'note': '한글 "quote" <tag> & more'}

XMP_FOREIGN = """<?xpacket begin="\ufeff" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 5.6-c140">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <!-- a comment that ElementTree drops -->
    <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+0.50" crs:HasCrop="False">
      <crs:ToneCurve><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurve>
    </rdf:Description>
    <rdf:Description rdf:about="" xmlns:luma="https://luma.local/ns/1.0/" luma:Settings="{}" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="1"/>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"""

XMP_NO_RDF = '<x:xmpmeta xmlns:x="adobe:ns:meta/"><other>keep &amp; me</other></x:xmpmeta>'

XMP_READ = """<?xml version="1.0" encoding="utf-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:lr="http://ns.adobe.com/lightroom/1.0/" xmlns:luma="https://luma.local/ns/1.0/">
<rdf:RDF><rdf:Description xmp:Rating="9" xmp:Label="Red &amp; &#10;Blue" luma:Settings='{"exposure": 1.5, "negative": true}' luma:Metadata='{"caption": "눈"}'>
<dc:subject><rdf:Bag><rdf:li>plain</rdf:li></rdf:Bag></dc:subject>
<lr:hierarchicalSubject><rdf:Bag><rdf:li>a|b</rdf:li><rdf:li>c</rdf:li><rdf:li>a|b</rdf:li><rdf:li/></rdf:Bag></lr:hierarchicalSubject>
</rdf:Description></rdf:RDF></x:xmpmeta>"""

XMP_READ2 = """<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/">
<rdf:RDF><rdf:Description xmp:Rating="-3"><dc:subject><rdf:Bag><rdf:li>one</rdf:li><rdf:li>two</rdf:li></rdf:Bag></dc:subject></rdf:Description>
<rdf:Description xmp:Label="Second"/></rdf:RDF></x:xmpmeta>"""

XMP_READ_BAD_RATING = '<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:xmp="http://ns.adobe.com/xap/1.0/"><rdf:RDF><rdf:Description xmp:Rating="x"/></rdf:RDF></x:xmpmeta>'
XMP_DOCTYPE = '<?xml version="1.0"?><!DOCTYPE foo [<!ENTITY x "y">]><x:xmpmeta xmlns:x="adobe:ns:meta/"/>'
XMP_BROKEN = '<x:xmpmeta xmlns:x="adobe:ns:meta/"><oops></x:xmpmeta>'

RECORD = {'rating': 4, 'label': 'Red\nline\tTab & <x> "q"', 'settings': SETTINGS, 'user_metadata': {'caption': '눈 ☃'},
          'keywords': 'sky, 한글 , ,Straße & co,<tag>'}
RECORD_EMPTY = {'rating': 0, 'settings': {}, 'keywords': ''}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def profile(data, name='Test'):
    return {'name': name, 'sha256': sha(data), 'data': base64.b64encode(data).decode()}


def all_scenarios():
    yield from library_cases()
    yield from location_cases()
    yield from sync_cases()
    yield from import_cases()
    yield from export_cases()
    yield from maintenance_cases()
    yield from workflow_cases()
    yield from recovery_cases()


def library_cases():
    yield 'duplicates', dict(setup=dict(
        files={'a.jpg': fill(65, 10), 'b.jpg': fill(65, 10), 'c.jpg': fill(66, 10), 'd.jpg': fill(65, 10),
               'e.jpg': fill(65, 11), 'sub/f.jpg': fill(66, 10), 'solo.jpg': fill(1, 3)},
        photos=photos('$ROOT/a.jpg', '$ROOT/b.jpg', '$ROOT/c.jpg', '$ROOT/d.jpg', '$ROOT/e.jpg', '$ROOT/sub/f.jpg',
                      '$ROOT/missing.jpg', '$ROOT/solo.jpg') + [{'virtual_of': 1}],
        db=[["INSERT INTO photos(path,name,edits,metadata,imported) VALUES(?,?,?,?,?)", ['$ROOT/A.JPG', 'A.JPG', '{}', '{}', 'x']]]),
        calls=[dict(fn='duplicate_groups'), dict(fn='file_digest', args=dict(path='$ROOT/a.jpg')),
               dict(fn='file_digest', args=dict(path='$ROOT/missing.jpg'))])

    yield 'move_files', dict(setup=dict(
        files={'a.jpg': fill(65, 10), 'b.jpg': fill(66, 12), 'c.jpg': fill(67, 5), '사진.jpg': fill(68, 6)},
        dirs=['dest', 'dest2'],
        photos=photos('$ROOT/a.jpg', '$ROOT/b.jpg', '$ROOT/c.jpg', '$ROOT/사진.jpg') + [{'virtual_of': 1}]),
        calls=[
            dict(fn='move_files', args=dict(mapping=[['$ROOT/a.jpg', '$ROOT/dest/a_moved.jpg'], ['$ROOT/b.jpg', '$ROOT/dest/한글 이름.jpg']])),
            dict(fn='move_files', args=dict(mapping=[['$ROOT/c.jpg', '$ROOT/c.jpg']])),
            dict(fn='move_files', args=dict(mapping=[['$ROOT/c.jpg', '$ROOT/dest/a_moved.jpg']])),
            dict(fn='move_files', args=dict(mapping=[['$ROOT/nothing.jpg', '$ROOT/dest/n.jpg']])),
            dict(fn='move_files', args=dict(mapping=[['$ROOT/c.jpg', '$ROOT/nowhere/c.jpg']])),
            dict(fn='move_files', args=dict(mapping=[['$ROOT/c.jpg', '$ROOT/dest2/X.jpg'], ['$ROOT/사진.jpg', '$ROOT/dest2/x.jpg']])),
            dict(fn='move_files', args=dict(mapping=[['$ROOT/사진.jpg', '$ROOT/dest2/그림.JPG']])),
            dict(fn='move_files', args=dict(mapping=[['$ROOT/c.jpg', '$ROOT/C.JPG']])),
            dict(fn='move_files', args=dict(mapping=[])),
        ])

    yield 'move_files_rollback', dict(setup=dict(
        files={'a.jpg': fill(65, 10), 'b.jpg': fill(66, 12)}, dirs=['dest'], photos=photos('$ROOT/a.jpg', '$ROOT/b.jpg'),
        db=[["CREATE TRIGGER boom BEFORE UPDATE OF path ON photos BEGIN SELECT RAISE(ABORT,'boom'); END", []]]),
        calls=[dict(fn='move_files', args=dict(mapping=[['$ROOT/a.jpg', '$ROOT/dest/a.jpg'], ['$ROOT/b.jpg', '$ROOT/dest/b.jpg']]))])

    yield 'relink', dict(setup=dict(
        files={'new/a.jpg': fill(65, 4), 'new/b.PNG': fill(66, 4), 'new/notes.txt': fill(67, 4), 'new/taken.jpg': fill(68, 4)},
        photos=photos('$ROOT/old/a.jpg', '$ROOT/old/b.jpg', '$ROOT/new/taken.jpg') + [{'virtual_of': 1}]),
        calls=[
            dict(fn='relink', args=dict(id=1, path='$ROOT/new/a.jpg')),
            dict(fn='relink', args=dict(id=2, path='$ROOT/new/notes.txt')),
            dict(fn='relink', args=dict(id=2, path='$ROOT/new/nothing.jpg')),
            dict(fn='relink', args=dict(id=99, path='$ROOT/new/b.PNG')),
            dict(fn='relink', args=dict(id=2, path='$ROOT/new/b.PNG')),
            dict(fn='relink', args=dict(id=2, path='$ROOT/new/taken.jpg')),
        ])

    tree = {'pics/a.jpg': fill(65, 3), 'pics/2026/b.jpg': fill(66, 4), 'pics/2026/raw/c.nef': fill(67, 5), 'other/z.jpg': fill(68, 6),
            'pics/한글 폴더/d.jpg': fill(69, 7)}
    yield 'move_folder', dict(setup=dict(
        files=tree, dirs=['pics/2026/raw/cache'],
        photos=photos('$ROOT/pics/a.jpg', '$ROOT/pics/2026/b.jpg', '$ROOT/pics/2026/raw/c.nef', '$ROOT/other/z.jpg',
                      '$ROOT/pics/한글 폴더/d.jpg', '$ROOT/pics/ghost.jpg') + [{'virtual_of': 2}],
        prefs={'removed_folders': ['$ROOT/pics/2026/raw/cache', '$ROOT/elsewhere']}),
        calls=[
            dict(fn='move_folder', args=dict(source='$ROOT/pics', destination='$ROOT/archive/pics2')),
            dict(fn='move_folder', args=dict(source='$ROOT/archive/pics2', destination='$ROOT/other')),
            dict(fn='move_folder', args=dict(source='$ROOT/archive/pics2', destination='$ROOT/archive/pics2/inner')),
            dict(fn='move_folder', args=dict(source='$ROOT/archive/pics2', destination='$ROOT/archive/pics2')),
            dict(fn='move_folder', args=dict(source='$ROOT/nothing', destination='$ROOT/x')),
            dict(fn='move_folder', args=dict(source='$ANCHOR', destination='$ROOT/x')),
            dict(fn='move_folder', args=dict(source='$ROOT/archive/pics2/2026', destination='$ROOT/Moved2026')),
        ])

    yield 'sidecars', dict(setup=dict(
        files={'foreign.xmp': text(XMP_FOREIGN), 'nordf.xmp': text(XMP_NO_RDF), 'read1.xmp': text(XMP_READ), 'read2.xmp': text(XMP_READ2),
               'badrating.xmp': text(XMP_READ_BAD_RATING), 'doctype.xmp': text(XMP_DOCTYPE), 'broken.xmp': text(XMP_BROKEN),
               'utf16.xmp': {'text16': XMP_READ2}, 'old.xmp': text('<?xml version=\'1.0\' encoding=\'utf-8\'?>\n<x:xmpmeta xmlns:x="adobe:ns:meta/" />')}),
        calls=[
            dict(fn='write_sidecar', args=dict(record=RECORD, path='$ROOT/new.xmp')),
            dict(fn='read_file', args=dict(path='$ROOT/new.xmp')),
            dict(fn='write_sidecar', args=dict(record=RECORD, path='$ROOT/new.xmp')),
            dict(fn='read_file', args=dict(path='$ROOT/new.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/new.xmp')),
            dict(fn='write_sidecar', args=dict(record=RECORD_EMPTY, path='$ROOT/empty.xmp')),
            dict(fn='read_file', args=dict(path='$ROOT/empty.xmp')),
            dict(fn='write_sidecar', args=dict(record=RECORD, path='$ROOT/foreign.xmp')),
            dict(fn='read_file', args=dict(path='$ROOT/foreign.xmp')),
            dict(fn='write_sidecar', args=dict(record=RECORD_EMPTY, path='$ROOT/nordf.xmp')),
            dict(fn='read_file', args=dict(path='$ROOT/nordf.xmp')),
            dict(fn='write_sidecar', args=dict(record=RECORD, path='$ROOT/old.xmp')),
            dict(fn='read_file', args=dict(path='$ROOT/old.xmp')),
            dict(fn='write_sidecar', args=dict(record=RECORD, path='$ROOT/broken.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/read1.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/read2.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/utf16.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/badrating.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/doctype.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/broken.xmp')),
            dict(fn='read_sidecar', args=dict(path='$ROOT/none.xmp')),
        ])

    yield 'restore_backup', dict(setup=dict(
        files={'a.jpg': fill(1, 1)}, photos=photos('$ROOT/a.jpg'), prefs={'mine': {'x': 1}},
        other={'good': dict(photos=photos('$ROOT/b.jpg', '$ROOT/c.jpg', '$ROOT/한글.jpg'), prefs={'theirs': ['한글', 2]}),
               'dangling': dict(photos=photos('$ROOT/d.jpg'),
                                db=[["UPDATE photos SET edits=? WHERE id=1",
                                     [json.dumps({'dcp_profile': {'name': 'x', 'sha256': 'ab' * 32, 'data_ref': 'ab' * 32}})]]])}),
        dump_tables=['history'],
        calls=[
            dict(fn='restore_backup', args=dict(name='dangling.sqlite')),
            dict(fn='restore_backup', args=dict(path='$ROOT/a.jpg')),
            dict(fn='restore_backup', args=dict(name='good.sqlite')),
        ])


def location_cases():
    yield 'access', dict(setup=dict(files={'a.jpg': fill(1, 1)}, dirs=['sub']),
                         calls=[dict(fn='access', args=dict(path='$ROOT/a.jpg')),
                                dict(fn='access', args=dict(path='$ROOT/sub')),
                                dict(fn='access', args=dict(path='$ROOT/sub', folder=True)),
                                dict(fn='access', args=dict(path='$ROOT/a.jpg', folder=True)),
                                dict(fn='access', args=dict(path='$ROOT/missing.jpg')),
                                dict(fn='present', args=dict(path='$ROOT/a.jpg')),
                                dict(fn='present', args=dict(path='$ROOT/sub')),
                                dict(fn='unreadable_folders', args=dict(roots=['$ROOT/sub', '$ROOT/missing'])),
                                dict(fn='directory_identity_shape', args=dict(path='$ROOT/sub')),
                                dict(fn='directory_identity_shape', args=dict(path='$ROOT/a.jpg')),
                                dict(fn='directory_identity_shape', args=dict(path='$ROOT/missing')),
                                dict(fn='rebase', args=dict(path='$ROOT/old/sub/x', source='$ROOT/old', destination='$ROOT/new')),
                                dict(fn='rebase', args=dict(path='$ROOT/old', source='$ROOT/old', destination='$ROOT/new')),
                                dict(fn='rebase', args=dict(path='$ROOT/OLD/Sub', source='$ROOT/old', destination='$ROOT/new')),
                                dict(fn='rebase', args=dict(path='$ROOT/other/sub', source='$ROOT/old', destination='$ROOT/new'))]), \

    lib = {'lib/2024/x.jpg': fill(1, 1), 'lib/2025/y.jpg': fill(2, 1), 'lib/2025/raw/z.nef': fill(3, 1), 'lib/readme.txt': fill(4, 1)}
    snap = dict(fn='fs_snapshot_identities', args=dict(name='snap', paths=['$ROOT/lib/2024', '$ROOT/lib/2025', '$ROOT/lib/2025/raw', '$ROOT/lib']))
    paths = ['$ROOT/lib/2024', '$ROOT/lib/2025/raw', '$ROOT/lib/2025', '$ROOT/gone/deep']
    yield 'scan_folders', dict(setup=dict(files=lib), calls=[
        snap,
        dict(fn='scan_folders', args=dict(paths=paths, roots=['$ROOT/lib'], expanded=[], identities='snap')),
        dict(fn='fs_rename', args={'from': '$ROOT/lib/2025', 'to': '$ROOT/lib/2025-renamed'}),
        dict(fn='scan_folders', args=dict(paths=paths, roots=['$ROOT/lib'], expanded=[], identities='snap')),
        dict(fn='scan_folders', args=dict(paths=paths, roots=['$ROOT/lib'], expanded=[], identities='snap', cancel=True)),
        dict(fn='scan_folders', args=dict(paths=paths, roots=['$ROOT/lib'], expanded=[], identities='snap', clock='expire')),
        dict(fn='scan_folders', args=dict(paths=['$ROOT/lib/2024'], roots=[], expanded=['$ROOT/lib/2024'], identities='snap')),
        dict(fn='scan_folders', args=dict(paths=['$ROOT/lib/2024'], roots=[], expanded=['$ROOT/lib'], identities='snap')),
        dict(fn='fs_mkdir', args=dict(path='$ROOT/lib/2025-renamed/twin')),
        dict(fn='fs_rename', args={'from': '$ROOT/lib/2025-renamed', 'to': '$ROOT/lib/2026-again'}),
        dict(fn='scan_folders', args=dict(paths=['$ROOT/lib/2025'], roots=['$ROOT/lib'], expanded=[], identities='snap')),
    ])

    spec = dict(setup=dict(
        files={'new/a.jpg': fill(1, 1), 'new/sub/b.jpg': fill(2, 1), 'keep/k.jpg': fill(3, 1), 'new2/a.jpg': fill(4, 1)},
        photos=photos('$ROOT/old/a.jpg', '$ROOT/old/sub/b.jpg', '$ROOT/old/sub/gone.jpg', '$ROOT/keep/k.jpg') + [{'virtual_of': 1}],
        prefs={'folder_panel': {'selected': '$ROOT/old/sub', 'expanded': ['$ROOT/old', '$ROOT/keep', '$ROOT/old/sub'], 'width': 3},
               'watch_folders': ['$ROOT/old/sub', '$ROOT/keep'],
               'folder_identities': {'$KEY:$ROOT/old/sub': {'path': '$ROOT/old/sub', 'id': [1, 2, 3]},
                                     '$KEY:$ROOT/keep': {'path': '$ROOT/keep', 'id': [4, 5, 6]}},
               'removed_folders': ['$ROOT/old/sub/cache', '$ROOT/trash']}),
        calls=[
            dict(fn='relink_plan', args=dict(source='$ROOT/old', destination='$ROOT/new')),
            dict(fn='relink_folder', args=dict(source='$ROOT/old', destination='$ROOT/new')),
            dict(fn='relink_folder', args=dict(source='$ROOT/new', destination='$ROOT/new')),
            dict(fn='relink_folder', args=dict(source='$ROOT/new', destination='$ROOT/NEW')),
            dict(fn='relink_folder', args=dict(source='$ROOT/new', destination='$ROOT/new/sub')),
            dict(fn='relink_folder', args=dict(source='$ROOT/new/sub', destination='$ROOT/new')),
            dict(fn='relink_folder', args=dict(source='$ROOT/new', destination='$ROOT/not-there')),
            dict(fn='relink_folder', args=dict(source='$ANCHOR', destination='$ROOT/new2')),
            dict(fn='relink_folder', args=dict(source='$ROOT/new', destination='$ROOT/keep')),
            dict(fn='relink_folder', args=dict(source='$ROOT/new', destination='$ROOT/new2')),
        ])
    yield 'relink_folder', spec

    yield 'relink_folders_batch', dict(setup=dict(
        files={'n1/a.jpg': fill(1, 1), 'n2/b.jpg': fill(2, 1), 'n2/deep/c.jpg': fill(3, 1)},
        photos=photos('$ROOT/o1/a.jpg', '$ROOT/o2/b.jpg', '$ROOT/o2/deep/c.jpg'),
        prefs={'folder_panel': {'expanded': ['$ROOT/o2']}, 'removed_folders': ['$ROOT/o1/x', '$ROOT/o2/deep']}),
        calls=[dict(fn='apply_relinks', args=dict(pairs=[['$ROOT/o1', '$ROOT/n1'], ['$ROOT/o2', '$ROOT/n2']]))])

    yield 'relink_collision', dict(setup=dict(
        files={'n/a.jpg': fill(1, 1)}, photos=photos('$ROOT/o/a.jpg', '$ROOT/n/a.jpg')),
        calls=[dict(fn='relink_plan', args=dict(source='$ROOT/o', destination='$ROOT/n'))])


def sync_cases():
    files = {'lib/a.jpg': fill(1, 2), 'lib/B.NEF': fill(2, 2), 'lib/c.txt': fill(3, 2), 'lib/sub/d.png': fill(4, 2),
             'lib/sub/e.tiff': fill(5, 2), 'lib/cache/f.jpg': fill(6, 2), 'lib/.hidden.jpg': fill(7, 2), 'lib/noext': fill(8, 2),
             'lib/sub/UPPER.JPG': fill(9, 2), 'lib/fresh.jpg': fill(10, 2)}
    mtimes = {'lib/a.jpg': BASE, 'lib/B.NEF': BASE + 10 * NS, 'lib/sub/d.png': BASE + 20 * NS, 'lib/sub/e.tiff': BASE + 30 * NS,
              'lib/cache/f.jpg': BASE + 40 * NS, 'lib/sub/UPPER.JPG': BASE + 20 * NS, 'lib/fresh.jpg': BASE + 100 * NS,
              'lib/.hidden.jpg': BASE, 'lib/noext': BASE, 'lib/c.txt': BASE}
    now = 1_800_000_000 + 102
    yield 'new_files', dict(setup=dict(files=files, mtimes=mtimes), calls=[
        dict(fn='new_files', args=dict(roots=['$ROOT/lib'], now=now)),
        dict(fn='new_files', args=dict(roots=['$ROOT/lib'], now=now, known=['$ROOT/lib/a.jpg'], removed=['$ROOT/lib/cache'])),
        dict(fn='new_files', args=dict(roots=['$ROOT/lib'], now=now, skipped=['$ROOT/lib/sub/e.tiff'], settle=1)),
        dict(fn='fs_utime', args=dict(path='$ROOT/lib/sub/e.tiff', ns=BASE + 35 * NS)),
        dict(fn='new_files', args=dict(roots=['$ROOT/lib'], now=now, skipped=['$ROOT/lib/sub/e.tiff'], settle=1)),
        dict(fn='new_files', args=dict(roots=['$ROOT/lib'], now=now, settle=1000)),
        dict(fn='new_files', args=dict(roots=['$ROOT/lib/cache'], now=now, removed=['$ROOT/lib/cache'])),
        dict(fn='new_files', args=dict(roots=['$ROOT/lib', '$ROOT/nothing', '$ROOT/lib/sub'], now=now, settle=0)),
        dict(fn='new_files', args=dict(roots=['$ROOT/lib'], now=now, cancel=True)),
    ])

    steps = [
        dict(fn='fs_init'),
        dict(fn='fs_scan'),
        dict(fn='fs_scan'),
        dict(fn='fs_run_job', args=dict()),
        dict(fn='fs_deliver'),
        dict(fn='fs_scan'),
        dict(fn='fs_host', args=dict(import_busy=True)),
        dict(fn='fs_scan'),
        dict(fn='fs_host', args=dict(import_busy=False, export_running=True)),
        dict(fn='fs_scan'),
        dict(fn='fs_host', args=dict(export_running=False, import_cancelled=True)),
        dict(fn='fs_scan'),
        dict(fn='fs_host', args=dict(import_cancelled=False)),
        dict(fn='fs_failed'),
        dict(fn='fs_scan'),
        dict(fn='fs_folder_removed'),
        dict(fn='fs_run_job'),
        dict(fn='fs_deliver'),
        dict(fn='fs_scan'),
        dict(fn='fs_run_job'),
        dict(fn='fs_deliver'),
        dict(fn='fs_import_failed', args=dict(path='$ROOT/lib/a.jpg')),
        dict(fn='fs_import_failed', args=dict(path='$ROOT/lib/a.jpg')),
        dict(fn='fs_import_failed', args=dict(path='$ROOT/lib/sub/d.png')),
        dict(fn='fs_imported', args=dict(path='$ROOT/lib/B.NEF')),
        dict(fn='fs_exclude', args=dict(paths=['$ROOT/lib/sub/e.tiff', '$ROOT/lib/한글.jpg'])),
        dict(fn='fs_imported', args=dict(path='$ROOT/lib/한글.jpg')),
        dict(fn='fs_release'),
        dict(fn='fs_host', args=dict(import_scans=2)),
        dict(fn='fs_release'),
        dict(fn='fs_host', args=dict(import_scans=0)),
        dict(fn='fs_release'),
        dict(fn='fs_note_output', args=dict(path='$ROOT/lib/export.jpg')),
        dict(fn='fs_note_output', args=dict(path='$ROOT/elsewhere/out.jpg')),
        dict(fn='fs_keep_outputs'),
        dict(fn='fs_set_enabled', args=dict(value=False)),
        dict(fn='fs_scan'),
        dict(fn='fs_set_enabled', args=dict(value=True)),
        dict(fn='fs_time', args=dict(mono=1010.0)),
        dict(fn='fs_activated', args=dict(active=True)),
        dict(fn='fs_time', args=dict(mono=1100.0)),
        dict(fn='fs_activated', args=dict(active=False)),
        dict(fn='fs_activated', args=dict(active=True)),
        dict(fn='fs_shutdown'),
    ]
    steps = [dict(s, args=dict(s.get('args', {}), **({'now': 1_800_000_000 + 102} if s['fn'] == 'fs_run_job' else {}))) for s in steps]
    yield 'folder_sync', dict(setup=dict(files=dict(files, **{'lib/export.jpg': fill(11, 2)}), mtimes=dict(mtimes, **{'lib/export.jpg': BASE}),
                                         photos=photos('$ROOT/lib/a.jpg')), calls=steps)


TEMPLATES = ['{stem}', '{date}_{time}_{n:04d}', '{camera}-{stem}', '{stem[0]}', '{stem!s:>10}', '{n:x}', '{stem:.3}', '{n:+d}', '{n:#06x}',
             '{n:,}', '{stem!r}', '{{stem}}', '{stem:*^12}', '{date:>12}{time}', '', '   ', '{foo}', '{0}', '{}', '{stem', 'a}b', 'x{', '{stem}{{',
             '{stem}/x', '{stem}.', ' {stem}', '{stem:', '{stem!x}', '{n:s}', '{stem:d}', '{stem.upper}', '{stem[a]}', '{n[0]}', '{stem[9]}',
             '{stem!}', '{stem!rr}', '{stem{x}}', '{n:{stem}}', '{stem:=10}', '{stem:+}', '{stem:#}', '{stem:,}', '{stem:010}', '{n:.2}', '{stem:é}',
             '{stem}:{n}', '{stem}?', '{stem}"', '{stem}|', '{stem}<', '{stem}\\', '{camera}', '{date}{time}{n}', '{stem:{n}}', '{stem[}']


def import_cases():
    yield 'import_templates', dict(calls=[dict(fn='check_template', args=dict(template=t)) for t in TEMPLATES]
                                   + [dict(fn='duplicate_key', args=dict(path='C:/x/Photo.JPG', metadata=m)) for m in [
                                       {'date': '2026:11:13 10:00:00', 'filename': 'IMG_1.JPG'}, {'size_mb': 12.345}, {'size_mb': '1.005'},
                                       {'size_mb': 0}, {'size_mb': -0.0}, {}, {'size_mb': None}, {'size_mb': 'abc'}, {'size_mb': [1]},
                                       {'date': '', 'size_mb': 3}, {'date': 20261113}, {'filename': '', 'size_mb': 2.5}, {'size_mb': 2.675},
                                       {'size_mb': 1e-05}, {'size_mb': True}, {'filename': 'D/E/f.NEF', 'date': 'x'}]])

    files = {'src/a.jpg': fill(1, 100), 'src/b.NEF': fill(2, 200), 'src/c.JPG': fill(3, 300), 'src/d.png': fill(4, 400), 'src/e.tif': fill(5, 1_500_000)}
    mtimes = {'src/a.jpg': NOON, 'src/b.NEF': NOON + 5 * NS, 'src/c.JPG': NOON + 3600 * NS, 'src/d.png': NOON, 'src/e.tif': NOON + 7200 * NS}
    meta = {'a.jpg': {'date': '2026:11:13 10:20:30', 'camera': 'X/100:V "Pro"'}, 'b.NEF': {'date': '2026:11:13 09:00:00', 'camera': ''},
            'c.JPG': {'date': 'broken'}, 'd.png': {'camera': 'Cam<>'}, 'e.tif': {'date': '2025:01:02 03:04:05.999', 'camera': None}}
    srcs = ['$ROOT/src/a.jpg', '$ROOT/src/b.NEF', '$ROOT/src/c.JPG', '$ROOT/src/d.png', '$ROOT/src/e.tif']
    calls = [dict(fn='capture_time', args=dict(path='$ROOT/src/a.jpg', metadata=m)) for m in
             [None, {}, {'date': '2026:11:13 10:20:30'}, {'date': '2026:11:13 10:20:30.5xyz'}, {'date': '2026:1:3 4:5:6'}, {'date': '2026:11:13T10:20:30'},
              {'date': '2026-11-13 10:20:30'}, {'date': '2026:02:30 10:20:30'}, {'date': '2026:11:13 10:20:61'}, {'date': 20261113}, {'date': '0000:01:01 00:00:00'},
              {'date': '2026:11:13  10:20:30'}, {'date': ' 2026:11:13 10:20:30'}, {'date': '2026:11: 3 10:20:30'}, {'date': '2026:11:13 24:00:00'}]]
    calls.append(dict(fn='capture_time', args=dict(path='$ROOT/src/none.jpg', metadata={})))
    calls.append(dict(fn='capture_time', args=dict(path='$ROOT/src/c.JPG', metadata={})))
    for sub in ('year/date', 'date', 'none'):
        calls.append(dict(fn='plan', args=dict(sources=srcs, meta=meta, subfolders=sub, template='{date}_{time}_{n:03d}_{camera}_{stem}')))
    calls += [dict(fn='plan', args=dict(sources=srcs, meta=meta, template='{stem}', known=[['a.jpg', '2026:11:13 10:20:30'], ['d.png', 0.0], ['b.nef', '2026:11:13 09:00:00']])),
              dict(fn='plan', args=dict(sources=srcs[:2], meta=meta, subfolders='weekly')),
              dict(fn='plan', args=dict(sources=srcs[:2], meta=meta, template='{bad}')),
              dict(fn='plan', args=dict(sources=['$ROOT/src/none.jpg'], meta=meta)),
              dict(fn='plan', args=dict(sources=[], meta=meta))]
    yield 'import_plan', dict(setup=dict(files=files, mtimes=mtimes), calls=calls)

    files = {'src/a.jpg': fill(1, 100), 'src/a.xmp': fill(9, 20), 'src/b.jpg': fill(2, 5_000_000), 'src/c.NEF': fill(3, 7), 'src/d.jpg': fill(4, 9),
             'dest/2026/a.jpg': fill(7, 3), 'dest/2026/a-1.jpg': fill(7, 3), 'dest/2026/a.xmp': fill(8, 3), 'back/2026/c.nef': fill(6, 1)}
    planned = [['$ROOT/src/a.jpg', '2026/a.jpg'], ['$ROOT/src/b.jpg', '2026/2026-11-13/b.jpg'], ['$ROOT/src/missing.jpg', '2026/m.jpg'],
               ['$ROOT/src/c.NEF', '2026/c.nef'], ['$ROOT/src/d.jpg', '2026/d.jpg']]
    yield 'import_execute', dict(setup=dict(files=files), calls=[
        dict(fn='free', args=dict(path='$ROOT/dest/2026/a.jpg')),
        dict(fn='free', args=dict(path='$ROOT/dest/2026/new.jpg')),
        dict(fn='free', args=dict(path='$ROOT/dest/2026/.x')),
        dict(fn='execute', args=dict(planned=planned, destination='$ROOT/dest', backup='$ROOT/back')),
        dict(fn='execute', args=dict(planned=planned[:2], destination='$ROOT/dest2', cancel_after=1)),
        dict(fn='execute', args=dict(planned=[], destination='$ROOT/dest3')),
        dict(fn='execute', args=dict(planned=planned[3:], destination='$ROOT/dest4', backup='$ROOT/dest4')),
    ])

    yield 'import_known_keys', dict(setup=dict(
        files={'a.jpg': fill(1, 1), 'b.jpg': fill(2, 1)},
        photos=[{'path': '$ROOT/a.jpg', 'metadata': {'date': '2026:11:13 10:00:00', 'filename': 'IMG_A.JPG', 'size_mb': 1}},
                {'path': '$ROOT/b.jpg', 'metadata': {'size_mb': 2.346}}, {'path': '$ROOT/c.jpg', 'metadata': {}},
                {'path': '$ROOT/d.jpg', 'metadata': {'size_mb': 'abc'}}, {'virtual_of': 1}]),
        calls=[dict(fn='known_keys')])


def export_cases():
    prefs_ok = {'folder': 'D:/out', 'format': 'JPEG', 'quality': 90, 'longest': 2048, 'color_space': 'sRGB', 'keep_metadata': True, 'naming': '{stem}_{n}'}
    calls = [dict(fn='preset', args=dict(kind=k)) for k in ('small', 'large', 'original', 'huge', '')]
    variants = [prefs_ok, dict(prefs_ok, grayscale=False, dither=False), dict(prefs_ok, quality=39), dict(prefs_ok, quality=100.0),
                dict(prefs_ok, quality=True), dict(prefs_ok, format='GIF'), dict(prefs_ok, format='TIFF HDR 32-bit'), dict(prefs_ok, longest=30001),
                dict(prefs_ok, longest=-1), dict(prefs_ok, color_space='XYZ'), dict(prefs_ok, keep_metadata=1), dict(prefs_ok, naming='{foo}'),
                dict(prefs_ok, naming='a/b'), dict(prefs_ok, naming='{stem}{n:d}'), dict(prefs_ok, naming='   '), dict(prefs_ok, naming='{stem'),
                dict(prefs_ok, folder=''), dict(prefs_ok, grayscale='yes'), dict(prefs_ok, naming='{stem}?'), dict(prefs_ok, naming='.'), dict(prefs_ok, naming=5),
                {k: v for k, v in prefs_ok.items() if k != 'naming'}, [1, 2], 'text', None, dict(prefs_ok, naming='..'), dict(prefs_ok, naming=' x ')]
    for v in variants:
        calls.append(dict(fn='pref', args=dict(key='previous_photo_export', value=v)))
        calls.append(dict(fn='previous', args=dict(hdr=False)))
    calls += [dict(fn='pref', args=dict(key='previous_photo_export', value=dict(prefs_ok, format='TIFF HDR 32-bit'))), dict(fn='previous', args=dict(hdr=True))]
    yield 'quick_export_presets', dict(calls=calls)

    record = {'path': '$ROOT/a.jpg', 'rating': 2, 'label': 'Green', 'settings': {'exposure': 1}, 'user_metadata': {}, 'keywords': 'k1, k2',
              'info': {'width': 10, 'height': 20}}
    yield 'quick_export_copy', dict(setup=dict(files={'a.jpg': fill(65, 9), 'out/taken.jpg': fill(1, 1), 'out/half.xmp': fill(2, 1)}, dirs=['out']), calls=[
        dict(fn='copy_original', args=dict(record=record, destination='$ROOT/out/a_copy.jpg')),
        dict(fn='copy_original', args=dict(record=record, destination='$ROOT/out/a_copy.jpg')),
        dict(fn='copy_original', args=dict(record=record, destination='$ROOT/out/taken.jpg')),
        dict(fn='copy_original', args=dict(record=record, destination='$ROOT/out/half.jpg')),
        dict(fn='copy_original', args=dict(record=dict(record, path='$ROOT/none.jpg'), destination='$ROOT/out/none.jpg')),
        dict(fn='copy_original', args=dict(record=dict(record, info={}), destination='$ROOT/out/noinfo.jpg')),
    ])

    def rec(w=4000, h=3000):
        return {'info': {'width': w, 'height': h}}
    cases = [([rec()], 16, 32 << 30), ([rec(), rec()], 4, 32 << 30), ([rec(), rec()], 8, 32 << 30), ([rec(), rec()], 16, 32 << 30),
             ([rec(), rec()], 24, 64 << 30), ([rec(9000, 9000)] * 3, 24, 8 << 30), ([rec(9000, 9000)] * 3, 24, 1 << 30), ([rec(0, 0)] * 2, 24, 4 << 30),
             ([rec(), rec()], 24, None), ([rec(), rec()], 24, 0), ([{'info': {'width': '4000', 'height': 3000.9}}] * 2, 24, 16 << 30),
             ([{'info': {}}] * 2, 12, 16 << 30), ([rec()] * 5, 48, 256 << 30), ([rec()] * 5, 7, 256 << 30), ([rec()] * 5, 8, 16 << 30)]
    yield 'quick_export_concurrency', dict(calls=[dict(fn='export_concurrency', args=dict(records=r, cpus=c, memory=m)) for r, c, m in cases]
                                           + [dict(fn='export_concurrency', args=dict(records=[{}, {}], cpus=24, memory=1 << 30))])


def maintenance_cases():
    d1, d2, d3 = b'DCP-ONE' * 20, b'DCP-TWO' * 31, b'DCP-ORPHAN' * 5
    inline = lambda data, name: json.dumps({'exposure': 1, 'dcp_profile': profile(data, name)})
    edits = [['UPDATE photos SET edits=? WHERE id=1', [inline(d1, 'one')]], ['UPDATE photos SET edits=? WHERE id=2', [inline(d1, 'one-again')]],
             ['UPDATE photos SET edits=? WHERE id=3', [inline(d2, 'two')]],
             ["INSERT INTO history(photo_id,label,edits,created) VALUES(1,'h',?,'t')", [inline(d2, 'hist')]],
             ["INSERT INTO presets(name,edits) VALUES('p',?)", [json.dumps([{'dcp_profile': profile(d1, 'preset')}])]],
             ["INSERT INTO preferences VALUES('undo:1',?)", [json.dumps({'undo': [{'dcp_profile': profile(d2, 'u')}]})]],
             ["INSERT INTO preferences VALUES('other',?)", [json.dumps({'dcp_profile': profile(d1, 'ignored')})]],
             ["INSERT INTO profile_assets VALUES(?,?,?)", [sha(d3), {'blob_hex': d3.hex()}, len(d3)]]]
    base = dict(files={'a.jpg': fill(1, 1), 'b.jpg': fill(2, 1), 'c.jpg': fill(3, 1)}, photos=photos('$ROOT/a.jpg', '$ROOT/b.jpg', '$ROOT/c.jpg'), db=edits)
    yield 'optimize_full', dict(setup=base, calls=[dict(fn='optimize', args=dict()), dict(fn='optimize', args=dict())])
    yield 'optimize_batches', dict(setup=base, calls=[dict(fn='optimize', args=dict(batch_size=1))])
    yield 'optimize_cancel_before', dict(setup=base, calls=[dict(fn='optimize', args=dict(cancel_before=True))])
    yield 'optimize_cancel_batch', dict(setup=base, calls=[dict(fn='optimize', args=dict(batch_size=1, cancel_at_stage='중복 프로파일 정리 중'))])
    yield 'optimize_cancel_late', dict(setup=base, calls=[dict(fn='optimize', args=dict(cancel_at_stage='프로파일 연결 검사 중'))])
    yield 'optimize_cancel_vacuum', dict(setup=base, calls=[dict(fn='optimize', args=dict(cancel_at_stage='빈 공간 회수 중'))])
    bad = [['UPDATE photos SET edits=? WHERE id=2', ['not json']],
           ['UPDATE photos SET edits=? WHERE id=3', [json.dumps({'dcp_profile': {'name': 'x', 'sha256': 'zz', 'data': 'AAAA'}})]],
           ["INSERT INTO preferences VALUES('undo:2',?)", [json.dumps({'dcp_profile': {'name': 'y', 'sha256': sha(d1), 'data': 'AAAA'}})]],
           ["INSERT INTO preferences VALUES('undo:3',?)", [json.dumps({'dcp_profile': {'name': 'y', 'sha256': sha(d1), 'data': [1]}})]]]
    yield 'optimize_warnings', dict(setup=dict(base, db=edits + bad), calls=[dict(fn='optimize', args=dict(batch_size=2))])
    yield 'optimize_empty', dict(setup=dict(files={}, photos=[]), calls=[dict(fn='optimize', args=dict())])


def workflow_cases():
    infos = [{'shutter': '1/250', 'iso': 100, 'aperture': 2.8}, {'shutter': 0.004, 'iso': '400', 'aperture': '5.6'},
             {'shutter': ' 1/60 ', 'iso': 1600, 'aperture': 1.4}, {'shutter': '1e-3', 'iso': 100, 'aperture': 4}, {'shutter': '.5', 'iso': 100, 'aperture': 2},
             {'shutter': '5.', 'iso': 100, 'aperture': 2}, {'shutter': '1_0/2', 'iso': 100, 'aperture': 2}, {'shutter': '1/0', 'iso': 100, 'aperture': 2},
             {'shutter': 0, 'iso': 100, 'aperture': 2}, {'shutter': -1, 'iso': 100, 'aperture': 2}, {'shutter': None, 'iso': 100, 'aperture': 2},
             {'shutter': True, 'iso': 100, 'aperture': 2}, {'shutter': 'abc', 'iso': 100, 'aperture': 2}, {'iso': 100, 'aperture': 2},
             {'shutter': 'inf', 'iso': 100, 'aperture': 2}, {'shutter': 'nan', 'iso': 100, 'aperture': 2}, {'shutter': '1/250', 'iso': 100},
             {'shutter': '+2', 'iso': '1,5', 'aperture': 2}, {'shutter': '1.5E+1', 'iso': 100, 'aperture': 2}, {'shutter': '1/2/3', 'iso': 100, 'aperture': 2},
             {'shutter': [1], 'iso': 100, 'aperture': 2}, {'shutter': '-1/2', 'iso': 100, 'aperture': 2},
             {'shutter': '1 / 2', 'iso': 100, 'aperture': 2}, {'shutter': '1.2.3', 'iso': 100, 'aperture': 2}, {'shutter': '2e', 'iso': 100, 'aperture': 2},
             {'shutter': '1/250', 'iso': 100, 'aperture': 0.0000001}, {'shutter': 30, 'iso': 3200, 'aperture': 22}]
    calls = [dict(fn='capture_exposure', args=dict(info=i)) for i in infos]
    ref = {'info': infos[0], 'settings': {'exposure': 0.7, 'contrast': 1}}
    tgt = {'info': infos[1], 'settings': {'exposure': -2, 'contrast': 3}}
    calls += [dict(fn='match_exposure', args=dict(reference=ref, target=tgt)),
              dict(fn='match_exposure', args=dict(reference=tgt, target=ref)),
              dict(fn='match_exposure', args=dict(reference={'info': {'shutter': 30, 'iso': 100, 'aperture': 22}, 'settings': {'exposure': 4.5}},
                                                  target={'info': {'shutter': 1 / 8000, 'iso': 100, 'aperture': 1.4}, 'settings': {}})),
              dict(fn='match_exposure', args=dict(reference={'info': infos[0], 'settings': {}}, target=tgt)),
              dict(fn='match_exposure', args=dict(reference={'info': infos[0], 'settings': {'exposure': None}}, target=tgt)),
              dict(fn='match_exposure', args=dict(reference=ref, target={'info': {}, 'settings': {}})),
              dict(fn='match_exposure', args=dict(reference=ref, target=tgt, extra=1))]
    yield 'workflow_exposure', dict(calls=calls, tolerance=1e-12)


def payload(seed, w=96, h=64):
    return json.dumps({'seed': seed, 'w': w, 'h': h})


NAMES = ['a.jpg', 'b.jpg', 'sub/c.jpg', 'sub/d.jpg']


def recovery_tree(folder, seeds, shift=0):
    files, mtimes = {}, {}
    for i, (name, seed) in enumerate(zip(NAMES, seeds)):
        files[f'lib/{folder}/{name}'] = text(payload(seed))
        mtimes[f'lib/{folder}/{name}'] = BASE + (10 + i) * NS + shift * NS
    return files, mtimes


def recovery_photos(folder, fingerprint_count, seeds=(1, 2, 3, 4), shift=0):
    result = []
    for i, name in enumerate(NAMES):
        item = {'path': f'$ROOT/lib/{folder}/{name}'}
        if i < fingerprint_count:
            item['metadata'] = {'file_fingerprint': {'size': len(payload(seeds[i])), 'mtime_ns': BASE + (10 + i) * NS + shift * NS}}
        result.append(item)
    return result


def recovery_cases():
    files, mtimes = recovery_tree('new', (1, 2, 3, 4))
    decoy_files, decoy_mtimes = recovery_tree('decoy', (1, 2, 3, 4), shift=1)
    stats_setup = dict(files=dict(files, **decoy_files), mtimes=dict(mtimes, **decoy_mtimes),
                       photos=recovery_photos('old', 3) + [{'virtual_of': 1}])
    yield 'recover_file_stats', dict(fake_images=True, setup=stats_setup, calls=[
        dict(fn='missing_directory', args=dict(path='$ROOT/lib/old')),
        dict(fn='missing_directory', args=dict(path='$ROOT/lib/new')),
        dict(fn='missing_directory', args=dict(path='$ROOT/lib/new/a.jpg')),
        dict(fn='recover'),
        dict(fn='validate'),
        dict(fn='recover', args=dict(cancel=True)),
        dict(fn='recover', args=dict(timeout=-1)),
        dict(fn='recover', args=dict(sources=['$ROOT/lib/old'])),
        dict(fn='recover', args=dict(sources=['$ROOT/lib/old/a.jpg'])),
        dict(fn='recover', args=dict(sources=['$ROOT/elsewhere'])),
        dict(fn='recover'),
        dict(fn='fs_utime', args=dict(path='$ROOT/lib/new/b.jpg', ns=BASE + 777 * NS)),
        dict(fn='validate'),
        dict(fn='recover'),
    ])
    yield 'recover_validate_stale', dict(fake_images=True, setup=stats_setup, calls=[
        dict(fn='recover'), dict(fn='fs_sql', args=dict(sql='UPDATE photos SET path=? WHERE id=2', args=['$ROOT/lib/old/b2.jpg'])),
        dict(fn='validate'), dict(fn='recover')])
    previews = [dict(id=i + 1, seed=i + 1) for i in range(4)]
    decoy2_files, decoy2_mtimes = recovery_tree('decoy', (5, 6, 7, 0))
    prev_setup = dict(files=dict(files, **decoy2_files), mtimes=dict(mtimes, **decoy2_mtimes), photos=recovery_photos('old', 0))
    yield 'recover_previews', dict(fake_images=True, setup=prev_setup, calls=[
        dict(fn='write_previews', args=dict(items=previews)), dict(fn='recover')])
    yield 'recover_previews_legacy', dict(fake_images=True, setup=prev_setup, calls=[
        dict(fn='write_previews', args=dict(items=[dict(i, legacy=True) for i in previews])), dict(fn='recover')])
    partial_files, partial_mtimes = recovery_tree('new', (1, 2, 6, 7))
    yield 'recover_previews_partial', dict(fake_images=True, setup=dict(files=partial_files, mtimes=partial_mtimes, photos=recovery_photos('old', 0)), calls=[
        dict(fn='write_previews', args=dict(items=previews)), dict(fn='recover')])
    mixed_files, mixed_mtimes = recovery_tree('new', (1, 6, 3, 4))
    yield 'recover_previews_one_miss', dict(fake_images=True, setup=dict(files=mixed_files, mtimes=mixed_mtimes, photos=recovery_photos('old', 0)), calls=[
        dict(fn='write_previews', args=dict(items=previews)), dict(fn='recover')])
    twin_files, twin_mtimes = recovery_tree('new_copy', (1, 2, 3, 4))
    yield 'recover_ambiguous', dict(fake_images=True, setup=dict(files=dict(files, **twin_files), mtimes=dict(mtimes, **twin_mtimes),
                                                                photos=recovery_photos('old', 3)), calls=[dict(fn='recover')])
    yield 'recover_insufficient', dict(fake_images=True, setup=dict(files=files, mtimes=mtimes, photos=recovery_photos('old', 2)),
                                       calls=[dict(fn='recover')])
    yield 'recover_collision', dict(fake_images=True, setup=dict(
        files=files, mtimes=mtimes, photos=recovery_photos('old', 3) + [{'path': '$ROOT/lib/new/a.jpg'}]), calls=[dict(fn='recover')])
    photos2 = recovery_photos('old1', 3) + recovery_photos('old2', 3)
    yield 'recover_same_destination', dict(fake_images=True, setup=dict(files=files, mtimes=mtimes, photos=photos2), calls=[dict(fn='recover')])
    yield 'recover_nothing_missing', dict(fake_images=True, setup=dict(files=files, mtimes=mtimes, photos=recovery_photos('new', 3)),
                                          calls=[dict(fn='recover')])
