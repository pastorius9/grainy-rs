"""Goldens of grainy-app's AI connection and Ask AI window (APP_PLAN package 30), made with the oracle's own code.

Nothing here starts a real AI program, installer, sign-in, browser or server, and nothing reads the owner's AI
configuration: `subprocess.Popen` and `ProcessTree` are replaced by recorders and scripted processes, `mcp_http.start`
by a fake server, the environment (PATH, LOCALAPPDATA, APPDATA, USERPROFILE, HOME, CODEX_HOME, GEMINI_CLI_HOME…) points
into a temporary folder, and every file is written there. Random choices (ports, keys) and clocks are fixed.

Cases (golden/app/ai_*/case.json):
* ai_texts: `install_commands` / `install_text` / `standard_folder`, `activity_text` in both languages.
* ai_events: `EventParser` over scripted streams of the three programs, `tool_event`, `redact`,
  `authentication_failed`, `parse_status`, `gemini_status`.
* ai_commands: `Request.instruction`, `claude_command`, `codex_command` (with `codex_catalog`), `gemini_command`
  and the files they write, with the temporary root written as <ROOT> (and <ROOT_URI> in file URIs).
* ai_runner: `AiRunner._run` over scripted processes: the events, the result, redaction.
* ai_paths: `find_tool`, `find_program`, `find_codex` over fake program folders.
* ai_launch: `launch_install` and `launch_sign_in` (the argv, folder and script they would start).
* ai_connection: `McpService` start / stop / switch / new key / other port / another Grainy, step by step: the
  settings file's values, the state, the recent activity and the files of the mcp folder.
* ai_mcp_dialog: `McpDialog.refresh` (status, Advanced, HTTP text, key, install texts, undo line, activity) for
  states of the connection, in both languages.
* ai_dialog: `AiDialog` driven as the person would (notice, program choice, sign-in states, send, events, finish,
  a second request with the conversation, a too-long request, new conversation) in both languages.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_ai.py
"""
import errno
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from common import GOLDEN, write_json  # noqa: F401

from PySide6.QtWidgets import QApplication, QMessageBox, QWidget

app = QApplication.instance() or QApplication([])

from luma import ai_auth, ai_install, ai_paths, ai_programs, auth as auth_module, i18n, mcp_dialog, mcp_service  # noqa: E402
from luma import mcp_http  # noqa: E402
from luma.ai_auth import AuthStatus  # noqa: E402
from luma.ai_programs import EventParser, Request  # noqa: E402
from luma.preferences import Preferences  # noqa: E402

OUT = GOLDEN / 'app'
SCRATCH = Path(tempfile.mkdtemp(prefix='grainy-ai-golden-'))
CLEAN_ENV = dict(os.environ)


def case(name, value):
    write_json(OUT / name / 'case.json', value)


def fresh(name):
    folder = SCRATCH / name
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    return folder


def isolated_env(root, extra=None):
    """Only the variables the AI code reads, all pointing into root."""
    env = {'PATH': str(root / 'empty-path'), 'PATHEXT': '.COM;.EXE;.BAT;.CMD',
           'LOCALAPPDATA': str(root / 'local'), 'APPDATA': str(root / 'roaming'),
           'USERPROFILE': str(root / 'home'), 'HOME': str(root / 'home'), 'ProgramFiles': str(root / 'programs'),
           'SystemRoot': CLEAN_ENV.get('SystemRoot', 'C:\\Windows'), 'NoDefaultCurrentDirectoryInExePath': '1'}
    env.update(extra or {})
    os.environ.clear()
    os.environ.update(env)


def restore_env():
    os.environ.clear()
    os.environ.update(CLEAN_ENV)


