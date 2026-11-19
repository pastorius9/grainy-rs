"""Write THIRD_PARTY_NOTICES.txt for the release build of grainy-app and grainy-cli on Windows.

    python tools/release/notices.py [-o OUTPUT] [--target x86_64-pc-windows-msvc]

Sources: `cargo tree` (normal dependencies of grainy-app and grainy-cli for the target: what is linked into the
two programs) and `cargo metadata` (licence expressions and each crate's own licence files in the local cargo
registry; nothing is downloaded), plus the vendored C/C++ libraries, ported code and data with their licence texts
from this repository. MIT and Apache-2.0 texts are given once, with the copyright lines each crate ships; every
other licence text is reproduced in full as the crate ships it.

A crate whose licence expression cannot be satisfied with MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, CC0 or
MPL-2.0 licences is FLAGGED: the flags are printed at the end and listed in the file. Exit code 0 even with
flags (they are decisions for a person, not build errors); 2 when cargo fails.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PROGRAMS = ['grainy-app', 'grainy-cli']
ALLOWED = {
    'MIT', 'MIT-0', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception', 'BSD-2-Clause', 'BSD-3-Clause', '0BSD',
    'ISC', 'Zlib', 'Unicode-3.0', 'Unicode-DFS-2016', 'CC0-1.0', 'MPL-2.0',
}
SHORT = {'MIT', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception'}
LICENSE_FILE = re.compile(r'(licen[cs]e|copying|notice|copyright|^ofl|^ufl)', re.IGNORECASE)
WIDTH = 100


# ---- SPDX ----------------------------------------------------------------------------------------------------------

def tokens(expression: str):
    expression = expression.replace('/', ' OR ')
    return re.findall(r'\(|\)|[A-Za-z0-9.+\-]+', expression)


def parse(expression: str):
    """The expression as alternatives (disjunctive normal form): a list of frozensets of licence ids."""
    items = tokens(expression)
    pos = 0

    def primary():
        nonlocal pos
        if items[pos] == '(':
            pos += 1
            value = disjunction()
            pos += 1  # ')'
            return value
        name = items[pos]
        pos += 1
        if pos < len(items) and items[pos] == 'WITH':
            name = f'{name} WITH {items[pos + 1]}'
            pos += 2
        return [frozenset([name.rstrip('+')])]

    def conjunction():
        nonlocal pos
        value = primary()
        while pos < len(items) and items[pos] == 'AND':
            pos += 1
            right = primary()
            value = [a | b for a in value for b in right]
        return value

    def disjunction():
        nonlocal pos
        value = conjunction()
        while pos < len(items) and items[pos] == 'OR':
            pos += 1
            value = value + conjunction()
        return value

    return disjunction()


def choose(expression: str | None):
    """(licences used, flagged): MIT/Apache first, else the smallest allowed alternative, else flagged."""
    if not expression:
        return None, True
    alternatives = parse(expression)
    short = [alt for alt in alternatives if alt <= SHORT]
    if short:
        return sorted(short, key=lambda alt: ('MIT' not in alt, len(alt)))[0], False
    allowed = sorted((a for a in alternatives if a <= ALLOWED), key=len)
    if allowed:
        return allowed[0], False
    return alternatives[0], True


# ---- cargo ---------------------------------------------------------------------------------------------------------

def cargo(*args) -> str:
    result = subprocess.run(['cargo', *args], cwd=REPO, capture_output=True, text=True, encoding='utf-8')
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(2)
    return result.stdout


def linked_crates(target: str):
    args = ['tree', '-e', 'normal', '--target', target, '--prefix', 'none', '-f', '{p}']
    for program in PROGRAMS:
        args += ['-p', program]
    out = set()
    for line in cargo(*args).splitlines():
        m = re.match(r'(\S+) v(\S+)', line.strip())
        if m and '(proc-macro)' not in line:
            out.add((m.group(1), m.group(2)))
    return out


def crate_files(folder: Path):
    """The crate's own licence/notice files (root and one level down), as (relative name, text)."""
    found = []
    candidates = [p for p in folder.iterdir() if p.is_file()]
    for sub in folder.iterdir():
        if sub.is_dir() and sub.name.lower() in ('fonts', 'license', 'licenses', 'licence', 'licences'):
            candidates += [p for p in sub.iterdir() if p.is_file()]
    for p in sorted(candidates):
        rel = p.relative_to(folder).as_posix()
        if p.suffix.lower() in ('.rs', '.toml', '.orig', '.lock', '.ttf', '.otf', '.md') and not \
                LICENSE_FILE.search(p.stem):
            continue
        fonts_text = p.parent.name.lower() == 'fonts' and p.suffix.lower() == '.txt'
        if LICENSE_FILE.search(p.name) or fonts_text:
            try:
                found.append((rel, p.read_text(encoding='utf-8', errors='replace')))
            except OSError:
                pass
    return found


