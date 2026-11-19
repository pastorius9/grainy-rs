"""Goldens of grainy-app's XMP sidecars (APP_PLAN package 28), made with the oracle's own code: `golden/app/xmp/`.

A small library is built with the Python Grainy's own `Catalog` (photos with ratings, labels, keywords, entered
metadata, edits, two virtual copies, a RAW photo, a file without an extension, a name with several dots). Scripted
sessions then run the oracle's own `LibraryManager.write_xmp` / `read_xmp` (unbound, on a stand-in for the window and
the manager: the pattern of `gen_app_sync.py`) over real files in a temporary folder.

Stored per step: the text of every `.xmp` file in the photo folder after the step (line ends as `\\n`: Windows writes
`\\r\\n`), the messages the window showed, the error the action raised, and the catalogue as the step leaves it
(rows, settings as the keys that differ from the defaults, history rows, undo preferences). The photo files are
empty: the sidecars are written next to them and nothing reads them.

The GPS map (`golden/app/xmp_map/`): the oracle's own `extras.MapView` is built over photos whose stored `info` and
entered metadata hold every kind of position (numbers, texts, out of range, booleans, junk); the markers it makes
(id, scene position, tooltip) and the number of land polygons are stored.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_xmp.py
"""
import json
import tempfile
from datetime import datetime
from pathlib import Path

from common import Case
from luma import catalog as catalog_module
from luma.catalog import Catalog
from luma.engine import DEFAULTS, normalized
from luma.extras import MapView
from luma.manager import LibraryManager
from PySide6.QtWidgets import QApplication, QGraphicsPathItem

STAMP = '2026-11-13T01:02:03.456789+00:00'
TOKEN = '0123456789abcdef0123456789abcdef'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock


class Nop:
    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False

    def __iter__(self):
        return iter(())


class Bar:
    def __init__(self):
        self.messages = []

    def showMessage(self, text, *args):
        self.messages.append(text)


class Window:
    """What `write_xmp` / `read_xmp` read from the window."""

    def __init__(self, catalog):
        self.catalog = catalog
        self.current_id = None
        self.selection = []
        self.bar = Bar()
        self.reloads = 0
        self.photo_model = Nop()

    def statusBar(self):
        return self.bar

    def commit(self):
        pass

    def save_keywords(self):
        pass

    def selected_ids(self):
        return sorted(self.selection) or ([self.current_id] if self.current_id else [])

    def clear_active_photo(self):
        pass

    def refresh_lists(self, *args, **kwargs):
        self.reloads += 1

    def activate(self, ident):
        pass


class Manager:
    write_xmp = LibraryManager.write_xmp
    read_xmp = LibraryManager.read_xmp
    selected_photos = LibraryManager.selected_photos
    edit_photo = LibraryManager.edit_photo
    reload_current = LibraryManager.reload_current

    def __init__(self, window):
        self.w = window


XMP_FOREIGN = """<?xpacket begin="\ufeff" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 5.6-c140">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+0.50" crs:HasCrop="False">
      <crs:ToneCurve><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurve>
    </rdf:Description>
    <rdf:Description rdf:about="" xmlns:luma="https://luma.local/ns/1.0/" luma:Settings="{}" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="1"/>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"""

NS = ('xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" '
      'xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" '
      'xmlns:lr="http://ns.adobe.com/lightroom/1.0/" xmlns:luma="https://luma.local/ns/1.0/"')

XMP_ALL = f"""<?xml version="1.0" encoding="utf-8"?>
<x:xmpmeta {NS}>
<rdf:RDF><rdf:Description xmp:Rating="9" xmp:Label="Red &amp; &#10;Blue" luma:Settings='{{"exposure": 1.5, "negative": true}}' luma:Metadata='{{"caption": "눈"}}'>
<dc:subject><rdf:Bag><rdf:li>plain</rdf:li></rdf:Bag></dc:subject>
<lr:hierarchicalSubject><rdf:Bag><rdf:li>a|b</rdf:li><rdf:li>c</rdf:li><rdf:li>a|b</rdf:li><rdf:li/></rdf:Bag></lr:hierarchicalSubject>
</rdf:Description></rdf:RDF></x:xmpmeta>"""

XMP_SETTINGS = (f'<x:xmpmeta {NS}><rdf:RDF><rdf:Description luma:Settings=\'{{"exposure": -0.5, "contrast": 20}}\'/>'
                '</rdf:RDF></x:xmpmeta>')
XMP_RATING = (f'<x:xmpmeta {NS}><rdf:RDF><rdf:Description xmp:Rating="-3"><dc:subject><rdf:Bag><rdf:li>one</rdf:li>'
              '<rdf:li>two</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>')
XMP_RAW = (f'<x:xmpmeta {NS}><rdf:RDF><rdf:Description luma:Settings=\'{{"raw_mode": "sample", "exposure": 0.3}}\' '
           'luma:Metadata=\'{"title": "from file"}\'/></rdf:RDF></x:xmpmeta>')
