"""Goldens of grainy-app work package 15 (collections and filters): golden/app/collections_*.

* `collections_dialog`: the oracle's own `FilterDialog` (manager.py) built on rules and edited like a user would:
  what the dialog shows first, and the rules (or the error text of the box) it answers;
* `collections_listings`: the listing `library_query.query` gives for every field of the dialog, for the
  complex rules a smart collection can hold, and for collections with rules, on the library of `golden/app/library`;
* `collections_steps`: scripted sessions through the oracle's own methods `LibraryManager.create_collection`,
  `add_to_collection`, `remove_collection`, `edit_collection`, `remove_from_collection`, `clear_filter`,
  `select_collection`, `refresh_collections` (a real QTreeWidget) and `WorkflowTools.quick_toggle`, `quick_show`,
  `containing_collection`, with the input boxes answered from the script; after each step the manager state, the
  tree, the listing and the catalogue tables.

The window is a stand-in with the attributes those methods read. Nothing is written outside a temporary folder and
`golden/app`. Deterministic: no times, clocks or random ids are stored.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_collections.py
"""
import json
import shutil
import sqlite3
import tempfile
import threading
from copy import deepcopy
from pathlib import Path

from common import GOLDEN, Case

from PySide6.QtCore import Qt  # noqa: E402
from PySide6.QtWidgets import QApplication, QCheckBox, QDialog, QPlainTextEdit, QTreeWidget  # noqa: E402

from luma import library_query, manager as manager_module, workflow_ui as workflow_module  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.manager import FilterDialog, LibraryManager  # noqa: E402
from luma.workflow_ui import WorkflowTools  # noqa: E402

app = QApplication.instance() or QApplication([])
START = GOLDEN / 'app' / 'library' / 'start.sqlite'


def dump_json(value):
    return json.dumps(value)           # what Catalog.add_collection and edit_collection store


# ---- the dialog itself ---------------------------------------------------------------------------------------

FIELD_KEYS = ['name', 'keywords', 'folder', 'camera', 'lens', 'extension', 'rating_min', 'rating_max', 'flag',
              'label', 'iso_min', 'iso_max', 'date_from', 'date_to']

COMPLEX = {'_adobe_smart': {'combine': 'intersect', 'children': [
    {'criteria': 'rating', 'operation': '>=', 'value': 3.0},
    {'combine': 'union', 'children': [
        {'criteria': 'labelColor', 'operation': '==', 'value': 1},
        {'criteria': 'captureTime', 'operation': 'inLast', 'value': 7, 'value_units': 'days'}]}]}}