COPYRIGHT = re.compile(r'^\s*(?:copyright|\(c\)|©)\b.*', re.IGNORECASE)
TEMPLATE = re.compile(r'\[yyyy\]|\{yyyy\}|<year>|\[year\]|copyright notice|copyright owner|copyright holder|'
                      r'copyright law|copyright \(c\) <|copyright and license|copyright statement|copyright licen|'
                      r'copyright and related|copyright interest', re.IGNORECASE)


def copyright_lines(files):
    lines = []
    for _, text in files:
        for line in text.splitlines():
            line = line.strip().strip('#*/ ').strip()
            if COPYRIGHT.match(line) and not TEMPLATE.search(line) and len(line) < 200 and line not in lines:
                lines.append(line)
    return lines


# ---- the file ------------------------------------------------------------------------------------------------------

def rule(title: str, char: str = '=') -> str:
    return f'\n\n{char * WIDTH}\n{title}\n{char * WIDTH}\n'


def repo_text(rel: str) -> str:
    return (REPO / rel).read_text(encoding='utf-8', errors='replace').replace('\r\n', '\n').rstrip() + '\n'


VENDORED = [
    ('LibRaw 0.22.1 (RAW decoding; compiled into the program)', [
        'Copyright LibRaw LLC and the contributors identified in its source files.',
        'Source: https://github.com/LibRaw/LibRaw/releases/tag/0.22.1 (unmodified retained source files).',
        'LibRaw is offered under a choice of LGPL-2.1 or CDDL-1.0. Grainy RS uses it under the',
        'COMMON DEVELOPMENT AND DISTRIBUTION LICENSE (CDDL) Version 1.0, reproduced below.',
        'What this requires: the source code of the covered LibRaw files (and any modification of them) must',
        'be available to recipients under the CDDL; this notice and the licence text accompany the program.',
        'The LibRaw files are unmodified and published with the program\'s source code, in the folder',
        'crates/grainy-raw/vendor/LibRaw-0.22.1/ of the repository https://github.com/pastorius9/grainy-rs',
        '(both upstream licence files, LICENSE.CDDL and LICENSE.LGPL, are kept there). The rest of Grainy RS is',
        'a separate work under the MIT licence (CDDL section 3.6 allows distributing the larger work under',
        'other terms). The MSVC OpenMP runtime (vcomp140.dll of the Microsoft Visual C++ Redistributable) is',
        'used by LibRaw\'s parallel decoders; it is part of Windows\' Visual C++ runtime and not shipped here.',
    ], ['crates/grainy-raw/vendor/LibRaw-0.22.1/LICENSE.CDDL']),
    ('Little CMS 2.19 (colour management; compiled into the program)', [
        'Copyright (c) 1998-2023 Marti Maria Saguer. Source: https://github.com/mm2/Little-CMS/tree/lcms2.19',
        '(unmodified). Licence: MIT, reproduced below.',
    ], ['crates/grainy-cms/vendor/lcms2-2.19/LICENSE']),
    ('libjpeg-turbo 2.1.5.1 (JPEG; compiled into the program)', [
        'This software is based in part on the work of the Independent JPEG Group.',
        'Copyright the libjpeg-turbo contributors and the Independent JPEG Group; see the source notices.',
        'Source: https://github.com/libjpeg-turbo/libjpeg-turbo/tree/2.1.5.1 (unmodified retained files).',
        'Licences: the IJG License, the Modified (3-clause) BSD License and the zlib License, as described',
        'in its LICENSE.md and README.ijg, both reproduced below in full.',
    ], ['crates/grainy-jpeg-sys/vendor/libjpeg-turbo-2.1.5.1/LICENSE.md',
        'crates/grainy-jpeg-sys/vendor/libjpeg-turbo-2.1.5.1/README.ijg']),
    ('zlib 1.3.1 (compiled into the program for LibRaw)', [
        'Copyright (C) 1995-2022 Jean-loup Gailly and Mark Adler. Source: https://github.com/madler/zlib',
        '(unmodified). Licence: zlib, reproduced below.',
    ], ['crates/grainy-raw/vendor/zlib-1.3.1/LICENSE']),
]