def placed(value, root):
    """Paths under root written as <ROOT> (and <ROOT_URI> for file URIs), for any platform's Rust test."""
    uri = root.as_uri()
    text = str(root)
    if isinstance(value, dict):
        return {placed(k, root): placed(v, root) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [placed(v, root) for v in value]
    if isinstance(value, Path):
        value = str(value)
    if isinstance(value, str):
        value = value.replace(uri, '<ROOT_URI>')
        value = value.replace(json.dumps(text)[1:-1], '<ROOT_JSON>')
        return value.replace(text, '<ROOT>')
    return value


def both_languages(fn):
    out = {}
    for language in ('ko', 'en'):
        i18n.set_language(language)
        out[language] = fn()
    i18n.set_language('ko')
    return out


# ---- texts -------------------------------------------------------------------------------------------------

def texts():
    root = fresh('texts')
    isolated_env(root)
    standard = mcp_dialog.standard_folder(True)
    folders = [None, str(standard), str(root / 'local' / 'GRAINY' / 'MCP'), "Z:\\Temp\\it's lib\\mcp",
               "/tmp/it's/mcp"]
    installs = []
    for windows in (True, False):
        for folder in folders:
            commands = mcp_dialog.install_commands(windows, folder)
            installs.append({'windows': windows, 'folder': placed(folder, root),
                             'commands': placed(commands, root),
                             'texts': placed({k: mcp_dialog.install_text(v) for k, v in commands.items()}, root)})
    restore_env()
    outcomes = ['started', 'stopped', 'could not start', 'port not saved', 'on in another Grainy', 'new key',
                'could not write the bridge files', 'refused 401', 'refused 403', 'ok', 'error', 'undone in Grainy',
                'job j1 done', 'job j12 failed', 'job j3 cancelled', 'job j4 stopped in Grainy', 'job x done',
                'refused -32602', 'error -32603', 'error 7', 'error: bad thing', 'failed: it broke\nbadly',
                'something new']
    names = ['server', 'files', 'key', 'apply_edits', 'tools/list']
    activity = both_languages(lambda: [[n, o, mcp_dialog.activity_text(n, o)] for n in names for o in outcomes])
    case('ai_texts', {'standard': placed(standard, root), 'installs': installs, 'activity': activity})


# ---- events --------------------------------------------------------------------------------------------------

CLAUDE_STREAM = [
    {'type': 'system', 'subtype': 'init', 'session_id': 'sess-1'},
    {'type': 'stream_event', 'event': {'type': 'message_start'}},
    {'type': 'stream_event', 'event': {'type': 'content_block_delta', 'delta': {'type': 'text_delta', 'text': '사진을 '}}},
    {'type': 'stream_event', 'event': {'type': 'content_block_delta', 'delta': {'type': 'text_delta', 'text': '봅니다.'}}},
    {'type': 'assistant', 'message': {'content': [
        {'type': 'text', 'text': '사진을 봅니다.'},
        {'type': 'tool_use', 'id': 't1', 'name': 'mcp__grainy__list_photos', 'input': {'limit': 5}}]}},
    {'type': 'assistant', 'message': {'content': [
        {'type': 'tool_use', 'id': 't1', 'name': 'mcp__grainy__list_photos', 'input': {'limit': 5}},
        {'type': 'tool_use', 'id': 't2', 'name': 'mcp__grainy__apply_edits',
         'input': {'edits': [{'ids': [1, 2], 'set': {}}, {'ids': [2, 3, 'x']}]}}]}},
    {'type': 'assistant', 'message': {'content': [{'type': 'text', 'text': '끝났습니다.'}]}},
    {'type': 'assistant', 'message': {'content': [{'type': 'tool_use', 'id': 't3', 'name': 'Bash', 'input': {}}]}},
    {'type': 'stream_event', 'event': {'type': 'error', 'error': {'message': 'overloaded'}}},
    {'type': 'result', 'is_error': False},
    {'type': 'user'},
]
CLAUDE_FAIL = [
    {'type': 'assistant', 'error': 'authentication_failed', 'message': {'content': []}},
    {'type': 'result', 'is_error': True, 'result': 'Please run /login'},
]
CODEX_STREAM = [
    {'type': 'thread.started', 'thread_id': 'th-9'},
    {'type': 'turn.started'},
    {'type': 'item.started', 'item': {'id': 'i1', 'type': 'mcp_tool_call', 'server': 'grainy', 'tool': 'get_photo',
                                      'arguments': {'ids': [4, 5]}}},
    {'type': 'item.completed', 'item': {'id': 'i1', 'type': 'mcp_tool_call', 'server': 'grainy', 'tool': 'get_photo',
                                        'arguments': {'ids': [4, 5]}}},
    {'type': 'item.completed', 'item': {'id': 'i2', 'type': 'agent_message', 'text': 'Done'}},
    {'type': 'item.completed', 'item': {'id': 'i2', 'type': 'agent_message', 'text': 'Done'}},
    {'type': 'item.started', 'item': {'id': 'i3', 'type': 'mcp_tool_call', 'server': 'other', 'tool': 'x'}},
    {'type': 'turn.completed'},
]
CODEX_FAIL = [{'type': 'thread.started', 'thread_id': 'th-2'},
              {'type': 'turn.failed', 'error': {'message': '401 Unauthorized'}}]
GEMINI_STREAM = [
    {'type': 'init', 'session_id': 'g-1'},
    {'type': 'message', 'role': 'user', 'content': 'hi'},
    {'type': 'message', 'role': 'assistant', 'content': 'Hello '},
    {'type': 'tool_use', 'tool_id': 'a', 'tool_name': 'grainy__set_labels', 'parameters': {'ids': [9]}},
    {'type': 'tool_use', 'tool_id': 'a', 'tool_name': 'grainy__set_labels', 'parameters': {'ids': [9]}},
    {'type': 'tool_use', 'tool_id': 'b', 'tool_name': 'contact_sheet', 'parameters': None},
    {'type': 'result', 'status': 'success'},
]
GEMINI_FAIL = [{'type': 'error', 'message': 'quota'}, {'type': 'result', 'status': 'error'}]


def parse_stream(kind, lines):
    parser = EventParser()
    method = getattr(EventParser, kind)
    out = []
    for data in lines:
        out.append([[e.kind, e.text, e.count] for e in method(parser, data)])
    return {'events': out, 'succeeded': parser.succeeded, 'failed': parser.failed, 'partial': parser.partial}


AUTH_TEXTS = ['HTTP 401 from server', 'Unauthenticated', 'authentication_failed', 'Authentication required',
              'failed to authenticate.', 'You are not logged in', 'sign-in is required', 'login required',
              'OAuth access token is invalid', 'invalid_grant', 'invalid api key', 'API_KEY_INVALID',
              'API key not valid. Please pass a valid API key.', 'refresh token has expired',
              'access token was revoked', 'missing credentials', 'could not load the api_key',
              'Please log in again', 'Please set an Auth method', 'failed to refresh', 'no authentication is configured',
              're-authenticate', 'rate limit 429', 'all good', '4010 items', '', 'please sign in']


def events():
    streams = {'claude': parse_stream('claude', CLAUDE_STREAM), 'claude_fail': parse_stream('claude', CLAUDE_FAIL),
               'codex': parse_stream('codex', CODEX_STREAM), 'codex_fail': parse_stream('codex', CODEX_FAIL),
               'gemini': parse_stream('gemini', GEMINI_STREAM), 'gemini_fail': parse_stream('gemini', GEMINI_FAIL)}
    tools = []
    for name, arguments in [('mcp__grainy__get_preview', {'ids': [1, 2, 3]}), ('grainy__list_photos', None),
                            ('apply_edits', {'edits': [{'ids': [1]}, {'ids': [1, 2]}], 'ids': [7]}),
                            ('apply_edits', {'edits': [{'ids': [1]}, {'x': 1}]}), ('rm', {}),
                            ('export_photos', {'ids': 'all'}), ('mcp__grainy__mcp__grainy__x', {})]:
        e = ai_programs.tool_event(name, arguments)
        tools.append({'name': name, 'arguments': arguments, 'event': [e.kind, e.text, e.count]})
    hex64 = '0123456789abcdef' * 4
    redactions = []
    for text, secrets in [('Authorization: Bearer abc.def', ()), ('key sk-ant-api03-XYZ_1 and sk-proj-2', ()),
                          (f'token {hex64} end', ()), ('my secret value here', ('secret',)),
                          ('nothing to hide', ('', 'zzz')), (f'{hex64.upper()}', ()), ('BEARER   x y', ())]:
        redactions.append({'text': text, 'secrets': list(secrets), 'out': ai_programs.redact(text, secrets)})
    auth = [[t, ai_auth.authentication_failed(t)] for t in AUTH_TEXTS]
    auth.append([{'error': {'type': 'authentication_error'}}, ai_auth.authentication_failed({'error': {'type': 'authentication_error'}})])
    statuses = []
    for key, code, stdout, stderr in [
            ('claude', 0, '{"loggedIn": true, "authMethod": "claude.ai"}', ''),
            ('claude', 0, '{"loggedIn": true, "authMethod": "api_key_helper"}', ''),
            ('claude', 0, '{"loggedIn": true, "authMethod": "third_party"}', ''),
            ('claude', 0, '{"loggedIn": true, "authMethod": "new"}', ''),
            ('claude', 1, '{"loggedIn": true}', ''), ('claude', 1, '{"loggedIn": false}', ''),
            ('claude', 2, '{"loggedIn": false}', ''), ('claude', 0, 'not json', ''), ('claude', 0, '[1]', ''),
            ('codex', 0, '', 'Logged in using ChatGPT\n'), ('codex', 0, 'logged in using an API key - sk-...', ''),
            ('codex', 1, 'Not logged in', ''), ('codex', 1, 'Logged in using ChatGPT', ''),
            ('codex', 0, 'something else', ''), ('gemini', 0, 'x', '')]:
        s = ai_auth.parse_status(key, code, stdout, stderr)
        statuses.append({'key': key, 'code': code, 'stdout': stdout, 'stderr': stderr, 'state': s.state, 'kind': s.kind})
    root = fresh('gemini-status')
    (root / 'with' / '.gemini').mkdir(parents=True)
    (root / 'with' / '.gemini' / 'oauth_creds.json').write_text('{}', encoding='utf-8')
    (root / 'cli' / '.gemini').mkdir(parents=True)
    (root / 'cli' / '.gemini' / 'oauth_creds.json').write_text('{}', encoding='utf-8')
    gemini = []
    for environ, home in [({'GEMINI_API_KEY': 'x'}, 'none'), ({'GOOGLE_API_KEY': 'y'}, 'with'),
                          ({}, 'with'), ({}, 'none'), ({'GEMINI_CLI_HOME': str(root / 'cli')}, 'none'),
                          ({'GEMINI_API_KEY': ''}, 'none')]:
        s = ai_auth.gemini_status(home=root / home, environ=environ)
        gemini.append({'environ': placed(environ, root), 'home': home, 'state': s.state, 'kind': s.kind})
    case('ai_events', {'streams': {k: {'lines': v, **streams[k]} for k, v in
                                   [('claude', CLAUDE_STREAM), ('claude_fail', CLAUDE_FAIL), ('codex', CODEX_STREAM),
                                    ('codex_fail', CODEX_FAIL), ('gemini', GEMINI_STREAM), ('gemini_fail', GEMINI_FAIL)]},
                       'tools': tools, 'redactions': redactions, 'authentication': auth, 'statuses': statuses,
                       'gemini': gemini})


# ---- commands ------------------------------------------------------------------------------------------------

SERVER = {'command': 'powershell', 'args': ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'C:\\x\\b.ps1']}


