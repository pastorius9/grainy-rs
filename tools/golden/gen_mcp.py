"""Goldens for grainy-mcp: the MCP protocol layer, tool definitions and argument checker of the oracle.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_mcp.py

Inputs that depend on key order (messages, values, schemas) are stored as JSON *text* (`*_json` fields), because
`common.write_json` sorts keys. Outputs the oracle produces as text (`dumps`) are stored as text and compared as
bytes in Rust.
"""
import base64
import copy
import json
import time

from common import Case, ORACLE  # noqa: F401  (puts the oracle on sys.path)
from luma import __version__, mcp_protocol as protocol, mcp_tools, photo_service
from luma.mcp_service import Unfinished
from luma.photo_service import Busy, ServiceError

MODULE = 'mcp'


def J(value):
    """JSON text with insertion order kept (ASCII)."""
    return json.dumps(value)


def D(value):
    """The oracle's wire bytes as text."""
    return protocol.dumps(value).decode('ascii')


# --- constants ----------------------------------------------------------------------------------

with Case(MODULE, 'constants') as case:
    case.json(version=__version__, modern=list(protocol.MODERN), legacy=list(protocol.LEGACY),
              supported=list(protocol.SUPPORTED), offline_version=protocol.OFFLINE_VERSION,
              cache=protocol.CACHE, meta_version=protocol.META_VERSION, meta_capabilities=protocol.META_CAPABILITIES,
              meta_server=protocol.META_SERVER, instructions=protocol.INSTRUCTIONS, offline_text=protocol.OFFLINE_TEXT,
              offline_note=protocol.OFFLINE_NOTE, offline_error=protocol.OFFLINE_ERROR,
              modern_enabled=protocol.MODERN_ENABLED, deadline_seconds=protocol.DEADLINE_SECONDS,
              server_info_json=J(protocol.SERVER_INFO), unfinished=mcp_tools.UNFINISHED, target_text=mcp_tools.TARGET_TEXT,
              max_id=photo_service.MAX_ID, max_targets=photo_service.MAX_TARGETS,
              curve_points=photo_service.CURVE_POINTS, sheet_cells=mcp_tools.SHEET_CELLS,
              export_formats=mcp_tools.EXPORT_FORMATS, color_spaces=mcp_tools.COLOR_SPACES,
              film_types=list(mcp_tools.FILM_TYPES), sorts=list(photo_service.SORTS),
              copy_groups=list(photo_service.COPY_GROUPS), flags=list(photo_service.FLAGS),
              labels=list(photo_service.LABELS),
              fields_json=J([[k, lo, hi] for k, (_, lo, hi) in photo_service.FIELDS.items()]))

# --- tool definitions -------------------------------------------------------------------------

with Case(MODULE, 'tools_list') as case:
    case.json(definitions_bytes=D(mcp_tools.DEFINITIONS),
              per_tool_bytes=[D(d) for d in mcp_tools.DEFINITIONS],
              names=[d['name'] for d in mcp_tools.DEFINITIONS],
              schemas_json=J(mcp_tools.SCHEMAS), schemas_bytes=D(mcp_tools.SCHEMAS), target_bytes=D(mcp_tools.TARGET(7)), edits_description=mcp_tools._edits_description(),
              target_json=J(mcp_tools.TARGET(7)), bare_filter_json=J(mcp_tools.BARE_FILTER),
              edit_entry_json=J(mcp_tools.EDIT_ENTRY), range_samples={f'{a},{b}': mcp_tools._range(a, b) for a, b in
                  [(-5, 5), (0, 100), (-200, 200), (3, 10), (0, 150), (-45, 45), (0, 1), (0, 270), (-3, 3)]})

# --- handle ------------------------------------------------------------------------------------

CASES = []
BLOBS = {}


def blob(text):
    """Bodies of more than 2000 characters (tools/list) are stored once, by digest."""
    if text is None or len(text) <= 2000:
        return text
    import hashlib
    digest = hashlib.sha256(text.encode()).hexdigest()[:16]
    BLOBS[digest] = text
    return {'blob': digest}


class FakeTools:
    def __init__(self, result=None, fail=False):
        self.calls = []
        self.result = result or {'content': [{'type': 'text', 'text': '{}'}], 'isError': False}
        self.fail = fail

    def definitions(self):
        return mcp_tools.DEFINITIONS

    def call(self, name, arguments, *, modern, deadline):
        self.calls.append([name, arguments, modern])
        if self.fail:
            raise RuntimeError('boom')
        return self.result


def run(name, message, headers=None, flavour='http', modern=False, result=None, fail=False):
    """One handle() case recorded with the oracle's reply."""
    protocol.MODERN_ENABLED = modern
    tools = FakeTools(result=result, fail=fail)
    pairs = list(headers or [])
    reply = protocol.handle(message, dict(pairs), flavour=flavour, tools=tools, deadline=time.monotonic() + 45)
    assert reply.headers == {}
    CASES.append({'name': name, 'message_json': J(message), 'headers': [list(p) for p in pairs], 'flavour': flavour,
                  'modern': modern, 'tool_result_json': J(tools.result), 'tool_fail': fail,
                  'status': reply.status, 'body': blob(None if reply.body is None else D(reply.body)),
                  'calls_json': J(tools.calls)})
    protocol.MODERN_ENABLED = False


def req(method, params=None, ident=1):
    message = {'jsonrpc': '2.0', 'id': ident, 'method': method}
    if params is not None:
        message['params'] = params
    return message


B64 = lambda text: '=?base64?' + base64.b64encode(text.encode()).decode() + '?='  # noqa: E731

for flavour in ('http', 'stdio'):
    f = flavour
    for version in protocol.SUPPORTED + ('2099-01-01', '', None, 7, 1.5, True, ['2025-11-25'], {'a': 1}):
        run(f'{f}_initialize_{version!r}', req('initialize', {'protocolVersion': version, 'capabilities': {},
                                                                'clientInfo': {'name': 't'}}), flavour=f)
    run(f'{f}_initialize_no_params', req('initialize'), flavour=f)
    run(f'{f}_initialize_null_params', {'jsonrpc': '2.0', 'id': 3, 'method': 'initialize', 'params': None}, flavour=f)
    run(f'{f}_initialize_empty_params', req('initialize', {}), flavour=f)
    run(f'{f}_initialize_params_list', req('initialize', [1]), flavour=f)
    run(f'{f}_initialize_params_string', req('initialize', 'x'), flavour=f)
    run(f'{f}_initialize_params_number', req('initialize', 5), flavour=f)
    run(f'{f}_initialize_string_id', req('initialize', {'protocolVersion': '2025-06-18'}, ident='a"b-é'), flavour=f)
    for message in ({'jsonrpc': '2.0', 'method': 'notifications/initialized'},
                    {'jsonrpc': '2.0', 'method': 'notifications/cancelled', 'params': {'requestId': 3}},
                    {'jsonrpc': '2.0', 'method': 'notifications/cancelled', 'params': 5},
                    {'jsonrpc': '2.0', 'method': 7},
                    {'jsonrpc': '2.0', 'id': 5, 'result': {}}, {'jsonrpc': '2.0', 'id': 5, 'error': {'code': 1, 'message': 'x'}},
                    {'jsonrpc': '2.0', 'result': 1}, {'jsonrpc': '2.0', 'id': True, 'result': 1}):
        run(f'{f}_note_{J(message)}', message, flavour=f)
    for message in ({'jsonrpc': '2.0', 'id': 4}, {'jsonrpc': '1.0', 'id': 4, 'method': 'ping'},
                    {'id': 4, 'method': 'ping'}, {'jsonrpc': 2.0, 'id': 4, 'method': 'ping'},
                    {'jsonrpc': '2.0', 'id': 4, 'method': 7}, {'jsonrpc': '2.0', 'id': 4, 'method': None},
                    {'jsonrpc': '2.0', 'id': 4, 'method': ['ping']}, {'jsonrpc': '2.0', 'method': 'ping', 'id': None},
                    {}, {'jsonrpc': '2.0', 'id': [1], 'method': 7}, {'jsonrpc': '2.0', 'id': 'x'},
                    {'jsonrpc': '2.0', 'id': 1.5}, {'jsonrpc': '2.0', 'id': 4, 'result': 1, 'error': 2}):
        run(f'{f}_invalid_{J(message)}', message, flavour=f)
    for message in ([], [req('ping')], 'text', 7, None, True, 1.5, [[]]):
        run(f'{f}_nondict_{J(message)}', message, flavour=f)
    for ident in (True, False, 1.5, None, [1], {'a': 1}, 'abc', '', 0, -7, 2**53, 'ü', 1e3):
        run(f'{f}_id_{J(ident)}', {'jsonrpc': '2.0', 'id': ident, 'method': 'ping'}, flavour=f)
    for method in ('ping', 'resources/list', 'resources/templates/list', 'prompts/list', 'logging/setLevel',
                   'completion/complete', 'nope', '', 'tools/list', 'server/discover', 'subscriptions/listen',
                   'notifications/initialized'):
        run(f'{f}_method_{method}', req(method), flavour=f)
        run(f'{f}_method_params_{method}', req(method, {'x': [1, 2], 'y': 'ü'}), flavour=f)
    run(f'{f}_unknown_with_string_id', req('nope', ident='id-é'), flavour=f)
    run(f'{f}_list_params_bad', req('tools/list', [1]), flavour=f)

    # tools/call
    for name, params in [
            ('ok', {'name': 'list_photos'}), ('ok_args', {'name': 'get_job', 'arguments': {'job': 'j1'}}),
            ('null_args', {'name': 'list_photos', 'arguments': None}), ('empty_args', {'name': 'list_photos', 'arguments': {}}),
            ('nested_args', {'name': 'apply_edits', 'arguments': {'edits': [{'ids': [1, 2], 'set': {'exposure': 1.5}}]}}),
            ('unknown', {'name': 'delete_everything'}), ('no_name', {}), ('null_name', {'name': None}),
            ('int_name', {'name': 7}), ('float_name', {'name': 1.5}), ('bool_name', {'name': True}),
            ('list_name', {'name': ['a', 1, None, True, 2.5, {'k': 'v'}]}), ('dict_name', {'name': {'a': 1, 'b': [2]}}),
            ('quote_name', {'name': "it's"}), ('dquote_name', {'name': 'say "x"'}), ('both_quotes', {'name': '\'"'}),
            ('unicode_name', {'name': 'tëst 한글 \U0001f600'}), ('nonprint_name', {'name': ['a\x00b\x7f\u00a0\u200b\ue000']}),
            ('control_name', {'name': ['\n\r\t\\ \x1b']}), ('empty_name', {'name': ''}), ('case_name', {'name': 'List_Photos'}),
            ('args_list', {'name': 'list_photos', 'arguments': [1]}), ('args_str', {'name': 'list_photos', 'arguments': 'x'}),
            ('args_num', {'name': 'list_photos', 'arguments': 0}), ('args_false', {'name': 'list_photos', 'arguments': False}),
            ('args_empty_list', {'name': 'list_photos', 'arguments': []}), ('unknown_args_list', {'name': 'zzz', 'arguments': [1]}),
            ('extra_params', {'name': 'list_photos', 'foo': 1, '_meta': {'progressToken': 1}})]:
        run(f'{f}_call_{name}', req('tools/call', params), flavour=f)
    run(f'{f}_call_fail', req('tools/call', {'name': 'list_photos'}), flavour=f, fail=True)
    run(f'{f}_call_fail_unknown', req('tools/call', {'name': 'zz'}), flavour=f, fail=True)
    run(f'{f}_call_is_error', req('tools/call', {'name': 'get_job', 'arguments': {'job': 'j1'}}), flavour=f,
        result={'content': [{'type': 'text', 'text': 'bad'}], 'isError': True})
    run(f'{f}_call_unicode_result', req('tools/call', {'name': 'list_photos'}), flavour=f,
        result={'content': [{'type': 'text', 'text': '{"name":"현상.png"}\n두 줄 \U0001f600 \x7f \x1f'}], 'isError': False})
    run(f'{f}_call_float_result', req('tools/call', {'name': 'list_photos'}), flavour=f,
        result={'content': [], 'isError': False, 'v': [1.0, 0.1, 1e16, 1e-5, 1e22, 123456789.123456789, -0.0, 5e-324,
                                                        1.7976931348623157e308, 100.0, 2**53, -2**63, 2**64 - 1, 0.5, 1e15, 12345678901234567.0]})
    run(f'{f}_call_key_order', req('tools/call', {'name': 'list_photos'}), flavour=f,
        result={'z': 1, 'a': {'y': 2, 'b': [{'q': 1, 'c': 2}]}, 'content': [], 'isError': False, '_meta': {'k': 1}})
    run(f'{f}_call_string_id', req('tools/call', {'name': 'list_photos'}, ident='x"y'), flavour=f)

