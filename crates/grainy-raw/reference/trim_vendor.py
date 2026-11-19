"""Prune tracked vendor files with git rm; never modify retained upstream bytes."""
from pathlib import Path
import re
import subprocess

crate = Path(__file__).resolve().parents[1]
root = crate.parents[1]
vendor = (crate / 'vendor').resolve()
assert vendor.is_relative_to(root.resolve()) and vendor.name == 'vendor'
build = (crate / 'build.rs').read_text()
jpeg = re.search(r'let jpeg_files = "([^"]+)"', build)[1].split()
zlib = 'adler32 compress crc32 deflate infback inffast inflate inftrees trees uncompr zutil'.split()
keep = set()
for library, stems in [('libjpeg-turbo-2.1.5.1', jpeg), ('zlib-1.3.1', zlib)]:
    directory = vendor / library
    pending = [directory / (stem + '.c') for stem in stems]
    pending.append(directory / ('jpeglib.h' if library.startswith('libjpeg') else 'zlib.h'))
    if library.startswith('libjpeg'):
        pending.append(directory / 'jversion.h.in')
    while pending:
        path = pending.pop().resolve()
        assert path.is_relative_to(vendor)
        if path in keep or not path.is_file():
            continue
        keep.add(path)
        for include in re.findall(r'#\s*include\s*"([^"]+)"', path.read_text(errors='replace')):
            pending.append(path.parent / include)
raw = vendor / 'LibRaw-0.22.1'
keep.update(p.resolve() for p in (raw / 'src').rglob('*.cpp') if not p.name.endswith('_ph.cpp'))
keep.update(p.resolve() for part in ['internal', 'libraw'] for p in (raw / part).rglob('*.h'))
for library in ['LibRaw-0.22.1', 'libjpeg-turbo-2.1.5.1', 'zlib-1.3.1']:
    keep.update(p.resolve() for p in (vendor / library).rglob('*') if p.is_file()
                and any(word in p.name.lower() for word in ['license', 'copying', 'copyright', 'readme']))
tracked = subprocess.check_output(['git', 'ls-files', '-z', '--', 'crates/grainy-raw/vendor'], cwd=root)
remove = [name for name in tracked.decode().split('\0') if name and (root / name).resolve() not in keep]
for start in range(0, len(remove), 40):
    subprocess.run(['git', 'rm', '-q', '--', *remove[start:start + 40]], cwd=root, check=True)
print(f'Removed {len(remove)} tracked files; retained {len(keep)} source/license/readme files.')