def tree_files(root):
    out = {}
    for path in sorted(root.rglob('*')):
        if path.is_file():
            out[path.relative_to(root).as_posix()] = path.read_text(encoding='utf-8')
    return out


def commands():
    root = fresh('commands')
    isolated_env(root, {'CODEX_HOME': str(root / 'codex-home')})
    instructions = []
    for language, context in [('ko', ''), ('en', ''), ('ko', 'What the person sees: 2 photos.'), ('fr', 'x')]:
        r = Request(('x',), root, SERVER, 'p', language, '', '', context)
        instructions.append({'language': language, 'context': context, 'text': r.instruction})
    runs = []

    def run(kind, name, request, build):
        workspace = request.workspace
        try:
            argv = build(request)
            error = None
        except (ValueError, OSError) as exc:
            argv = None
            error = f'{type(exc).__name__}: {exc}'
        files = tree_files(workspace) if workspace.exists() else {}
        runs.append({'kind': kind, 'name': name, 'request': placed({
            'executable': list(request.executable), 'workspace': str(request.workspace), 'server': request.server,
            'prompt': request.prompt, 'language': request.language, 'model': request.model,
            'session_id': request.session_id, 'context': request.context}, root),
            'argv': placed(argv, root), 'error': error, 'files': placed(files, root)})

    claude = ('C:\\Programs\\claude.exe',)
    run('claude', 'plain', Request(claude, root / 'w' / 'claude1', SERVER, '사진 정리해 줘', 'ko'), ai_programs.claude_command)
    run('claude', 'resume', Request(claude, root / 'w' / 'claude2', SERVER, 'go "on"', 'en', 'opus', 'sess-1', 'ctx'),
        ai_programs.claude_command)
    codex = ('C:\\Programs\\codex.exe',)
    run('codex', 'no-cache-no-model', Request(codex, root / 'w' / 'codex0', SERVER, 'x', 'ko'), ai_programs.codex_command)
    run('codex', 'model-only', Request(codex, root / 'w' / 'codex1', SERVER, '하나', 'ko', 'gpt-5'), ai_programs.codex_command)
    (root / 'codex-home').mkdir()
    (root / 'codex-home' / 'models_cache.json').write_text(json.dumps({'models': [
        {'slug': 'gpt-5', 'apply_patch_tool_type': 'freeform', 'shell_type': 'local', 'extra': [1, 2]},
        {'slug': 'mini', 'tool_mode': 'x'}]}), encoding='utf-8')
    run('codex', 'cache', Request(codex, root / 'w' / 'codex2', SERVER, '둘 "따옴표"', 'en', '', 'th-1', 'Seen.'),
        ai_programs.codex_command)
    (root / 'codex-home' / 'models_cache.json').write_text('{"models": [1, 2]}', encoding='utf-8')
    run('codex', 'bad-cache', Request(codex, root / 'w' / 'codex3', SERVER, 'x', 'ko'), ai_programs.codex_command)
    npm = root / 'npm'
    (npm / 'node_modules' / '@google' / 'gemini-cli' / 'dist').mkdir(parents=True)
    entry = npm / 'node_modules' / '@google' / 'gemini-cli' / 'dist' / 'index.js'
    entry.write_text('// entry', encoding='utf-8')
    run('gemini', 'no-storage', Request(('node', str(entry)), root / 'w' / 'gemini0', SERVER, 'x', 'ko'),
        ai_programs.gemini_command)
    storage = npm / 'node_modules' / '@google' / 'gemini-cli' / 'node_modules' / '@google' / 'gemini-cli-core' / 'dist' / 'src' / 'config'
    storage.mkdir(parents=True)
    (storage / 'storage.js').write_text('// storage', encoding='utf-8')
    run('gemini', 'plain', Request(('node', str(entry)), root / 'w' / 'gemini1', SERVER, '세 번째', 'ko'),
        ai_programs.gemini_command)
    run('gemini', 'resume', Request(('node', str(entry)), root / 'w' / 'gemini2', SERVER, 'go', 'en', 'flash', 'g-1', 'c'),
        ai_programs.gemini_command)
    run('gemini', 'not-js', Request(('C:\\gemini.exe',), root / 'w' / 'gemini3', SERVER, 'go', 'en'), ai_programs.gemini_command)
    restore_env()
    case('ai_commands', {'instructions': placed(instructions, root), 'runs': runs,
                         'disabled': list(ai_programs.CODEX_DISABLED)})


