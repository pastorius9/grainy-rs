"""Goldens of the HDR package of grainy-app (APP_PLAN package 32): `golden/app_hdr/`.

The shipped oracle keeps its HDR editing mode switched off (`engine.HDR_FEATURE = False`, 0.5.73). Its own tests
and self-test switch it on in their process; this generator does the same (`engine.HDR_FEATURE = True` below),
without editing the oracle.

* `normalized`: `engine.normalized` of stored settings with the switch on (and off, for comparison).
* `present_<name>`: the preview path of `MainWindow.render` (luma/app.py, the `work()` of a final render) for one
  photo and settings: `develop(source, settings, output_space=None)` (the limited extended working pixels), then
  `hdr.to_srgb_extended`, `hdr.sdr_rendition` (when the SDR preview is on), `output_rgb`, `hdr.visualize` (when
  the HDR range view is on), `qimage` quantisation, `hdr.histogram`, `clipping_channels(hdr_pixels, ceiling)`,
  `Histogram.set_hdr_ranges` with the SDR screen (`display_stops = 0`), and the clipping overlay of
  `PhotoView.drawForeground` (highlight mask: `peak > hdr_display_ceiling + 1e-5`, shadows: any channel < 1/510).
* `formats`: the export dialog's format list with the switch on, and the fields each enables (`format_changed`).

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_hdr.py
"""
from common import Case, inputs, np, write_json, GOLDEN

from luma import engine  # noqa: E402

engine.HDR_FEATURE = True   # as tests/conftest.py and selftest.py of the oracle do; never edited in the oracle

from luma.engine import develop, normalized, output_rgb, to_srgb  # noqa: E402
from luma import hdr  # noqa: E402
from luma.widgets import clipping_channels  # noqa: E402

MODULE = 'app_hdr'


def quantise(rgb):
    """`qimage(display, display=False)`: np.clip(rgb,0,1)*255+.5 as uint8."""
    part = np.clip(rgb, 0, 1).astype(np.float32)
    part *= 255
    part += .5
    return part.astype(np.uint8)


def present(name, source, settings, sdr_preview=False, visualize=False):
    with Case(MODULE, f'present_{name}') as case:
        case.array('source', source)
        case.json(settings=settings, sdr_preview=sdr_preview, visualize=visualize)
        s = normalized(settings)
        assert s['hdr']
        working = develop(source, settings, True, output_space=None)
        case.array('working', working)
        hdr_pixels = hdr.to_srgb_extended(working, s['working_space'])
        case.array('hdr_pixels', hdr_pixels)
        thumb = hdr.sdr_rendition(working, s)
        shown = thumb if sdr_preview else working
        image = output_rgb(shown, s['working_space'])
        display = image
        if visualize:
            display = hdr.visualize(hdr_pixels)
        case.array('display', quantise(display))
        bins = hdr.histogram(hdr_pixels)
        case.array('bins', np.stack(bins).astype(np.int64))
        ceiling = float(to_srgb(np.float32(2 ** s['hdr_limit'])))
        shadows, highlights = clipping_channels(hdr_pixels, ceiling)
        peak = hdr_pixels.max(axis=-1)
        display_ceiling = float(to_srgb(np.float32(2 ** 0.)))
        inside = bool(np.any((peak > 1 + 1e-5) & (peak <= display_ceiling + 1e-5)))
        outside = bool(np.any(peak > display_ceiling + 1e-5))
        case.json(ceiling=ceiling, shadows=list(shadows), highlights=list(highlights), inside=inside, outside=outside)
        case.array('highlight_mask', peak > display_ceiling + 1e-5)
        case.array('shadow_mask', np.any(hdr_pixels < 1 / 510, axis=-1))


def main():
    images = inputs()
    with Case(MODULE, 'normalized') as case:
        stored = {'hdr': True, 'exposure': 1.5, 'hdr_limit': 2.5}
        on = normalized(stored)
        engine.HDR_FEATURE = False
        off = normalized(stored)
        engine.HDR_FEATURE = True
        case.json(stored=stored, on=on, off=off)
    wide = images['wide']
    garden = np.ascontiguousarray(images["garden"][::2, ::2]) * np.float32(4)
    colours = images['colours'] * np.float32(8)
    base = {'hdr': True}
    present('wide_default', wide, {**base})
    present('garden_limit', garden, {**base, 'hdr_limit': 2., 'hdr_brightness': 30., 'exposure': .5})
    present('colours_prophoto', colours, {**base, 'working_space': 'ProPhoto', 'hdr_brightness': -40.})
    present('garden_sdr', garden, {**base, 'sdr_exposure': .5, 'sdr_compression': 40.}, sdr_preview=True)
    present('colours_visualize', colours, {**base, 'contrast': 20.}, visualize=True)
    present('wide_low_limit', wide, {**base, 'hdr_limit': 0.}, sdr_preview=True)
    from luma.quick_export import FORMATS
    rows = []
    for value in FORMATS:
        rows.append({'format': value, 'enabled': {
            'quality': value in ('JPEG', 'AVIF', 'JPEG XL'),
            'color_space': value not in ('TIFF HDR 32-bit', 'Original', 'JPEG XL'),
            'longest': value != 'Original', 'keep_metadata': value != 'Original',
            'grayscale': value in ('JPEG', 'PNG', 'TIFF 8-bit', 'TIFF 16-bit'),
            'dither': value in ('JPEG', 'AVIF', 'PNG', 'TIFF 8-bit')}})
    write_json(GOLDEN / MODULE / 'formats' / 'case.json', {'listed': list(FORMATS), 'rows': rows})


if __name__ == '__main__':
    main()