DIALOG_CASES = [   # (name, initial rules, title, edits)
    ('empty', {}, '상세 필터', {}),
    ('empty_default_title', None, '상세 필터', {}),
    ('fill_text_fields', {}, '상세 필터', {'name': '  garden ', 'keywords': 'Travel|Seoul', 'folder': 'roll 2',
                                          'camera': 'EOS', 'lens': '50mm', 'extension': 'JPG'}),
    ('numbers', {}, '상세 필터', {'rating_min': '3', 'rating_max': ' 5 ', 'flag': '-1', 'iso_min': '+100',
                                  'iso_max': '1_600'}),
    ('dates', {}, '스마트 컬렉션 규칙', {'date_from': '2024-01-02', 'date_to': '2025-12-31'}),
    ('dates_unpadded', {}, '상세 필터', {'date_from': '2024-1-2', 'date_to': '2024-2-29'}),
    ('checks_and_orientation', {}, '상세 필터', {'missing': True, 'virtual': True, 'orientation': 'portrait'}),
    ('orientation_square', {}, '상세 필터', {'orientation': 'square'}),
    ('orientation_landscape', {}, '상세 필터', {'orientation': 'landscape'}),
    ('everything', {}, '상세 필터', {'name': 'a', 'keywords': 'b', 'folder': 'c', 'camera': 'd', 'lens': 'e',
                                     'extension': 'f', 'rating_min': '1', 'rating_max': '4', 'flag': '1',
                                     'label': '빨강', 'iso_min': '50', 'iso_max': '800', 'date_from': '2020-01-01',
                                     'date_to': '2026-11-13', 'missing': True, 'virtual': True,
                                     'orientation': 'landscape'}),
    ('existing', {'rating_min': 4, 'label': '빨강', 'virtual': True, 'orientation': 'portrait',
                  'name': 'x'}, '스마트 컬렉션 규칙', {}),
    ('existing_edit', {'rating_min': 4, 'label': '빨강'}, '스마트 컬렉션 규칙', {'rating_min': '2', 'label': ''}),
    ('existing_numbers_as_text', {'rating_max': 3, 'iso_min': 200, 'flag': -1}, '상세 필터', {}),
    ('existing_float', {'rating_min': 2.5, 'missing': False}, '상세 필터', {}),
    ('existing_none_and_list', {'camera': None, 'lens': ['a', 'b']}, '상세 필터', {}),
    ('complex_kept', COMPLEX, '스마트 컬렉션 규칙', {'name': 'garden'}),
    ('complex_dropped', COMPLEX, '스마트 컬렉션 규칙', {'name': 'garden', 'keep_complex': False}),
    ('complex_with_fields', dict(COMPLEX, rating_min=2, virtual=True), '스마트 컬렉션 규칙', {'camera': 'EOS'}),
    ('any_all_rules', {'any': [{'rating_min': 4}, {'label': '파랑'}], 'rating_max': 5}, '상세 필터', {}),
    ('other_keys', {'foo': 1, 'bar': {'x': [1, 2]}}, '상세 필터', {'name': 'k'}),
    ('other_keys_dropped', {'foo': 1}, '상세 필터', {'keep_complex': False}),
    ('error_int', {}, '상세 필터', {'rating_min': 'abc'}),
    ('error_int_float', {}, '상세 필터', {'iso_min': '1.5'}),
    ('error_int_blank_inside', {}, '상세 필터', {'flag': '1 2'}),
    ('error_int_hex', {}, '상세 필터', {'rating_max': '0x10'}),
    ('error_int_second', {}, '상세 필터', {'rating_min': '1', 'iso_max': 'x\'y'}),
    ('error_date_format', {}, '상세 필터', {'date_from': '2024/01/02'}),
    ('error_date_trailing', {}, '상세 필터', {'date_from': '2024-01-02 10:00'}),
    ('error_date_trailing_digits', {}, '상세 필터', {'date_to': '2024-01-021'}),
    ('error_date_month', {}, '상세 필터', {'date_from': '2024-13-01'}),
    ('error_date_day', {}, '상세 필터', {'date_to': '2023-02-30'}),
    ('error_date_leap', {}, '상세 필터', {'date_to': '2023-02-29'}),
    ('error_date_year_zero', {}, '상세 필터', {'date_from': '0000-01-01'}),
    ('error_date_short_year', {}, '상세 필터', {'date_from': '24-01-01'}),
    ('error_date_word', {}, '상세 필터', {'date_to': 'tomorrow'}),
    ('error_int_before_date', {}, '상세 필터', {'date_from': 'x', 'flag': 'y'}),
    ('error_describe', {'_adobe_smart': {'combine': 'intersect', 'children': [{'operation': '=='}]}},
     '스마트 컬렉션 규칙', {}),
    ('error_describe_type', {'_adobe_smart': 5}, '스마트 컬렉션 규칙', {}),
]


def dialog_state(dialog):
    details = dialog.findChild(QPlainTextEdit)
    return dict(
        title=dialog.windowTitle(),
        fields={key: dialog.fields[key].text() for key in FIELD_KEYS},
        missing=dialog.missing.isChecked(), virtual=dialog.virtual.isChecked(),
        orientation=dialog.orientation.currentData(),
        orientation_items=[[dialog.orientation.itemText(i), dialog.orientation.itemData(i)]
                           for i in range(dialog.orientation.count())],
        extra=dialog.extra_rules, keep_complex_shown=not dialog.keep_complex.isHidden(),
        keep_complex=dialog.keep_complex.isChecked(),
        details=details.toPlainText() if details is not None else None)


def dialog_case(name, initial, title, edits):
    entry = dict(name=name, initial_text=json.dumps(initial), title=title, edits=edits)   # text keeps the key order
    try:
        dialog = FilterDialog(None, initial, title) if initial is not None else FilterDialog(None, title=title)
    except Exception as error:   # noqa: BLE001 - the oracle's message is the golden
        entry['constructor_error'] = str(error)
        return entry
    entry['state'] = dialog_state(dialog)
    for key, value in edits.items():
        if key in dialog.fields:
            dialog.fields[key].setText(value)
        elif key == 'missing':
            dialog.missing.setChecked(value)
        elif key == 'virtual':
            dialog.virtual.setChecked(value)
        elif key == 'orientation':
            dialog.orientation.setCurrentIndex(max(0, dialog.orientation.findData(value)))
        elif key == 'keep_complex':
            dialog.keep_complex.setChecked(value)
        else:
            raise ValueError(key)
    try:
        entry['rules'] = dialog.rules()
        entry['rules_text'] = dump_json(entry['rules'])
    except ValueError as error:
        entry['error'] = str(error)
    return entry


