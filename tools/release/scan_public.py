"""Scan a folder (the public copy of the repository) for private data before it is published.

    python tools/release/scan_public.py [FOLDER] [--names FILE] [--max-mb 50]

FOLDER defaults to `<repository>/../grainy-rs-public`. Every file is read as bytes (text and binary alike) and
searched case-insensitively in UTF-8 and UTF-16LE (both byte alignments) for:

* the Windows user name of this machine (read from the environment at run time; never printed),
* e-mail addresses,
* the company name, in any case (kept rot13-encoded below so that this file does not contain it),
* the owner's personal names: one per line in the names file (default `<repository>/.scan-names.txt`, which must
  stay untracked; the names are never printed: hits show `<name #k>`),
* `E:\\` paths and `C:\\Users\\` paths (both slash directions),
* the signing folder's name, private keys and tokens (GitHub `ghp_` and `github_pat_`, `sk-`, `AIza`, PEM
  headers). The patterns are assembled at run time so that this file does not match itself.

Containers are opened and their members scanned too: zip archives (`.zip`, `.npz`, ...), gzip files, and the
compressed text chunks of PNG files (zTXt, iTXt; tEXt and eXIf are plain bytes and found directly).

Every match is classified:

* **real**: counts; the scan fails (exit code 1) when there is at least one.
* **random binary match**: a match inside binary data whose neighbourhood is not printable text (a pattern that
  occurs by chance in pixel arrays); listed with file and offset, does not count. Literals of five or more
  characters (user name, names, company name, signing folder, PEM header) always count, also in binary data.
* **generic example path**: a `C:\\Users\\<x>` path whose user folder does not exist on this machine
  (`someone`), or an `E:\\<x>` path whose first folder does not exist on this machine (`E:\\Card`); test data.
* **placeholder e-mail**: `example.com/.org/.net`, `.example`, `.test`, `.invalid`, `localhost` and the
  public commit identity's GitHub noreply address.
* **third-party e-mail** / **third-party name match**: an address, or a word equal to one of the personal names,
  inside unmodified upstream sources, data and licence files (`vendor/`, `reference/`, the lensfun database,
  `LICENSE*`, `NOTICE*`, `COPYING*`, `SOURCE-NOTICES*`, `README.ijg`...): authors' contact addresses and
  names that the upstream licences require to be kept unchanged.

Pixel data is not text: the compressed image data of PNG files (IDAT) and the payload of numeric `.npy` arrays are
skipped (the `.npy` header is scanned; string arrays are decoded and scanned).

The report also lists files over the size limit (the scan fails when there is one), the largest file and the
total size. Matched private text (user name, names) is never written to the output.
"""

from __future__ import annotations

import argparse
import codecs
import gzip
import io
import os
import re
import string
import sys
import zipfile
import zlib
from collections import Counter, defaultdict
from pathlib import Path

try:
    import numpy as np  # only speeds up the UTF-16 views
except ImportError:  # pragma: no cover
    np = None

REPO = Path(__file__).resolve().parents[2]
import layout

DEFAULT_FOLDER = layout.public_folder()
DEFAULT_NAMES = REPO / '.scan-names.txt'
NOREPLY = '177642243+pastorius9@users.noreply.github.com'