# headers on the legacy path (http checks the protocol version header, stdio does not)
for flavour in ('http', 'stdio'):
    f = flavour
    for header_name in ('mcp-protocol-version', 'MCP-Protocol-Version', 'Mcp-Protocol-Version'):
        for value in ('2025-11-25', '2025-06-18', '2025-03-26', '2024-11-05', '2026-07-28', '2099-01-01', '', 'x',
                      B64('2025-06-18'), ' 2025-06-18'):
            run(f'{f}_hdr_{header_name}_{value}', req('tools/list'), [(header_name, value)], flavour=f)
    run(f'{f}_hdr_two_cases', req('ping'), [('MCP-Protocol-Version', '2099'), ('mcp-protocol-version', '2025-06-18')], flavour=f)
    run(f'{f}_hdr_two_cases_rev', req('ping'), [('mcp-protocol-version', '2025-06-18'), ('MCP-Protocol-Version', '2099')], flavour=f)
    run(f'{f}_hdr_discover', req('server/discover'), [('MCP-Protocol-Version', '2026-07-28')], flavour=f)
    run(f'{f}_hdr_initialize_ignores_version', req('initialize', {'protocolVersion': '2025-06-18'}),
        [('MCP-Protocol-Version', '2099-01-01')], flavour=f)
    run(f'{f}_hdr_call_bad', req('tools/call', {'name': 'list_photos'}), [('mcp-protocol-version', '2099-01-01')], flavour=f)
    run(f'{f}_hdr_other', req('ping'), [('Accept', 'application/json'), ('Content-Type', 'application/json')], flavour=f)

# a v2 client against the shipped (modern off) server: _meta is ignored
META = {'_meta': {protocol.META_VERSION: '2026-07-28', protocol.META_CAPABILITIES: {}}}
for method in ('server/discover', 'tools/list', 'ping', 'tools/call', 'initialize'):
    params = {**META, **({'name': 'list_photos', 'arguments': {}} if method == 'tools/call' else {})}
    run(f'off_meta_{method}', req(method, params),
        [('MCP-Protocol-Version', '2026-07-28'), ('Mcp-Method', method)])
    run(f'off_meta_nohdr_{method}', req(method, params))
    run(f'off_meta_stdio_{method}', req(method, params), [('MCP-Protocol-Version', '2026-07-28')], flavour='stdio')


# modern era switched on
def mreq(method, params=None, headers=None, version='2026-07-28', capabilities=True, ident=1, flavour='http'):
    meta = {protocol.META_VERSION: version}
    if capabilities is True:
        meta[protocol.META_CAPABILITIES] = {}
    elif capabilities is not False:
        meta[protocol.META_CAPABILITIES] = capabilities
    sent = {'MCP-Protocol-Version': version if isinstance(version, str) else None, 'Mcp-Method': method}
    if method == 'tools/call':
        sent['Mcp-Name'] = (params or {}).get('name') if isinstance((params or {}).get('name'), str) else None
    sent.update(headers or {})
    return dict(message=req(method, {**(params or {}), '_meta': meta}, ident=ident),
                headers=[(k, v) for k, v in sent.items() if v is not None], flavour=flavour)


def mrun(name, result=None, fail=False, **kw):
    spec = mreq(**kw)
    run('on_' + name, spec['message'], spec['headers'], flavour=spec['flavour'], modern=True, result=result, fail=fail)


for method in ('server/discover', 'tools/list', 'resources/list', 'resources/templates/list', 'prompts/list'):
    mrun(f'ok_{method}', method=method)
    mrun(f'ok_{method}_str_id', method=method, ident='abc')
    mrun(f'stdio_{method}', method=method, flavour='stdio')
mrun('call_ok', method='tools/call', params={'name': 'list_photos', 'arguments': {}})
mrun('call_no_args', method='tools/call', params={'name': 'list_photos'})
mrun('call_unknown', method='tools/call', params={'name': 'zz'})
mrun('call_bad_args', method='tools/call', params={'name': 'list_photos', 'arguments': 'x'})
mrun('call_fail', method='tools/call', params={'name': 'list_photos', 'arguments': {}}, fail=True)
mrun('call_is_error', method='tools/call', params={'name': 'get_job', 'arguments': {'job': 'j1'}},
     result={'content': [{'type': 'text', 'text': 'bad 한'}], 'isError': True})
mrun('call_result_with_meta', method='tools/call', params={'name': 'list_photos'},
     result={'content': [], 'isError': False, '_meta': {'mine': 1}, 'resultType': 'partial'})
mrun('call_b64_name', method='tools/call', params={'name': 'list_photos', 'arguments': {}},
     headers={'Mcp-Name': B64('list_photos')})
mrun('call_b64_wrong', method='tools/call', params={'name': 'list_photos'}, headers={'Mcp-Name': B64('get_photo')})
mrun('call_name_mismatch', method='tools/call', params={'name': 'list_photos'}, headers={'Mcp-Name': 'get_photo'})
mrun('call_no_name_header', method='tools/call', params={'name': 'list_photos'}, headers={'Mcp-Name': None})
mrun('call_no_name_no_header', method='tools/call', params={}, headers={'Mcp-Name': None})
mrun('call_null_name_no_header', method='tools/call', params={'name': None}, headers={'Mcp-Name': None})
mrun('call_int_name_no_header', method='tools/call', params={'name': 7}, headers={'Mcp-Name': None})
mrun('call_bad_b64_name', method='tools/call', params={'name': 'list_photos'}, headers={'Mcp-Name': '=?base64?!!!?='})
mrun('call_unicode_b64', method='tools/call', params={'name': '한글'}, headers={'Mcp-Name': B64('한글')})
mrun('hdr_method_mismatch', method='tools/list', headers={'Mcp-Method': 'tools/call'})
mrun('hdr_method_missing', method='tools/list', headers={'Mcp-Method': None})
mrun('hdr_method_b64', method='tools/list', headers={'Mcp-Method': B64('tools/list')})
mrun('hdr_version_other_legacy', method='tools/list', headers={'MCP-Protocol-Version': '2025-06-18'})
mrun('hdr_version_missing', method='tools/list', headers={'MCP-Protocol-Version': None})
mrun('hdr_version_b64', method='tools/list', headers={'MCP-Protocol-Version': B64('2026-07-28')})
mrun('hdr_version_bad_b64', method='tools/list', headers={'MCP-Protocol-Version': '=?base64?%%%?='})
mrun('hdr_version_lower_names', method='tools/list',
     headers={'MCP-Protocol-Version': None, 'Mcp-Method': None, 'mcp-protocol-version': '2026-07-28', 'mcp-method': 'tools/list'})
mrun('version_unsupported', method='tools/list', version='2099-01-01')
mrun('version_legacy_in_meta', method='tools/list', version='2025-11-25')
mrun('version_empty', method='tools/list', version='')
mrun('version_int_no_header', method='tools/list', version=7)
mrun('version_null_no_header', method='tools/list', version=None)
mrun('version_list_no_header', method='tools/list', version=['2026-07-28'])
mrun('version_dict_with_header', method='tools/list', version={'a': 1}, headers={'MCP-Protocol-Version': '2026-07-28'})
mrun('version_unsupported_str_id', method='tools/list', version='2099-01-01', ident='ü')
mrun('caps_missing', method='tools/list', capabilities=False)
mrun('caps_list', method='tools/list', capabilities=[])
mrun('caps_null', method='tools/list', capabilities=None)
mrun('caps_filled', method='tools/list', capabilities={'sampling': {}, 'roots': {'listChanged': True}})
for method in ('subscriptions/listen', 'ping', 'logging/setLevel', 'completion/complete', 'notifications/initialized', 'initialize'):
    mrun(f'unknown_{method}', method=method)
# modern on, but the request does not carry the modern _meta: legacy rules, checked
for params in (None, {}, {'_meta': {}}, {'_meta': {'x': 1}}, {'_meta': [1]}, {'_meta': 'v'}, {'_meta': None},
               {'_meta': {protocol.META_CAPABILITIES: {}}}):
    run(f'on_legacy_meta_{J(params)}', req('tools/list', params), modern=True)
    run(f'on_legacy_meta_hdr_{J(params)}', req('tools/list', params), [('mcp-protocol-version', '2026-07-28')], modern=True)
run('on_legacy_initialize', req('initialize', {'protocolVersion': '2025-06-18'}), modern=True)
run('on_legacy_discover', req('server/discover'), modern=True)
run('on_legacy_call', req('tools/call', {'name': 'list_photos'}), [('mcp-protocol-version', '2025-06-18')], modern=True)
run('on_notification', {'jsonrpc': '2.0', 'method': 'notifications/initialized'}, modern=True)
run('on_invalid', {'jsonrpc': '2.0', 'id': 4}, modern=True)
run('on_params_list', req('tools/list', [1]), modern=True)
for method in ('server/discover', 'tools/list'):
    spec = mreq(method=method, flavour='stdio')
    run(f'on_stdio_ignores_modern_{method}', spec['message'], spec['headers'], flavour='stdio', modern=True)

with Case(MODULE, 'handle') as case:
    case.json(version=__version__, cases=CASES, blobs=BLOBS)
print('handle cases:', len(CASES))

# --- offline templates --------------------------------------------------------------------------

with Case(MODULE, 'offline') as case:
    case.json(version=__version__, templates=protocol.offline_templates(),
              order=list(protocol.offline_templates()),
              initialize_offline_bytes=D(protocol.result('__ID__', protocol.initialize_result('2025-03-26', offline=True))),
              initialize_online_bytes=D(protocol.result(1, protocol.initialize_result('2025-03-26'))),
              error_samples=[[J(ident), code, msg, J(data), D(protocol.error(ident, code, msg, data))]
                             for ident, code, msg, data in [(1, -32600, 'Invalid Request', None), ('a"é', -32022, 'x', {'supported': ['a'], 'requested': None}),
                                                           (None, -32603, 'Internal error', {}), (0, 1, '한', [])]],
              result_samples=[[J(ident), J(value), D(protocol.result(ident, value))]
                              for ident, value in [(1, {}), ('é', {'a': [1, 2.5, None, True]}), (None, [])]])

# --- header_value ---------------------------------------------------------------------------------

HEADERS = [None, '', 'abc', 'aGVsbG8=', '=?base64?aGVsbG8=?=', '=?base64?aGVsbG8?=', '=?base64?aGVsbG8==?=', '=?base64?aGVsbG8===?=',
           '=?base64?/w==?=', '=?base64?gA==?=', '=?base64?=', '=?base64??=', '=?base64?a?=', '=?base64?YQ?=', '=?base64?YQ=?=',
           '=?base64?YQ==?=', '=?base64?YR==?=', '=?base64?YQ==YQ==?=', '=?base64?=YQ==?=', '=?base64?Y=Q=?=',
           '=?base64?YWI=?=', '=?base64?YWJj?=', '=?base64?YWJjZA==?=', '=?base64?YWJjZA=?=', '=?base64?YWJjZA?=',
           '=?base64?aG Vs bG8=?=', '=?base64?aGVs\nbG8=?=', '=?base64?aGVs-bG8=?=', '=?base64?aGVs_bG8=?=',
           '=?BASE64?aGVsbG8=?=', '=?base64?aGVsbG8=', 'base64?aGVsbG8=?=', ' =?base64?aGVsbG8=?=', '=?base64?aGVsbG8=?= ',
           B64('한글 \U0001f600'), B64(''), B64('line\nbreak'), '=?base64?7+8=?=', '=?base64?////?=', '=?base64?w6k=?=',
           '=?base64?wA==?=', '=?base64?7Zi4?=', '=?base64?4pyT?=', '=?base64?7aCA?=', '=?base64?\u00e9?=', '=?base64?YQ==\n?=',
           '?=', '=?', '=?base64?', '=?base64?=?=', '=?base64?===?=']