XMP_BROKEN = '<x:xmpmeta xmlns:x="adobe:ns:meta/"><oops></x:xmpmeta>'

# The library: ('add', file, info) / ('update', id, fields) / ('meta', id, values) / ('edit', id, label, changes) /
# ('copy', id). The photo folder is the one the files are made in.
BUILD = [
    ['add', 'a.jpg', {'format': 'jpg', 'width': 24, 'height': 16}],
    ['update', 1, {'rating': 3, 'label': '빨강', 'keywords': '여행|일본, 필름'}],
    ['meta', 1, [['title', 'Garden'], ['latitude', 37.5]]],   # pairs: the golden's JSON sorts keys, Python's dicts do not
    ['edit', 1, '보정 변경', {'exposure': 0.4, 'contrast': 15.0}],
    ['copy', 1],
    ['edit', 2, '보정 변경', {'exposure': 1.0, 'saturation': -10.0}],
    ['add', 'b.png', {'format': 'png', 'width': 24, 'height': 16}],
    ['add', 'c.name.tif', {'format': 'tif', 'width': 24, 'height': 16}],
    ['update', 4, {'rating': 5, 'keywords': 'Straße & co, <tag>'}],
    ['add', 'noext', {'format': 'png', 'width': 24, 'height': 16}],
    ['update', 5, {'label': '파랑', 'flag': 1}],
    ['add', 'd.nef', {'format': 'nef', 'make': 'NIKON', 'camera': 'Nikon Z 6', 'width': 24, 'height': 16}],
    ['edit', 6, '보정 변경', {'exposure': 0.2, 'raw_mode': 'sample', 'raw_neutral': [0.5, 0.4, 0.6]}],
    ['copy', 6],
]


# Steps: ('write', ids) / ('read', ids) / ('update', id, fields) / ('file', name, text) / ('delete', name)
SCRIPT = [
    ['file', 'b.xmp', XMP_FOREIGN],
    ['write', [1, 2, 3, 4, 5, 6, 7]],
    ['write', [1]],
    ['update', 1, {'rating': 1, 'keywords': 'new, 한글'}],
    ['write', [1]],
    ['write', []],
    ['file', 'a.xmp', XMP_ALL],
    ['file', 'a.luma-copy-2.xmp', XMP_SETTINGS],
    ['file', 'noext.xmp', XMP_RATING],
    ['file', 'd.xmp', XMP_RAW],
    ['delete', 'c.name.xmp'],
    ['read', [1, 2, 3, 4, 5, 6]],
    ['read', [1, 2, 3, 4, 5, 6]],
    ['file', 'd.luma-copy-7.xmp', XMP_BROKEN],
    ['read', [5, 6, 7, 1]],
    ['read', [7]],
]


def sparse(settings):
    changed = {k: v for k, v in settings.items() if k not in DEFAULTS or v != DEFAULTS[k]}
    return {'keys': len(settings), 'changed': changed}


def project(c):
    db = c.db
    photos = {}
    for row in db.execute('SELECT id,edits,extras,rating,flag,label,keywords FROM photos ORDER BY id'):
        ident, edits, extras, rating, flag, label, keywords = row
        photos[str(ident)] = {'settings': sparse(json.loads(edits)), 'extras': json.loads(extras or '{}'),
                              'rating': rating, 'flag': flag, 'label': label, 'keywords': keywords}
    history = [{'id': i, 'photo': p, 'label': label, 'settings': sparse(json.loads(edits)), 'created': created}
               for i, p, label, edits, created in db.execute(
                   'SELECT id,photo_id,label,edits,created FROM history ORDER BY id')]
    preferences = {}
    for key, value in db.execute('SELECT key,value FROM preferences ORDER BY key'):
        value = json.loads(value)
        if key.startswith('undo:') and isinstance(value, dict) and 'undo' in value:
            value = {'undo': [sparse(s) for s in value['undo']], 'redo': [sparse(s) for s in value['redo']]}
        preferences[key] = value
    return {'photos': photos, 'history': history, 'preferences': preferences}


def sidecars(folder):
    return {p.name: p.read_bytes().decode('utf-8').replace('\r\n', '\n') for p in sorted(folder.glob('*.xmp'))}


def build(c, folder):
    for op in BUILD:
        kind = op[0]
        if kind == 'add':
            (folder / op[1]).write_bytes(b'')
            c.add(str(folder / op[1]), settings=None, metadata=op[2], resolved=True)
        elif kind == 'update':
            c.update(op[1], **op[2])
        elif kind == 'meta':
            c.set_user_metadata(op[1], dict(op[2]))
        elif kind == 'edit':
            settings = dict(c.photo(op[1])['settings'])
            settings.update(op[3])
            c.edit(op[1], normalized(settings), op[2])
        elif kind == 'copy':
            c.virtual_copy(op[1])
        else:
            raise AssertionError(kind)