# ---- runner --------------------------------------------------------------------------------------------------

class ScriptedProcess:
    def __init__(self, stdout, stderr, code):
        self.stdout = io.BytesIO(stdout)
        self.stderr = io.BytesIO(stderr)
        self.code = code

    def wait(self, timeout=None):
        return self.code


class ScriptedTree:
    script = None
    seen = []

    def __init__(self, argv, cwd, env=None):
        stdout, stderr, code, fail = ScriptedTree.script
        if fail:
            raise OSError('cannot start')
        ScriptedTree.seen.append({'argv': list(argv), 'cwd': str(cwd),
                                  'env': {k: env[k] for k in sorted(env) if k.startswith(('CLAUDE_CODE', 'MCP_', 'GEMINI_CLI_SYSTEM'))}})
        self.process = ScriptedProcess(stdout, stderr, code)

    def stop(self):
        pass

    def close(self):
        pass


def lines(items):
    return b''.join((json.dumps(i, ensure_ascii=False) if not isinstance(i, str) else i).encode('utf-8') + b'\n' for i in items)


def runner():
    root = fresh('runner')
    hex64 = 'fedcba9876543210' * 4
    isolated_env(root, {'MY_API_KEY': 'supersecretvalue', 'SHORT_TOKEN': 'abc', 'OTHER': 'notsecretvalue'})
    ai_programs.ProcessTree = ScriptedTree
    scenarios = [
        ('claude', 'done', lines(CLAUDE_STREAM[:7] + [{'type': 'result', 'is_error': False}]), b'', 0, False, ()),
        ('claude', 'secret', lines([{'type': 'assistant', 'message': {'content': [
            {'type': 'text', 'text': f'key {hex64} and supersecretvalue and the-token-x and Bearer zz'}]}},
            {'type': 'result', 'is_error': False}]), b'', 0, False, ('the-token-x',)),
        ('claude', 'auth-stderr', lines([{'type': 'result', 'is_error': True}]), b'Error: not logged in\n', 1, False, ()),
        ('claude', 'auth-line', b'not json: please log in\n', b'', 0, False, ()),
        ('claude', 'exit', lines([{'type': 'assistant', 'message': {'content': [{'type': 'text', 'text': 'hi'}]}}]), b'boom', 3, False, ()),
        ('claude', 'no-result', lines([{'type': 'system', 'session_id': 's'}]), b'', 0, False, ()),
        ('codex', 'done', lines(CODEX_STREAM), b'', 0, False, ()),
        ('codex', 'error-event', lines([{'type': 'error', 'message': 'quota exceeded'}, {'type': 'turn.completed'}]), b'', 0, False, ()),
        ('gemini', 'auth-code', lines([{'type': 'init', 'session_id': 'g'}]), b'', 41, False, ()),
        ('gemini', 'done', lines(GEMINI_STREAM), b'', 0, False, ()),
        ('gemini', 'list-and-junk', b'[1,2]\n\n{bad\n' + lines([{'type': 'result', 'status': 'success'}]), b'', 0, False, ()),
        ('codex', 'launch', b'', b'', 0, True, ()),
    ]
    profiles = {p.key: p for p in ai_programs.PROFILES}
    out = []
    for key, name, stdout, stderr, code, fail, secrets in scenarios:
        ScriptedTree.script = (stdout, stderr, code, fail)
        ScriptedTree.seen = []
        runner = ai_programs.AiRunner()
        events = []
        finished = []
        runner.event.connect(lambda e, events=events: events.append([e.kind, e.text, e.count]))
        runner.finished.connect(finished.append)
        request = Request(('prog',), root / 'w' / name, SERVER, 'p', 'ko')
        request.workspace.mkdir(parents=True, exist_ok=True)
        runner._run(profiles[key], request, ['prog', 'p'], secrets, 30)
        out.append({'key': key, 'name': name, 'stdout': stdout.decode('utf-8'), 'stderr': stderr.decode('utf-8'),
                    'code': code, 'fail': fail, 'secrets': list(secrets), 'events': events, 'result': finished[0],
                    'session': runner.session_id, 'diagnostic': runner.diagnostic,
                    'started': placed(ScriptedTree.seen, root)})
    restore_env()
    case('ai_runner', {'env': {'MY_API_KEY': 'supersecretvalue', 'SHORT_TOKEN': 'abc', 'OTHER': 'notsecretvalue'},
                       'scenarios': out})