with Case('app', 'collections_dialog') as case:
    case.json(fields=FIELD_KEYS, cases=[dialog_case(*c) for c in DIALOG_CASES])


# ---- listings ------------------------------------------------------------------------------------------------

LISTING_RULES = [
    ('none', {}),
    ('name', {'name': 'GARDEN'}), ('name_png', {'name': '.png'}),
    ('keywords', {'keywords': 'sunset'}), ('keywords_utf8', {'keywords': '바다'}), ('keywords_studio', {'keywords': 'studio'}),
    ('folder', {'folder': 'roll 2'}), ('folder_sub', {'folder': 'sub'}),
    ('camera', {'camera': 'canon'}), ('lens', {'lens': '50mm'}),
    ('extension_png', {'extension': 'png'}), ('extension_jpg', {'extension': 'JPG'}),
    ('rating_min', {'rating_min': 4}), ('rating_max', {'rating_max': 2}),
    ('rating_range', {'rating_min': 2, 'rating_max': 4}),
    ('flag_pick', {'flag': 1}), ('flag_reject', {'flag': -1}), ('flag_none', {'flag': 0}),
    ('label', {'label': '빨강'}), ('label_blue', {'label': '파랑'}),
    ('iso_min', {'iso_min': 100}), ('iso_max', {'iso_max': 800}),
    ('date_from', {'date_from': '2024-01-01'}), ('date_to', {'date_to': '2024-12-31'}),
    ('date_range', {'date_from': '2024-01-01', 'date_to': '2025-06-01'}),
    ('date_unpadded', {'date_from': '2024-1-2'}),
    ('missing', {'missing': True}), ('present', {'missing': False}),
    ('virtual', {'virtual': True}), ('not_virtual', {'virtual': False}),
    ('orientation_landscape', {'orientation': 'landscape'}), ('orientation_portrait', {'orientation': 'portrait'}),
    ('orientation_square', {'orientation': 'square'}),
    ('dialog_everything', {'name': 'a', 'extension': 'png', 'rating_min': 1, 'rating_max': 5, 'flag': 1,
                           'date_from': '2020-01-01', 'date_to': '2026-12-31', 'virtual': False}),
    ('text_and_number', {'extension': 'jpg', 'rating_min': 4}),
    ('any', {'any': [{'rating_min': 5}, {'label': '파랑'}]}),
    ('all', {'all': [{'rating_min': 4}, {'extension': 'png'}]}),
    ('smart_rating', {'_adobe_smart': {'criteria': 'rating', 'operation': '>=', 'value': 4}}),
    ('smart_combined', {'_adobe_smart': {'combine': 'intersect', 'children': [
        {'criteria': 'rating', 'operation': '>=', 'value': 3},
        {'criteria': 'labelColor', 'operation': '!=', 'value': 3}]}}),
    ('smart_dates', {'_adobe_smart': {'criteria': 'captureTime', 'operation': '>', 'value': '2025-01-01'}}),
    ('smart_plus_fields', {'_adobe_smart': {'criteria': 'pick', 'operation': '==', 'value': 1}, 'rating_min': 4}),
    ('smart_keyword', {'_adobe_smart': {'criteria': 'keywords', 'operation': 'any', 'value': 'sunset 스튜디오'}}),
    ('empty_values_ignored', {'name': '', 'keywords': None, 'camera': [], 'lens': {}}),
]
LISTING_CONTEXT = [   # (name, options besides the rules)
    ('mode_picked', {'mode': 'picked', 'rules': {'rating_min': 4}}),
    ('mode_rated_extension', {'mode': 'rated', 'rules': {'extension': 'png'}}),
    ('search_and_rules', {'search': 'a', 'rules': {'rating_min': 4}}),
    ('collection_plain_and_rules', {'collection': 1, 'rules': {'rating_min': 4}}),
    ('collection_smart_and_rules', {'collection': 2, 'rules': {'extension': 'jpg'}}),
    ('folder_and_rules', {'folder': 'ROLL2', 'rules': {'rating_min': 5}}),
    ('sorted_by_name', {'sort': 'name', 'rules': {'rating_min': 1}}),
    ('collapsed', {'collapsed': True, 'rules': {'rating_min': 1}}),
]