PRINTABLE = set(bytes(string.printable, 'ascii'))
EMAIL = re.compile(rb'[a-z0-9._%+\-]{1,64}@[a-z0-9\-]{1,63}(?:\.[a-z0-9\-]{1,63})*\.[a-z]{2,24}(?![a-z0-9])')
E_PATH = re.compile(rb'(?<![a-z0-9])e:(?:\\|/)+([^\\/"\'\s<>|*?:\x00]{0,80})')
USERS_PATH = re.compile(rb'(?<![a-z0-9])c:(?:\\|/)+users(?:\\|/)+([^\\/"\'\s<>|*?:\x00]{0,80})')
TOKENS = [
    ('token ghp_', re.compile(rb'(?<![a-z0-9_])ghp_[a-z0-9]{30,}')),
    ('token github_pat_', re.compile(rb'(?<![a-z0-9_])github_pat_[a-z0-9_]{20,}')),
    ('token sk-', re.compile(rb'(?<![a-z0-9_\-])sk-(?:proj-|ant-)?[a-z0-9_\-]{20,}')),
    ('token AIza', re.compile(rb'(?<![a-z0-9_])aiza[0-9a-z_\-]{30,}')),
    ('private key / certificate', re.compile(re.escape(b'-' * 5 + b'begin'))),
    ('signing folder', re.compile(re.escape(b'.grainy' + b'-signing'))),
    ('company name', re.compile(re.escape(codecs.decode('ilnfg', 'rot13').encode()))),
]
# literal patterns long enough to be real wherever they occur, also in binary data
STRONG_TOKENS = ('private key / certificate', 'signing folder', 'company name')
PLACEHOLDER_DOMAINS = ('example.com', 'example.org', 'example.net', '.example', '.test', '.invalid', 'localhost')
THIRD_PARTY_PARTS = ('vendor', 'reference', 'lensfun')
THIRD_PARTY_NAMES = re.compile(r'^(license|licence|notice|copying|source-notices|readme\.ijg|authors|copyright)',
                               re.IGNORECASE)
ZIP_SUFFIXES = {'.zip', '.npz', '.docx', '.xlsx', '.jar', '.whl'}


def literal_patterns(user: str, names: list[str]) -> list[tuple[str, bytes, str, bool]]:
    """(category, lower-case bytes, label shown, strong) for literal tokens in UTF-8 and UTF-16LE."""
    out = []
    items = []
    if user:
        items.append(('user name', user, '<user name>'))
    for k, name in enumerate(names, 1):
        items.append(('personal name', name, f'<name #{k}>'))
    for category, text, label in items:
        low = text.lower()
        # a literal of 5 or more characters does not occur by chance in binary data: always a real hit there
        strong = len(text) >= 5
        for encoding in ('utf-8', 'utf-16-le'):
            out.append((category, low.encode(encoding), label, strong))
        if text != low:  # non-ASCII letters: lower() of raw bytes does not fold them; search the original too
            for encoding in ('utf-8', 'utf-16-le'):
                out.append((category, text.encode(encoding), label, strong))
    return out


def utf16_views(data: bytes):
    """ASCII-only projections of UTF-16LE text at both alignments: (bytes, map back to offsets)."""
    for start in (0, 1):
        chunk = data[start:]
        if len(chunk) < 4:
            continue
        lows = chunk[0::2]
        highs = chunk[1::2]
        n = min(len(lows), len(highs))
        if n == 0 or highs[:n].count(0) < 4:
            continue
        # keep a character only where its high byte is zero; others become a separator
        if np is not None:
            lo = np.frombuffer(lows[:n], dtype=np.uint8)
            hi = np.frombuffer(highs[:n], dtype=np.uint8)
            view = np.where(hi == 0, lo, 0).astype(np.uint8).tobytes()
        else:
            view = bytes(lo if hi == 0 else 0 for lo, hi in zip(lows[:n], highs[:n]))
        yield view, start


def printable_context(data: bytes, start: int, end: int, step: int) -> bool:
    """Whether the bytes around a match look like text (16 characters on each side, 75% printable)."""
    before = data[max(0, start - 16 * step):start][::step] if step == 1 else data[max(0, start - 32):start:2]
    after = data[end:end + 16 * step:step]
    around = before + after
    if not around:
        return True
    good = sum(1 for b in around if b in PRINTABLE)
    return good >= 0.75 * len(around)


def is_text(data: bytes) -> bool:
    if b'\x00' in data[:65536]:
        return False
    try:
        data[:65536].decode('utf-8')
        return True
    except UnicodeDecodeError as e:
        return e.start > 65530  # a character cut at the end of the sample


