"""Goldens of the automatic monitor profile (HANDOFF2 B): `golden/app/monitor_query/`.

Made with the oracle's own code (nothing is reimplemented): luma/display_profiles.py `windows_profile_path` runs
against a scripted stand-in for gdi32 (`ctypes.WinDLL`, `ctypes.get_last_error`, `ctypes.set_last_error` are replaced
inside this process only), so every branch of the oracle's `GetICMProfileW` loop is recorded without asking the real
system: the screen name check, `CreateDCW` failing, "no profile" answers, a path too long, the size retry, a profile
changing while it is read, and the `DeleteDC` that always follows. Each case keeps the script, the result (the path,
`None`, or the error text exactly as `resolve` would format it with `str(error)`), the number of queries and whether the
device context was deleted.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_monitor.py
"""
import ctypes

from common import Case

from luma import display_profiles  # noqa: E402


class Script:
    """The scripted gdi32: `CreateDCW` answers `create` (a handle, 0 for a failure with `create_error`), each
    `GetICMProfileW` takes the next answer `{ok, size, error, value}`: the size it reports, the last-error code it
    leaves and the path it writes into a given buffer."""

    def __init__(self, case):
        self.case = case
        self.answers = list(case.get('answers', []))
        self.error = 0
        self.queries = 0
        self.deleted = False
        self.created = None


def run(case):
    script = Script(case)

    class Function:
        def __init__(self, body):
            self.body = body

        def __call__(self, *args):
            return self.body(*args)

    def create(driver, device, port, mode):
        script.created = (driver, device)
        if case.get('create', 1):
            return case.get('create', 1)
        script.error = case.get('create_error', 0)
        return 0

    def query(dc, size_ref, buffer):
        script.queries += 1
        answer = script.answers.pop(0)
        size_ref._obj.value = answer.get('size', 0)
        if 'error' in answer:
            script.error = answer['error']
        if answer.get('ok') and buffer is not None:
            buffer.value = answer.get('value', '')
        return 1 if answer.get('ok') else 0

    def delete(dc):
        script.deleted = True
        return 1

    class Gdi:
        CreateDCW = Function(create)
        GetICMProfileW = Function(query)
        DeleteDC = Function(delete)

    saved = ctypes.WinDLL, ctypes.get_last_error, ctypes.set_last_error
    ctypes.WinDLL = lambda name, use_last_error=False: Gdi()
    ctypes.get_last_error = lambda: script.error
    ctypes.set_last_error = lambda code: setattr(script, 'error', code)
    try:
        try:
            result = dict(path=display_profiles.windows_profile_path(case['screen']))
        except Exception as error:  # noqa: BLE001 - every error becomes a message in resolve
            result = dict(error=str(error))
    finally:
        ctypes.WinDLL, ctypes.get_last_error, ctypes.set_last_error = saved
    return dict(case, **result, queries=script.queries, deleted=script.deleted,
                created=script.created is not None)


D = '\\\\.\\DISPLAY1'
PATH = 'C:\\Windows\\System32\\spool\\drivers\\color\\Monitor.icm'

CASES = [
    dict(name='no screen', screen=''),
    dict(name='not a device name', screen='HDMI-1'),
    dict(name='no number', screen='\\\\.\\DISPLAY'),
    dict(name='trailing text', screen='\\\\.\\DISPLAY1x'),
    dict(name='other prefix', screen='\\\\?\\DISPLAY1'),
    dict(name='lower case', screen='\\\\.\\display12', answers=[dict(ok=0, size=len(PATH) + 1, error=122),
                                                                 dict(ok=1, size=len(PATH) + 1, value=PATH)]),
    dict(name='CreateDC fails', screen=D, create=0, create_error=1801),
    dict(name='no profile, success', screen=D, answers=[dict(ok=1, size=0)]),
    dict(name='no profile, code 0', screen=D, answers=[dict(ok=0, size=0)]),
    dict(name='no profile, code 2', screen=D, answers=[dict(ok=0, size=0, error=2)]),
    dict(name='no profile, code 1168', screen=D, answers=[dict(ok=0, size=0, error=1168)]),
    dict(name='no profile, code 2015', screen=D, answers=[dict(ok=0, size=0, error=2015)]),
    dict(name='size query fails', screen=D, answers=[dict(ok=0, size=0, error=5)]),
    dict(name='path too long', screen=D, answers=[dict(ok=0, size=32769, error=122)]),
    dict(name='longest path size', screen=D, answers=[dict(ok=0, size=32768, error=122),
                                                     dict(ok=1, size=32768, value=PATH)]),
    dict(name='a profile', screen=D, answers=[dict(ok=0, size=60, error=122), dict(ok=1, size=60, value=PATH)]),
    dict(name='an empty path', screen=D, answers=[dict(ok=0, size=60, error=122), dict(ok=1, size=60, value='')]),
    dict(name='grown once', screen=D, answers=[dict(ok=0, size=10, error=122), dict(ok=0, size=60, error=122),
                                              dict(ok=1, size=60, value=PATH)]),
    dict(name='grown twice', screen=D, answers=[dict(ok=0, size=10, error=122), dict(ok=0, size=20, error=122),
                                               dict(ok=0, size=60, error=122)]),
    dict(name='not grown', screen=D, answers=[dict(ok=0, size=60, error=122), dict(ok=0, size=60, error=122)]),
    dict(name='grown too far', screen=D, answers=[dict(ok=0, size=60, error=122), dict(ok=0, size=40000, error=122)]),
    dict(name='other error', screen=D, answers=[dict(ok=0, size=60, error=122), dict(ok=0, size=80, error=5)]),
    dict(name='stale error code', screen=D, answers=[dict(ok=0, size=60, error=5), dict(ok=0, size=80)]),
    dict(name='a path with Korean letters', screen='\\\\.\\DISPLAY3',
         answers=[dict(ok=0, size=40, error=122), dict(ok=1, size=40, value='D:\\프로파일\\모니터.icc')]),
]


def main():
    with Case('app', 'monitor_query') as case:
        case.json(cases=[run(c) for c in CASES])


if __name__ == '__main__':
    main()