rows = []
for value in HEADERS:
    rows.append([value, protocol.header_value(value)])
with Case(MODULE, 'header_value') as case:
    case.json(rows=rows)

# --- check ------------------------------------------------------------------------------------------

CHECKS = []
SEEN = {}


def check_case(schema, value, path='arguments', tool=None):
    try:
        mcp_tools.check(schema, value, path)
        error = None
    except ServiceError as caught:
        error = str(caught)
    # The mutation walkers produce thousands of near-duplicates: keep two per (tool, error message).
    if tool is not None:
        key = (tool, error)
        SEEN[key] = SEEN.get(key, 0) + 1
        if SEEN[key] > (2 if error else 3):
            return
    CHECKS.append({'tool': tool, 'schema_json': None if tool else J(schema), 'value_json': J(value), 'path': path, 'error': error})


BAD = [None, True, False, 0, 1, -1, 5, 6, 100, 2**53 - 1, 2**53, -2**53, 1.5, 0.0, -0.5, 1e300, 0.1, '', 'x', 'j1', 'x' * 81,
       'x' * 201, 'x' * 1025, 'é' * 80, '한' * 81, '\U0001f600' * 80, 'b0a1b2', 'selection', 'JPEG', 'none', 'red', [], [1], [1, 1],
       ['a'], [[1, 2]], [[1, 2], [1, 2]], [1, 'a'], list(range(60)), {}, {'a': 1}, {'ids': [1]}, {'job': 'j1'}]


def sample(schema):
    """A plausible valid-ish value of a schema (for mutation)."""
    kind = schema.get('type')
    if isinstance(kind, list):
        kind = kind[0]
    if 'enum' in schema:
        return schema['enum'][0]
    if kind == 'object':
        return {key: sample(sub) for key, sub in list(schema.get('properties', {}).items())[:0]}
    if kind == 'array':
        count = max(schema.get('minItems', 0), 1)
        return [sample(schema['items']) for _ in range(count)] if 'items' in schema else []
    if kind == 'integer':
        return max(schema.get('minimum', 0), 0) or 1
    if kind == 'number':
        return 0.5
    if kind == 'string':
        return 'a' * max(schema.get('minLength', 1), 1) if 'pattern' not in schema else ('b0a1b2' if 'b[' in schema['pattern'] else 'j1')
    if kind == 'boolean':
        return True
    return None


# Simple explicit mutation walker (clear and deterministic): for every property schema reachable through
# objects and arrays of objects, put each bad value in that spot of an otherwise valid skeleton.
def skeleton(schema, required_only=True):
    """A valid object for an object schema: required keys filled, nothing else."""
    out = {}
    for key in schema.get('required', ()):
        out[key] = sample(schema['properties'][key])
    for key in schema.get('required', ()):
        sub = schema['properties'][key]
        if sub.get('type') == 'object':
            out[key] = skeleton(sub)
    if 'minProperties' in schema and not out:
        key, sub = next(iter(schema['properties'].items()))
        out[key] = sample(sub)
    return out


def walk(tool, schema, container, trail, depth):
    """Yields (path-parts, bad values applied) by mutating each property of `container` according to `schema`."""
    for key, sub in schema.get('properties', {}).items():
        for bad in BAD + [sample(sub)]:
            root = copy.deepcopy(ROOT[tool])
            spot = root
            for part in trail:
                spot = spot[part]
            spot[key] = bad
            check_case(mcp_tools.SCHEMAS[tool], root, tool=tool)
        if depth < 3:
            if sub.get('type') == 'object' and sub.get('properties'):
                filled = skeleton(sub)
                if not filled:
                    k0, s0 = next(iter(sub['properties'].items()))
                    filled = {k0: sample(s0)}
                root = copy.deepcopy(ROOT[tool])
                spot = root
                for part in trail:
                    spot = spot[part]
                spot[key] = filled
                ROOT_STACK.append(ROOT[tool])
                ROOT[tool] = root
                walk(tool, sub, filled, trail + [key], depth + 1)
                ROOT[tool] = ROOT_STACK.pop()
            elif sub.get('type') == 'array' and isinstance(sub.get('items'), dict) and sub['items'].get('type') == 'object' \
                    and sub['items'].get('properties'):
                filled = skeleton(sub['items'])
                if not filled:
                    k0, s0 = next(iter(sub['items']['properties'].items()))
                    filled = {k0: sample(s0)}
                root = copy.deepcopy(ROOT[tool])
                spot = root
                for part in trail:
                    spot = spot[part]
                spot[key] = [filled]
                ROOT_STACK.append(ROOT[tool])
                ROOT[tool] = root
                walk(tool, sub['items'], filled, trail + [key, 0], depth + 1)
                ROOT[tool] = ROOT_STACK.pop()


ROOT = {}
ROOT_STACK = []
for tool_name, tool_schema in mcp_tools.SCHEMAS.items():
    ROOT[tool_name] = skeleton(tool_schema)
    check_case(tool_schema, ROOT[tool_name], tool=tool_name)
    check_case(tool_schema, {}, tool=tool_name)
    check_case(tool_schema, [], tool=tool_name)
    check_case(tool_schema, None, tool=tool_name)
    check_case(tool_schema, {'zzz': 1, **ROOT[tool_name]}, tool=tool_name)
    walk(tool_name, tool_schema, ROOT[tool_name], [], 0)

# explicit cases (the oracle's own tests and every error kind)
EDIT = mcp_tools.SCHEMAS['apply_edits']
for arguments in [
        {'edits': [{'ids': [1], 'set': {'exposure': True}}]}, {'edits': [{'ids': [True], 'set': {'exposure': 1}}]},
        {'edits': [{'ids': [2**53], 'set': {'exposure': 1}}]}, {'edits': [{'ids': [1, 1], 'set': {'exposure': 1}}]},
        {'edits': [{'ids': [1, 1.0], 'set': {'exposure': 1}}]}, {'edits': [{'ids': [1, 2, 3], 'set': {'exposure': 1}}]},
        {'edits': [{'scope': 'all', 'set': {'exposure': 1}}]}, {'edits': [{'ids': [1], 'set': {'path': 1}}]},
        {'edits': [{'ids': [1], 'set': {}}]}, {'edits': []}, {}, {'edits': [{'ids': [1], 'set': {'exposure': 1}}], 'extra': 1},
        {'edits': [{'ids': [1], 'set': {'rotation': 45}}]},
        {'edits': [{'ids': [1], 'set': {'rotation': 90.0}}]}, {'edits': [{'ids': [1], 'set': {'rotation': 90}}]},
        {'edits': [{'ids': [1], 'set': {'film_look': 5}}]}, {'edits': [{'ids': [1], 'set': {'flip': 1}}]},
        {'edits': [{'ids': [1], 'set': {'curve': [[0, 0]]}}]}, {'edits': [{'ids': [1], 'set': {'curve': [[0, 0], [1, 2]]}}]},
        {'edits': [{'ids': [1], 'set': {'curve': [[0, 0], [1, 1, 1]]}}]}, {'edits': [{'ids': [1], 'set': {'curve': [[0, 0]] * 17}}]},
        {'edits': [{'ids': [1], 'set': {'rgb_curves': [[[0, 0], [1, 1]]] * 2}}]},
        {'edits': [{'ids': [1], 'set': {'rgb_curves': [[[0, 0], [1, 1]]] * 3}}]},
        {'edits': [{'ids': [1], 'crop': None}]}, {'edits': [{'ids': [1], 'crop': [0, 0, 1]}]}, {'edits': [{'ids': [1], 'crop': 'x'}]},
        {'edits': [{'ids': [1], 'crop': [0, 0, 1, 1.5]}]}, {'edits': [{'ids': [1], 'crop': [0, 0, 1, 1]}]},
        {'edits': [{'ids': [1], 'groups': ['light', 'light']}]}, {'edits': [{'ids': [1], 'groups': ['nope']}]},
        {'edits': [{'ids': [1], 'groups': []}]}, {'edits': [{'ids': [1], 'copy_from': 0}]},
        {'edits': [{'where': {}}]}, {'edits': [{'where': {'rating_min': 2.5}}]}, {'edits': [{'where': {'zzz': 1}}]},
        {'edits': [{'where': {'rating_min': 0}}]}, {'edits': [{'where': {'label': 'pink'}}]}, {'edits': [{'where': {'text': ''}}]},
        {'edits': [{'ids': [1], 'add': {'exposure': 'a'}}]}, {'edits': [{'ids': [1], 'add': {'monochrome': 1}}]},
        {'edits': [{'ids': [1], 'add': {'contrast': 500}}]}, {'edits': ['x']}, {'edits': [[]]}, {'edits': {}}, {'edits': 'x'},
        {'edits': [{'ids': list(range(1, 502))}]}, {'edits': [{} for _ in range(101)]}, {'edits': [{'ids': [0]}]},
        {'edits': [{'ids': [1], 'preset': ''}]}, {'edits': [{'ids': [1], 'preset': 'x' * 201}]}]:
    check_case(EDIT, arguments, tool='apply_edits')
for arguments in [{'job': 'x1'}, {'job': 'j1'}, {'job': 'j1\n'}, {'job': 'j1\n\n'}, {'job': 'j'}, {'job': 'j1 '}, {'job': 'J1'},
                  {'job': 'j١'}, {'job': ''}, {'job': 'j' + '9' * 40}, {'job': 'j1', 'wait': 41}, {'job': 'j1', 'wait': -1},
                  {'job': 'j1', 'wait': 40}, {'job': 'j1', 'wait': 2.0}, {'job': 'j1', 'wait': '5'}, {'job': 5}, {'wait': 5}]:
    check_case(mcp_tools.SCHEMAS['get_job'], arguments, tool='get_job')
for arguments in [{'batch': 'b0a1b2'}, {'batch': 'b0a1b2\n'}, {'batch': 'b0a1b'}, {'batch': 'b0a1b2c'}, {'batch': 'B0A1B2'},
                  {'batch': 'b0A1b2'}, {'batch': 'bzzzzzz'}, {'batch': 'xb0a1b2'}, {'redo': 1}, {'redo': True}, {'batch': None}]:
    check_case(mcp_tools.SCHEMAS['undo_last_batch'], arguments, tool='undo_last_batch')
for arguments in [{'ids': [1]}, {'scope': 'current'}, {'ids': list(range(1, 51))}, {'ids': list(range(1, 52))},
                  {'ids': [1.0]}, {'ids': [[1]]}, {'ids': 'x'}, {'ids': [-1]}, {'ids': [2**53 - 1]}, {'ids': [2**53]}]:
    check_case(mcp_tools.SCHEMAS['get_photo'], arguments, tool='get_photo')
for arguments in [{'where': {'rating_min': 2.5}}, {'where': {'collection': 2**53}}, {'where': {'folder': 'x' * 1025}},
                  {'where': {'recursive': 'yes'}}, {'where': {'rating_max': 6}}, {'scope': 'selection'}, {'scope': 'all'}]:
    check_case(mcp_tools.SCHEMAS['auto_tone'], arguments, tool='auto_tone')
