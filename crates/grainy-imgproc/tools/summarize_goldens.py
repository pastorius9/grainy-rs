"""Turn cargo test -- --nocapture output into a checked, per-variant error report."""
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
ROOT = CRATE.parents[1]
GOLDEN = ROOT / 'golden' / 'imgproc'
manifest = json.loads((GOLDEN / 'manifest.json').read_text(encoding='utf-8'))
raw = (CRATE / 'validation.log').read_bytes()
log = raw.decode('utf-16') if raw.startswith(b'\xff\xfe') else raw.decode('utf-8',errors='replace')
if 'test result: FAILED' in log or 'error: could not compile' in log:
    sys.exit('Refusing to summarize a failing test run')
measured = {}
names = set(manifest['cases'])
for label,error in re.findall(r'^([^\r\n]+): max abs error ([0-9.eE+\-]+)',log,re.M):
    if 'tier2/' in label or label.endswith('strided NS'):
        continue
    # libtest progress (including its trailing "ok") can precede a println
    # on the same line when golden groups run concurrently.
    matches = [label] if label in names else [name for name in names if label.endswith(name)]
    assert len(matches) == 1, label
    measured[matches[0]] = float(error)
groups = defaultdict(list)
cases = []
for name in manifest['cases']:
    meta = json.loads((GOLDEN/name/'case.json').read_text(encoding='utf-8'))
    if meta.get('expected_error'):
        assert f'{name}: rejected degenerate system' in log
        continue
    assert name in measured, f'No measured result for {name}'
    error = measured[name]
    # The Rust comparison is full precision; its stdout rounds to significant digits.
    assert error <= meta['tolerance']*1.001, (name,error,meta['tolerance'])
    variant = meta['dtype'] or meta['output_dtype']
    for field in ['precision','code','operation','interpolation','border','kind','normalize']:
        if meta.get(field) is not None:
            variant += f', {field}={meta[field]}'
    groups[(meta['op'],variant)].append((error,meta['tolerance'],name))
    cases.append({'case':name,'max_abs_error':error,'tolerance':meta['tolerance']})

mapping = {'kernel':'get_gaussian_kernel / _f64','gaussian':'gaussian_blur','sep':'sep_filter_2d',
           'box':'box_filter','median':'median_blur','rotation':'get_rotation_matrix_2d',
           'perspective_matrix':'get_perspective_transform','structuring':'get_structuring_element',
           'color':'cvt_color','morph':'erode / dilate / morphology_ex'}
lines = ['# Oracle validation', '', f"OpenCV {manifest['opencv']}, NumPy {manifest['numpy']}. "
         f"{len(manifest['cases'])} committed cases: {len(cases)} numeric comparisons and "
         f"{len(manifest['cases'])-len(cases)} deliberate invalid-input rejection.", '',
         f"CPU features: `{manifest['cpu_features']}`. Optimizations and IPP remain enabled; OpenCL is disabled.", '',
         'All errors are maximum absolute differences in native output units. Uint8 errors are byte levels; '
         'HSV/HLS hue errors are degrees. Numbers below retain the test log precision. '
         'Every numeric case and its individual tolerance is recorded in `golden_errors.json`.', '',
         '| Rust function | Variant | Cases | Maximum error | Tolerance ceiling |',
         '|---|---|---:|---:|---:|']
for (op,variant),values in sorted(groups.items()):
    lines.append(f"| {mapping.get(op,op)} | {variant} | {len(values)} | {max(v[0] for v in values):.9g} | {max(v[1] for v in values):.9g} |")
lines += ['', 'Regenerate goldens with the oracle interpreter and `-B`; run '
          '`cargo test -p grainy-imgproc -- --nocapture > crates/grainy-imgproc/validation.log 2>&1`, '
          'then run this script. The test suite also checks invalid inputs, exact identities, '
          'noncontiguous layouts, and identical output with one and four Rayon threads.', '',
          'The degenerate perspective correspondence golden retains OpenCV 5\'s nonunique SVD matrix '
          'for inspection; the Rust API deliberately returns an error. See README.md.', '']