# ---- program discovery ---------------------------------------------------------------------------------------

def touch(path, data=b'x'):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def paths():
    out = []
    layouts = {
        'empty': [],
        'path-exe': ['bin/claude.exe', 'bin/codex.exe', 'bin/node.exe', 'bin/npm.cmd'],
        'npm-cmd': ['roaming/npm/claude.cmd', 'roaming/npm/gemini.cmd', 'roaming/npm/codex.cmd',
                    'roaming/npm/node_modules/@anthropic-ai/claude-code/cli.js',
                    'roaming/npm/node_modules/@google/gemini-cli/dist/index.js',
                    'roaming/npm/node_modules/@openai/codex/vendor/x86_64-pc-windows-msvc/codex/codex.exe',
                    'programs/nodejs/node.exe'],
        'npm-no-node': ['roaming/npm/gemini.cmd', 'roaming/npm/node_modules/@google/gemini-cli/dist/index.js'],
        'npm-no-script': ['roaming/npm/claude.cmd', 'programs/nodejs/node.exe'],
        'local-bin': ['home/.local/bin/claude.exe', 'local/Programs/OpenAI/Codex/bin/codex.exe',
                      'local/Microsoft/WindowsApps/gemini.exe'],
        'codex-platform': ['roaming/npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/b/bin/codex.exe',
                           'roaming/npm/node_modules/@openai/codex/vendor/a/codex/codex.exe'],
        'codex-install-dir': ['custom/codex.exe'],
    }
    for name, files in layouts.items():
        root = fresh('paths-' + name)
        for f in files:
            touch(root / f)
        extra = {'PATH': str(root / 'bin')} if name == 'path-exe' else {}
        if name == 'codex-install-dir':
            extra['CODEX_INSTALL_DIR'] = str(root / 'custom')
        isolated_env(root, extra)
        found = {'claude': ai_programs.find_program('claude'), 'gemini': ai_programs.find_program('gemini'),
                 'codex': ai_programs.codex_program(), 'node': ai_paths.find_tool('node'), 'npm': ai_paths.find_tool('npm'),
                 'node_ready': ai_install.node_ready(),
                 'folders': [str(p) for p in ai_paths.tool_folders('win32')],
                 'install_path': ai_paths.install_environment('win32')['PATH']}
        restore_env()
        out.append({'name': name, 'files': files, 'env': placed(extra, root), 'found': placed(found, root)})
    case('ai_paths', {'layouts': out})


# ---- launching -----------------------------------------------------------------------------------------------

class Recorder:
    calls = []

    def __init__(self, argv, **kwargs):
        Recorder.calls.append({'argv': list(argv), 'cwd': str(kwargs.get('cwd')),
                               'path': (kwargs.get('env') or {}).get('PATH'),
                               'flags': kwargs.get('creationflags', 0)})
        self.code = None

    def poll(self):
        return self.code


def launch():
    root = fresh('launch')
    isolated_env(root)
    real = subprocess.Popen
    subprocess.Popen = Recorder
    out = {'install': [], 'sign_in': []}
    try:
        for language in ('ko', 'en'):
            i18n.set_language(language)
            for key in ('claude', 'codex', 'gemini'):
                Recorder.calls = []
                session = ai_install.launch_install(ai_install.INSTALLERS[key]['win32'], root / 'settings', platform='win32')
                out['install'].append({'language': language, 'key': key, 'installer': placed(vars(ai_install.INSTALLERS[key]['win32']), root),
                                       'calls': placed(Recorder.calls, root), 'done': session.done is not None})
            Recorder.calls = []
            ai_install.launch_install(ai_install.NODE_WINDOWS, root / 'settings', platform='win32')
            out['install'].append({'language': language, 'key': 'node', 'installer': vars(ai_install.NODE_WINDOWS),
                                   'calls': placed(Recorder.calls, root), 'done': False})
        i18n.set_language('ko')
        for profile in ai_programs.PROFILES:
            Recorder.calls = []
            ai_auth.launch_sign_in(profile, ('C:\\p\\' + profile.key + '.exe',), root / 'settings', platform='win32')
            out['sign_in'].append({'key': profile.key, 'calls': placed(Recorder.calls, root)})
        errors = []
        for fn in (lambda: ai_auth.launch_sign_in(ai_programs.PROFILES[0], None, root),
                   lambda: ai_auth.launch_sign_in(ai_programs.PROFILES[0], ('x',), root, platform='linux'),
                   lambda: ai_install.launch_install(ai_install.NODE_WINDOWS, root, platform='linux')):
            try:
                fn()
                errors.append(None)
            except (OSError, ValueError) as exc:
                errors.append(f'{type(exc).__name__}: {exc}')
        out['errors'] = errors
    finally:
        subprocess.Popen = real
        restore_env()
    out['installers'] = {key: {p: vars(i) for p, i in table.items()} for key, table in ai_install.INSTALLERS.items()}
    out['node_windows'] = vars(ai_install.NODE_WINDOWS)
    out['urls'] = {'node': ai_install.NODE_URL}
    out['profiles'] = [{'key': p.key, 'name': p.name, 'resume': p.resume, 'persistent': p.persistent,
                        'status_args': p.status_args, 'login_args': list(p.login_args),
                        'auth_exit_codes': list(p.auth_exit_codes)} for p in ai_programs.PROFILES]
    case('ai_launch', out)


# ---- the connection ------------------------------------------------------------------------------------------

class FakeServer:
    def __init__(self, port):
        self.server_address = ('127.0.0.1', port)
        self.in_flight = 0

    def shutdown(self):
        pass

    def close_connections(self):
        pass

    def server_close(self):
        pass