PORTED = [
    ('NumPy (random number generator and numeric conventions, ported to Rust)', [
        'grainy-core and grainy-geometry re-implement NumPy\'s SeedSequence, PCG64 (after the PCG library by',
        'Melissa O\'Neill, Apache-2.0/MIT), normal sampling with its ziggurat tables (copied bit for bit),',
        'choice and pairwise summation. NumPy is BSD-3-Clause:',
    ], ['crates/grainy-core/NOTICE']),
    ('Pillow (bicubic rotation and affine transforms, ported to Rust)', [], ['crates/grainy-geometry/NOTICE']),
    ('OpenCV (image-processing primitives, source-derived Rust ports; no OpenCV code or binary is shipped)', [],
     ['crates/grainy-imgproc/NOTICE', 'crates/grainy-imgproc/SOURCE-NOTICES.txt',
      'crates/grainy-imgproc/LICENSE-OpenCV']),
    ('rawpy (LibRaw parameter handling ported in the C shim)', [
        'rawpy 0.27.1, https://github.com/letmaik/rawpy, MIT:'], ['crates/grainy-raw/reference/LICENSE.rawpy']),
    ('Unicode data (case folding tables generated from Unicode 15.1)', [],
     ['crates/grainy-raw/reference/LICENSE.unicode']),
    ('Adobe DNG SDK (algorithm reference only)', [
        'The DNG three-illuminant weighting and spectrum normalisation in grainy-color follow the Adobe DNG SDK',
        '1.7.1 (dng_xy_coord.cpp/.h, Copyright 2006-2019 Adobe Systems Incorporated). No DNG SDK source file',
        'is included; the computation was re-implemented. Source: https://www.adobe.com/go/dng_sdk',
    ], []),
]

DATA = [
    ('Lensfun lens database (built into the program)', [], ['crates/grainy-lens/data/lensfun/NOTICE']),
    ('CIE colorimetric data', [], ['crates/grainy-color/assets/ILLUMINANTS-NOTICE.txt']),
    ('Film looks: CIE data, makers\' data sheets, trademarks', [], ['crates/grainy-film/assets/film/NOTICE.txt']),
    ('Natural Earth (offline map)', [
        'The GPS map uses Natural Earth 1:110m countries (crates/grainy-app/assets/world.geojson).',
        'Made with Natural Earth. Free vector and raster map data @ naturalearthdata.com. Public domain.',
    ], []),
]

MIT_TEXT = """Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
IN THE SOFTWARE.
"""


def section(out, title, lines, files):
    out.append(rule(title, '-'))
    if lines:
        out.append('\n'.join(lines) + '\n')
    for rel in files:
        out.append(f'\n--- {Path(rel).name} ---\n\n')
        out.append(repo_text(rel))