for arguments in [{'ids': [1], 'size': 127}, {'ids': [1], 'size': 1569}, {'ids': [1], 'size': 512, 'quality': 39},
                  {'ids': [1], 'size': 512, 'quality': 90}, {'ids': [1, 2, 3, 4, 5]}, {'ids': [1], 'quality': 90.5}]:
    check_case(mcp_tools.SCHEMAS['get_preview'], arguments, tool='get_preview')
for arguments in [{'ids': [1], 'variants': [{'film_look': 'x'}]}, {'ids': [1], 'variants': [{'zzz': 1}]}, {'ids': [1], 'variants': []},
                  {'ids': [1], 'variants': [{}] * 13}, {'ids': [1], 'variants': [{'label': 'x' * 81}]}, {'ids': [1], 'save': 'x'},
                  {'ids': [1], 'size': 511}, {'ids': [1], 'cell': 95}, {'ids': [1], 'cell': 256}, {'ids': [1], 'count': 37},
                  {'ids': [1], 'destination': 'nope'}, {'ids': [1], 'destination': 'path', 'path': '/x', 'subfolder': 'y'},
                  {'ids': [1], 'subfolder': 'y' * 81}]:
    check_case(mcp_tools.SCHEMAS['contact_sheet'], arguments, tool='contact_sheet')
for arguments in [{'ids': [1], 'format': 'GIF'}, {'ids': [1], 'format': 'JPEG XL'}, {'ids': [1], 'quality': 39}, {'ids': [1], 'longest': -1},
                  {'ids': [1], 'longest': 30001}, {'ids': [1], 'width': 0}, {'ids': [1], 'fit': 'fill'}, {'ids': [1], 'color_space': 'XYZ'},
                  {'ids': [1], 'naming': ''}, {'ids': [1], 'naming': '{stem}'}, {'ids': [1], 'dry_run': 1}, {'ids': [1], 'keep_metadata': None}]:
    check_case(mcp_tools.SCHEMAS['export_photos'], arguments, tool='export_photos')
for arguments in [{'id': 1, 'count': 20}, {'id': 1, 'count': 21}, {'id': 1, 'count': 0}, {'id': 1}, {'count': 1}, {'id': 2**53, 'count': 1},
                  {'id': '1', 'count': 1}]:
    check_case(mcp_tools.SCHEMAS['create_virtual_copies'], arguments, tool='create_virtual_copies')
for arguments in [{'ids': [1], 'rating': 6}, {'ids': [1], 'rating': -1}, {'ids': [1], 'flag': 'x'}, {'ids': [1], 'label': 'none'},
                  {'ids': [1], 'label': 'pink'}, {'entries': []}, {'entries': [{'ids': [1], 'rating': 3}]}, {'entries': [{'zzz': 1}]},
                  {'entries': [{'ids': [1], 'label': 'x'}]}, {'entries': ['x']}, {'entries': [{} for _ in range(101)]}]:
    check_case(mcp_tools.SCHEMAS['set_labels'], arguments, tool='set_labels')
for arguments in [{'action': 'save'}, {'action': 'x'}, {'action': 'list', 'name': ''}, {'action': 'save', 'name': 'x' * 81}, {}, {'name': 'a'}]:
    check_case(mcp_tools.SCHEMAS['snapshot'], arguments, tool='snapshot')
for arguments in [{'id': 1}, {'id': 1, 'view': 'x'}, {'id': 1, 'view': 'edit'}, {'view': 'edit'}]:
    check_case(mcp_tools.SCHEMAS['open_photo'], arguments, tool='open_photo')
for arguments in [{'type': 'presets'}, {'type': 'cine'}, {'type': 'x'}, {'type': 1}, {'type': None}]:
    check_case(mcp_tools.SCHEMAS['list_films'], arguments, tool='list_films')
for arguments in [{'path': '/x'}, {'path': ''}, {'path': 'x' * 1025}, {'path': '/x', 'recursive': 'a'}, {}]:
    check_case(mcp_tools.SCHEMAS['import_folder'], arguments, tool='import_folder')
for arguments in [{'folder': 'a', 'text': 'b', 'rating_min': 1, 'rating_max': 5, 'flag': 'picked', 'label': 'red', 'collection': 3,
                   'scope': 'selection', 'group': 'folders', 'sort': 'name', 'descending': True, 'offset': 0, 'limit': 200},
                  {'limit': 201}, {'limit': 0}, {'offset': -1}, {'sort': 'size'}, {'group': 'x'}, {'rating_min': 0}, {'rating_min': 6},
                  {'flag': 'Picked'}, {'collection': 0}, {'collection': 2**53 - 1}, {'collection': 2**53}, {'text': 'é' * 200},
                  {'text': 'é' * 201}, {'descending': 'true'}]:
    check_case(mcp_tools.SCHEMAS['list_photos'], arguments, tool='list_photos')
for arguments in [{'strength': 101}, {'strength': -1}, {'strength': 50.5}, {'strength': 100, 'ids': [1]}, {'roll': 1}]:
    check_case(mcp_tools.SCHEMAS['auto_color'], arguments, tool='auto_color')
    check_case(mcp_tools.SCHEMAS['invert_negatives'], arguments, tool='invert_negatives')
# paths other than 'arguments'
check_case(mcp_tools.SCHEMAS['get_job'], {'job': 'x'}, path='args', tool='get_job')
check_case(mcp_tools.SCHEMAS['get_job'], {}, path='', tool='get_job')
check_case(mcp_tools.SCHEMAS['get_job'], [], path='a.b[3]', tool='get_job')

# inline schemas: every keyword of the subset, with floats, bools, lists of types and unusual values
INLINE = [
    ({'type': 'integer'}, [1, True, 1.0, 1.5, '1', None, [], {}]), ({'type': 'number'}, [1, 1.5, True, '1', None, 1e300, -0.0]),
    ({'type': 'string'}, ['', 'a', 1, None]), ({'type': 'boolean'}, [True, 0, 'true']), ({'type': 'object'}, [{}, [], None]),
    ({'type': 'array'}, [[], {}, 'a']), ({'type': 'null'}, [None, 0, '']), ({'type': ['array', 'null']}, [None, [], 1, 'x', {}]),
    ({'type': ['string', 'integer']}, [1, 'a', 1.5, None]), ({'type': ['object', 'null', 'number']}, [None, {}, 'x']),
    ({'type': ['integer', 'null']}, ['x']), ({'type': ['object', 'array']}, ['x']),
    ({'type': 'integer', 'enum': [0, 90, 180, 270]}, [0, 90, 91, 90.0, True, -90, 1, 270]),
    ({'type': 'string', 'enum': ['a', 'b', 'c']}, ['a', 'd', '', 'A']), ({'enum': [1, 2.5, True, None, 'x']}, [1, 2.5, True, None, 'x', 3, False, 0]),
    ({'enum': [1, 'a']}, [True, 1.0, 2, 'b']), ({'enum': [1.5, 2]}, [3]),
    ({'type': 'integer', 'minimum': 5}, [4, 5, 6, 4.9]), ({'type': 'number', 'minimum': 0.5}, [0.4, 0.5, 1]),
    ({'type': 'number', 'minimum': 1.5}, [1]), ({'type': 'number', 'maximum': 2.25}, [3, 2.25]),
    ({'type': 'number', 'maximum': 1e-5}, [1]), ({'type': 'number', 'maximum': 1e7}, [1e8]),
    ({'type': 'number', 'minimum': 123456789.5}, [1]), ({'type': 'number', 'maximum': 0.000123456789}, [1]),
    ({'type': 'number', 'maximum': 100.0}, [101]), ({'type': 'number', 'minimum': -2.5e-7}, [-1]),
    ({'type': 'number', 'minimum': -100}, [-101, -100]), ({'type': 'number', 'maximum': 1e16}, [1e17]),
    ({'type': 'integer', 'maximum': 2**53 - 1}, [2**53, 2**53 - 1]), ({'type': 'integer', 'minimum': -2**53}, [-2**53 - 1]),
    ({'type': 'string', 'minLength': 2}, ['', 'a', 'ab', '한']), ({'type': 'string', 'maxLength': 2}, ['abc', 'ab', '한한한', '\U0001f600\U0001f600\U0001f600']),
    ({'type': 'string', 'pattern': '^[a-z]+$'}, ['abc', 'abc\n', 'abc\nd', 'ABC', '', 'abc ']),
    ({'type': 'string', 'pattern': '^b[0-9a-f]{6}$'}, ['b0a1b2', 'b0a1b2\n', 'b0a1b', 'b0a1b2c']),
    ({'type': 'string', 'pattern': '[0-9]+'}, ['abc', 'a1', '']), ({'type': 'string', 'pattern': '^j[0-9]+$'}, ['j', 'j1', 'j1x']),
    ({'type': 'string', 'pattern': '^a?b*c$'}, ['c', 'abbc', 'aabc', 'bc', 'abc\n']),
    ({'type': 'array', 'minItems': 2}, [[], [1], [1, 2]]), ({'type': 'array', 'maxItems': 2}, [[1, 2, 3], [1, 2]]),
    ({'type': 'array', 'uniqueItems': True}, [[1, 1], [1, 1.0], [1, True], [True, True], [None, None], ['a', 'a'], [[1], [1]], [{'a': 1}, {'a': 1}],
        [{'a': 1, 'b': 2}, {'b': 2, 'a': 1}], [1.0, 1.0], [0.1, 0.1], [1e16, 1e16], [1.5, 1.50], ['a', 'b'], [[1], [2]], [0, False],
        ['é', 'é'], ["it's", "it's"], ['a"b', 'a"b'], ["'\"", "'\""], ['\x00', '\x00'], [[], []], [{}, {}], [-0.0, 0.0], [1e-7, 1e-7]]),
    ({'type': 'array', 'uniqueItems': False}, [[1, 1]]), ({'type': 'array', 'items': {'type': 'integer'}}, [[1, 2], [1, 'a'], [[1]], ['x', 2]]),
    ({'type': 'array', 'items': {'type': 'array', 'items': {'type': 'number', 'maximum': 1}}}, [[[0.5], [2]], [[1], 3]]),
    ({'type': 'object', 'properties': {'a': {'type': 'integer'}}}, [{'a': 1}, {'a': 'x'}, {'b': 1}, {}]),
    ({'type': 'object', 'required': ['a', 'b'], 'properties': {'a': {}, 'b': {}}}, [{}, {'a': 1}, {'b': 1}, {'a': 1, 'b': 2}]),
    ({'type': 'object', 'additionalProperties': False, 'properties': {'a': {}}}, [{'a': 1}, {'b': 1}, {'b': 1, 'c': 2}, {'a': 1, 'z': 1}, {'z': 1, 'a': 1}]),
    ({'type': 'object', 'additionalProperties': True, 'properties': {'a': {}}}, [{'b': 1}]),
    ({'type': 'object', 'minProperties': 2}, [{}, {'a': 1}, {'a': 1, 'b': 2}]),
    ({'type': 'object', 'required': ['x'], 'additionalProperties': False, 'minProperties': 2, 'properties': {'x': {'type': 'integer'}}},
        [{}, {'y': 1}, {'x': 'a', 'y': 1}, {'x': 1}, {'x': 'a'}, {'x': 1, 'y': 2}]),
    ({'type': 'object', 'properties': {'a': {'type': 'object', 'properties': {'b': {'type': 'array', 'items': {'type': 'integer', 'minimum': 1}}}}}},
        [{'a': {'b': [1, 0]}}, {'a': {'b': [1, 'x']}}, {'a': {'b': 5}}, {'a': 1}, {'a': {'b': [2, 3]}}]),
    ({}, [1, 'a', None, [], {}]), ({'description': 'x'}, [1]),
    ({'type': 'integer', 'minimum': 1, 'maximum': 5, 'enum': [1, 2, 9]}, [9, 0, 3, 2]),
    ({'type': 'unknown'}, [1, None]), ({'type': ['unknown', 'integer']}, [1, 'a']),
]
for schema, values in INLINE:
    for value in values:
        check_case(schema, value, 'v')
        check_case(schema, value)