class Scan:
    def __init__(self, root: Path, user: str, names: list[str]):
        self.root = root
        self.user = user
        self.literals = literal_patterns(user, names)
        self.mask = [(user, '<user name>')] if user else []
        self.mask += [(n, f'<name #{k}>') for k, n in enumerate(names, 1)]
        self.hits = defaultdict(list)  # class -> [(file, offset, category, shown)]
        self.files = 0
        self.members = 0
        self.total = 0
        self.sizes = []

    def masked(self, text: str) -> str:
        for secret, label in self.mask:
            text = re.sub(re.escape(secret), label, text, flags=re.IGNORECASE)
        return text

    def add(self, cls: str, where: str, offset: int, category: str, shown: str):
        self.hits[cls].append((where, offset, category, self.masked(shown)))

    # ---- one file -------------------------------------------------------------------------------------------

    def scan_bytes(self, where: str, rel: Path, data: bytes, depth: int = 0, name: bool = False):
        text = is_text(data)
        if not name:
            data = self.neutralize(where, rel, data, depth)
        low = data.lower()
        third_party = any(p in THIRD_PARTY_PARTS for p in rel.parts[:-1]) or bool(THIRD_PARTY_NAMES.match(rel.name))
        views = [(low, 0, 1)] + [(v.lower(), s, 2) for v, s in utf16_views(data)]
        for view, shift, step in views:
            def offset(i):
                return i if step == 1 else shift + 2 * i

            def binary_noise(i, j):
                if text and step == 1:
                    return False
                return not printable_context(data, offset(i), offset(j), step)

            # literal tokens (UTF-8 and UTF-16LE bytes): only on the plain view
            if step == 1:
                for category, token, label, strong in self.literals:
                    for m in re.finditer(re.escape(token), view):
                        noise = not strong and binary_noise(m.start(), m.end())
                        cls = 'random binary match' if noise else 'real'
                        if cls == 'real' and third_party and category == 'personal name':
                            cls = 'third-party name match'
                        self.add(cls, where, m.start(), category, label)
            for m in EMAIL.finditer(view):
                if binary_noise(m.start(), m.end()):
                    self.add('random binary match', where, offset(m.start()), 'e-mail', '(binary)')
                    continue
                address = m.group(0).decode('ascii', 'replace')
                domain = address.split('@', 1)[1]
                if address == NOREPLY or any(domain == d.lstrip('.') or domain.endswith(d if d.startswith('.')
                                                                                      else '.' + d) or domain == d
                                                 for d in PLACEHOLDER_DOMAINS):
                    self.add('placeholder e-mail', where, offset(m.start()), 'e-mail', address)
                elif third_party:
                    self.add('third-party e-mail', where, offset(m.start()), 'e-mail', '…@' + domain)
                else:
                    local = address.split('@', 1)[0]
                    self.add('real', where, offset(m.start()), 'e-mail', local[:1] + '***@' + domain)
            for regex, category in ((E_PATH, 'E:\\ path'), (USERS_PATH, 'C:\\Users path')):
                for m in regex.finditer(view):
                    if binary_noise(m.start(), m.end()):
                        self.add('random binary match', where, offset(m.start()), category, '(binary)')
                        continue
                    segment = m.group(1).decode('latin-1').strip().rstrip('.`);,]')
                    # Machine-independent test folders used by the tests and goldens (a memory card, a
                    # fallback drive, GitHub's runner account); anything else on E:\ or under C:\Users counts.
                    examples = {'', 'public', 'default', 'all users', 'default user', 'someone',
                                'example', 'user', 'test', 'testuser', 'generated', 'photos',
                                'card', 'dcim', 'archive', 'x', 'fallback', 'runner~1', 'runneradmin'}
                    if not text and len(segment) <= 2 and segment.casefold() not in examples:
                        # Two bytes after "e:/" inside a binary file (pixel or table data), not a folder name.
                        self.add('random binary match', where, offset(m.start()), category, '(binary)')
                        continue
                    cls = 'generic example path' if segment.casefold() in examples else 'real'
                    shown = '(path withheld)'
                    self.add(cls, where, offset(m.start()), category, shown)
            for category, regex in TOKENS:
                for m in regex.finditer(view):
                    strong = category in STRONG_TOKENS
                    cls = 'random binary match' if not strong and binary_noise(m.start(), m.end()) else 'real'
                    shown = m.group(0).decode('latin-1')
                    if category.startswith('token'):
                        shown = shown[:6] + '...'
                    if category == 'company name':
                        shown = '<company name>'
                    if category == 'private key / certificate':
                        shown = data[offset(m.start()):offset(m.start()) + 40 * step:step].decode('latin-1')
                    self.add(cls, where, offset(m.start()), category, shown)
        if depth < 3 and not name:
            self.containers(where, rel, data, depth)

    def neutralize(self, where: str, rel: Path, data: bytes, depth: int) -> bytes:
        """`data` with its pixel payloads zeroed (same length, so offsets stay): PNG IDAT, numeric .npy arrays."""
        if data[:8] == b'\x89PNG\r\n\x1a\n':
            out = bytearray(data)
            pos = 8
            while pos + 8 <= len(data):
                length = int.from_bytes(data[pos:pos + 4], 'big')
                if data[pos + 4:pos + 8] == b'IDAT':
                    out[pos + 8:pos + 8 + length] = bytes(min(length, len(data) - pos - 8))
                if data[pos + 4:pos + 8] == b'IEND':
                    break
                pos += 12 + length
            return bytes(out)
        if data[:6] == b'\x93NUMPY':
            major = data[6]
            if major == 1:
                hlen, start = int.from_bytes(data[8:10], 'little'), 10
            else:
                hlen, start = int.from_bytes(data[8:12], 'little'), 12
            header = data[start:start + hlen].decode('latin-1')
            m = re.search(r"'descr':\s*'([<>|=]?)([a-zA-Z])(\d*)'", header)
            kind = m.group(2) if m else 'O'
            body = start + hlen
            if kind == 'U':
                self.scan_bytes(where + '!unicode', rel.with_suffix('.txt'),
                                data[body:].decode('utf-32-le', 'replace').replace('\x00', '\n').encode('utf-8'),
                                depth + 1)
            if kind in 'SO':
                return data
            return data[:body] + bytes(len(data) - body)
        return data

    def containers(self, where: str, rel: Path, data: bytes, depth: int):
        if data[:4] == b'PK\x03\x04' or rel.suffix.lower() in ZIP_SUFFIXES:
            try:
                with zipfile.ZipFile(io.BytesIO(data)) as archive:
                    for info in archive.infolist():
                        if info.is_dir():
                            continue
                        self.members += 1
                        member = archive.read(info)
                        name = f'{where}!{info.filename}'
                        self.scan_bytes(name, rel / info.filename, member, depth + 1)
                        # the member's own name
                        self.scan_bytes(name + ' (name)', rel / info.filename, info.filename.encode(), depth + 1,
                                        name=True)
            except (zipfile.BadZipFile, NotImplementedError, OSError, RuntimeError) as e:
                if rel.suffix.lower() in ZIP_SUFFIXES:
                    self.add('real', where, 0, 'unreadable archive', str(e))
        elif data[:2] == b'\x1f\x8b':
            try:
                self.scan_bytes(where + '!gunzip', rel.with_suffix(''), gzip.decompress(data), depth + 1)
            except (OSError, EOFError, zlib.error):
                pass
        elif data[:8] == b'\x89PNG\r\n\x1a\n':
            pos = 8
            while pos + 8 <= len(data):
                length = int.from_bytes(data[pos:pos + 4], 'big')
                kind = data[pos + 4:pos + 8]
                body = data[pos + 8:pos + 8 + length]
                if kind == b'zTXt':
                    key, _, rest = body.partition(b'\x00')
                    self.inflate(where, rel, f'zTXt {key[:40]!r}', rest[1:], depth)
                elif kind == b'iTXt':
                    key, _, rest = body.partition(b'\x00')
                    if len(rest) >= 2 and rest[0] == 1:
                        parts = rest[2:].split(b'\x00', 2)
                        if len(parts) == 3:
                            self.inflate(where, rel, f'iTXt {key[:40]!r}', parts[2], depth)
                elif kind == b'iCCP':
                    key, _, rest = body.partition(b'\x00')
                    self.inflate(where, rel, 'iCCP', rest[1:], depth)
                if kind == b'IEND':
                    break
                pos += 12 + length

    def inflate(self, where, rel, label, payload, depth):
        try:
            self.scan_bytes(f'{where}!{label}', rel, zlib.decompress(payload), depth + 1)
        except zlib.error:
            self.add('real', where, 0, 'unreadable PNG chunk', label)

    def run(self):
        for path in sorted(self.root.rglob('*')):
            rel = path.relative_to(self.root)
            if rel.parts and rel.parts[0] == '.git':
                continue
            if path.is_symlink():
                self.add('real', rel.as_posix(), 0, 'symbolic link', 'symbolic links are not expected')
                continue
            # file and folder names themselves
            self.scan_bytes(rel.as_posix() + ' (name)', rel, str(rel).encode('utf-8'), name=True)
            if not path.is_file():
                continue
            size = path.stat().st_size
            self.files += 1
            self.total += size
            self.sizes.append((size, rel.as_posix()))
            self.scan_bytes(rel.as_posix(), rel, path.read_bytes())