def build(target: str):
    metadata = json.loads(cargo('metadata', '--format-version', '1', '--filter-platform', target))
    packages = {(p['name'], p['version']): p for p in metadata['packages']}
    workspace = set(metadata['workspace_members'])
    crates = []
    for key in sorted(linked_crates(target)):
        p = packages.get(key)
        if p is None or p['id'] in workspace or p.get('source') is None:
            continue
        crates.append(p)

    rows, flags, short_lines, full_texts = [], [], [], {}
    apache = None
    for p in crates:
        folder = Path(p['manifest_path']).parent
        expression = p.get('license') or (f"see {p['license_file']}" if p.get('license_file') else None)
        used, flagged = choose(p.get('license'))
        files = crate_files(folder)
        label = f"{p['name']} {p['version']}"
        rows.append((label, expression or 'NONE', 'FLAG' if flagged else '', p.get('repository') or ''))
        if flagged:
            flags.append((label, expression or 'no licence field'))
        if apache is None:
            for name, text in files:
                if 'apache' in name.lower() and 'TERMS AND CONDITIONS FOR USE' in text:
                    apache = text
        if used is not None and used <= SHORT and not flagged:
            lines = copyright_lines(files)
            if not lines and p.get('authors'):
                lines = ['Copyright the authors: ' + ', '.join(re.sub(r'\s*<[^>]*>', '', a) for a in p['authors'])]
            short_lines.append((label, ' + '.join(sorted(used)), lines))
        else:
            for name, text in files:
                if re.search(r'apache', name, re.IGNORECASE) and 'Apache-2.0' not in ' '.join(used or []):
                    continue
                digest = hashlib.sha256(text.strip().encode()).hexdigest()
                full_texts.setdefault(digest, (text, []))[1].append(f'{label} ({name})')
            if not files:
                flags.append((label, 'no licence file found in the crate package'))

    out = []
    out.append('Grainy RS 0.1.0: third-party notices\n')
    out.append('=' * WIDTH + '\n')
    out.append('Grainy RS (GrainyRS.exe, grainy-cli.exe) is MIT licensed, copyright (c) 2026 pastorius9 (see LICENSE).\n'
               'It includes the third-party software and data below, each under its own licence. Generated by\n'
               'tools/release/notices.py from the release build of grainy-app and grainy-cli for ' + target + '.\n'
               'Sections: 1. libraries in C/C++ compiled into the program; 2. code ported from other projects;\n'
               '3. data; 4. Rust crates (list, copyright lines, licence texts).\n')
    out.append(rule('1. C/C++ libraries compiled into the program'))
    for title, lines, files in VENDORED:
        section(out, title, lines, files)
    out.append(rule('2. Code ported from other projects'))
    for title, lines, files in PORTED:
        section(out, title, lines, files)
    out.append(rule('3. Data'))
    for title, lines, files in DATA:
        section(out, title, lines, files)

    out.append(rule(f'4. Rust crates ({len(rows)} crates linked into grainy-app and grainy-cli)'))
    out.append('Every crate with its version and licence expression as published on crates.io. FLAG marks an\n'
               'expression that cannot be met with MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, CC0 or MPL-2.0\n'
               'licences alone; their licence texts are reproduced in full in 4.4.\n\n')
    width = max(len(r[0]) for r in rows) + 2
    for label, expression, flag, _ in rows:
        out.append(f'{label:<{width}}{expression}{"   [" + flag + "]" if flag else ""}\n')
    if flags:
        out.append('\nFlagged:\n')
        for label, why in flags:
            out.append(f'  {label}: {why}\n')

    out.append(rule('4.1 Crates used under the MIT or the Apache-2.0 licence: copyright lines', '-'))
    for label, used, lines in short_lines:
        out.append(f'{label} ({used})\n')
        for line in lines:
            out.append(f'    {line}\n')
    out.append(rule('4.2 The MIT licence (for the crates in 4.1 marked MIT)', '-'))
    out.append('Copyright (c) the copyright holders named for each crate in 4.1.\n\n' + MIT_TEXT)
    out.append(rule('4.3 The Apache License, Version 2.0 (for the crates in 4.1 marked Apache-2.0)', '-'))
    out.append((apache or 'Apache License 2.0: https://www.apache.org/licenses/LICENSE-2.0\n').replace('\r\n', '\n'))
    out.append(rule('4.4 Other licence texts, in full, as the crates ship them', '-'))
    for text, users in full_texts.values():
        out.append('\nUsed by: ' + ', '.join(users) + '\n\n')
        out.append(text.replace('\r\n', '\n').rstrip() + '\n')
        out.append('-' * 40 + '\n')
    text = ''.join(out)
    text = re.sub(r'[ \t]+\n', '\n', text)
    return text, rows, flags


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('-o', '--output', default=str(REPO / 'target' / 'release' / 'THIRD_PARTY_NOTICES.txt'))
    parser.add_argument('--target', default='x86_64-pc-windows-msvc')
    args = parser.parse_args(argv)
    text, rows, flags = build(args.target)
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(text.replace('\n', '\r\n').encode('utf-8'))
    print(f'{output.name}: {len(rows)} Rust crates, {len(text.encode()) / 1024:.0f} KiB')
    if flags:
        print(f'{len(flags)} flagged:')
        for label, why in flags:
            print(f'  {label}: {why}')
    else:
        print('no licence flags')
    return 0


if __name__ == '__main__':
    sys.exit(main())
