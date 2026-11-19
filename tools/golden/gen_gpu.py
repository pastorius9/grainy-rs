"""Compact settings matrices and native_gpu support decisions for grainy-gpu.

Only golden/gpu is written. Existing CPU case metadata supplies the exact CPU settings matrices;
neither the existing CPU goldens nor the read-only oracle are changed. No bytecode is written.
Pixels are computed by the Rust CPU stages in tests. Only the nonstandard hue wheel and
nine private native_gpu Gaussian kernels remain as arrays. Regeneration is byte-stable.
"""
import sys
sys.dont_write_bytecode = True
import json
from common import Case, GOLDEN, inputs, np, settings
from luma import native_gpu

I = inputs()
INDEX = {"tone": [], "color": [], "chain": [], "grain": [], "fallback": [], "detail": [], "detail_chain": []}
MATRIX = {"tone": [], "color": []}
DEFAULTS = settings()
SETTINGS = []

def record(kind, name, s, **meta):
    changes = {k: v for k, v in s.items() if k not in DEFAULTS or v != DEFAULTS[k]}
    if changes not in SETTINGS:
        SETTINGS.append(changes)
    index = SETTINGS.index(changes)
    INDEX[kind].append(dict(name=kind + "_" + name, settings=index, **meta))
    return index

def emit(kind, name, a, s, source, crop=None):
    if kind == "tone":
        supported = native_gpu._tone_params(a.shape, s, a) is not None
    elif kind == "color":
        supported = native_gpu._color_params(a.shape, s) is not None
    else:
        supported = True
    index = record(kind, name, s, source=source, crop=crop, supported=supported, pixel_scale=0.7)
    if kind in MATRIX and supported and index not in MATRIX[kind]:
        MATRIX[kind].append(index)

# Every tone stage setting from gen_tone, including supported HDR and rejected film/DRT settings.
for path in sorted((GOLDEN / "tone").glob("stage_*/case.json")):
    meta = json.loads(path.read_text(encoding="utf-8"))
    if "input" not in meta:
        continue
    name = meta["input"]
    a = I["noise"] - np.float32(0.2) if name == "neg" else I[name]
    y0,y1,x0,x1 = meta["crop"]
    a = a[y0:y1,x0:x1]
    emit("tone", path.parent.name, a, meta["settings"], name, meta["crop"])

# Cover the colour stage AND all component matrices through the public GPU colour stage.
cg = json.loads((GOLDEN / "colorgrade/index/case.json").read_text(encoding="utf-8"))
custom = {}
for name in dict.fromkeys(cg["stage"] + cg["mix"] + cg["tools"]):
    meta = json.loads((GOLDEN / "colorgrade" / name / "case.json").read_text(encoding="utf-8"))
    kind,key = meta["image"].split(":")
    a = I[key] if kind == "std" else np.load(GOLDEN / "colorgrade/inputs" / (key+".npy"))
    # Non-finite pixels are intentionally rejected at the new typed API boundary.
    if not np.isfinite(a).all():
        continue
    if kind != "std":
        custom[key] = a
    emit("color", name, a, settings(**meta["changes"]), key if kind == "std" else "custom:" + key)

for name,a in I.items():
    for variant,changes in [("default",{}),("mixed",dict(exposure=.7,temperature=23.,tint=-17.,
            shadows=40.,highlights=-60.,blacks=-25.,whites=15.,contrast=20.,
            saturation=25.,vibrance=-30.,parametric=[20.,-30.,40.,-20.],grain=65.,grain_size=40.,grain_roughness=70.))]:
        emit("chain",name+"_"+variant,a,settings(**changes),name)
    for amount,size,rough in [(0.,25.,50.),(100.,25.,50.),(50.,70.,20.),(65.,40.,100.)]:
        emit("grain",f"{name}_{amount}_{size}_{rough}",a,settings(grain=amount,grain_size=size,grain_roughness=rough),name)
    emit("tone","default_"+name,a,settings(),name)
    emit("color","default_"+name,a,settings(),name)