for value in ({'a': [1, {'b': 'ü'}], 'c': None}, [1, 2.5, 'x', None, True, False, {}, []]):
    check_case({'type': 'object', 'additionalProperties': False, 'properties': {}}, value, 'arguments.deep[2].x')

with Case(MODULE, 'check') as case:
    case.json(cases=CHECKS)
print('check cases:', len(CHECKS))

# --- helpers: show, text_block, image_block, failure ---------------------------------------------------

SHOWN = [0, 1, -5, 2**53 - 1, 1.5, 0.5, 100.0, 1e-5, 1e16, 123456.789, 1234567.0, 0.0001, 0.00001234, -0.0, 1e100, 12.0, 3.14159265,
         0.1, 1e5, 999999.5, 99999.95, 2.5e-7, 1e21, 5e-324, 1.7976931348623157e308, 100000.0, 123456.0, 1234567.8, 0.000123456789, 2.0, -45.5]
shown = [[J(v), mcp_tools._show(v), f'{v:g}'] for v in SHOWN]
BLOCKS = [{'a': 1, 'b': ['한글', 'é', '\u00a0', '\U0001f600', '"q"', '\\', '\n', '\x7f', '\x1f'], 'c': 1.0, 'd': None, 'e': 1e16, 'z': {'y': 1, 'x': 2}},
          [], {}, 'plain', 5, None, True, 0.1, [1.0, 2.5e-7]]
with Case(MODULE, 'helpers') as case:
    case.json(shown=shown,
              text_blocks=[[J(v), D(mcp_tools.text_block(v))] for v in BLOCKS],
              text_blocks_raw=[[J(v), mcp_tools.text_block(v)['text']] for v in BLOCKS],
              image_blocks=[[list(data), D(mcp_tools.image_block(data))] for data in
                            (b'', b'a', b'ab', b'abc', b'abcd', bytes(range(256)), b'\xff\xd8\xff\xe0' * 5)],
              failures=[[t, D(mcp_tools.failure(t))] for t in ('x', '', 'Grainy is busy: "x"', '한글')])

# --- repr / str / dumps of values ------------------------------------------------------------------------

VALUES = [None, True, False, 0, -1, 7, 2**53, 1.5, 1.0, -0.0, 0.1, 1e16, 1e15, 1e-5, 0.0001, 0.00001, 123456789.123456789, 5e-324,
          1.7976931348623157e308, 1e22, 1e21, 123456789012345678.0, 0.30000000000000004, 2.5e-7, 100.0, 12345.6789, '', 'x', "it's", 'say "x"',
          '\'"', 'a\\b', 'tab\tx', 'nl\nx', 'cr\rx', '\x00\x01\x1f\x7f\x80\x9f\xa0\xad', '\u200b\u2028\u2029\u3000\ue000\ufeff', 'é한\U0001f600',
          '\U000e0001\U0010ffff\U0001f4a9', '\u0600\u061c\u180e\u2060', '\ud7ff\uf8ff\ufffe\uffff', [], [1, 'a', None, True, 2.5, [], {}],
          {}, {'a': 1, 'b': [1, {'c': "d'e"}]}, {'z': 1, 'a': 2, 'm': {'y': 1, 'b': 2}}, {"it's": 'x', 'say "x"': 1}, {'한': 'é'},
          [[1, 2], [3, [4, [5]]]]]
rows = []
for v in VALUES:
    rows.append([J(v), repr(v), str(v), D(v), type(v).__name__])
with Case(MODULE, 'pyjson') as case:
    case.json(rows=rows)

# --- call wrapper (Tools.call in the oracle) ---------------------------------------------------------------


class Job:
    def __init__(self, ident, state, error, view):
        self.id, self.state, self.error, self._view = ident, state, error, view

    def view(self):
        return self._view


class Stub:
    def note(self, name, text):
        pass


class Scenario(mcp_tools.Tools):
    """Tools.call of the oracle with the tool_<name> implementation replaced by the scenario's action."""


def call_case(name, arguments, action, outcome, extra=None):
    ran = []

    def wrapped():
        ran.append(1)
        return action()

    setattr(Scenario, 'tool_' + name, lambda self, arguments, deadline: wrapped())
    result = Scenario(Stub()).call(name, arguments, modern=False, deadline=time.monotonic() + 45)
    CALLS.append({'tool': name, 'arguments_json': J(arguments), 'outcome': outcome, 'extra_json': J(extra or {}),
                  'ran': bool(ran), 'result_json': J(result), 'result_bytes': D(result)})


CALLS = []
OK = [{'type': 'text', 'text': '{"ok":1}'}, {'type': 'image', 'data': 'AA==', 'mimeType': 'image/jpeg'}]


def ret(value):
    return lambda: value


def boom(error):
    def action():
        raise error
    return action


call_case('get_job', {'job': 'j1'}, ret(OK), 'ok', {'content': OK})
call_case('list_photos', {}, ret([]), 'ok', {'content': []})
call_case('get_job', {'job': 'j1'}, boom(ServiceError('no such job: j1')), 'service', {'text': 'no such job: j1'})
call_case('get_job', {'job': 'j1'}, boom(ServiceError('한글 "q"')), 'service', {'text': '한글 "q"'})
call_case('get_job', {'job': 'j1'}, boom(Busy('the photo list is still loading')), 'busy', {'text': 'the photo list is still loading'})
call_case('get_job', {'job': 'j1'}, boom(Unfinished()), 'unfinished')
job = Job('j7', 'failed', 'disk full', {'id': 'j7', 'state': 'failed', 'done': 2, 'total': 5, 'note': '한글'})
call_case('export_photos', {'ids': [1]}, boom(mcp_tools.JobFailure(job)), 'job',
          {'message': 'job j7 failed: disk full', 'view': {'id': 'j7', 'state': 'failed', 'done': 2, 'total': 5, 'note': '한글'}})
job = Job('j8', 'cancelled', '', {'id': 'j8', 'state': 'cancelled'})
call_case('export_photos', {'ids': [1]}, boom(mcp_tools.JobFailure(job)), 'job',
          {'message': 'job j8 cancelled: no reason given', 'view': {'id': 'j8', 'state': 'cancelled'}})
for error, kind in [(PermissionError('x'), 'PermissionDenied'), (FileNotFoundError('x'), 'NotFound'), (FileExistsError('x'), 'AlreadyExists'),
                    (OSError('x'), 'Io'), (RuntimeError('x'), 'Named:RuntimeError'), (KeyError('x'), 'Named:KeyError')]:
    call_case('auto_tone', {'ids': [1]}, boom(error), 'other', {'last': photo_service.english_error(error), 'kind': kind})
for arguments in [{}, {'job': 'x'}, {'job': 'j1', 'extra': 1}, {'job': 5}]:
    call_case('get_job', arguments, ret(OK), 'invalid')
call_case('apply_edits', {'edits': [{'ids': [1, 1], 'set': {'exposure': 1}}]}, ret(OK), 'invalid')
call_case('apply_edits', {'edits': [{'ids': [1], 'set': {'exposure': 1}}]}, ret(OK), 'ok', {'content': OK})
english = [[kind, photo_service.english_error(err)] for kind, err in
           [('PermissionDenied', PermissionError()), ('NotFound', FileNotFoundError()), ('AlreadyExists', FileExistsError()),
            ('Io', OSError()), ('Named:ValueError', ValueError()), ('Named:ZeroDivisionError', ZeroDivisionError())]]
english.append(['Internal', photo_service.english_error('not an exception')])
with Case(MODULE, 'call') as case:
    case.json(cases=CALLS, english=english)
print('call cases:', len(CALLS))


# =================================================================================================
# Run 2: the tools working. Scripted sessions against the oracle's own Tools and McpService on temporary
# libraries made with the oracle's Catalog, with a fake window (FakeWindow) that the Rust headless host mirrors
# method for method. Clocks, batch ids and the local time are fixed; spawned work and timers wait in a queue that
# the script runs explicitly (op run_jobs), so job states over time are deterministic.
#
# golden/mcp/sessions/<name>/case.json: every step records the call (arguments with <ROOT>), isError, the text
# blocks (exactly, paths as <ROOT>), the images (JPEG byte length and size; decoded pixels as .npy where kept), the
# pixels handed to photo_images.jpeg (raw .npy: the exact comparison target), the job views, the fake window's UI
# events, the files written under Pictures/ and out/, and the catalogue tables that changed (as gen_service.py).
# =================================================================================================

import hashlib  # noqa: E402
import io as _io  # noqa: E402
import os  # noqa: E402
import shutil  # noqa: E402
import tempfile  # noqa: E402
from copy import deepcopy  # noqa: E402
from datetime import datetime  # noqa: E402
from pathlib import Path  # noqa: E402
from types import SimpleNamespace  # noqa: E402
from unittest.mock import patch  # noqa: E402

import numpy as np  # noqa: E402
from PIL import Image, ImageFont  # noqa: E402

from common import GOLDEN, _plain  # noqa: E402
from luma import (mcp_service, photo_images, photo_labels, command_batch, catalog as catalog_module,  # noqa: E402
                  library_query, workflow_data, engine, i18n, film_look, quick_export)
from luma.catalog import Catalog  # noqa: E402

i18n.set_language('ko')
STAMP = '2026-11-13T01:02:03.456789+00:00'
LOCAL = '2026-11-13 10:02'
PLACE = '<ROOT>'
LONG = 400
SESSIONS = GOLDEN / MODULE / 'sessions'
FILES = GOLDEN / MODULE / '_files'
SEP = os.sep


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


class Tokens:
    """secrets.token_hex(3) stand-in: 000001, 000002, ..."""
    def __init__(self):
        self.used = 0

    def token_hex(self, n):
        assert n == 3
        self.used += 1
        return f'{self.used:06x}'


class LocalTime:
    """The time module with a fixed local clock for strftime (snapshot names, activity lines)."""
    def __getattr__(self, name):
        return getattr(time, name)

    def strftime(self, fmt, *args):
        return {'%Y-%m-%d %H:%M': LOCAL, '%H:%M:%S': '10:02:03'}[fmt]


QUEUE = []


class FakeTimer:
    """QTimer: timers wait in QUEUE until run_queue."""
    def __init__(self, parent=None):
        self.callbacks = []
        self.active = False

    @property
    def timeout(self):
        return SimpleNamespace(connect=self.callbacks.append)

    def setInterval(self, ms):
        pass

    def start(self):
        self.active = True
        QUEUE.append(('repeat', self))

    def stop(self):
        self.active = False

    @staticmethod
    def singleShot(ms, context, fn):
        QUEUE.append(('once', fn))