def run(manager, folder, step):
    w = manager.w
    w.bar.messages.clear()
    error = None
    try:
        kind = step[0]
        if kind in ('write', 'read'):
            w.selection = list(step[1])
            w.current_id = None
            getattr(manager, kind + '_xmp')()
        elif kind == 'update':
            w.catalog.update(step[1], **step[2])
        elif kind == 'file':
            (folder / step[1]).write_bytes(step[2].encode('utf-8'))
        elif kind == 'delete':
            (folder / step[1]).unlink(missing_ok=True)
        else:
            raise AssertionError(kind)
    except Exception as exc:
        error = f'{type(exc).__name__}: {exc}'
    return error


with tempfile.TemporaryDirectory(prefix='grainy-app-xmp-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    folder = work / 'Photos'
    folder.mkdir()
    c = Catalog(work / 'Library')
    c.register_folder(str(folder))
    build(c, folder)
    start = project(c)
    start_rows = [{**{k: p[k] for k in ('id', 'name', 'rating', 'flag', 'label', 'keywords', 'virtual_source')},
                   'extras': json.loads(p['extras'] or '{}')} for p in c.photos()]
    manager = Manager(Window(c))
    steps = []
    for step in SCRIPT:
        error = run(manager, folder, step)
        steps.append({'step': step, 'error': error, 'messages': list(manager.w.bar.messages),
                      'files': sidecars(folder), 'catalog': project(c), 'reloads': manager.w.reloads})
    c.close()

with Case('app', 'xmp') as case:
    case.json(stamp=STAMP, token=TOKEN, build=BUILD, script=SCRIPT, start=start, start_rows=start_rows, steps=steps)


# ---- the GPS map ---------------------------------------------------------------------------------------------------

app = QApplication.instance() or QApplication([])

POSITIONS = [   # (name, stored info, entered metadata)
    ('p01.jpg', {'latitude': 37.5, 'longitude': 127.0}, {}),
    ('p02.jpg', {'latitude': '-33.8688', 'longitude': ' 151.2093 '}, {}),
    ('p03.jpg', {'latitude': 10, 'longitude': 10}, {'latitude': '48.8566', 'longitude': 2.3522}),
    ('p04.jpg', {'latitude': 91, 'longitude': 0}, {}),
    ('p05.jpg', {'latitude': 0, 'longitude': -180.5}, {}),
    ('p06.jpg', {'latitude': -90, 'longitude': 180}, {}),
    ('p07.jpg', {'latitude': 12.5}, {}),
    ('p08.jpg', {'latitude': 'abc', 'longitude': 'def'}, {}),
    ('p09.jpg', {'latitude': True, 'longitude': False}, {}),
    ('p10.jpg', {'latitude': '1_0', 'longitude': '2_0'}, {}),
    ('p11.jpg', {'latitude': 'inf', 'longitude': 'nan'}, {}),
    ('p12.jpg', {'latitude': None, 'longitude': None}, {}),
    ('p13.jpg', {'latitude': [1], 'longitude': [2]}, {}),
    ('p14.jpg', {'latitude': '1e1', 'longitude': '.5'}, {}),
    ('p15 한글.jpg', {'latitude': 35.123456789, 'longitude': 139.7000004}, {}),
    ('p16.jpg', {'latitude': '', 'longitude': ''}, {}),
    ('p17.jpg', {'latitude': '+5', 'longitude': '-0'}, {}),
    ('p18.jpg', {'latitude': '0x10', 'longitude': '1__0'}, {}),
    ('p19.jpg', {'latitude': '_1', 'longitude': '1_'}, {}),
    ('p20.jpg', {'latitude': '45', 'longitude': '90'}, {'latitude': None}),
    ('p21.jpg', {}, {'latitude': 1.5, 'longitude': 2.5}),
    ('p22.jpg', {'latitude': '  \t7.25\n', 'longitude': '-7.75'}, {}),
    ('p23.jpg', {'latitude': 'Infinity', 'longitude': '0'}, {}),
    ('p24.jpg', {'latitude': 1e-7, 'longitude': -1e-7}, {}),
]

with tempfile.TemporaryDirectory(prefix='grainy-app-xmp-map-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    c = Catalog(work / 'Library')
    ids = []
    for name, info, extras in POSITIONS:
        ident = c.add(str(work / name), settings=None, metadata=info, resolved=True)
        c.update(ident, extras=json.dumps(extras, ensure_ascii=False))
        ids.append(ident)
    activated = []
    view = MapView([c.photo(i) for i in ids], activated.append)
    markers = [dict(id=m.data(0), x=m.pos().x(), y=m.pos().y(), tip=m.toolTip()) for m in view.points]
    polygons = sum(1 for item in view.scene().items() if isinstance(item, QGraphicsPathItem))
    c.close()

with Case('app', 'xmp_map') as case:
    case.json(photos=[dict(name=n, info=i, extras=e) for n, i, e in POSITIONS], markers=markers, polygons=polygons)