if (GOLDEN/'manifest_tier2.json').exists():
    manifest2=json.loads((GOLDEN/'manifest_tier2.json').read_text(encoding='utf-8'))
    measured2={}
    for match in re.finditer(r'tier2/(\w+)/(\d+)(?:/(\d+))?(?:/[^\r\n:]+)?: (equal|max abs error ([0-9.eE+\-]+))',log):
        op,index,sub,kind,error=match.groups()
        key=(op,int(index),None if sub is None else int(sub))
        assert key not in measured2,key
        measured2[key]=0. if kind=='equal' else float(error)
    rows2=defaultdict(list);cases2=[];variants2=0
    def add_result(op,index,meta,variant='',sub=None):
        key=(op,index,sub)
        assert key in measured2,f'Missing tier 2 measurement: {key}'
        error=measured2[key]
        tolerance=meta.get('tolerance',1e-5 if op=='moments' else 0.)
        assert error<=tolerance*1.001,(key,error,tolerance)
        rows2[(op,variant)].append((error,tolerance))
        cases2.append(dict(function=op,variant=variant,index=index,sub=sub,label=meta.get('label'),
                           max_abs_error=error,tolerance=tolerance))
    for name in manifest2['cases']:
        meta=json.loads((GOLDEN/name/'case.json').read_text(encoding='utf-8'))
        op=meta['op'];variants2+=len(meta['variants'])
        for i,j in enumerate(meta['variants']):
            if op=='lines':
                add_result('lsd',i,j,'LSD_REFINE_STD/defaults')
                for k,p in enumerate(j['hough']):add_result('hough',i,dict(p,label=j['label']),'oracle and alternate rho/theta/gap',k)
            else:
                variant=', '.join(f'{k}={j[k]}' for k in ['size','connectivity','mode','thickness','binary','dtype','method'] if k in j)
                add_result(op,i,j,variant)
    lines+=['## Tier 2','',f"{len(manifest2['cases'])} packed folders, {variants2} variants; "
            'the line variants each test both LSD and three Hough parameter sets. All arrays load through '
            '`grainy-testkit`. All structural and integer comparisons use exact equality.', '',
            '| Rust function | Variants | Cases | Largest error | Tolerance |','|---|---|---:|---:|---:|']
    mapping2={'distance':'distance_transform','components':'connected_components / connected_components_with_stats',
        'flood':'flood_fill','contours':'find_contours + contour geometry','polygon':'contour_area / arc_length / bounding_rect / is_contour_convex',
        'hull':'convex_hull','drawing':'draw_contours','lsd':'line_segment_detector','hough':'hough_lines_p',
        'bilateral':'bilateral_filter','template':'match_template'}
    for (op,variant),values in sorted(rows2.items()):
        maximum=max(v[0] for v in values);tolerance=max(v[1] for v in values)
        result='equal' if maximum==0 else f'{maximum:.9g}'
        lines.append(f'| {mapping2.get(op,op)} | {variant or "all golden inputs"} | {len(values)} | {result} | {tolerance:.9g} |')
    lines+=['', 'Every contour case also checks signed/unsigned area, open/closed arc length, bounding rectangle, '
            'strict convexity, and both convex-hull windings, all exactly. Each components case tests both public functions. '
            'The private LSD Gaussian and INTER_LINEAR_EXACT stages are separately byte-exact on every line input.', '',
            'Tier 2 also checks invalid inputs, empty/full-mask identities, strided storage, extreme offscreen drawing '
            'coordinates, and one/four-thread equality. No global error bound or full-resolution throughput claim is implied '
            'by the sampled goldens. The float32 IPP distance reference and cross-correlation rounding can diverge more '
            'as dimensions and value magnitudes increase; the listed measurements retain native units.', '',
            'Run the generator with `--tier2` to regenerate only the added cases and call-site inventory. '
            'The ordinary generator invocation regenerates both tiers. `golden_errors_tier2.json` contains every '
            'tier 2 measured error; the `case.json` files specify the oracle arguments.', '']
    (CRATE/'golden_errors_tier2.json').write_text(json.dumps(cases2,indent=1)+'\n',encoding='utf-8',newline='\n')
    print(f'Tier 2: {variants2} variants, {len(cases2)} comparisons')
    overall2=defaultdict(list)
    for (op,variant),values in rows2.items():overall2[op].extend(values)
    for op,values in sorted(overall2.items()):print(f'{op:22} cases={len(values):4} max={max(v[0] for v in values):.9g}')
(CRATE/'VALIDATION.md').write_text('\n'.join(lines),encoding='utf-8',newline='\n')
(CRATE/'golden_errors.json').write_text(json.dumps(cases,indent=1)+'\n',encoding='utf-8',newline='\n')
overall = defaultdict(list)
for (op,variant),values in groups.items():
    overall[(op,'u8' if variant.startswith('uint8') else 'float')].extend(values)
for (op,dtype),values in sorted(overall.items()):
    worst = max(values)
    print(f'{op:22} {dtype:5} cases={len(values):4} max={worst[0]:.9g} ({worst[2]})')
