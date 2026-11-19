"""Check the 20 MB budget, shared pools, and byte-identical regeneration.

Run --snapshot, run gen_raw.py again, then run --check. Snapshot includes the
generated Unicode Rust source as well as every file below golden/raw.
"""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--snapshot', action='store_true')
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
crate = Path(__file__).resolve().parents[1]
root = crate.parents[1]
golden = root / 'golden/raw'
files = sorted(p for p in golden.rglob('*') if p.is_file())
size = sum(p.stat().st_size for p in files)
assert size <= 20_000_000, size
references = set()
sampled = full = 0
for path in golden.glob('*/case.json'):
    meta = json.loads(path.read_text(encoding='utf8'))
    for key in meta.get('arrays', {}).values():
        array = golden / '_arrays' / (key + '.npy')
        if array not in references:
            assert array.exists() and hashlib.sha256(array.read_bytes()).hexdigest() == key
        references.add(array)
    if 'sample' in meta:
        sampled += 1
    elif 'output' in meta.get('arrays', {}):
        full += 1
assert references == set((golden / '_arrays').glob('*.npy')), 'unreferenced arrays'
snapshot = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in files + [crate / 'src/unicode_data.rs']}
record = crate / 'reference/golden-snapshot.json'
if args.snapshot:
    record.write_text(json.dumps(snapshot, sort_keys=True), encoding='utf8')
if args.check:
    old = json.loads(record.read_text(encoding='utf8'))
    changed = [key for key in snapshot.keys() | old.keys() if snapshot.get(key) != old.get(key)]
    assert not changed, changed
result = dict(bytes=size, files=len(files), shared_arrays=len(references),
              sampled_outputs=sampled, full_outputs=full,
              reproducible=args.check, tree_sha256=hashlib.sha256(
                  json.dumps(snapshot, sort_keys=True).encode()).hexdigest())
print(json.dumps(result, indent=2))
if args.check:
    (crate / 'FIXTURE_VALIDATION.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf8', newline='\n')