def listing(path, options, stack_orders):
    full = dict(search='', folder=None, recursive=True, mode='all', rules={}, collection=None, collapsed=False,
                stack_orders=stack_orders, sort='imported', descending=False)
    full.update(options)
    result = library_query.query(path, full, threading.Event())
    return dict(ids=result['ids'], total=result['total'])


with tempfile.TemporaryDirectory(prefix='grainy-app-collections-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    query_copy = work / 'query.sqlite'
    shutil.copyfile(START, query_copy)
    snapshot = work / 'snapshot'
    snapshot.mkdir()
    shutil.copyfile(START, snapshot / 'catalog.sqlite')
    c = Catalog(snapshot)
    stack_orders = c.preference('stack_orders', {})
    c.close()
    folder_root = str(sqlite3.connect(START).execute('SELECT path FROM folder_roots ORDER BY path').fetchone()[0])
    listings = []
    for name, rules in LISTING_RULES:
        listings.append(dict(name=name, options=dict(rules=rules), **listing(query_copy, dict(rules=rules), stack_orders)))
    for name, options in LISTING_CONTEXT:
        options = deepcopy(options)
        if options.get('folder') == 'ROLL2':
            options['folder'] = folder_root + r'\roll 2'
        listings.append(dict(name=name, options=options, **listing(query_copy, options, stack_orders)))
    with Case('app', 'collections_listings') as case:
        case.json(listings=listings)


# ---- scripted sessions ---------------------------------------------------------------------------------------

class Answers:
    """The input boxes: `QInputDialog.getText/getItem` and `FilterDialog` answer from the script, in order."""

    def __init__(self):
        self.queue = []
        self.shown = []

    def push(self, *items):
        self.queue.extend(items)

    def take(self):
        if not self.queue:
            raise AssertionError('a box opened that the script did not answer')
        return self.queue.pop(0)


ANSWERS = Answers()


class ScriptedInput:
    @staticmethod
    def getText(parent, title, label, *args, text='', **kwargs):
        ANSWERS.shown.append(dict(box='text', title=title, label=label, text=text))
        value, ok = ANSWERS.take()
        return value, ok

    @staticmethod
    def getItem(parent, title, label, items, *args, editable=True, **kwargs):
        ANSWERS.shown.append(dict(box='item', title=title, label=label, items=list(items), editable=editable))
        index = ANSWERS.take()
        if index is None:
            return '', False
        return items[index], True


class ScriptedFilter:
    def __init__(self, parent, rules=None, title='상세 필터'):
        ANSWERS.shown.append(dict(box='filter', title=title, rules=deepcopy(rules)))
        self.answer = ANSWERS.take()

    def exec(self):
        return QDialog.DialogCode.Accepted if self.answer is not None else QDialog.DialogCode.Rejected

    def rules(self):
        return deepcopy(self.answer)


manager_module.QInputDialog = ScriptedInput
manager_module.FilterDialog = ScriptedFilter
workflow_module.QInputDialog = ScriptedInput


class Status:
    def __init__(self, log):
        self.log = log

    def showMessage(self, text, *args):
        self.log.append(text)


class Search:
    def __init__(self):
        self.text = ''

    def blockSignals(self, value):
        pass

    def clear(self):
        self.text = ''


class Window:
    """What the manager's and the workflow's methods read of the main window."""

    def __init__(self, catalog):
        self.catalog = catalog
        self.selection = []
        self.current_id = None
        self.folder_filter = None
        self.filter_mode = 'all'
        self.search = Search()
        self.refreshes = []
        self.modes = []
        self.messages = []
        self.manager = None

    def selected_ids(self):
        return list(self.selection) or ([self.current_id] if self.current_id else [])

    def statusBar(self):
        return Status(self.messages)

    def refresh_lists(self, *args, ensure_current=False):
        self.refreshes.append(bool(ensure_current))

    def set_filter(self, mode):
        self.filter_mode = mode
        if mode == 'all':
            self.folder_filter = None
        self.refresh_lists(ensure_current=True)
        self.set_mode(1)

    def set_mode(self, mode):
        self.modes.append(mode)


class Manager:
    create_collection = LibraryManager.create_collection
    add_to_collection = LibraryManager.add_to_collection
    remove_collection = LibraryManager.remove_collection
    edit_collection = LibraryManager.edit_collection
    remove_from_collection = LibraryManager.remove_from_collection
    clear_filter = LibraryManager.clear_filter
    filter_dialog = LibraryManager.filter_dialog
    select_collection = LibraryManager.select_collection
    refresh_collections = LibraryManager.refresh_collections

    def __init__(self, window):
        self.w = window
        self.rules = {}
        self.collection = None
        self.tree = QTreeWidget()
        window.manager = self
        self.refresh_collections()


class Workflow:
    quick_toggle = WorkflowTools.quick_toggle
    select_collection = WorkflowTools.select_collection
    quick_show = WorkflowTools.quick_show
    containing_collection = WorkflowTools.containing_collection

    def __init__(self, window):
        self.w = window


class Item:
    def __init__(self, ident):
        self.ident = ident

    def data(self, column, role):
        return self.ident


def tree_of(manager):
    rows = []

    def walk(item, depth):
        rows.append(dict(text=item.text(0), id=item.data(0, Qt.ItemDataRole.UserRole), depth=depth))
        for index in range(item.childCount()):
            walk(item.child(index), depth + 1)
    for index in range(manager.tree.topLevelItemCount()):
        walk(manager.tree.topLevelItem(index), 0)
    current = manager.tree.currentItem()
    return rows, current.data(0, Qt.ItemDataRole.UserRole) if current else None


TABLES = {'collections': ['id', 'name', 'parent_id', 'rules'], 'collection_members': ['collection_id', 'photo_id'],
          'preferences': ['key', 'value']}


def tables(db):
    out = {}
    for table, columns in TABLES.items():
        cols = ','.join('"' + c + '"' for c in columns)
        rows = db.execute(f'SELECT {cols} FROM {table} ORDER BY {cols}').fetchall()
        if table == 'preferences':
            rows = [r for r in rows if r[0] in ('quick_collection',)]
        out[table] = [dict(zip(columns, row)) for row in rows]
    return out


def run_script(name, script, folder_root):
    folder = work / name
    folder.mkdir()
    shutil.copyfile(START, folder / 'catalog.sqlite')
    catalog = Catalog(folder)
    window = Window(catalog)
    manager = Manager(window)
    workflow = Workflow(window)
    steps = []
    previous_tables = None
    for step in script:
        action, a = step[0], step[1:]
        ANSWERS.queue.clear()
        ANSWERS.shown.clear()
        window.refreshes.clear()
        window.modes.clear()
        window.messages.clear()
        error = None
        if action == 'select':
            window.selection = list(a[0])
        elif action == 'open':
            window.current_id = a[0]
        elif action == 'search':
            window.search.text = a[0]
        elif action == 'folder':
            window.folder_filter = a[0].replace('{root}', folder_root)
        elif action == 'mode':
            window.filter_mode = a[0]
        else:
            try:
                if action == 'create':                       # name, smart rules (None: a plain collection)
                    name_text, rules = a[0], a[1]
                    ANSWERS.push((name_text, bool(name_text is not None)))
                    if rules is not None:
                        ANSWERS.push(rules if rules != 'cancel' else None)
                    manager.create_collection(rules is not None)
                elif action == 'create_cancelled_name':
                    ANSWERS.push(('x', False))
                    manager.create_collection(False)
                elif action == 'add_to':
                    ANSWERS.push(a[0])
                    manager.add_to_collection()
                elif action == 'pick':                       # a click on a tree item
                    manager.select_collection(Item(a[0]))
                elif action == 'pick_nothing':
                    manager.select_collection(None)
                elif action == 'remove_collection':
                    manager.remove_collection()
                elif action == 'edit':                       # new name, new rules for a smart collection
                    ANSWERS.push((a[0], a[0] is not None))
                    if len(a) > 1:
                        ANSWERS.push(a[1])
                    manager.edit_collection()
                elif action == 'remove_from':
                    manager.remove_from_collection()
                elif action == 'clear_filter':
                    manager.clear_filter()
                elif action == 'filter':                     # the 상세 필터 box, answered with these rules
                    ANSWERS.push(a[0])
                    manager.filter_dialog()
                elif action == 'quick_toggle':
                    workflow.quick_toggle()
                elif action == 'quick_show':
                    workflow.quick_show()
                elif action == 'containing':
                    ANSWERS.push(a[0] if a else None)
                    workflow.containing_collection()
                else:
                    raise ValueError(action)
            except Exception as exc:   # noqa: BLE001 - the oracle's `run` shows the message in a box
                error = str(exc)
        catalog.db.commit()
        catalog.db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        copy = work / f'{name}-query.sqlite'
        shutil.copyfile(folder / 'catalog.sqlite', copy)
        options = dict(search=window.search.text, folder=window.folder_filter, recursive=True,
                       mode=window.filter_mode, rules=deepcopy(manager.rules), collection=manager.collection,
                       collapsed=False, stack_orders=catalog.preference('stack_orders', {}), sort='imported',
                       descending=False)
        result = library_query.query(copy, options, threading.Event())
        rows, current = tree_of(manager)
        now_tables = tables(sqlite3.connect(folder / 'catalog.sqlite'))
        steps.append(dict(
            step=step, error=error, collection=manager.collection, rules=manager.rules, mode=window.filter_mode,
            folder=window.folder_filter, search=window.search.text, refreshes=list(window.refreshes),
            modes=list(window.modes), messages=list(window.messages), boxes=list(ANSWERS.shown),
            ids=result['ids'], total=result['total'], tree=rows, tree_current=current,
            **({} if now_tables == previous_tables else {'tables': now_tables})))   # only when they changed
        previous_tables = now_tables
    catalog.close()
    return steps


SCRIPT = [
    ['select', [2, 4]],
    ['create', '  출사  ', None],                           # a plain collection from the selection (name stripped)
    ['select', [6]],
    ['create', '하위', None],                               # nested under the selected collection
    ['create', '별 다섯', {'rating_min': 5}],               # a smart collection under it
    ['create', '취소할 규칙', 'cancel'],                    # the rules box cancelled: nothing is created
    ['create', '   ', None],                                # an empty name: nothing
    ['create_cancelled_name'],
    ['select', [1, 3]],
    ['add_to', 0], ['add_to', 1], ['add_to', None],         # choose from the plain collections; cancel
    ['pick', 1],                                            # 여행
    ['select', [2]],
    ['remove_from'],
    ['edit', '여행 2023'],                                  # name only
    ['pick', 2],                                            # the smart 좋은 사진
    ['edit', '좋은 별', {'rating_min': 3, 'label': '빨강'}],
    ['edit', None],                                         # cancelled name
    ['pick', 2], ['pick_nothing'],                          # the same item again; nothing selected
    ['search', 'a'],
    ['filter', {'extension': 'png', 'rating_max': 3}],
    ['filter', None],
    ['filter', {}],
    ['select', [1, 2]],
    ['quick_toggle'], ['quick_toggle'],
    ['select', [1, 3]],
    ['quick_toggle'],
    ['select', [3, 5]],
    ['quick_toggle'],                                       # only some are in it: all are added
    ['select', []],
    ['open', 3],
    ['quick_toggle'],                                       # the open photo
    ['folder', '{root}'],
    ['mode', 'picked'],
    ['quick_show'],                                         # clears search, rules, folder, mode
    ['open', 2],
    ['containing', 0],
    ['open', 7],
    ['containing'],                                         # a photo in no collection
    ['open', 3],
    ['containing', None],                                   # cancelled
    ['pick', 3],
    ['remove_collection'],                                  # children keep living with no parent
    ['remove_collection'],                                  # nothing selected now
    ['edit', 'nothing'],                                    # no collection: nothing
    ['remove_from'],
    ['pick', 1],
    ['filter', {'rating_min': 2}],
    ['clear_filter'],
]

SCRIPT_NO_PLAIN = [
    ['pick', 1], ['remove_collection'],
    ['select', [1]],
    ['add_to', 0],                                          # no plain collection: the oracle's error
    ['quick_toggle'],
    ['quick_show'],
    ['add_to', 0],
    ['create', 'Top', None],
    ['create', 'Zed', None],
    ['create', 'Alpha', {'extension': 'png'}],
    ['pick', 4],
    ['create', 'Child of Zed', {}],
]

with tempfile.TemporaryDirectory(prefix='grainy-app-collections-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    folder_root = str(sqlite3.connect(START).execute('SELECT path FROM folder_roots ORDER BY path').fetchone()[0])
    sessions = [dict(name=name, steps=run_script(name, script, folder_root))
                for name, script in [('main', SCRIPT), ('no_plain', SCRIPT_NO_PLAIN)]]
    with Case('app', 'collections_steps') as case:
        case.json(folder_root=folder_root, sessions=sessions,
                  scripts={name: json.dumps(script) for name, script in
                           [('main', SCRIPT), ('no_plain', SCRIPT_NO_PLAIN)]})   # text keeps the key order