class FakeWindow:
    """What mcp_tools and mcp_service use of MainWindow, headless (grainy_mcp::HeadlessHost mirrors it)."""
    def __init__(self, catalog, service):
        self.catalog = catalog
        self.closing = False
        self.command_running = False
        self.maintenance_running = False
        self.import_busy = False
        self.import_scans = []
        self.export_running = False
        self.current_id = None
        self.settings = None
        self.last_saved = None
        self.undo_stack = []
        self.redo_stack = []
        self.library_loading = False
        self.selection = []
        self.listed = None
        self.adjusting = False
        self.nested = False
        self.loop_level = 1
        self.inline = False
        self.mode = 0
        self.studio = SimpleNamespace(pending_stroke=None, healing_strokes=[])
        self.view = SimpleNamespace(crop_mode=False, sample_mode=False)
        self.events = []
        self.import_quiet = set()
        self.import_pending_paths = set()
        self.import_completion_errors = {}
        self.mcp = SimpleNamespace(service=service, pool=None, update=lambda: None)
        self.photo_selection = SimpleNamespace(blockSignals=lambda value: None)

    @property
    def all_presets(self):
        built = {name: engine.normalized({'tone_version': engine.defaults()['tone_version'], **values})
                 for name, values in engine.PRESETS.items()}
        return {**built, **self.catalog.presets()}

    @property
    def photo_model(self):
        ids = self.listed if self.listed is not None else [r[0] for r in self.catalog.db.execute('SELECT id FROM photos')]
        return SimpleNamespace(positions=set(ids))

    def interacting(self):
        return self.adjusting

    def selected_ids(self):
        if self.library_loading:
            return []
        result = list(self.selection)
        return result or ([self.current_id] if self.current_id in self.photo_model.positions else [])

    def commit(self, label='보정 변경'):
        if self.current_id and self.settings != self.last_saved:
            self.undo_stack.append(deepcopy(self.last_saved))
            self.redo_stack.clear()
            self.catalog.edit(self.current_id, self.settings, label)
            self.last_saved = deepcopy(self.settings)
            self.catalog.save_preference(f'undo:{self.current_id}', {'undo': self.undo_stack[-80:], 'redo': self.redo_stack[-80:]})
            self.events.append(['commit', self.current_id, label])

    def save_keywords(self):
        pass

    def load_state(self):
        self.settings = deepcopy(self.catalog.photo(self.current_id)['settings'])
        self.last_saved = deepcopy(self.settings)
        state = self.catalog.preference(f'undo:{self.current_id}', {})
        self.undo_stack = [engine.normalized(s) for s in state.get('undo', [])]
        self.redo_stack = [engine.normalized(s) for s in state.get('redo', [])]

    def refresh_command_edits(self, ids, label=None):
        self.events.append(['refresh_command_edits', list(ids), label])
        if self.current_id in ids:
            self.load_state()

    def refresh_lists(self):
        self.events.append(['refresh_lists'])

    def load_controls(self):
        self.events.append(['load_controls'])

    def activate(self, ident, force=False):
        if ident == self.current_id and not force:
            return
        self.commit()
        self.save_keywords()
        self.current_id = ident
        self.load_state()
        self.events.append(['activate', ident])

    def set_mode(self, mode):
        self.mode = mode
        self.events.append(['set_mode', mode])

    def active_list(self):
        def restore(ids, current):
            self.selection = list(ids)
            self.events.append(['restore_selection', list(ids), current])
        return SimpleNamespace(restore_selection=restore)

    def spawn(self, work, done, failed=None, pool=None):
        if self.inline:
            try:
                value = work()
            except Exception as error:  # noqa: BLE001
                failed(error)
            else:
                done(value)
        else:
            QUEUE.append(('spawn', work, done, failed))

    def start_export(self, ids, folder, format='JPEG', quality=95, longest=0, color_space='sRGB', keep_metadata=False,
                     naming='{stem}_grainy', grayscale=True, dither=True, *, remember=True, on_done=None, on_error=None,
                     cancel=None, progress=None, protocol=False):
        if self.export_running or not ids:
            return False
        self.commit()
        records = [self.catalog.photo(i) for i in ids]
        folder = Path(folder)
        self.export_running = True
        self.events.append(['export_started', len(records)])

        def work():
            return photo_service.export_records(records, folder, format, quality, longest, color_space, keep_metadata, naming,
                                                grayscale, dither, cancel=cancel, progress=progress, english=protocol)

        def finished(result):
            self.export_running = False
            if result['outputs']:
                workflow_data.record_exports(self.catalog, [{k: o[k] for k in ('path', 'photo_id', 'edit_signature')}
                                                            for o in result['outputs']])
            on_done(result)

        def failed(error):
            self.export_running = False
            on_error(error)
        self.spawn(work, finished, failed)
        return True

    def queue_import_files(self, files):
        """The import queue: adds the files that load (engine.load_image at 64 px) when its turn in QUEUE comes."""
        from luma.folders import path_key
        keys = [path_key(f) for f in files]
        self.import_pending_paths |= set(keys)
        self.events.append(['queue_import_files', len(files)])

        def work():
            for file, key in zip(files, keys):
                try:
                    engine.load_image(file, 64)
                except Exception:  # noqa: BLE001
                    pass
                else:
                    self.catalog.add(file, resolved=True)
                self.import_pending_paths.discard(key)
        QUEUE.append(('import', work))

    def cancel_import(self):
        self.events.append(['cancel_import'])


class FakeGui:
    def __init__(self):
        self.closed = False

    def call(self, fn, deadline):
        if self.closed:
            raise photo_service.Busy(mcp_service.CLOSED)
        return fn()


def run_queue(limit=None):
    """Run queued work and timers in order: at most `limit` items, or until the queue is empty (at most 100)."""
    count = 0
    while QUEUE and count < (100 if limit is None else limit):
        item = QUEUE.pop(0)
        count += 1
        if item[0] == 'spawn':
            _, work, done, failed = item
            try:
                value = work()
            except Exception as error:  # noqa: BLE001
                failed(error)
            else:
                done(value)
        elif item[0] in ('once', 'import'):
            item[1]()
        elif item[0] == 'repeat':
            timer = item[1]
            if timer.active:
                for callback in timer.callbacks:
                    callback()
                if timer.active:
                    QUEUE.append(('repeat', timer))
    return count


class SessionScrub:
    def __init__(self, root):
        r = str(root)
        self.forms = []
        for form in (json.dumps(r, ensure_ascii=False)[1:-1], r, json.dumps(r.lower(), ensure_ascii=False)[1:-1], r.lower()):
            if form not in self.forms:
                self.forms.append(form)

    def text(self, s):
        for form in self.forms:
            s = s.replace(form, PLACE)
        return s

    def __call__(self, value):
        value = _plain(value)
        if isinstance(value, dict):
            return {self.text(str(k)): self(v) for k, v in value.items()}
        if isinstance(value, list):
            return [self(v) for v in value]
        if isinstance(value, str):
            return self.text(value)
        return value


def unplace(value, root):
    if isinstance(value, dict):
        return {k: unplace(v, root) for k, v in value.items()}
    if isinstance(value, list):
        return [unplace(v, root) for v in value]
    if isinstance(value, str):
        return value.replace(PLACE, str(root))
    return value


def dump(db, scrub):
    result = {}
    tables = [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    for table in tables:
        if table == 'preview_state':
            continue
        columns = [r[1] for r in db.execute('PRAGMA table_info(' + table + ')')]
        rows = []
        for row in db.execute('SELECT * FROM ' + table + ' ORDER BY ' + ','.join('"' + c + '"' for c in columns)):
            item = {}
            for c, v in zip(columns, row):
                if isinstance(v, bytes):
                    v = {'blob': hashlib.sha256(v).hexdigest(), 'len': len(v)}
                elif isinstance(v, str):
                    v = scrub.text(v)
                    if len(v) > LONG:
                        v = {'sha': hashlib.sha256(v.encode('utf-8')).hexdigest(), 'len': len(v)}
                item[c] = v
            rows.append(item)
        result[table] = rows
    return result


def write_text_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, separators=(',', ':')) + '\n', encoding='utf-8', newline='\n')


# --- input files: written once into golden/mcp/_files; the generator and the Rust test copy them -----------------
INPUTS = {name: np.load(GOLDEN / '_inputs' / f'{name}.npy') for name in ('garden', 'patches', 'colours', 'ramp')}


def u8(a):
    return np.uint8(np.clip(a, 0, 1) * 255 + .5)


def lines_image():
    """Tilted dark lines (an auto level finds them) inside a dark scan border."""
    yy, xx = np.mgrid[:160, :240]
    base = .25 + .5 * (xx / 240 * .7 + yy / 160 * .3)
    for height in (30, 70, 110):
        base[np.abs(yy - (height + .05 * xx)) < 1.5] = .04
    base[:8, :] = .01
    base[-8:, :] = .01
    base[:, :10] = .01
    base[:, -10:] = .01
    return np.stack([base, base * .95, base * .9], -1)


MADE = {
    'garden.png': lambda p: Image.fromarray(u8(INPUTS['garden'])).save(p),
    'patches.jpg': lambda p: Image.fromarray(u8(INPUTS['patches'])).save(p, quality=90),
    'colours.tif': lambda p: Image.fromarray(u8(INPUTS['colours'])).save(p),
    'ramp.png': lambda p: Image.fromarray(u8(INPUTS['ramp'])).save(p),
    'negative.png': lambda p: Image.fromarray(u8(np.array([.86, .56, .36], np.float32) * (1 - .8 * INPUTS['garden'][:, ::-1]) + .02)).save(p),
    'portrait.png': lambda p: Image.fromarray(u8(np.ascontiguousarray(INPUTS['garden'].transpose(1, 0, 2)[::2, ::2]))).save(p),
    'bayer0.dng': lambda p: shutil.copyfile(GOLDEN / 'rawload' / '_files' / 'bayer0.dng', p),
    'junk.png': lambda p: p.write_bytes(b'this is not an image\n'),
    'clip.mov': lambda p: p.write_bytes(b'not a real video\n'),
    'lines.png': lambda p: Image.fromarray(u8(lines_image())).save(p),
}
FILES.mkdir(parents=True, exist_ok=True)
for _name, _make in MADE.items():
    if not (FILES / _name).exists():
        _make(FILES / _name)


