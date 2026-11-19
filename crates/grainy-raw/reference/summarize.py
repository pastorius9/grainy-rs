"""Summarize `cargo test -p grainy-raw -- --nocapture` without changing goldens."""
import json
import re
from pathlib import Path

root = Path(__file__).resolve().parents[1]
raw = (root / 'reference/tests.log').read_bytes()
text = raw.decode('utf-16' if raw.startswith(b'\xff\xfe') else 'utf-8')
if 'test result: ok. 17 passed; 0 failed;' not in text or 'panicked at' in text:
    raise RuntimeError('A complete successful run of the 17 tests is required')
cases = []
functions = {}
for line in text.splitlines():
    metric = re.search(r'METRIC (\w+): ([\deE.+-]+)', line)
    if metric:
        functions[metric[1]] = float(metric[2])
    m = re.search(r'([A-Za-z0-9_/-]+): max (?:abs )?error ([\deE.+-]+) \(tolerance ([\deE.+-]+)\)', line)
    if not m:
        continue
    name, error, tolerance = m[1], float(m[2]), float(m[3])
    cases.append(dict(case=name, max_abs_error=error, tolerance=tolerance))
    function = ('decode' if '/source/' in name else 'postprocess') if name.startswith('decode_') else next(
        (fn for prefix,fn in [('develop_','develop_camera'),('camera_math_','develop_camera'),
                             ('plain_','linear_frame_plain'),('linear_frame_','linear_frame'),
                             ('false_color_','reduce_false_color'),('table_','table_map'),('tone_','tone_map')]
         if name.startswith(prefix)),None)
    if function:
        functions[function] = max(functions.get(function,0),error)
cases.sort(key=lambda case: case['case'])
golden = root.parents[1] / 'golden/raw'
counts = {}
sampled = 0
supported = 0
for p in golden.glob('*/case.json'):
    group = p.parent.name.split('_')[0]
    counts[group] = counts.get(group,0)+1
    value = json.loads(p.read_text(encoding='utf8'))
    sampled += 'sample' in value
    supported += group == 'decode' and 'error' not in value
expected = supported * 8 + sum(counts[group] for group in ['develop', 'camera', 'linear', 'false', 'plain', 'table', 'tone'])
assert len(cases) == len({case['case'] for case in cases}) == expected + sampled
statistics = [case for case in cases if case['case'].endswith('/statistics')]
cases = [case for case in cases if not case['case'].endswith('/statistics')]
assert len(cases) == expected and len(statistics) == sampled
result = dict(tests=17, failed=0, golden_cases=sum(counts.values()), golden_groups=counts,
              measured_function_max_abs_error=functions,
              assertions_without_numeric_error=['load_profile/compiled digest and payload', 'camera_matches',
                  'enabled/retarget', 'RAW defaults keys, serials, ISO values, rule identity and notes',
                  'Unicode 15.1 keys', 'RAW EXIF fields and CR3/RAF containers', 'invalid data rejection',
                  'one/four thread equality', 'development covering design and representative full frames'],
              pixel_comparisons=cases, full_frame_statistics_comparisons=statistics)
(root/'VALIDATION.json').write_text(json.dumps(result,indent=2,sort_keys=True)+'\n',encoding='utf-8',newline='\n')
print(json.dumps(dict(golden_cases=result['golden_cases'],comparisons=len(cases),maxima=functions),indent=2))