class Net:
    taken = set()
    bound = []

    @staticmethod
    def start(port, service, gui):
        Net.bound.append(port)
        if port in Net.taken:
            raise OSError(errno.EADDRINUSE, 'address in use')
        return FakeServer(port)


class Sequence:
    def __init__(self, values):
        self.values = list(values)
        self.used = []

    def __call__(self, *args):
        value = self.values.pop(0)
        self.used.append(value)
        return value


class Session:
    def __init__(self, folder):
        self.preferences = Preferences(folder / 'settings.json')
        self.data_dir = folder / 'Library'
        self.window = None


def mcp_files(folder):
    folder = folder / 'mcp'
    if not folder.exists():
        return {}
    out = {}
    for path in sorted(folder.iterdir()):
        if path.name in ('endpoint', 'auth-header'):
            out[path.name] = path.read_text(encoding='utf-8')
        else:
            out[path.name] = path.stat().st_size if path.name.endswith(('.ps1', '.sh', '.lock')) else 'template'
    return out


def view(service, folder):
    return {'preferences': json.loads((folder / 'settings.json').read_text(encoding='utf-8'))
            if (folder / 'settings.json').exists() else None,
            'running': service.running, 'error': service.error, 'bound_port': service.bound_port,
            'port': service.port, 'enabled': service.enabled, 'token': service.token, 'elsewhere': service.elsewhere,
            'port_changed': service.port_changed, 'owner': service.owner is not None,
            'activity': [list(a) for a in service.activity], 'files': mcp_files(folder)}


def connection():
    root = fresh('connection')
    folder = root / 'settings'
    mcp_http.start = Net.start
    real_strftime = mcp_service.time.strftime
    mcp_service.time.strftime = lambda fmt, *rest: '12:00:00' if not rest else real_strftime(fmt, *rest)
    tokens = Sequence(['a' * 64, 'b' * 64, 'c' * 64, 'd' * 64])
    mcp_service.secrets.token_hex = lambda n=None: tokens() if n == 32 else '00000000'
    ports = Sequence([40001, 47821, 40002, 40003, 41000, 41001, 41002])
    mcp_service.random.randint = ports
    steps = []
    service = mcp_service.McpService(Session(folder))

    def step(name, action):
        Net.bound = []
        try:
            value = action()
        except OSError as exc:
            value = f'OSError: {exc}'
        steps.append({'step': name, 'returned': value, 'bound': list(Net.bound), **view(service, folder)})

    for language in ('ko',):
        i18n.set_language(language)
    step('first start (default port taken)', lambda: (Net.taken.add(47821), service.start_if_enabled())[1])
    step('start again (running)', lambda: service.start())
    step('stop', lambda: service.stop())
    step('start with the saved port', lambda: service.start())
    step('new key', lambda: service.regenerate_token())
    step('other port (first candidate is the old port)', lambda: service.change_port())
    step('switch off', lambda: service.set_enabled(False))
    (folder / 'mcp' / 'endpoint').write_text('stale', encoding='utf-8')
    step('start at launch while switched off (stale files go)', lambda: service.start_if_enabled())
    step('switch on', lambda: service.set_enabled(True))
    other = mcp_service.McpService(Session(folder))
    steps.append({'step': 'another Grainy on the same settings', 'returned': other.start(), 'bound': [],
                  **view(other, folder)})
    step('saved port taken', lambda: (service.stop(), Net.taken.add(service.port), service.start())[2])
    i18n.set_language('en')
    step('saved port taken (English)', lambda: service.start())
    i18n.set_language('ko')
    Net.taken.clear()
    # A settings file that cannot be written: the folder is a file.
    broken = fresh('connection-broken')
    (broken / 'settings').write_text('not a folder', encoding='utf-8')
    bad = mcp_service.McpService(Session(broken / 'settings' / 'inner'))
    bad_steps = []
    for name, action in [('start without a key', lambda: bad.start()), ('switch off', lambda: bad.set_enabled(False)),
                         ('switch on', lambda: bad.set_enabled(True))]:
        value = action()
        bad_steps.append({'step': name, 'returned': value, 'running': bad.running, 'error': bad.error,
                          'activity': [list(a) for a in bad.activity]})
    mcp_service.time.strftime = real_strftime
    case('ai_connection', {'steps': placed(steps, root), 'broken': placed(bad_steps, broken),
                           'ports_used': ports.used, 'bridges': {n: (mcp_service.bridge_source() / n).stat().st_size
                                                                 for n in mcp_service.BRIDGE_FILES}})


# ---- the MCP dialog ------------------------------------------------------------------------------------------

class FakeJob:
    def __init__(self, state):
        self.state = state


class FakeConnection:
    def __init__(self, folder, **kw):
        self.preferences = Preferences(folder / 'settings.json')
        self.running = kw.get('running', False)
        self.error = kw.get('error')
        self.elsewhere = kw.get('elsewhere', False)
        self.port_changed = kw.get('port_changed', False)
        self.bound_port = kw.get('bound_port')
        self.job = kw.get('job')
        self.activity = kw.get('activity', [])
        self.folder = folder / 'mcp'
        for key, value in kw.get('prefs', {}).items():
            self.preferences.values[key] = value

    @property
    def enabled(self):
        return self.preferences.values.get('mcp_enabled') is not False

    @property
    def port(self):
        return mcp_service.McpService.port.fget(self)

    @property
    def token(self):
        return mcp_service.McpService.token.fget(self)

    def export_root(self):
        return mcp_service.McpService.export_root(self)


class FakeCatalog:
    def __init__(self, entries):
        self.entries = entries


class FakeWindow(QWidget):
    def __init__(self, entry, closing=False):
        super().__init__()
        self.closing = closing
        self.catalog = FakeCatalog(entry)