def session(name, layout, photos, script):
    """Run one scripted session on a fresh temporary library and write golden/mcp/sessions/<name>/."""
    temporary = Path(tempfile.mkdtemp(prefix='grainy-mcp-')).resolve()
    folder = SESSIONS / name
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    tokens = Tokens()
    recorded = []
    captured = []
    original_jpeg = photo_images.jpeg

    def recording_jpeg(array, quality=70, max_bytes=None):
        image = array if isinstance(array, Image.Image) else Image.fromarray(np.asarray(array, dtype=np.uint8))
        captured.append(np.asarray(image.convert('RGB'), dtype=np.uint8).copy())
        return original_jpeg(array, quality, max_bytes)
    saved_nested, saved_thread = mcp_service.nested, mcp_service.QThread
    try:
        for target, source in layout.items():
            path = temporary / target
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(FILES / source, path)
        scrub = SessionScrub(temporary)
        with patch.object(catalog_module, 'datetime', Clock), patch.object(command_batch, 'datetime', Clock), \
                patch.object(photo_service, 'datetime', Clock), patch.object(photo_labels, 'datetime', Clock), \
                patch.object(library_query, 'datetime', Clock), patch.object(workflow_data, 'datetime', Clock), \
                patch.object(photo_service, 'secrets', tokens), patch.object(photo_labels, 'secrets', tokens), \
                patch.object(mcp_service, 'QTimer', FakeTimer), patch.object(mcp_service, 'WAIT_SECONDS', 0), \
                patch.object(mcp_service, 'time', LocalTime()), patch.object(mcp_tools, 'time', LocalTime()), \
                patch.object(mcp_service, 'pictures_folder', lambda: temporary / 'Pictures' / 'Grainy'), \
                patch.object(photo_images, 'jpeg', recording_jpeg):
            catalog = Catalog(temporary / 'library')
            for target, metadata in photos:
                catalog.add(str(temporary / target), metadata=metadata)
            preferences = SimpleNamespace(path=temporary / 'settings' / 'settings.json', values={})
            host = SimpleNamespace(preferences=preferences, data_dir=temporary / 'library', window=None)
            service = mcp_service.McpService(host)
            service.running = True
            service.generation = 1
            window = FakeWindow(catalog, service)
            host.window = window
            gui = FakeGui()
            mcp_service.nested = lambda: window.nested
            mcp_service.QThread = SimpleNamespace(currentThread=lambda: SimpleNamespace(loopLevel=lambda: window.loop_level))
            QUEUE.clear()
            previous = dump(catalog.db, scrub)
            initial = previous
            for index, step in enumerate(script):
                step = deepcopy(step)
                op = step['op']
                args = unplace(deepcopy(step.get('args', {})), temporary)
                window.events = []
                captured.clear()
                if op == 'call':
                    bound = None if step.get('unbound') else gui
                    result = service.tools.call(step['tool'], args, deadline=time.monotonic() + 45, gui=bound)
                    step['error'] = result['isError']
                    step['texts'] = [scrub.text(c['text']) for c in result['content'] if c['type'] == 'text']
                    images = []
                    for k, block in enumerate([c for c in result['content'] if c['type'] == 'image']):
                        data = base64.b64decode(block['data'])
                        with Image.open(_io.BytesIO(data)) as decoded:
                            pixels = np.asarray(decoded.convert('RGB'), dtype=np.uint8)
                        file = None
                        if step.get('keep'):
                            file = f's{index:03d}_jpeg{k}.npy'
                            np.save(folder / file, pixels, allow_pickle=False)
                        images.append({'file': file, 'bytes': len(data), 'w': int(pixels.shape[1]), 'h': int(pixels.shape[0])})
                    step['images'] = images
                    raws = []
                    for k, pixels in enumerate(captured):
                        file = f's{index:03d}_raw{k}.npy'
                        np.save(folder / file, pixels, allow_pickle=False)
                        raws.append(file)
                    step['raw'] = raws
                    if step.get('settings_of'):
                        step['settings'] = {str(i): catalog.photo(i)['settings'] for i in step['settings_of']}
                elif op == 'window':
                    for key, value in args.get('set', {}).items():
                        if key == 'pending_stroke':
                            window.studio.pending_stroke = object() if value else None
                        elif key == 'healing_strokes':
                            window.studio.healing_strokes = [1] if value else []
                        elif key in ('crop_mode', 'sample_mode'):
                            setattr(window.view, key, value)
                        elif key == 'import_scans':
                            window.import_scans = [1] if value else []
                        else:
                            setattr(window, key, value)
                    if 'open' in args:
                        window.activate(args['open'])
                    for key, value in args.get('edit', {}).items():
                        window.settings[key] = value
                    if args.get('commit'):
                        window.commit()
                elif op == 'gui':
                    gui.closed = args['closed']
                elif op == 'run_jobs':
                    step['ran'] = run_queue(args.get('limit'))
                elif op == 'stop_job':
                    job = service.job
                    if job is not None and job.state == 'running':
                        job.stopped = True
                        job.cancel.set()
                elif op == 'restart':
                    service.generation += 1
                elif op == 'limits':
                    mcp_service.RETRY_SECONDS = args['retry']
                elif op == 'preference':
                    catalog.save_preference(args['key'], args['value'])
                elif op == 'mcp_preference':
                    preferences.values[args['key']] = args['value']
                elif op == 'delete':
                    (temporary / args['path']).unlink()
                elif op == 'mkdir':
                    (temporary / args['path']).mkdir(parents=True, exist_ok=True)
                elif op == 'save_preset':
                    catalog.save_preset(args['name'], args['settings'])
                else:
                    raise ValueError(op)
                step['jobs'] = [scrub(job.view()) for job in service.jobs.values()]
                step['queue'] = len(QUEUE)
                step['events'] = scrub(window.events)
                listing = []
                for base in ('Pictures', 'out'):
                    if (temporary / base).exists():
                        for path in sorted((temporary / base).rglob('*')):
                            if path.is_file():
                                listing.append([path.relative_to(temporary).as_posix(), path.stat().st_size])
                step['outputs'] = listing
                current = dump(catalog.db, scrub)
                step['delta'] = {k: v for k, v in current.items() if v != previous.get(k)}
                previous = current
                recorded.append(step)
            catalog.close()
        write_text_json(folder / 'case.json', {'stamp': STAMP, 'local': LOCAL, 'layout': layout, 'photos': photos,
                                              'initial': initial, 'steps': recorded, 'tokens_used': tokens.used})
        print('session', name, len(recorded), 'steps')
    finally:
        mcp_service.nested, mcp_service.QThread = saved_nested, saved_thread
        mcp_service.RETRY_SECONDS = 60
        shutil.rmtree(temporary, ignore_errors=True)


def C(tool, args=None, **extra):
    return {'op': 'call', 'tool': tool, 'args': args or {}, **extra}


def W(**args):
    return {'op': 'window', 'args': args}


def RUN(limit=None):
    return {'op': 'run_jobs', 'args': {} if limit is None else {'limit': limit}}


def JOB(ident):
    return C('get_job', {'job': ident, 'wait': 0})


def P(*parts):
    return SEP.join((PLACE,) + parts)