def read_names(path: Path) -> list[str] | None:
    if not path.is_file():
        return None
    names = []
    for line in path.read_text(encoding='utf-8-sig').splitlines():
        line = line.strip()
        if line and not line.startswith('#'):
            names.append(line)
    return names


def windows_user() -> str:
    for value in (os.environ.get('USERNAME'), Path(os.environ.get('USERPROFILE', '')).name):
        if value and value.strip():
            return value.strip()
    return ''


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('folder', nargs='?', default=str(DEFAULT_FOLDER))
    parser.add_argument('--names', default=str(DEFAULT_NAMES), help='one personal name per line (never printed)')
    parser.add_argument('--max-mb', type=float, default=50.0)
    parser.add_argument('--show', type=int, default=40, help='entries listed per class (real hits: all)')
    args = parser.parse_args(argv)
    root = Path(args.folder)
    if not root.is_dir():
        print(f'no such folder: {root}')
        return 2
    names = read_names(Path(args.names))
    user = windows_user()
    print(f'scanning {root}')
    print(f'Windows user name: {"read from the environment" if user else "NOT FOUND in the environment"}')
    if not names:
        print('publication scan requires a nonempty personal names file')
        return 2
    else:
        print(f'personal names: {len(names)} read from the names file')
    scan = Scan(root, user, names)
    scan.run()

    print(f'\nfiles: {scan.files} (+ {scan.members} archive members), total size {scan.total / 1e6:.1f} MB')
    scan.sizes.sort(reverse=True)
    if scan.sizes:
        size, name = scan.sizes[0]
        print(f'largest file: {name} ({size / 1e6:.2f} MB)')
    big = [(s, n) for s, n in scan.sizes if s > args.max_mb * 1e6]
    print(f'files over {args.max_mb:g} MB: {len(big)}')
    for s, n in big:
        print(f'  {n} ({s / 1e6:.1f} MB)')

    order = ['real', 'random binary match', 'generic example path', 'placeholder e-mail', 'third-party e-mail',
             'third-party name match']
    for cls in order:
        entries = scan.hits.get(cls, [])
        print(f'\n{cls}: {len(entries)}')
        if not entries:
            continue
        by_category = Counter(c for _, _, c, _ in entries)
        print('  by kind: ' + ', '.join(f'{k} {v}' for k, v in sorted(by_category.items())))
        if cls in ('third-party e-mail', 'third-party name match'):
            files = Counter(w.split('!')[0] for w, _, _, _ in entries)
            print(f'  in {len(files)} upstream files, e.g. ' + ', '.join(f for f, _ in files.most_common(5)))
            continue
        if cls in ('generic example path', 'placeholder e-mail'):
            shown = Counter((c, s) for _, _, c, s in entries)
            for (c, s), k in shown.most_common(args.show):
                print(f'  {k:5d} x {c}: {s}')
            continue
        limit = None if cls == 'real' else args.show
        for where, offset, category, shown in entries[:limit]:
            print(f'  {where} @ {offset}: {category}: {shown}')
        if limit is not None and len(entries) > limit:
            print(f'  ... {len(entries) - limit} more')
    real = len(scan.hits.get('real', []))
    ok = real == 0 and not big
    print(f'\nRESULT: {"PASS" if ok else "FAIL"} ({real} real hits, {len(big)} files over the limit)')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