# Invalid shapes/curves/points and supported exceptions (identity-unsorted, unknown film, zero strength).
fallbacks = [dict(film_look="kodak-portra-400"),dict(film_look="unknown-film"),
    dict(film_look="kodak-portra-400",film_strength=0.),dict(tone_mapping="drt"),dict(hdr=True),
    dict(photo_filter_enabled=True),dict(curve=[[0.,0.]]),dict(curve=[[0.,0.],[1.,.5],[.5,1.]]),
    dict(curve=[[1.,1.],[0.,0.]]),dict(rgb_curves=[[[0.,0.]]]),
    dict(point_colors=[dict(rgb=[0.,1.],hue=1.)]),
    dict(point_colors=[dict(version=2,range=0.,hue=1.)]),
    dict(point_colors=[dict(version=2,range=0.,hue=0.)])]
for i,change in enumerate(fallbacks):
    s=settings(**change)
    record("fallback", str(i), s,
           tone=native_gpu._tone_params(I["colours"].shape,s,I["colours"]) is not None,
           color=native_gpu._color_params(I["colours"].shape,s) is not None)

# Tests compare full frames directly to grainy_detail::develop_detail (grain disabled).
detail_cases = [("default",{}),("mono",dict(monochrome=True)),
    ("texture_up",dict(texture=100.)),("texture_down",dict(texture=-100.)),
    ("clarity_up",dict(clarity=100.)),("clarity_down",dict(clarity=-100.)),
    ("dehaze_up",dict(dehaze=100.)),("dehaze_down",dict(dehaze=-100.)),
    ("defringe",dict(defringe=100.)),
    ("sharp_legacy",dict(sharpen=100.,detail_version=1,sharpen_detail=100.)),
    ("sharp_luma",dict(sharpen=100.,detail_version=2,sharpen_detail=100.)),
    ("sharp_threshold",dict(sharpen=150.,sharpen_detail=0.,sharpen_radius=2.)),
    ("sharp_mask",dict(sharpen=100.,sharpen_mask=100.,sharpen_radius=.5)),
    ("vignette_up",dict(vignette=100.)),("vignette_down",dict(vignette=-100.)),
    ("vignette_round",dict(vignette=80.,vignette_roundness=100.,vignette_midpoint=0.,vignette_feather=100.,vignette_highlights=100.)),
    ("vignette_box",dict(vignette=100.,vignette_roundness=-100.,vignette_midpoint=100.,vignette_feather=0.)),
    ("bilateral",dict(noise_luma=100.,detail_version=1)),
    ("bilateral_v2",dict(noise_luma=100.,detail_version=2)),
    ("chroma",dict(noise_color=100.)),("chroma_low",dict(noise_color=5.)),
    ("wavelet",dict(noise_luma=100.)),("wavelet_low",dict(noise_luma=20.)),
    ("noise_combo",dict(noise_luma=75.,noise_color=65.)),
    ("noise_skipped",dict(noise_luma=100.,noise_color=100.)),
    ("wavelet_profile",dict(noise_luma=55.,wavelet_profiles=[[1.+i*.2 for i in range(8)]]*3)),
    ("combo",dict(noise_luma=40.,noise_color=35.,texture=35.,dehaze=30.,defringe=70.,clarity=20.,sharpen=50.,sharpen_mask=20.,vignette=35.))]
for name in [*I, "tiny", "row", "column"]:
    for variant,change in detail_cases:
        scale = .1 if variant == "noise_skipped" else .7 if variant in ("combo","noise_combo","sharp_mask") else 1.
        s=settings(**change)
        record("detail", f"{name}_{variant}", s, pixel_scale=scale, source=name, encoded=True)

for variant,change in detail_cases:
    if variant in ("default","chroma","wavelet","combo","vignette_round","sharp_mask"):
        record("detail", "large_"+variant, settings(**change), pixel_scale=1., source="large", encoded=True)

for name in I:
    change=detail_cases[-1][1].copy();change.update(exposure=.6,shadows=35.,highlights=-50.,vibrance=25.,grain=35.)
    record("detail_chain", name, settings(**change), pixel_scale=.7, source=name)

with Case("gpu", "inputs") as case:
    for name, a in sorted(custom.items()):
        case.array(name, a)

with Case("gpu","gaussian") as case:
    sigmas=[.01,.05,.3,.7,1.,1.3,2.2,12.8,21.];case.json(sigmas=sigmas)
    for i,sigma in enumerate(sigmas):case.array(f"kernel_{i}",native_gpu._gaussian(sigma))

with Case("gpu","index") as case:
    case.json(cases=INDEX, matrices=MATRIX, settings=SETTINGS, defaults=DEFAULTS)
print({kind:len(names) for kind,names in INDEX.items()})
print("matrix settings",{kind:len(values) for kind,values in MATRIX.items()})
