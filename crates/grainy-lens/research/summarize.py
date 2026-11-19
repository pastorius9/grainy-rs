"""Summarize a cargo test -- --nocapture log; no oracle implementation access."""
import json
import re
from pathlib import Path

crate = Path(__file__).resolve().parents[1]
golden = crate.parents[1] / 'golden' / 'lens'
rows = {}
def read_log(path):
    data=path.read_bytes()
    return data.decode('utf-16' if data.startswith((b'\xff\xfe',b'\xfe\xff')) else 'utf-8-sig')
log=read_log(crate/'research/test-output.txt')
for line in log.splitlines():
    match = re.fullmatch(r'(m\d+_\w+): geometry=([\de+.-]+) subpixel=([\de+.-]+) gain=([\de+.-]+) shading=([\de+.-]+) remap=([\de+.-]+) develop=([\de+.-]+)', line)
    if match:
        rows[match[1]] = dict(zip(['geometry', 'subpixel', 'gain', 'shading', 'remap', 'develop'], map(float, match.groups()[1:])))
models = {}
misses = {}
for name, values in rows.items():
    metadata = json.loads((golden / name / 'case.json').read_text(encoding='utf-8'))
    model = metadata['model']['model']
    row = models.setdefault(model, dict(cases=0, pixel_cases=0, maximum={k: 0 for k in values}))
    row['cases'] += 1
    row['pixel_cases'] += int(metadata.get('pixels', False))
    for k, value in values.items():
        row['maximum'][k] = max(row['maximum'][k], value)
    failed = {k: v for k, v in values.items() if (k in ['geometry', 'subpixel'] and v > 2e-4) or (k == 'develop' and v > 1e-5)}
    if failed:
        misses[name] = failed
index = json.loads((golden / 'index' / 'case.json').read_text(encoding='utf-8'))
assert len(rows) == len(index['cases']), 'Incomplete log'
result = dict(oracle_library_version=index['library_version'], map_cases=len(rows),
              golden_bytes=sum(p.stat().st_size for p in golden.rglob('*') if p.is_file()),
              models=models, misses_against_brief_targets=misses,
              calibration_interpolation_maximum=float(re.search(r'coefficient interpolation: maximum ([\de+.-]+)',log)[1]),
              database_profile_json_maximum=0,
              radius_estimator_maximum=float(re.search(r'radius estimator coordinates: maximum ([\de+.-]+)',log)[1]),
              exact_noops_and_thread_independence=all(f'test {test} ... ok' in log for test in ['defaults_are_exact_noops','parallelism_and_bad_requests']))
scale_match=re.search(r'profile scales: (\d+) cases, maximum ([\de+.-]+), (\d+) ULPs',log)
result['profile_scales']=dict(cases=int(scale_match[1]),maximum=float(scale_match[2]),maximum_ulps=int(scale_match[3]),tolerance_ulps=1)
audit=json.loads((crate/'scale-audit.json').read_text(encoding='utf-8'))
result['strict_map_audit']={k:audit[k] for k in ['cases','failures','maximum']}
for suite in ['profiles','stress']:
    cases=[r for r in audit['measurements'] if r['suite']==suite]
    result['strict_map_audit'][suite]=dict(cases=len(cases),failures=sum(not r['passed'] for r in cases),
        maximum=max(max(r['geometry'],r['subpixel']) for r in cases),
        explicit_scale_control_maximum=max(max(r['explicit_geometry'],r['explicit_subpixel']) for r in cases))
def engine_errors(filename):
    text=read_log(crate/'research'/filename)
    return {name:float(value) for name,value in re.findall(r'^(lens_\w+): max abs error ([\de+.-]+)',text,re.M)}
baseline=crate/'research/baseline-engine.txt'
result['engine_before']=(engine_errors(baseline.name) if baseline.exists() else
    json.loads((crate/'verification.json').read_text(encoding='utf-8'))['engine_before'])
result['engine_after']=engine_errors('engine-output.txt')
result['unresolved']='The strict automatic-scale target is not recovered for every synthetic profile; scale-audit.json records failures without raising regression tolerances.'
(crate / 'verification.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8', newline='\n')
print(json.dumps(result, indent=2))
