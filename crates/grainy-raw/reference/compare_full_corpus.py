"""Audit native changes against the original, uncompressed 1,971 comparisons."""
import hashlib
import json
import re
from pathlib import Path

crate = Path(__file__).resolve().parents[1]
pattern = r'([A-Za-z0-9_/-]+): max (?:abs )?error ([\deE.+-]+) \(tolerance ([\deE.+-]+)\)'
metrics = []
hashes = []
for name in ['baseline.log', 'native-full.log']:
    raw = (crate / 'reference' / name).read_bytes()
    text = raw.decode('utf-16' if raw.startswith(b'\xff\xfe') else 'utf-8')
    tests = 16 if name == 'baseline.log' else 17
    assert f'test result: ok. {tests} passed; 0 failed;' in text and 'panicked at' not in text
    comparisons = {name: [float(error), float(tolerance)] for name, error, tolerance in re.findall(pattern, text)}
    assert len(comparisons) == 1971
    scalars = {name: float(value) for name, value in re.findall(r'METRIC (\w+): ([\deE.+-]+)', text)}
    metrics.append(dict(pixels=comparisons, scalars=scalars))
    hashes.append(hashlib.sha256(raw).hexdigest())
assert metrics[0] == metrics[1], 'native changes altered a measured full-corpus error'
result = dict(baseline_revision='1ad2337', golden_cases=1886, baseline_tests=16, current_tests=17,
              full_image_comparisons=1971, identical_measured_errors=True,
              baseline_log_sha256=hashes[0], native_log_sha256=hashes[1])
(crate / 'FULL_CORPUS_VALIDATION.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf8', newline='\n')
print(json.dumps(result, indent=2))