def mcp_dialog_views():
    root = fresh('mcp-dialog')
    mcp_service.pictures_folder = lambda: Path('P:/Pictures/Grainy')
    from luma import photo_service
    real_last = photo_service.last_batch
    states = [
        ('off', {'prefs': {'mcp_enabled': False}}, None),
        ('on', {'running': True, 'bound_port': 47821, 'prefs': {'mcp_token': 'e' * 64, 'mcp_port': 47821},
                'activity': [('10:00:00', 'server', 'started'), ('10:01:00', 'apply_edits', 'ok'),
                             ('10:02:00', 'auto_tone', 'job j1 done')]},
         {'label': 'AI 자동 톤', 'count': 1234, 'created': '2026-11-13T01:02:03', 'tool': 'auto_tone'}),
        ('on, key shown', {'running': True, 'bound_port': 41234, 'prefs': {'mcp_token': 'e' * 64,
                                                                           'mcp_export_folder': 'D:\\AI out'}},
         {'label': 'AI 보정', 'count': 2, 'created': 'bad', 'tool': 'apply_edits'}),
        ('port taken', {'error': '포트 47821을(를) 쓸 수 없습니다: address in use', 'prefs': {'mcp_port': 47821}}, None),
        ('elsewhere', {'error': '이 컴퓨터에서 실행 중인 다른 Grainy가 AI 연결을 쓰고 있습니다.', 'elsewhere': True}, None),
        ('job running', {'running': True, 'bound_port': 47821, 'port_changed': True, 'job': FakeJob('running'),
                         'prefs': {'mcp_token': 'not a key', 'mcp_port': 80}},
         {'label': 'AI 라벨', 'count': 3, 'created': '2026-01-02T03:04:05+00:00', 'tool': 'set_labels'}),
    ]
    out = []
    for language in ('ko', 'en'):
        i18n.set_language(language)
        for name, kw, entry in states:
            photo_service.last_batch = lambda catalog, entry=entry: entry
            service = FakeConnection(root, **kw)
            dialog = mcp_dialog.McpDialog(service, FakeWindow(entry))
            if name == 'on, key shown':
                dialog.toggle_key()
            dialog.refresh()
            setups = {key: [dialog.setups[key][0].text(), dialog.setups[key][1].toPlainText()]
                      for key, _ in mcp_dialog.CLIENTS}
            out.append({'language': language, 'name': name, 'enabled': dialog.enable.isChecked(),
                        'status': dialog.status.text(), 'advanced': dialog.advanced_toggle.isChecked(),
                        'port_button': dialog.port_button.isEnabled(), 'port_note': not dialog.port_note.isHidden(),
                        'http': dialog.http.text(), 'key': dialog.key.text(),
                        'key_shown': dialog.key.echoMode() == dialog.key.EchoMode.Normal,
                        'show_key': dialog.show_key.text(), 'key_enabled': dialog.key.isEnabled(),
                        'export_folder': dialog.export_folder.text(), 'setups': placed(setups, root),
                        'undo_label': dialog.undo_label.text(), 'undo_enabled': dialog.undo_button.isEnabled(),
                        'activity': [dialog.activity.item(i).text() for i in range(dialog.activity.count())],
                        'title': dialog.windowTitle(),
                        'tabs': [dialog.tabs.tabText(i) for i in range(dialog.tabs.count())]})
            dialog.deleteLater()
    i18n.set_language('ko')
    photo_service.last_batch = real_last
    case('ai_mcp_dialog', {'views': out, 'local_offset_note': 'undo times are local: tests compare with the same zone',
                           'explain': mcp_dialog.EXPLAIN, 'regen': mcp_dialog.REGEN})


# ---- the Ask AI window ---------------------------------------------------------------------------------------

class FakeRunner:
    def __init__(self):
        from PySide6.QtCore import QObject, Signal

        class Signals(QObject):
            event = Signal(object)
            finished = Signal(str)
        self.signals = Signals()
        self.event = self.signals.event
        self.finished = self.signals.finished
        self.running = False
        self.session_id = ''
        self.started = []
        self.fail = False

    def start(self, profile, request, *, secrets=(), timeout=600):
        if self.fail:
            raise OSError('no')
        self.started.append((profile.key, request, secrets))
        self.running = True

    def stop(self):
        self.running = False

    def shutdown(self):
        self.running = False


class AskService:
    def __init__(self, folder):
        self.preferences = Preferences(folder / 'settings.json')
        self.running = True
        self.error = None
        self.folder = folder / 'mcp'
        self.token = 'f' * 64
        self.job = None
        self.started = 0
        self.bridges_ok = True

    @property
    def enabled(self):
        return self.preferences.values.get('mcp_enabled') is not False

    def set_enabled(self, on):
        self.preferences.save('mcp_enabled', bool(on))

    def start(self):
        self.started += 1
        self.running = True
        return True

    def write_bridges(self):
        return self.bridges_ok

    def write_connection(self):
        return self.bridges_ok


class AskCatalog:
    def photo(self, ident):
        return {'name': 'garden "1".png'} if ident == 7 else None


class Subfolders:
    def __init__(self, on):
        self.on = on

    def isChecked(self):
        return self.on


class AskWindow(QWidget):
    def __init__(self):
        super().__init__()
        self.closing = False
        self.catalog = AskCatalog()
        self.folder_filter = 'D:\\Photos\\여름'
        self.include_subfolders = Subfolders(True)
        self.visible_ids = [7, 8, 9]
        self.current_id = 7
        self.selection = [7, 8]

    def selected_ids(self):
        return list(self.selection)


def make_profile(key, found):
    base = next(p for p in ai_programs.PROFILES if p.key == key)
    from dataclasses import replace
    return replace(base, detect=(lambda: found))


class AuthNone:
    """The window's auth controller without threads: checks are recorded, nothing runs."""


