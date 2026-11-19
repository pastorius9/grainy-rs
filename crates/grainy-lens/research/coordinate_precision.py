"""Compare grid arithmetic using public numeric outputs, never native source.

Run with the oracle interpreter from the worktree root. Reads generated cases
and our measured square-root table; does not write into the oracle repository.
"""
from probe import *
from gen_lens import measured_scale
import itertools
import json
import math
import re

crate = Path(__file__).resolve().parents[1]
table = np.array([int(x, 16) for x in re.findall('0x[0-9a-f]+',
                  (crate / 'src/sqrt_table.rs').read_text())], np.uint32).view(np.float32)


def roots(r2):
    bits = r2.view(np.uint32)
    exponent = (bits >> 23).astype(np.int64) - 127
    index = ((bits & 0x7fffff) | ((exponent % 2).astype(np.uint32) << 23)) >> 12
    bits = table[index].view(np.uint32).astype(np.int64) + ((exponent // 2) << 23)
    return bits.astype(np.uint32).view(np.float32)


for name in ['m0_focal1', 'm4_flags100', 'm5_focal1']:
    j = json.loads((crate.parents[1] / 'golden/lens' / name / 'case.json').read_text(encoding='utf-8'))
    s = j['settings']; p = s['lensfun']; w = j['width']; h = j['height']
    _, lens = optics._saved_lens(p['xml'])
    native = optics._modifier(s, (h, w, 3)).apply_geometry_distortion()
    flags = 8 | (1 if s['lensfun_tca'] else 0) | 32
    scales = measured_scale(lens, p['crop'], w, h, p['focal'], p['aperture'], p['distance'], flags)[0] if s['lensfun_scale'] else [1]
    scale = np.float32(scales[0])
    a, b, c = np.array(lens.interpolate_distortion(p['focal']).terms, np.float32)
    d = np.float32(1 - (a + b + c))
    aspect = ET.fromstring(p['xml']).findtext('./lens/aspect-ratio', '1.5')
    ar = float(np.float32(float(aspect) if ':' not in aspect else
                         float(aspect.split(':')[0]) / float(aspect.split(':')[1])))
    for ratio32, step32 in itertools.product([False, True], repeat=2):
        q = (ar * ar + 1) / ((w - 1) ** 2 + (h - 1) ** 2)
        norm = 2 * math.sqrt(float(np.float32(q)) if ratio32 else q) * lens.crop_factor / float(np.float32(p['crop']))
        cx = (w - 1) * norm / 2; cy = (h - 1) * norm / 2
        x = np.float32(-cx); xs = []
        for _ in range(w):
            xs.append(x)
            x = np.float32(x + np.float32(norm)) if step32 else np.float32(float(x) + norm)
        xs = np.array(xs, np.float32) * np.float32(1 / scale)
        y = np.float32(-cy); out = np.zeros_like(native)
        for yi in range(h):
            yy = np.float32(y * np.float32(1 / scale))
            r2 = xs * xs + yy * yy; r = np.sqrt(r2)
            f = a * r2 * r + b * r2 + c * r + d
            fx = f.copy(); fy = f.copy()
            count = w // 4 * 4 if (yi * w) % 2 == 0 else 0
            if count:
                rr = roots(r2[:count])
                ff = a * r2[:count] * rr + b * r2[:count] + c * rr + d
                ix = np.arange(count) // 4 * 4 + (np.arange(count) % 2) * 2
                fx[:count] = ff[ix]; fy[:count] = ff[ix + 1]
            out[yi, :, 0] = (np.float64(xs * fx) + cx) / norm
            out[yi, :, 1] = (np.float64(yy * fy) + cy) / norm
            y = np.float32(y + np.float32(norm)) if step32 else np.float32(float(y) + norm)
        print(name, dict(squared_ratio_float32=ratio32, step_float32=step32,
                         maximum=float(np.max(abs(out - native)))))