FILMS = film_look.catalog()
COLOR_FILM = next(e['id'] for e in FILMS if e.get('kind') == 'colour-negative')
BW_FILM = next(e['id'] for e in FILMS if e.get('kind') == 'bw-negative')
A = P('photos', 'A')
MAIN_LAYOUT = {
    'photos/A/garden.png': 'garden.png', 'photos/A/patches.jpg': 'patches.jpg', 'photos/A/colours.tif': 'colours.tif',
    'photos/A/sub/ramp.png': 'ramp.png', 'photos/B/negative.png': 'negative.png', 'photos/B/bayer0.dng': 'bayer0.dng',
    'photos/A/clip.mov': 'clip.mov', 'photos/A/gone.png': 'garden.png', 'photos/A/portrait.png': 'portrait.png',
    'import/c1.png': 'ramp.png', 'import/c2.jpg': 'patches.jpg', 'import/junk.png': 'junk.png',
    'import/deep/c3.png': 'portrait.png', 'import2/d1.png': 'colours.tif', 'photos/A/lines.png': 'lines.png',
}
MAIN_PHOTOS = [
    ('photos/A/garden.png', {'width': 240, 'height': 160}), ('photos/A/patches.jpg', {'width': 96, 'height': 64}),
    ('photos/A/colours.tif', {'width': 96, 'height': 64}), ('photos/A/sub/ramp.png', {'width': 96, 'height': 64}),
    ('photos/B/negative.png', {'width': 240, 'height': 160}), ('photos/B/bayer0.dng', {}),
    ('photos/A/clip.mov', {}), ('photos/A/gone.png', {'width': 240, 'height': 160}),
    ('photos/A/portrait.png', {'width': 80, 'height': 120}), ('photos/A/lines.png', {'width': 240, 'height': 160}),
]
MAIN = [
    {'op': 'delete', 'args': {'path': 'photos/A/gone.png'}},
    # list_photos
    C('list_photos'), C('list_photos', {'folder': A}), C('list_photos', {'folder': A, 'recursive': True, 'sort': 'name'}),
    C('list_photos', {'text': 'gar'}), C('list_photos', {'sort': 'name', 'descending': True, 'limit': 3, 'offset': 2}),
    C('list_photos', {'group': 'folders'}), C('list_photos', {'group': 'folders', 'text': '/b'}),
    C('list_photos', {'group': 'folders', 'folder': P('photos'), 'limit': 1}),
    C('list_photos', {'group': 'folders', 'scope': 'selection'}), C('list_photos', {'group': 'folders', 'sort': 'name'}),
    C('list_photos', {'scope': 'selection', 'rating_min': 1}), C('list_photos', {'recursive': True}),
    C('list_photos', {'rating_max': 0, 'limit': 2}), C('list_photos', {'collection': 99}),
    C('list_photos', {'scope': 'current'}), C('list_photos', {'limit': 201}),
    W(open=1), C('list_photos', {'scope': 'current'}), W(set={'selection': [2, 3]}), C('list_photos', {'scope': 'selection'}),
    W(set={'library_loading': True}), C('list_photos', {'scope': 'selection'}), W(set={'library_loading': False}),
    # get_photo
    C('get_photo', {'ids': [1, 2, 99]}), W(edit={'exposure': 0.7}), C('get_photo', {'ids': [1]}),
    W(edit={'exposure': 0.9}, set={'adjusting': True}), C('get_photo', {'ids': [1]}), W(set={'adjusting': False}),
    # get_preview
    C('get_preview', {'ids': [1, 2]}, keep=True), C('get_preview', {'ids': [9], 'size': 128, 'quality': 40}, keep=True),
    C('get_preview', {'ids': [7, 8, 99, 6]}), C('get_preview', {'ids': [5, 3]}),
    # list_films
    C('list_films'), C('list_films', {'type': 'bw'}), C('list_films', {'type': 'cine'}),
    {'op': 'save_preset', 'args': {'name': 'My look', 'settings': {'exposure': 0.3, 'saturation': 10}}},
    C('list_films', {'type': 'presets'}),
    # contact_sheet
    C('contact_sheet', {'ids': [1, 2, 3, 4, 5, 6, 7, 8, 9], 'cell': 96}, keep=True),
    C('contact_sheet', {'where': {'folder': A}, 'offset': 1, 'count': 2, 'cell': 96}),
    C('contact_sheet', {'scope': 'selection', 'cell': 128}),
    C('contact_sheet', {'ids': [1], 'variants': [{'film_look': COLOR_FILM}, {'film_look': ''},
                                                  {'preset': 'amber FILM', 'label': 'warm'},
                                                  {'film_look': BW_FILM, 'label': 'bw'}], 'cell': 96}),
    C('contact_sheet', {'ids': [1, 2], 'variants': [{'film_look': ''}]}),
    C('contact_sheet', {'ids': [1], 'variants': [{'film_look': '', 'preset': 'x'}]}),
    C('contact_sheet', {'ids': [1], 'size': 600}), C('contact_sheet', {'where': {'folder': A}, 'offset': 50}),
    C('contact_sheet', {'where': {'text': 'zzz'}}), C('contact_sheet', {'ids': [1], 'variants': [{'preset': 'nope'}]}),
    C('contact_sheet', {'ids': [1, 2, 9], 'save': True, 'size': 512, 'cell': 96}),
    C('contact_sheet', {'ids': [1, 2, 9], 'save': True, 'size': 512, 'cell': 96}),
    C('contact_sheet', {'ids': [1], 'save': True, 'destination': 'path', 'path': P('library')}),
    C('contact_sheet', {'ids': [1], 'save': True, 'destination': 'last'}),
    # apply_edits
    C('apply_edits', {'edits': [{'ids': [1, 2], 'set': {'exposure': 0.5, 'contrast': 20}}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'add': {'exposure': 0.3, 'saturation': -10}},
                                {'ids': [2], 'set': {'film_look': COLOR_FILM, 'film_strength': 80}},
                                {'ids': [3], 'preset': 'Soft daylight'}, {'ids': [4], 'copy_from': 1, 'groups': ['light']},
                                {'ids': [9], 'preset': 'My look'}]}),
    C('apply_edits', {'edits': [{'ids': [3], 'crop': [0.1, 0.1, 0.9, 0.8], 'set': {'rotation': 90, 'flip': True}}]}),
    C('apply_edits', {'edits': [{'ids': [4], 'set': {'curve': [[0, 0], [0.5, 0.6], [1, 1]], 'monochrome': True,
                                                     'rgb_curves': [[[0, 0], [1, 1]], [[0, 0.1], [1, 1]], [[0, 0], [1, 0.9]]]}}]}),
    C('apply_edits', {'edits': [{'where': {'folder': A, 'text': 'gar'}, 'add': {'exposure': -0.2}},
                                {'scope': 'current', 'add': {'vibrance': 15}}]}),
    C('apply_edits', {'edits': [{'scope': 'selection', 'add': {'whites': 300}}]}),
    C('apply_edits', {'edits': [{'ids': [1, 7, 99], 'set': {'exposure': 0.1}}]}),
    C('apply_edits', {'edits': [{'ids': [7, 99], 'set': {'exposure': 0.1}}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'set': {'exposure': 9}}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'set': {'foo': 1}}]}), C('apply_edits', {'edits': [{'ids': [1]}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'preset': 'Soft daylight', 'copy_from': 2}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'groups': ['light'], 'set': {'exposure': 1}}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'preset': 'nope'}]}), C('apply_edits', {'edits': [{'ids': [2], 'copy_from': 7}]}),
    C('apply_edits', {'edits': [{'ids': [2], 'copy_from': 99}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'crop': [0.5, 0.1, 0.4, 0.9]}]}),
    C('apply_edits', {'edits': [{'ids': [1], 'scope': 'current', 'set': {'exposure': 0.2}}]}),
    C('apply_edits', {'edits': [{'ids': [1, 9], 'auto_straighten': True, 'auto_crop': True}]}),
    C('apply_edits', {'edits': [{'ids': [10], 'auto_straighten': True}, {'ids': [10], 'auto_crop': True}]}, settings_of=[10]),
    C('apply_edits', {'edits': [{'ids': [8], 'add': {'exposure': 0.1}}]}),
    C('apply_edits', {'edits': [{'ids': [2], 'set': {'film_strength': 80}}]}),
    # undo
    C('undo_last_batch'), C('undo_last_batch', {'redo': True}), C('undo_last_batch', {'batch': 'b000099'}),
    C('undo_last_batch', {'redo': True}),
    C('apply_edits', {'edits': [{'ids': [1], 'add': {'contrast': 5}}]}), W(open=1, edit={'exposure': 1.25}, commit=True),
    C('undo_last_batch'),
    # set_labels
    C('set_labels', {'ids': [1, 2], 'rating': 3, 'flag': 'picked', 'label': 'red'}),
    C('set_labels', {'entries': [{'ids': [3], 'rating': 5}, {'where': {'folder': P('photos', 'B')}, 'flag': 'rejected'},
                                 {'scope': 'selection', 'label': 'none'}]}),
    C('set_labels', {'ids': [1], 'entries': [{'ids': [1], 'rating': 1}]}), C('set_labels', {'ids': [1]}),
    C('set_labels', {'ids': [7, 99], 'rating': 2}), C('undo_last_batch'),
    C('list_photos', {'flag': 'picked'}), C('list_photos', {'label': 'red', 'rating_min': 3}),
    # snapshot
    C('snapshot', {'action': 'save', 'ids': [1, 2], 'name': 'Look A'}), C('snapshot', {'action': 'list', 'ids': [1, 2, 99]}),
    C('apply_edits', {'edits': [{'ids': [1, 2], 'set': {'exposure': -1}}]}),
    C('snapshot', {'action': 'restore', 'ids': [1, 3], 'name': 'look a'}),
    C('snapshot', {'action': 'list', 'scope': 'current', 'name': 'look a'}),
    C('snapshot', {'action': 'save', 'ids': [1]}), C('snapshot', {'action': 'save', 'ids': [1], 'name': '  '}),
    C('snapshot', {'action': 'restore', 'ids': [3], 'name': 'zzz'}), C('undo_last_batch'),
    # virtual copies
    C('create_virtual_copies', {'id': 1, 'count': 2}), C('create_virtual_copies', {'id': 7, 'count': 1}),
    C('create_virtual_copies', {'id': 99, 'count': 1}), C('undo_last_batch'), C('list_photos', {'text': 'garden'}),
    # open_photo
    C('open_photo', {'id': 2}), C('open_photo', {'id': 3, 'view': 'library'}), C('open_photo', {'id': 7}),
    C('open_photo', {'id': 99}), W(set={'listed': [1, 2]}), C('open_photo', {'id': 4}), W(set={'listed': None}),
    # automatic edits as jobs
    C('auto_tone', {'ids': [1, 3]}), JOB('j1'), C('apply_edits', {'edits': [{'ids': [2], 'add': {'exposure': 0.1}}]}),
    RUN(), JOB('j1'), C('undo_last_batch'),
    C('auto_color', {'ids': [1, 2], 'strength': 50}), RUN(), JOB('j2'),
    {'op': 'preference', 'args': {'key': 'auto_color_amount', 'value': 30}}, C('auto_color', {'scope': 'current'}), RUN(), JOB('j3'),
    C('invert_negatives', {'ids': [5]}), RUN(), JOB('j4'),
    C('invert_negatives', {'ids': [5, 6], 'roll': True}), RUN(), JOB('j5'),
    C('invert_negatives', {'ids': [5], 'roll': True}),
    C('invert_negatives', {'ids': [5, 8], 'roll': True}), RUN(), JOB('j6'),
    W(set={'inline': True}), C('auto_tone', {'ids': [9]}), C('auto_tone', {'ids': [8, 9]}),
    C('invert_negatives', {'ids': [5, 8], 'roll': True}), W(set={'inline': False}),
    C('auto_tone', {'ids': [2]}), W(set={'command_running': True}), RUN(1), JOB('j10'), RUN(1), JOB('j10'), W(set={'command_running': False}), RUN(), JOB('j10'),
    {'op': 'limits', 'args': {'retry': 0}}, C('auto_tone', {'ids': [2]}), W(set={'maintenance_running': True}), RUN(1), JOB('j11'),
    W(set={'maintenance_running': False}), {'op': 'limits', 'args': {'retry': 60}},
    C('auto_tone', {'ids': [3]}), {'op': 'stop_job'}, RUN(), JOB('j12'),
    C('auto_tone', {'ids': [3]}), {'op': 'restart'}, RUN(), JOB('j13'),
    C('auto_tone', {'ids': [1, 2]}), W(open=1, edit={'contrast': 33}, commit=True), RUN(), JOB('j14'),
    # export_photos
    C('export_photos', {'ids': [1, 2, 6], 'dry_run': True}),
    C('export_photos', {'ids': [1, 9], 'dry_run': True, 'width': 100, 'height': 100, 'fit': 'cover', 'format': 'PNG',
                        'naming': '{n:02d}-{stem}'}),
    C('export_photos', {'ids': [1], 'longest': 100, 'width': 50}), C('export_photos', {'ids': [1], 'fit': 'cover'}),
    C('export_photos', {'ids': [1], 'format': 'JPEG XL', 'color_space': 'Adobe RGB'}),
    C('export_photos', {'ids': [1], 'naming': 'a/b'}),
    C('export_photos', {'ids': [1], 'destination': 'path', 'path': P('library')}),
    C('export_photos', {'ids': [1], 'destination': 'path'}), C('export_photos', {'ids': [1], 'destination': 'last'}),
    C('export_photos', {'ids': [1, 2, 8], 'longest': 120}), JOB('j15'), C('export_photos', {'ids': [3]}), RUN(), JOB('j15'),
    C('export_photos', {'ids': [1], 'longest': 120}), RUN(), JOB('j16'),
    C('export_photos', {'ids': [1, 3], 'destination': 'path', 'path': P('out'), 'subfolder': 'set', 'format': 'PNG',
                        'naming': '{stem}'}), RUN(), JOB('j17'),
    C('export_photos', {'ids': [1], 'destination': 'last'}),
    {'op': 'preference', 'args': {'key': quick_export.PREFERENCE, 'value': {
        'folder': P('out'), 'format': 'JPEG', 'quality': 90, 'longest': 0, 'color_space': 'sRGB', 'keep_metadata': False,
        'naming': '{stem}_grainy'}}},
    C('export_photos', {'ids': [1], 'destination': 'last', 'dry_run': True}),
    C('export_photos', {'ids': [1, 2]}), {'op': 'stop_job'}, RUN(), JOB('j18'),
    W(set={'export_running': True}), C('export_photos', {'ids': [1]}), W(set={'export_running': False}),
    {'op': 'mcp_preference', 'args': {'key': 'mcp_export_folder', 'value': P('out', 'ai')}},
    C('export_photos', {'ids': [2], 'dry_run': True}), {'op': 'mkdir', 'args': {'path': 'out/ai'}},
    C('export_photos', {'ids': [2], 'dry_run': True}),
    # import_folder
    C('import_folder', {'path': P('import')}), RUN(1), JOB('j19'), RUN(), JOB('j19'),
    C('import_folder', {'path': P('import'), 'recursive': True}), RUN(), JOB('j20'),
    C('import_folder', {'path': 'relative' + SEP + 'x'}), C('import_folder', {'path': P('nothing')}),
    C('import_folder', {'path': P('library')}),
    C('import_folder', {'path': P('import2')}), RUN(1), {'op': 'stop_job'}, RUN(), JOB('j21'),
    C('undo_last_batch'), C('get_job', {'job': 'j99'}),
    # Busy and closed
    W(set={'nested': True}), C('apply_edits', {'edits': [{'ids': [2], 'add': {'exposure': 0.1}}]}), W(set={'nested': False}),
    W(set={'closing': True}), C('list_photos'), C('get_photo', {'ids': [1]}), W(set={'closing': False}),
    {'op': 'gui', 'args': {'closed': True}}, C('get_photo', {'ids': [1]}), {'op': 'gui', 'args': {'closed': False}},
    C('get_photo', {'ids': [1]}, unbound=True),
    W(set={'import_busy': True}), C('set_labels', {'ids': [2], 'rating': 1}), W(set={'import_busy': False}),
    W(open=1, set={'pending_stroke': True}), C('apply_edits', {'edits': [{'ids': [1], 'add': {'exposure': 0.1}}]}),
    W(set={'pending_stroke': False, 'crop_mode': True}), C('apply_edits', {'edits': [{'ids': [1], 'add': {'exposure': 0.1}}]}),
    W(set={'crop_mode': False, 'adjusting': True}), C('apply_edits', {'edits': [{'ids': [1], 'add': {'exposure': 0.1}}]}),
    W(set={'adjusting': False, 'loop_level': 2}, edit={'exposure': 0.11}), C('get_photo', {'ids': [1]}),
    C('apply_edits', {'edits': [{'ids': [1], 'add': {'exposure': 0.1}}]}), W(set={'loop_level': 1}),
    C('apply_edits', {'edits': [{'ids': [1], 'add': {'exposure': 0.1}}]}),
    # A saved sheet of small cells: thumbnails go through Image.reduce (reducing_gap) before the resampling.
    C('contact_sheet', {'ids': [1, 2, 3, 4, 5, 9, 10], 'save': True, 'size': 512, 'cell': 96}),
]
session('main', MAIN_LAYOUT, MAIN_PHOTOS, MAIN)

MANY_PHOTOS = [('many/p000.png', {'width': 96, 'height': 64})] + [(f'many/p{n:03d}.png', {'width': 96, 'height': 64}) for n in range(1, 501)]
MANY = [
    C('list_photos', {'limit': 3, 'offset': 498}), C('list_photos', {'limit': 2, 'offset': 600}),
    C('set_labels', {'where': {'folder': P('many')}, 'rating': 1}),
    C('export_photos', {'ids': list(range(1, 121)), 'dry_run': True}),
    C('contact_sheet', {'where': {'folder': P('many')}, 'cell': 96, 'count': 12}),
    C('set_labels', {'ids': list(range(1, 121)), 'rating': 2}), C('undo_last_batch'),
    C('apply_edits', {'edits': [{'ids': [1] + list(range(600, 631)), 'add': {'exposure': 0.1}}]}),
    C('list_photos', {'group': 'folders'}),
]
session('many', {'many/p000.png': 'ramp.png'}, MANY_PHOTOS, MANY)

# --- the caption font as Pillow lays it out (what photo_images.mark/contact_sheet draw with) ---------------------
FONT_SAMPLES = ['#1', '#123  3*  P', '#45  5*  X', 'not ready', 'garden.png', 'Kodak Portra 400', '한글 이름.jpg', '…', 'W', 'iiii']
font_cases = []
for size in range(13, 23):
    font = photo_images._font(size)
    font_cases.append({'size': size, 'path': getattr(font, 'path', None), 'metrics': list(font.getmetrics()),
                       'lengths': [font.getlength(t) for t in FONT_SAMPLES],
                       'bbox_lm': [list(font.getbbox(t, anchor='lm')) for t in FONT_SAMPLES],
                       'bbox_mm': [list(font.getbbox(t, anchor='mm')) for t in FONT_SAMPLES]})
write_text_json(GOLDEN / MODULE / 'fonts.json', {'samples': FONT_SAMPLES, 'cases': font_cases,
                                                 'marks': [[i, r, f, photo_images.mark(i, r, f)] for i, r, f in
                                                           [(1, 0, 0), (12, 3, 1), (7, 5, -1), (9, 0, -1), (100, 2.0, 0)]]})
print('fonts:', len(font_cases))
