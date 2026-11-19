"""Goldens for grainy-i18n: tr() in both languages, keys()/key_hint()/written() on both platforms, the shortcut table
and the edit fields. Also copies the oracle's compiled translation table into the crate."""
import json
import shutil
import sys

from common import Case, ORACLE, ROOT

from luma import edit_fields, i18n, shortcut_list

# The compiled table is embedded in the crate.
shutil.copyfile(ORACLE / 'assets' / 'i18n' / 'en.json', ROOT / 'crates' / 'grainy-i18n' / 'assets' / 'en.json')
table = json.loads((ORACLE / 'assets' / 'i18n' / 'en.json').read_text(encoding='utf-8'))


def run_tr(language, source, values):
    """tr() result, or the name of the exception class it raises."""
    i18n.set_language(language)
    try:
        return {'ok': i18n.tr(source, *values)}
    except Exception as error:  # noqa: BLE001 - the class is the datum
        return {'error': type(error).__name__}


# Every key of the table, both languages, no values / one value / enough values.
many = ['v0', 'v1', 'v2', 'v3', 'v4', 'v5', 'v6']
rows = []
for source in table:
    row = {'source': source}
    for language in ('ko', 'en'):
        row[language] = run_tr(language, source, [])
        row[language + '_one'] = run_tr(language, source, many[:1])
        row[language + '_many'] = run_tr(language, source, many)
    rows.append(row)
with Case('i18n', 'tr_table') as case:
    case.json(count=len(table), rows=rows)

# Hand-made strings: placeholders, too few values, unknown strings, braces that are not placeholders, spec forms.
synthetic = [
    '', '평범한 문장', '{0}', '{0} / {1}', '{0} {1} {2}', '{} {}', '{} {0}', '{0} {}', '{0} {0}', '{1}', '{2} {0}',
    '{{문자 그대로}}', '{{{0}}}', '{', '}', '{0', '0}', 'a { b', 'a } b', '{{', '}}', '{ }', '{0:}', '{0!s}', '{0!r}',
    '{0:>5}', '{0:<5}', '{0:^6}', '{0:*^7}', '{0:5}', '{0:d}', '{0:03d}', '{stem}', '{n:03d}',
    '{00}', '{0!}', '{0!sx}', 'JSON {"a": 1}', 'a{{b}}c{0}', '한글 {0}개의 사진', '{0}\n{1}', 'unknown string {0}',
]
cases = []
for text in synthetic:
    for language in ('ko', 'en'):
        for values in ([], ['x'], ['x', 'y'], ['x', 'y', 'z'], ['가', '나'], ['12', '34']):
            cases.append({'source': text, 'language': language, 'values': values, **run_tr(language, text, values)})
with Case('i18n', 'tr_synthetic') as case:
    case.json(cases=cases)

# keys() on both platforms.
real = sys.platform
platforms = {'windows': 'win32', 'macos': 'darwin', 'linux': 'linux'}
texts = ['', 'Ctrl', 'Ctrl+', 'Ctrl+Z', 'Ctrl+Shift+Z', 'Ctrl+Alt+V / Ctrl+Y', 'Ctrl+Ctrl+', 'ctrl+z', 'No Control here',
         'Ctrl+/ Ctrl+[ Ctrl+]', 'Press Ctrl++', '단축키 Ctrl+E 를 누르세요', 'CtrlCtrl', 'Shift+/']
for entry in shortcut_list.TABLE:
    texts += list(entry.keys) + [entry.label, ' / '.join(entry.keys), entry.text]
keys_rows = []
for name, value in platforms.items():
    sys.platform = value
    try:
        for text in texts:
            keys_rows.append({'platform': name, 'text': text, 'result': i18n.keys(text)})
    finally:
        sys.platform = real
with Case('i18n', 'keys') as case:
    case.json(rows=keys_rows)

# The shortcut table, written() and key_hint() on every platform.
dump = [{'group': e.group, 'keys': list(e.keys), 'text': e.text, 'label': e.label, 'repeat': e.repeat, 'focus': e.focus}
        for e in shortcut_list.TABLE]
written = []
hints = []
all_keys = [key for e in shortcut_list.TABLE for key in e.keys]
for name, value in platforms.items():
    sys.platform = value
    try:
        for number, entry in enumerate(shortcut_list.TABLE):
            for language in ('ko', 'en'):
                i18n.set_language(language)
                written.append({'platform': name, 'language': language, 'row': number, 'result': shortcut_list.written(entry)})
        for key in all_keys:
            hints.append({'platform': name, 'key': key, 'result': shortcut_list.key_hint(key)})
        try:
            shortcut_list.key_hint('F13')
            missing = 'found'
        except StopIteration:
            missing = 'StopIteration'
    finally:
        sys.platform = real
with Case('i18n', 'shortcuts') as case:
    case.json(groups=list(shortcut_list.GROUPS), table=dump, masks=[list(m) for m in shortcut_list.MASKS],
              labels=[[k, v] for k, v in shortcut_list.LABELS.items()], written=written, hints=hints,
              missing_hint=missing)

with Case('i18n', 'edit_fields') as case:
    case.json(fields=[[key, label, low, high] for key, (label, low, high) in edit_fields.EDIT_FIELDS.items()],
              types=[[type(low).__name__, type(high).__name__] for _, (_, low, high) in edit_fields.EDIT_FIELDS.items()])

i18n.set_language('ko')
print(f'{len(table)} table keys, {len(cases)} synthetic tr cases, {len(shortcut_list.TABLE)} shortcut rows')
