"""Golden for grainy-mcp's built-in caption font: Pillow's `ImageFont.load_default(size)`.

`photo_images._font` tries Malgun Gothic, then DejaVu Sans, then `ImageFont.load_default(size)`: on macOS (no Malgun
Gothic, usually no DejaVu Sans) the contact sheets' captions are drawn with Pillow's own font, a subset of Aileron
Regular embedded in `PIL/ImageFont.py`. grainy-mcp carries the same bytes (`crates/grainy-mcp/assets/
pillow-default-font.ttf`). This writes

* `golden/mcp/builtin_font.json`: the bytes' SHA-256 and how Pillow lays text out with them (the samples and sizes of
  `fonts.json` of gen_mcp.py), compared on every system;
* `crates/grainy-mcp/assets/pillow-default-font-advances.json`: Pillow's advance of every character of the font at
  the caption sizes 13-22 (FreeType hints this font automatically, which changes advances by whole pixels; Pillow's
  basic layout adds them up with no kerning), which the Rust layout uses so captions are placed and clipped as the
  oracle's are.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_mcp_builtin_font.py
"""
import base64
import hashlib
import inspect
import json
import re
import struct

from common import GOLDEN, ROOT  # noqa: F401  (puts the oracle on sys.path)
from PIL import ImageFont, __version__ as PILLOW

MODULE = 'mcp'
SAMPLES = ['#1', '#123  3*  P', '#45  5*  X', 'not ready', 'garden.png', 'Kodak Portra 400', '한글 이름.jpg', '…', 'W', 'iiii']
SIZES = range(13, 23)          # photo_images.contact_sheet: max(13, min(22, cell // 20))
MISSING = '一'             # a character the font does not have: its .notdef glyph


def characters(data):
    """The characters the font maps to a glyph (its Unicode cmap, format 4)."""
    count = struct.unpack('>H', data[4:6])[0]
    tables = {data[12 + 16 * i:16 + 16 * i].decode(): struct.unpack('>II', data[20 + 16 * i:28 + 16 * i]) for i in range(count)}
    cmap = tables['cmap'][0]
    found = set()
    for i in range(struct.unpack('>H', data[cmap + 2:cmap + 4])[0]):
        platform, encoding, offset = struct.unpack('>HHI', data[cmap + 4 + 8 * i:cmap + 12 + 8 * i])
        table = cmap + offset
        if (platform, encoding) not in ((3, 1), (0, 3)) or struct.unpack('>H', data[table:table + 2])[0] != 4:
            continue
        segments = struct.unpack('>H', data[table + 6:table + 8])[0] // 2
        def field(n, k):
            return struct.unpack('>H', data[table + 14 + n * 2 * segments + (2 if n else 0) + 2 * k:][:2])[0]
        for k in range(segments):
            end, start, delta, range_offset = field(0, k), field(1, k), field(2, k), field(3, k)
            for code in range(start, end + 1):
                if code == 0xFFFF:
                    continue
                if range_offset == 0:
                    glyph = (code + delta) & 0xFFFF
                else:
                    at = table + 14 + 3 * 2 * segments + 2 + 2 * k + range_offset + 2 * (code - start)
                    glyph = struct.unpack('>H', data[at:at + 2])[0]
                    glyph = (glyph + delta) & 0xFFFF if glyph else 0
                if glyph:
                    found.add(chr(code))
    return ''.join(sorted(found))


# The bytes load_default decodes (its FreeType branch), read from Pillow's own source.
source = inspect.getsource(ImageFont.load_default)
data = base64.b64decode(re.search(r'base64\.b64decode\(\s*b"""(.*?)"""', source, re.S).group(1))
chars = characters(data)
assert MISSING not in chars
cases, advances = [], {}
for size in SIZES:
    font = ImageFont.load_default(size)
    cases.append({'size': size, 'metrics': list(font.getmetrics()), 'lengths': [font.getlength(t) for t in SAMPLES]})
    table = [font.getlength(c) for c in chars] + [font.getlength(MISSING)]
    assert all(a == int(a) for a in table), 'hinted advances are whole pixels'
    advances[str(size)] = [int(a) for a in table]
    # Basic layout: a text's length is the sum of its characters' advances (no kerning in this font).
    lookup = dict(zip(chars, table))
    for text in SAMPLES:
        assert sum(lookup.get(c, table[-1]) for c in text) == font.getlength(text), (size, text)
text = (
    '{\n "pillow": "' + PILLOW + '",\n "bytes": ' + str(len(data)) + ',\n "sha256": "' + hashlib.sha256(data).hexdigest()
    + '",\n "samples": ' + json.dumps(SAMPLES, ensure_ascii=False) + ',\n "cases": [\n'
    + ',\n'.join('  ' + json.dumps(c) for c in cases) + '\n ]\n}\n'
)
(GOLDEN / MODULE / 'builtin_font.json').write_text(text, encoding='utf-8', newline='\n')
asset = ROOT / 'crates' / 'grainy-mcp' / 'assets' / 'pillow-default-font-advances.json'
asset.write_text(
    '{\n "about": "Pillow ' + PILLOW + ' ImageFont.load_default(size).getlength of each character, then of a missing one'
    ' (tools/golden/gen_mcp_builtin_font.py)",\n "chars": ' + json.dumps(chars, ensure_ascii=True) + ',\n "advances": {\n'
    + ',\n'.join(f'  "{size}": {json.dumps(advances[size])}' for size in advances) + '\n }\n}\n',
    encoding='utf-8', newline='\n')
print('builtin font:', len(data), 'bytes,', len(chars), 'characters,', len(cases), 'sizes, Pillow', PILLOW)