def ask_dialog():
    from luma import ai_dialog, photo_service
    real_last = photo_service.last_batch
    photo_service.last_batch = lambda catalog: None
    out = []
    for language in ('ko', 'en'):
        i18n.set_language(language)
        root = fresh('ask-' + language)
        isolated_env(root)
        service = AskService(root)
        window = AskWindow()
        runner = FakeRunner()
        found = {'claude': ('C:\\p\\claude.exe',), 'codex': None, 'gemini': ('node', 'C:\\g\\index.js')}
        profiles = tuple(make_profile(k, found[k]) for k in ('claude', 'codex', 'gemini'))
        checks = []
        ai_dialog.AuthController.check = lambda self, profile, executable: checks.append(profile.key)
        dialog = ai_dialog.AiDialog(service, window, profiles=profiles, runner=runner)
        log = []

        def snap(label):
            log.append({'label': label, 'notice': not dialog.notice.isHidden(),
                        'program': dialog.program.currentIndex(),
                        'programs': [dialog.program.itemText(i) for i in range(dialog.program.count())],
                        'send': dialog.send_button.isEnabled(), 'stop': dialog.stop_button.isEnabled(),
                        'new': dialog.new_button.isEnabled(), 'enable_visible': not dialog.enable.isHidden(),
                        'connection': dialog.connection.text(), 'status': dialog.status.text(),
                        'hints': not dialog.hints.isHidden(), 'hints_text': dialog.hints.text(),
                        'auth': {k: v.text() for k, v in dialog.auth_labels.items()},
                        'login_visible': {k: not v.isHidden() for k, v in dialog.login_buttons.items()},
                        'login_enabled': {k: v.isEnabled() for k, v in dialog.login_buttons.items()},
                        'login_bold': {k: bool(v.styleSheet()) for k, v in dialog.login_buttons.items()},
                        'install_visible': {k: not v.isHidden() for k, v in dialog.install_buttons.items()},
                        'install_enabled': {k: v.isEnabled() for k, v in dialog.install_buttons.items()},
                        'login_help': dialog.login_help.text() if not dialog.login_help.isHidden() else None,
                        'transcript': dialog.transcript.toPlainText(), 'undo_label': dialog.undo_label.text(),
                        'undo_enabled': dialog.undo_button.isEnabled(), 'checks': list(checks),
                        'preferences': json.loads(service.preferences.path.read_text(encoding='utf-8'))
                        if service.preferences.path.exists() else None})

        snap('opened')
        dialog.send()
        snap('send before the notice')
        dialog.acknowledge()
        snap('notice acknowledged')
        dialog.auth_status_ready('claude', AuthStatus('signed_in', 'Claude'))
        dialog.auth_status_ready('gemini', AuthStatus('timeout'))
        dialog.auth_status_ready('codex', AuthStatus('signed_in', 'ChatGPT'))
        snap('statuses')
        dialog.input.setPlainText('   ')
        dialog.send()
        snap('empty request')
        dialog.input.setPlainText('이 사진들 밝게 해 줘')
        dialog.send()
        started = []
        run_keys = {}

        def request_view(item):
            key, request, secrets = item
            # The run key is a random uuid4().hex: stored as <RUN1>, <RUN2>... in the order of first use.
            run = run_keys.setdefault(request.workspace.name, f'<RUN{len(run_keys) + 1}>')
            workspace = str(request.workspace.parent / run)
            return {'key': key, 'executable': list(request.executable), 'workspace': workspace,
                    'server': request.server, 'prompt': request.prompt, 'language': request.language,
                    'model': request.model, 'session_id': request.session_id, 'context': request.context,
                    'secrets': list(secrets)}
        started.append(request_view(runner.started[-1]))
        snap('sent')
        for kind, text, count in [('session', 'sess-7', None), ('text', '밝게 ', None), ('text', '하겠습니다.', None),
                                  ('tool', 'list_photos', None), ('tool', 'apply_edits', 2), ('error', '', None)]:
            dialog.on_event(ai_programs.Event(kind, text, count))
        snap('events')
        runner.running = False
        dialog.on_finished('done')
        snap('finished')
        dialog.model.setText('opus')
        dialog.choice_changed()
        snap('model changed (new conversation)')
        dialog.input.setPlainText('a' * 8001)
        dialog.send()
        snap('too long')
        dialog.input.setPlainText('다시')
        dialog.send()
        started.append(request_view(runner.started[-1]))
        dialog.on_event(ai_programs.Event('auth_error'))
        runner.running = False
        dialog.on_finished('auth_required')
        snap('auth required')
        dialog.program.setCurrentIndex(2)
        snap('gemini chosen')
        dialog.input.setPlainText('세 번째 요청')
        dialog.send()
        started.append(request_view(runner.started[-1]))
        runner.running = False
        dialog.on_finished('stopped')
        dialog.input.setPlainText('네 번째')
        dialog.send()
        started.append(request_view(runner.started[-1]))
        runner.running = False
        dialog.on_event(ai_programs.Event('exit', '3'))
        dialog.on_finished('failed')
        snap('second turn')
        for result in ('timeout', 'other'):
            dialog.on_finished(result)
        snap('other results')
        service.bridges_ok = False
        dialog.input.setPlainText('다섯')
        dialog.send()
        snap('bridges missing')
        service.bridges_ok = True
        runner.fail = True
        dialog.send()
        snap('launch failed')
        runner.fail = False
        service.set_enabled(False)
        dialog.refresh()
        snap('connection off')
        dialog.enable_connection()
        snap('connection on again')
        dialog.new_conversation()
        snap('new conversation')
        dialog.detect_program(1)
        snap('codex detected again (still missing)')
        dialog.timer.stop()
        dialog.deleteLater()
        restore_env()
        out.append({'language': language, 'log': placed(log, root), 'started': placed(started, root),
                    'context': dialog.grainy_context()})
    photo_service.last_batch = real_last
    i18n.set_language('ko')
    case('ai_dialog', {'sessions': out, 'notice': ai_dialog.NOTICE})


def main():
    QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.StandardButton.No)
    texts()
    events()
    commands()
    runner()
    paths()
    launch()
    connection()
    mcp_dialog_views()
    ask_dialog()
    shutil.rmtree(SCRATCH, ignore_errors=True)


if __name__ == '__main__':
    main()
