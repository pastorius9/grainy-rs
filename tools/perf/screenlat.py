"""On-screen latency of a slider drag, measured the same way for any application.

The script drags a slider with real mouse input (SendInput) and samples screen regions with GDI BitBlt
(a few ms per sample) to find when the pixels actually change on screen.

  python screenlat.py shot <out.png>                       full-screen capture (to find coordinates)
  python screenlat.py focus <title substring>              bring a window to the front
  python screenlat.py step  <cfg.json> <reps> <out.jsonl>  press, move dx, hold, release; time main/thumb changes
  python screenlat.py sweep <cfg.json> <reps> <out.jsonl>  continuous 1 s drag; count visible updates, settle time

cfg.json: {"title": "...", "handle": [x, y], "dx": 40, "main": [x, y, w, h], "thumb": [x, y, w, h] | null}
Coordinates are physical screen pixels (the process is per-monitor DPI aware).
"""
import ctypes, json, sys, time
from ctypes import wintypes

import numpy as np

ctypes.windll.shcore.SetProcessDpiAwareness(2)
user32 = ctypes.WinDLL('user32', use_last_error=True)
gdi32 = ctypes.WinDLL('gdi32', use_last_error=True)
gdi32.CreateDIBSection.restype = wintypes.HBITMAP
gdi32.CreateDIBSection.argtypes = [wintypes.HDC, ctypes.c_void_p, wintypes.UINT, ctypes.POINTER(ctypes.c_void_p),
                                   wintypes.HANDLE, wintypes.DWORD]
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.SelectObject.restype = wintypes.HGDIOBJ
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi32.BitBlt.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.HDC,
                         ctypes.c_int, ctypes.c_int, wintypes.DWORD]
user32.GetDC.restype = wintypes.HDC
user32.GetDC.argtypes = [wintypes.HWND]

SRCCOPY = 0x00CC0020
now = time.perf_counter


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [('biSize', wintypes.DWORD), ('biWidth', ctypes.c_long), ('biHeight', ctypes.c_long),
                ('biPlanes', wintypes.WORD), ('biBitCount', wintypes.WORD), ('biCompression', wintypes.DWORD),
                ('biSizeImage', wintypes.DWORD), ('biXPelsPerMeter', ctypes.c_long),
                ('biYPelsPerMeter', ctypes.c_long), ('biClrUsed', wintypes.DWORD),
                ('biClrImportant', wintypes.DWORD)]


class Region:
    """A screen rectangle captured into a reusable DIB section."""

    def __init__(self, x, y, w, h):
        self.x, self.y, self.w, self.h = x, y, w, h
        self.screen = user32.GetDC(None)
        self.dc = gdi32.CreateCompatibleDC(self.screen)
        info = BITMAPINFOHEADER(ctypes.sizeof(BITMAPINFOHEADER), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
        self.bits = ctypes.c_void_p()
        bmp = gdi32.CreateDIBSection(self.dc, ctypes.byref(info), 0, ctypes.byref(self.bits), None, 0)
        gdi32.SelectObject(self.dc, bmp)
        self.size = w * h * 4

    def grab(self):
        gdi32.BitBlt(self.dc, 0, 0, self.w, self.h, self.screen, self.x, self.y, SRCCOPY)
        return np.frombuffer(ctypes.string_at(self.bits, self.size), np.uint8).reshape(self.h, self.w, 4)[..., :3]


def differs(a, b, threshold=2.0):
    """Mean absolute difference over the region above `threshold` grey levels (ignores cursor-size noise)."""
    return float(np.abs(a.astype(np.int16) - b.astype(np.int16)).mean()) > threshold


# ---- mouse ----------------------------------------------------------------------------------------------------
class MOUSEINPUT(ctypes.Structure):
    _fields_ = [('dx', ctypes.c_long), ('dy', ctypes.c_long), ('mouseData', wintypes.DWORD),
                ('dwFlags', wintypes.DWORD), ('time', wintypes.DWORD), ('dwExtraInfo', ctypes.c_void_p)]


class INPUT(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [('mi', MOUSEINPUT), ('pad', ctypes.c_byte * 32)]
    _anonymous_ = ('u',)
    _fields_ = [('type', wintypes.DWORD), ('u', _U)]


MOVE, LDOWN, LUP, ABSOLUTE, VIRTUALDESK = 0x1, 0x2, 0x4, 0x8000, 0x4000


def mouse(x, y, flags=0):
    vx, vy = user32.GetSystemMetrics(76), user32.GetSystemMetrics(77)
    vw, vh = user32.GetSystemMetrics(78), user32.GetSystemMetrics(79)
    nx = int((x - vx) * 65535 / (vw - 1))
    ny = int((y - vy) * 65535 / (vh - 1))
    i = INPUT(type=0)
    i.mi = MOUSEINPUT(nx, ny, 0, MOVE | ABSOLUTE | VIRTUALDESK | flags, 0, None)
    user32.SendInput(1, ctypes.byref(i), ctypes.sizeof(INPUT))


def focus(title):
    found = []

    # "pid:1234" picks the process's visible top-level window; anything else matches the title.
    want_pid = int(title[4:]) if str(title).startswith('pid:') else None

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(hwnd, _):
        n = user32.GetWindowTextLengthW(hwnd)
        if n and user32.IsWindowVisible(hwnd):
            buf = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(hwnd, buf, n + 1)
            pid = wintypes.DWORD()
            user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if (want_pid is not None and pid.value == want_pid) or (
                    want_pid is None and title.lower() in buf.value.lower()):
                found.append((hwnd, buf.value))
        return True

    user32.EnumWindows(each, 0)
    if not found:
        raise SystemExit(f'no window with {title!r}')
    hwnd, name = found[0]
    user32.keybd_event(0x12, 0, 0, 0)  # ALT lets a background process take the foreground
    user32.keybd_event(0x12, 0, 2, 0)
    if user32.IsIconic(hwnd):
        user32.ShowWindow(hwnd, 9)  # restore only a minimized window (keeps a maximized one maximized)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.3)
    return name


def settle(regions, quiet=0.6, limit=8.0):
    """Wait until no region changes for `quiet` seconds; returns the last frames."""
    start = now()
    last = [r.grab() for r in regions]
    changed = now()
    while now() - changed < quiet and now() - start < limit:
        time.sleep(0.01)
        cur = [r.grab() for r in regions]
        if any(differs(a, b, 0.3) for a, b in zip(cur, last)):
            changed = now()
        last = cur
    return last


def watch(regions, base, t0, limit, quiet):
    """Times (ms after t0) of the first change from `base` and of the last change, per region, plus the count of
    distinct updates."""
    first = [None] * len(regions)
    lastchange = [None] * len(regions)
    updates = [0] * len(regions)
    prev = list(base)
    quiet_since = now()
    while now() - t0 < limit:
        cur = [r.grab() for r in regions]
        t = now()
        for i, (c, p, b) in enumerate(zip(cur, prev, base)):
            if differs(c, p, 0.3):
                updates[i] += 1
                lastchange[i] = (t - t0) * 1000
                quiet_since = t
            if first[i] is None and differs(c, b):
                first[i] = (t - t0) * 1000
        prev = cur
        if all(f is not None for f in first[:1]) and t - quiet_since > quiet:
            break
    return first, lastchange, updates


def regions_of(cfg):
    rs = [Region(*cfg['main'])]
    if cfg.get('thumb'):
        rs.append(Region(*cfg['thumb']))
    return rs


def step(cfg, reps, out):
    focus(cfg['title'])
    regions = regions_of(cfg)
    hx, hy = cfg['handle']
    dx = cfg['dx']
    rows = []
    x = hx
    for rep in range(reps):
        sign = 1 if rep % 2 == 0 else -1
        mouse(x, hy)
        time.sleep(0.15)
        base = settle(regions)
        mouse(x, hy, LDOWN)
        time.sleep(0.12)
        t0 = now()
        mouse(x + sign * dx, hy)
        first, last, updates = watch(regions, base, t0, limit=6.0, quiet=0.7)
        held = [r.grab() for r in regions]
        t1 = now()
        mouse(x + sign * dx, hy, LUP)
        rfirst, rlast, rupdates = watch(regions, held, t1, limit=6.0, quiet=1.0)
        x += sign * dx
        row = {'rep': rep, 'drag_main_first': first[0], 'drag_main_last': last[0],
               'release_main_change': rlast[0], 'thumb_after_release': rlast[1] if len(regions) > 1 else None,
               'thumb_during_drag': first[1] if len(regions) > 1 else None}
        rows.append(row)
        print(json.dumps(row))
        time.sleep(0.4)
    with open(out, 'a', encoding='utf-8') as f:
        for row in rows:
            f.write(json.dumps({'mode': 'step', 'title': cfg['title'], **row}) + '\n')
    summarize(rows)


def sweep(cfg, reps, out):
    """Drag continuously for 1 s (60 moves, `dx` px in total), then hold still: visible main-view updates per
    second while moving, and the time from the last move until the picture stops changing."""
    focus(cfg['title'])
    regions = regions_of(cfg)[:1]
    hx, hy = cfg['handle']
    dx = cfg['dx']
    rows = []
    x = hx
    for rep in range(reps):
        sign = 1 if rep % 2 == 0 else -1
        mouse(x, hy)
        time.sleep(0.15)
        base = settle(regions)
        mouse(x, hy, LDOWN)
        time.sleep(0.12)
        t0 = now()
        seen, prev, first = 0, base[0], None
        for k in range(1, 61):
            target = t0 + k / 60
            while now() < target:
                cur = regions[0].grab()
                if differs(cur, prev, 0.3):
                    seen += 1
                    if first is None:
                        first = (now() - t0) * 1000
                prev = cur
            mouse(int(x + sign * dx * k / 60), hy)
        tstop = now()
        _, last, more = watch(regions, prev, tstop, limit=6.0, quiet=0.8)
        mouse(int(x + sign * dx), hy, LUP)
        time.sleep(0.3)
        settle(regions, quiet=0.8)
        x += sign * dx
        row = {'rep': rep, 'updates_while_moving_per_s': seen / (tstop - t0), 'first_update_ms': first,
               'settle_after_stop_ms': last[0]}
        rows.append(row)
        print(json.dumps(row))
    with open(out, 'a', encoding='utf-8') as f:
        for row in rows:
            f.write(json.dumps({'mode': 'sweep', 'title': cfg['title'], **row}) + '\n')
    summarize(rows)


def summarize(rows):
    keys = [k for k in rows[0] if k != 'rep']
    for k in keys:
        vals = sorted(v for v in (r[k] for r in rows) if v is not None)
        if vals:
            print(f'  {k:28s} median {vals[len(vals) // 2]:8.1f}   min {vals[0]:8.1f}   max {vals[-1]:8.1f}   n={len(vals)}')
        else:
            print(f'  {k:28s} (no change seen)')


def shot(path):
    from PIL import Image
    w, h = user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)
    img = Region(0, 0, w, h).grab()
    Image.fromarray(img[..., ::-1].copy()).save(path)
    print(path, w, h)


if __name__ == '__main__':
    cmd = sys.argv[1]
    if cmd == 'shot':
        shot(sys.argv[2])
    elif cmd == 'focus':
        print(focus(sys.argv[2]))
    elif cmd in ('click', 'dclick'):
        # click <window> x y: focus the window, then click (twice for dclick)
        focus(sys.argv[2])
        x, y = int(sys.argv[3]), int(sys.argv[4])
        mouse(x, y)
        time.sleep(0.1)
        for _ in range(2 if cmd == 'dclick' else 1):
            mouse(x, y, LDOWN)
            time.sleep(0.03)
            mouse(x, y, LUP)
            time.sleep(0.06)
    else:
        cfg = json.load(open(sys.argv[2], encoding='utf-8'))
        {'step': step, 'sweep': sweep}[cmd](cfg, int(sys.argv[3]), sys.argv[4])
