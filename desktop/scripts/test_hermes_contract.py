#!/usr/bin/env python3
"""Opt-in, container-isolated native Hermes contract; responses are synthetic fixtures.

Supplemental interpreter tests never substitute for selected-launcher acceptance.
Production application capability wiring is a separate, fail-closed acceptance boundary.
"""
import unittest
import os
import tempfile
import json
import urllib.request
import threading
import time
import sys
import shlex
import subprocess
import hashlib
import argparse
import io
from datetime import datetime, timezone
from types import SimpleNamespace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

PROFILE = '550E8400-E29B-41D4-A716-446655440000'
HELPER_SHA256 = '1fd9bdb167a1df12be09bcd5a52fb7586dd1065ae823cd30793ba3e8b920a494'



class NativeContractTests(unittest.TestCase):
    @unittest.skipUnless('--native-case' in sys.argv, 'opt-in installed runtime only')
    def test_resolved_scratch_config_has_no_fallback_or_tools(self):
        from hermes_cli.config import load_config
        from hermes_cli.fallback_config import get_fallback_chain
        from hermes_cli.tools_config import _get_platform_tools
        from model_tools import get_tool_definitions
        cfg = load_config()
        self.assertEqual(get_fallback_chain(cfg), [])
        tools = _get_platform_tools(cfg, 'cli')
        self.assertEqual(tools, set())
        self.assertEqual(get_tool_definitions(enabled_toolsets=list(tools),
                         disabled_toolsets=cfg['agent']['disabled_toolsets'], quiet_mode=True), [])
        self.assertFalse(cfg['memory']['memory_enabled'])
        self.assertFalse(cfg['auxiliary']['title_generation']['enabled'])

    @unittest.skipUnless('--native-case' in sys.argv, 'opt-in installed runtime only')
    def test_cli_unset_really_removed_stored_fields(self):
        import hermes_yaml as yaml
        data = yaml.safe_load((Path(os.environ['HERMES_HOME']) / 'config.yaml').read_text())
        for key in ('providers.keyzapper', 'model.provider', 'model.default', 'model.base_url'):
            node = data
            exists = True
            for part in key.split('.'):
                if not isinstance(node, dict) or part not in node:
                    exists = False
                    break
                node = node[part]
            self.assertFalse(exists)

    @unittest.skipUnless('--native-case' in sys.argv, 'opt-in installed runtime only')
    def test_actual_parser_formats_and_redacted_failure(self):
        from agent.command_token_source import _mint, CommandTokenError
        folder = Path(os.environ['TMPDIR'])
        expiry = datetime.fromtimestamp(time.time() + 1800, timezone.utc).isoformat()
        cases = [('synthetic-static', False, False),
                 (json.dumps({'access_token': 'synthetic-static'}), False, False),
                 (json.dumps({'access_token': 'synthetic-static', 'expires_in': 1800}), True, False),
                 (json.dumps({'access_token': 'synthetic-static', 'expiry': expiry}), True, False),
                 (json.dumps({'access_token': 'synthetic-static', 'expiresOn': expiry}), True, False),
                 ('one\ntwo', False, True), ('{}', False, True), ('', False, True)]
        for index, (output, ttl_expected, fails) in enumerate(cases):
            fixture = folder / f'parser-fixture-{index}.sh'
            fixture.write_text('#!/bin/sh\nprintf %s ' + shlex.quote(output) + '\n')
            command = '/bin/sh ' + shlex.quote(str(fixture))
            if fails:
                with self.assertRaises(CommandTokenError):
                    _mint(command, 'fixture')
            else:
                token, ttl = _mint(command, 'fixture')
                self.assertTrue(token == 'synthetic-static')
                self.assertEqual(ttl is not None, ttl_expected)
                if ttl_expected:
                    self.assertTrue(ttl is not None and 1700 < ttl <= 1800)
        fixture = folder / 'parser-failure-fixture.sh'
        fixture.write_text('#!/bin/sh\nprintf synthetic-do-not-print\nprintf synthetic-do-not-print >&2\nexit 67\n')
        command = '/bin/sh ' + shlex.quote(str(fixture))
        with self.assertRaises(CommandTokenError) as caught:
            _mint(command, 'fixture')
        self.assertNotIn('synthetic-do-not-print', str(caught.exception))
        self.assertNotIn(str(fixture), str(caught.exception))

    @unittest.skipUnless('--native-case' in sys.argv, 'opt-in installed runtime only')
    def test_same_live_source_calls_real_helper_after_expiry(self):
        from agent import command_token_source as native
        command = shlex.quote(os.environ['CONTRACT_HELPER']) + f' credential --profile {PROFILE} --json'
        clock = SimpleNamespace(monotonic=lambda: 100.0, time=time.time)
        with patch.object(native, 'time', clock):
            source = native.CommandTokenSource(command, 'keyzapper')
            first = source()
            self.assertTrue(first.startswith('synthetic-access-'))
            self.assertTrue(source() == first)
            # Make the REAL helper refresh (wall clock) and the SAME source remint (monotonic).
            path = Path(os.environ['KEYZAPPER_HOME']) / 'sso-tokens.json'
            data = json.loads(path.read_text())
            data[PROFILE]['expiresAt'] = int(time.time()) - 10
            path.write_text(json.dumps(data))
            clock.monotonic = lambda: 4000.0
            second = source()
            self.assertTrue(second.startswith('synthetic-access-'))
            self.assertTrue(second != first)
            self.assertTrue(source() == second)

    @unittest.skipUnless('--native-case' in sys.argv, 'opt-in installed runtime only')
    def test_native_adapter_json_stream_and_tool_parsing_without_execution(self):
        from agent.anthropic_adapter import build_anthropic_client, create_anthropic_message
        from agent.command_token_source import CommandTokenSource
        command = shlex.quote(os.environ['CONTRACT_HELPER']) + f' credential --profile {PROFILE} --json'
        client = build_anthropic_client(CommandTokenSource(command), os.environ['CONTRACT_BASE'] + '/v1', 10)
        kwargs = {'model': 'team/mock', 'max_tokens': 32,
                  'messages': [{'role': 'user', 'content': 'synthetic fixture prompt'}]}
        json_message = create_anthropic_message(client, kwargs.copy(), prefer_stream=False)
        stream_message = create_anthropic_message(client, kwargs.copy())
        if os.environ.get('CONTRACT_TOOL') == '1':
            for message in (json_message, stream_message):
                self.assertEqual(message.stop_reason, 'tool_use')
                block = message.content[0]
                self.assertEqual(block.name, 'contract_echo')
                self.assertEqual(block.input, {'text': 'harmless fixture'})
            # Deliberately no dispatcher/AIAgent: parsed tool data never executes.
        else:
            self.assertEqual(json_message.content[0].text, 'SYNTHETIC CONTRACT OK')
            self.assertEqual(stream_message.content[0].text, 'SYNTHETIC CONTRACT OK')
        client.close()


def isolated_env(home):
    # Construct from scratch: unknown SDK/provider variables are excluded too.
    return {
        'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'HOME': str(home / 'user'),
        'HERMES_HOME': str(home / 'hermes'), 'KEYZAPPER_HOME': str(home / 'keyzapper'),
        'TMPDIR': str(home / 'tmp'), 'XDG_CONFIG_HOME': str(home / 'user/config'),
        'XDG_CACHE_HOME': str(home / 'user/cache'), 'XDG_DATA_HOME': str(home / 'user/data'),
        'HERMES_SAFE_MODE': '1', 'HERMES_IGNORE_RULES': '1',
        'HERMES_DISABLE_LAZY_INSTALLS': '1', 'PYTHONDONTWRITEBYTECODE': '1',
        'AWS_EC2_METADATA_DISABLED': 'true', 'NO_COLOR': '1',
    }


def safe_failure(stage, error):
    # No stringification of child exceptions (CalledProcessError contains argv/stdout).
    result = {'stage': stage, 'error_type': type(error).__name__}
    if isinstance(error, ContractFailure):
        result['check'] = error.code
        result['observations'] = error.observations
    return result


def scratch_settings(base):
    auxiliary = {name: {'provider': 'keyzapper', 'model': 'team/mock', 'base_url': base + '/v1',
                       'api_mode': 'anthropic_messages', 'enabled': False}
                 for name in ('vision', 'compression', 'approval', 'review', 'mcp', 'title_generation',
                              'memory_query_rewrite', 'tts_audio_tags', 'voice_chat', 'triage_specifier',
                              'kanban_decomposer', 'profile_describer', 'goal_judge', 'group_router',
                              'curator', 'monitor', 'background_review', 'moa_reference', 'moa_aggregator')}
    auxiliary['title_generation']['model_upgrade_enabled'] = False
    return {
        'fallback_providers': [], 'fallback_model': [], 'platform_toolsets': {'cli': []},
        'toolsets': [], 'mcp_servers': {}, 'compression': {'enabled': False},
        'memory': {'memory_enabled': False, 'user_profile_enabled': False, 'provider': ''},
        'plugins': {'enabled': [], 'auto_update_check_hours': 0, 'auto_apply': False},
        'curator': {'enabled': False, 'consolidate': False}, 'auxiliary': auxiliary,
        'updates': {'check': False}, 'checkpoints': {'enabled': False},
        'agent': {'max_turns': 4, 'disabled_toolsets': ['all'], 'prefill_messages': [],
                  'tool_use_enforcement': False},
        'delegation': {'provider': 'keyzapper', 'model': 'team/mock', 'base_url': base + '/v1',
                       'fallback_providers': []},
        'display': {'interface': 'cli'}, 'lsp': {'enabled': False},
        'model': {'context_length': 65536}, 'models_dev': {'url': base + '/models-catalog'},
        'telemetry': {'shared_metrics': {'enabled': False, 'send': False}},
    }


def message_fixture(tool=False):
    """Declared Anthropic Messages fixtures, NOT model-generated responses."""
    content = ({'type': 'tool_use', 'id': 'fixture-call', 'name': 'contract_echo',
                'input': {'text': 'harmless fixture'}} if tool else
               {'type': 'text', 'text': 'SYNTHETIC CONTRACT OK'})
    return {'id': 'msg_fixture', 'type': 'message', 'role': 'assistant', 'model': 'team/mock',
            'content': [content], 'stop_reason': 'tool_use' if tool else 'end_turn',
            'stop_sequence': None, 'usage': {'input_tokens': 1, 'output_tokens': 1}}


def sse_fixture(tool=False):
    message = message_fixture(tool)
    block = message['content'][0]
    start = {**block, 'input': {}} if tool else {'type': 'text', 'text': ''}
    delta = ({'type': 'input_json_delta', 'partial_json': json.dumps(block['input'])} if tool else
             {'type': 'text_delta', 'text': block['text']})
    events = [
        {'type': 'message_start', 'message': {**message, 'content': [], 'stop_reason': None}},
        {'type': 'content_block_start', 'index': 0, 'content_block': start},
        {'type': 'content_block_delta', 'index': 0, 'delta': delta},
        {'type': 'content_block_stop', 'index': 0},
        {'type': 'message_delta', 'delta': {'stop_reason': message['stop_reason'], 'stop_sequence': None},
         'usage': {'output_tokens': 1}},
        {'type': 'message_stop'},
    ]
    return ''.join('event: ' + e['type'] + '\ndata: ' + json.dumps(e) + '\n\n' for e in events).encode()


class MockGateway:
    def __init__(self):
        self.requests = []  # Boolean observations only, never request bodies or headers.
        self.refreshes = 0
        self.discovery_count = 0
        self.other_requests = 0
        self.metadata_requests = 0
        self.other_routes = []
        self.expected = 'synthetic-access'
        self.refresh = 'synthetic-refresh-0'
        self.tool = False
        self.lock = threading.Lock()

    def __enter__(self):
        state = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format, *args):
                pass

            def setup(self):
                super().setup()
                self.connection.settimeout(5)

            def reply(self, status, payload, content_type='application/json'):
                data = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
                self.send_response(status)
                self.send_header('Content-Type', content_type)
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                if self.path == '/models-catalog':
                    state.metadata_requests += 1
                    self.reply(200, {})  # Declared empty local catalog, never models.dev.
                    return
                if self.path in ('/api/v1/models', '/api/tags', '/v1/props', '/props', '/version', '/v1/models', '/models'):
                    state.metadata_requests += 1
                    self.reply(200, {'data': [{'id': 'team/mock', 'context_length': 65536}],
                                     'default_generation_settings': {'n_ctx': 65536}})
                    return
                if self.path != '/issuer/.well-known/openid-configuration':
                    state.other_requests += 1
                    from urllib.parse import urlsplit
                    url = urlsplit(self.path)
                    state.other_routes.append({'method': 'GET', 'host': url.hostname, 'path': url.path})
                    self.reply(404, {})
                    return
                state.discovery_count += 1
                self.reply(200, {'issuer': state.base + '/issuer',
                    'authorization_endpoint': state.base + '/authorize',
                    'token_endpoint': state.base + '/token'})

            def do_CONNECT(self):
                state.other_requests += 1
                state.other_routes.append({'method': 'CONNECT', 'host': self.path.split(':')[0]})
                self.reply(403, {})

            def do_POST(self):
                from urllib.parse import parse_qs
                length = int(self.headers.get('Content-Length', '0'))
                if length <= 0 or length > 2_000_000:
                    self.reply(400, {})
                    return
                raw = self.rfile.read(length)
                if self.path == '/token':
                    fields = parse_qs(raw.decode())
                    with state.lock:
                        valid = fields == {'grant_type': ['refresh_token'],
                            'refresh_token': [state.refresh], 'client_id': ['synthetic-client'],
                            'scope': ['openid profile offline_access']}
                        if not valid:
                            self.reply(400, {'error': 'invalid_grant'})
                            return
                        # Let the second helper contend on the actual inter-process lock.
                        time.sleep(0.15)
                        state.refreshes += 1
                        state.expected = f'synthetic-access-{state.refreshes}'
                        state.refresh = f'synthetic-refresh-{state.refreshes}'
                        self.reply(200, {'access_token': state.expected, 'expires_in': 3600,
                            'refresh_token': state.refresh, 'token_type': 'Bearer'})
                    return
                try:
                    body = json.loads(raw)
                except ValueError:
                    body = None
                auth_ok = self.headers.get('Authorization') == 'Bearer ' + state.expected
                status = validate_request(self.path, 'Bearer synthetic-access' if auth_ok else '', body)
                state.requests.append({'accepted': status == 200, 'path_ok': self.path == '/v1/messages',
                    'auth_ok': auth_ok, 'model_ok': isinstance(body, dict) and body.get('model') == 'team/mock',
                    'stream': bool(body.get('stream')) if isinstance(body, dict) else False,
                    'tools_absent': not body.get('tools') if isinstance(body, dict) else True})
                if len(state.requests) > 64 or status != 200:
                    self.reply(status if status != 200 else 429, {'type': 'error'})
                elif isinstance(body, dict) and body.get('stream'):
                    self.reply(200, sse_fixture(state.tool), 'text/event-stream')
                else:
                    self.reply(200, message_fixture(state.tool))

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.base = f'http://127.0.0.1:{self.server.server_port}'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


def prepare_home(home):
    root_text = os.environ.get('TMPDIR', '')
    root = Path(root_text)
    home = Path(home)
    if (not root_text or not root.is_absolute() or root.is_symlink()
            or not root.is_dir() or not home.is_absolute() or '..' in home.parts):
        raise ValueError('unsafe scratch target')
    root = root.resolve(strict=True)
    if home == root or root not in home.parents:
        raise ValueError('home must be a fresh child of TMPDIR')
    for part in (home, *home.parents):
        if part.is_symlink():
            raise ValueError('symlink scratch target')
    if home.exists() or not home.parent.is_dir():
        raise ValueError('home must be new with an existing parent')
    home.mkdir(mode=0o700)
    return home


def validate_request(path, authorization, body):
    if path != '/v1/messages':
        return 404
    if authorization != 'Bearer synthetic-access':
        return 401
    if not isinstance(body, dict) or body.get('model') != 'team/mock':
        return 400
    return 200


class ContractFixtureTests(unittest.TestCase):
    def test_diagnostic_refuses_unverified_source_before_any_installed_import(self):
        with tempfile.TemporaryDirectory(dir=os.environ['TMPDIR']) as temporary:
            root = Path(temporary)
            marker = root / 'imported'
            (root / 'hermes_bootstrap.py').write_text('from pathlib import Path\nPath(' + repr(str(marker)) + ').touch()\n')
            manifest = root / 'manifest.json'
            manifest.write_text(json.dumps({'publicSourceFiles': ['hermes_bootstrap.py'],
                'supportedBuilds': [{'passed': True, 'fullPass': True, 'sourceTreeSha256': 'wrong'}]}))
            probe = Path(__file__).parents[1] / 'core/src/hermes/compatibility_probe.py'
            result = subprocess.run([sys.executable, '-I', str(probe), str(root), str(manifest)],
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(marker.exists(), 'unverified installed code was imported')

    def test_public_source_fingerprint_binds_all_allowlisted_files_and_rejects_escape(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location('compatibility_probe', Path(__file__).parents[1] / 'core/src/hermes/compatibility_probe.py')
        assert spec is not None and spec.loader is not None
        probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(probe)
        with tempfile.TemporaryDirectory(dir=os.environ['TMPDIR']) as temporary:
            root = Path(temporary)
            (root / 'a.py').write_text('a')
            (root / 'b.py').write_text('b')
            before = probe.source_fingerprint(root, ['a.py', 'b.py'])
            (root / 'b.py').write_text('changed')
            self.assertNotEqual(before, probe.source_fingerprint(root, ['a.py', 'b.py']))
            with self.assertRaises(ValueError):
                probe.source_fingerprint(root, ['../outside.py'])

    def test_developer_runtime_uses_selected_console_entrypoint_and_version_root(self):
        with tempfile.TemporaryDirectory(dir=os.environ['TMPDIR']) as temporary:
            root = Path(temporary)
            (root / 'hermes_bootstrap.py').touch()
            entry = root / 'hermes'
            entry.write_text('#!' + sys.executable + '\nfrom hermes_cli.main import main\n')
            version = 'Hermes Agent vunknown (2026.9.24)\nInstall directory: ' + str(root) + '\n'
            self.assertEqual(developer_runtime(entry, version), (Path(sys.executable), root))
            entry.write_text('#!/bin/sh\necho unsupported\n')
            with self.assertRaises(ContractFailure):
                developer_runtime(entry, version)
            entry.write_text('#!' + sys.executable + '\nfrom hermes_cli.main import main\n')
            with self.assertRaises(ContractFailure):
                developer_runtime(entry, 'Hermes Agent vunknown\n')

    def test_tool_fixture_is_harmless_data_not_a_dispatch_request(self):
        payload = message_fixture(tool=True)
        self.assertEqual(payload['stop_reason'], 'tool_use')
        self.assertEqual(payload['content'][0]['name'], 'contract_echo')
        events = [json.loads(line[6:]) for line in sse_fixture(tool=True).decode().splitlines()
                  if line.startswith('data: ')]
        delta = events[2]['delta']
        self.assertEqual(delta['type'], 'input_json_delta')
        self.assertEqual(json.loads(delta['partial_json']), {'text': 'harmless fixture'})

    def test_cli_roundtrip_sequence_fixture_is_not_native_evidence(self):
        fields = {}
        calls = []

        def fixture_cli(args):
            calls.append(args)
            if args[:2] == ['config', 'set']:
                value = args[3]
                fields[args[2]] = json.loads(value) if value.startswith('{') else value
                return 0, b'', b''
            if args[:3] == ['config', 'get', '--json']:
                return (0, json.dumps(fields[args[3]]).encode(), b'') if args[3] in fields else (1, b'', b'')
            if args[:2] == ['config', 'unset']:
                fields.pop(args[2], None)
                return 0, b'', b''
            self.fail('unexpected fixture CLI operation')

        provider = {'base_url': 'http://127.0.0.1:12345/v1', 'api_mode': 'anthropic_messages',
                    'key_cmd': "'/synthetic/keyzapper-helper' credential --profile " + PROFILE + ' --json'}
        self.assertTrue(cli_roundtrip(fixture_cli, provider))
        self.assertEqual(fields, {})
        self.assertEqual(len(calls), 13)

    def test_scratch_settings_disable_fallback_tools_and_auxiliary_routes(self):
        settings = scratch_settings('http://127.0.0.1:12345')
        self.assertEqual(settings.get('fallback_providers'), [])
        self.assertEqual(settings.get('fallback_model'), [])
        self.assertEqual(settings.get('platform_toolsets'), {'cli': []})
        self.assertFalse(settings['memory']['memory_enabled'])
        self.assertFalse(settings['compression']['enabled'])
        self.assertFalse(settings['auxiliary']['title_generation']['enabled'])
        self.assertFalse(settings['auxiliary']['background_review']['enabled'])
        self.assertEqual(settings['plugins']['enabled'], [])
        self.assertEqual(settings['delegation']['fallback_providers'], [])

    def test_declared_json_and_sse_fixtures_over_loopback(self):
        with MockGateway() as server:
            for stream in (False, True):
                request = urllib.request.Request(server.base + '/v1/messages',
                    json.dumps({'model': 'team/mock', 'stream': stream}).encode(),
                    {'Authorization': 'Bearer synthetic-access', 'Content-Type': 'application/json'})
                with urllib.request.urlopen(request, timeout=5) as response:
                    data = response.read().decode()
                if stream:
                    events = [json.loads(line[6:]) for line in data.splitlines() if line.startswith('data: ')]
                    self.assertEqual([event['type'] for event in events], [
                        'message_start', 'content_block_start', 'content_block_delta',
                        'content_block_stop', 'message_delta', 'message_stop'])
                else:
                    self.assertEqual(json.loads(data)['content'][0]['text'], 'SYNTHETIC CONTRACT OK')
            self.assertEqual(len(server.requests), 2)
            self.assertTrue(all(entry['accepted'] for entry in server.requests))
            self.assertNotIn('Bearer', json.dumps(server.requests))

    def test_environment_is_allowlisted_not_secret_name_filtered(self):
        with patch.dict(os.environ, {
            'OPENAI_API_KEY': 'do-not-copy', 'AWS_ACCESS_KEY_ID': 'do-not-copy',
            'AZURE_CLIENT_SECRET': 'do-not-copy', 'GOOGLE_APPLICATION_CREDENTIALS': '/live',
            'ODD_VENDOR_KEY': 'do-not-copy', 'HERMES_HOME': '/live',
            'HERMES_PROFILE': 'live', 'HERMES_FALLBACK_MODEL': 'external',
            'AUXILIARY_BASE_URL': 'https://external.invalid', 'PYTHONPATH': '/live',
        }):
            env = isolated_env(Path('/synthetic'))
        self.assertFalse(any('do-not-copy' in v or '/live' in v for v in env.values()))
        self.assertEqual(env['HERMES_HOME'], '/synthetic/hermes')
        self.assertEqual(env['KEYZAPPER_HOME'], '/synthetic/keyzapper')
        self.assertNotIn('HERMES_PROFILE', env)
        self.assertNotIn('PYTHONPATH', env)

    def test_failures_never_reproduce_child_diagnostics(self):
        self.assertEqual(safe_failure('parser', RuntimeError('Bearer do-not-print /secret')),
                         {'stage': 'parser', 'error_type': 'RuntimeError'})

    def test_home_guard_uses_environment_root_and_refuses_reuse(self):
        root = Path(os.environ['TMPDIR']).resolve()
        with tempfile.TemporaryDirectory(dir=root) as temporary:
            parent = Path(temporary)
            for unsafe in (parent, root, root.parent, Path.home(), Path('relative')):
                with self.subTest(case=str(unsafe)):
                    with self.assertRaises(ValueError):
                        prepare_home(unsafe)
            child = parent / 'fresh'
            self.assertEqual(prepare_home(child), child)
            self.assertTrue(child.is_dir())
            (child / 'sentinel').touch()
            with self.assertRaises(ValueError):
                prepare_home(child)
            link = parent / 'link'
            link.symlink_to(child, target_is_directory=True)
            with self.assertRaises(ValueError):
                prepare_home(link / 'other')
            with self.assertRaises(ValueError):
                prepare_home(parent / '..' / 'escape')

    def test_request_contract(self):
        for path, bearer, body, expected in [
            ('/messages', 'Bearer synthetic-access', {'model': 'team/mock'}, 404),
            ('/v1/messages', 'Bearer wrong', {'model': 'team/mock'}, 401),
            ('/v1/messages', 'Bearer synthetic-access', {'model': 'wrong'}, 400),
            ('/v1/messages', 'Bearer synthetic-access', {'model': 'team/mock'}, 200),
        ]:
            with self.subTest(status=expected):
                self.assertEqual(validate_request(path, bearer, body), expected)


class ContractFailure(RuntimeError):
    def __init__(self, code, observations=None):
        self.code = code
        self.observations = observations or {}


def require(condition, stage):
    if not condition:
        raise ContractFailure(stage)


def cli_roundtrip(invoke, provider):
    """The four scoped fields only; caller distinguishes real CLI from sequencing fixtures."""
    fields = {'providers.keyzapper': provider, 'model.provider': 'keyzapper',
              'model.default': 'team/mock', 'model.base_url': provider['base_url']}
    for key, value in fields.items():
        # Hermes preserves string argv verbatim. JSON-quoting a scalar stores the quotes!
        argument = value if isinstance(value, str) else json.dumps(value)
        rc, _, _ = invoke(['config', 'set', key, argument])
        require(rc == 0, 'config_set_exit')
        rc, out, _ = invoke(['config', 'get', '--json', key])
        require(rc == 0 and json.loads(out) == value, 'config_get_readback')
    for key in reversed(fields):
        rc, _, _ = invoke(['config', 'unset', key])
        require(rc == 0, 'config_unset_exit')
    rc, _, _ = invoke(['config', 'get', '--json', 'providers.keyzapper'])
    require(rc != 0, 'provider_absent_after_unset')
    return True


def captured(command, env, cwd, timeout=30):
    """No child diagnostics/credential stdout is ever emitted or persisted."""
    import signal
    with subprocess.Popen(command, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True) as child:
        try:
            out, err = child.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise ContractFailure('child_timeout') from None
    return child.returncode, out, err


def seed_sso(home, base):
    folder = home / 'keyzapper'
    profile = {'id': PROFILE, 'name': 'Synthetic Task8', 'endpoint': base + '/v1',
        'authType': 'oidc', 'oidcIssuer': base + '/issuer', 'oidcClientId': 'synthetic-client',
        'oidcScope': 'openid profile offline_access',
        'credential': {'service': 'KeyZapper.LiteLLM', 'account': PROFILE}}
    data = json.dumps({'schemaVersion': 1, 'profiles': [profile], 'bindings': []})
    for name in ('state.json', 'state.full.json'):
        (folder / name).write_text(data)
    (folder / 'sso-tokens.json').write_text(json.dumps({PROFILE: {
        'refreshToken': 'synthetic-refresh-0', 'accessToken': 'synthetic-expired',
        'expiresAt': int(time.time()) - 10, 'tokenType': 'access', 'expiryVerified': True}}))


def native_case(hermes, case, env, home):
    # Use the installed dependency interpreter, not a fake Hermes implementation. The selected
    # launcher is independently exercised below; a source test cannot waive a broken CLI contract.
    rc, out, err = captured([env.get('CONTRACT_PYTHON', sys.executable), '-I', str(Path(__file__).resolve()),
                             '--native-case', case], env, home / 'user', 45)
    try:
        result = json.loads(out)
    except ValueError:
        data = (out + err).lower()
        categories = {name: needle in data for name, needle in {
            'missing_module': b'modulenotfounderror', 'missing_dependency': b'dependenc',
            'permission_denied': b'permission', 'missing_file': b'no such file',
            'runtime_install_required': b'install', 'runpy_usage': b'runpy.py',
        }.items()}
        raise ContractFailure('native_result_invalid', {'exit': rc, **categories}) from None
    if rc != 0 or not result.get('passed'):
        raise ContractFailure('native_' + case, result)
    return result


def native_main(case):
    # This is a test bootstrap in the actual installed interpreter/modules. No source patches.
    sys.dont_write_bytecode = True
    sys.path.insert(0, os.environ['CONTRACT_SOURCE_ROOT'])
    scratch = Path(os.environ['HOME']).parent.resolve()
    port = int(os.environ['CONTRACT_BASE'].rsplit(':', 1)[1])
    import pwd
    real_home = Path(pwd.getpwuid(os.getuid()).pw_dir).resolve()
    source_root = Path(os.environ['CONTRACT_SOURCE_ROOT']).resolve()
    readable_code = (source_root, Path(sys.prefix).resolve(), Path(sys.base_prefix).resolve(),
                     source_root.parent / 'installs', source_root.parent / 'tools', Path(__file__).resolve())

    def audit(event, args):
        if event == 'socket.connect':
            address = args[1]
            if isinstance(address, tuple) and address[:2] != ('127.0.0.1', port):
                raise PermissionError('nonlocal network denied')
        if event == 'open' and isinstance(args[0], (str, bytes)):
            path = Path(os.fsdecode(args[0])).resolve()
            in_scratch = path == scratch or scratch in path.parents
            sensitive = path.name in {'.env', '.op.env', 'auth.json', 'config.yaml',
                                     'sso-tokens.json', 'keys.json', 'SOUL.md', 'USER.md', 'MEMORY.md'}
            writing = bool(args[2] & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC))
            private_user_file = (path == real_home or real_home in path.parents) and not any(
                path == root or root in path.parents for root in readable_code)
            if path != Path(os.devnull) and not in_scratch and (sensitive or writing or private_user_file):
                raise PermissionError('outside scratch file denied')

    sys.addaudithook(audit)
    if case == 'identity':
        import hermes_bootstrap
        import importlib.util
        from contextlib import redirect_stdout
        from hermes_cli._startup_fast import print_fast_version_info
        output = io.StringIO()
        with redirect_stdout(output):
            print_fast_version_info(check_updates=False)
        print(json.dumps({'passed': True, 'source_root': str(Path(hermes_bootstrap.__file__).parent),
                          'python': sys.version.split()[0], 'version_line': output.getvalue().splitlines()[0],
                          'anthropic_sdk_available': importlib.util.find_spec('anthropic') is not None}))
        return 0
    if case == 'cli':
        sys.argv = ['hermes', *sys.argv[3:]]
        from hermes_cli.main import main
        return main() or 0
    names = {'parser': 'test_actual_parser_formats_and_redacted_failure',
             'rotation': 'test_same_live_source_calls_real_helper_after_expiry',
             'adapter': 'test_native_adapter_json_stream_and_tool_parsing_without_execution',
             'isolation': 'test_resolved_scratch_config_has_no_fallback_or_tools',
             'stored_absence': 'test_cli_unset_really_removed_stored_fields'}
    suite = unittest.TestSuite([NativeContractTests(names[case])])
    class SafeResult(unittest.TestResult):
        def __init__(self):
            super().__init__()
            self.error_sites = []

        def addError(self, test, err):
            import traceback
            frames = traceback.extract_tb(err[2])
            last = frames[-1]
            self.error_sites.append({'type': err[0].__name__ if err[0] else 'UnknownError', 'file': Path(last.filename).name,
                                     'line': last.lineno, 'function': last.name})
            super().addError(test, err)

    from contextlib import redirect_stdout, redirect_stderr
    result = SafeResult()
    with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
        suite.run(result)
    print(json.dumps({'passed': result.wasSuccessful(), 'tests': result.testsRun,
                      'failures': len(result.failures), 'errors': len(result.errors),
                      'skipped': len(result.skipped), 'error_sites': result.error_sites}))
    return 0 if result.wasSuccessful() else 1


def developer_runtime(executable, version):
    """Bind the standard console script to its interpreter and reported public source root."""
    lines = version.splitlines()
    roots = [line.removeprefix('Install directory: ') for line in lines
             if line.startswith('Install directory: ')]
    require(len(roots) == 1, 'developer_install_directory_unavailable')
    root = Path(roots[0])
    require(root.is_absolute() and root.resolve() == root
            and (root / 'hermes_bootstrap.py').is_file(), 'developer_install_directory_invalid')
    script = executable.read_text()
    require(len(script) < 4096 and 'from hermes_cli.main import main' in script,
            'developer_entrypoint_unrecognized')
    python = Path(script.splitlines()[0].removeprefix('#!'))
    require(python.is_absolute() and python.is_file() and os.access(python, os.X_OK),
            'developer_interpreter_unavailable')
    return python, root


def require_container_isolation():
    # A parent audit hook does not constrain detached native gateways. Require the approved
    # outer, read-only / network-none sandbox before starting the selected CLI chat.
    require(Path('/.dockerenv').is_file(), 'chat_requires_isolated_container')
    interfaces = {line.split(':')[0].strip() for line in Path('/proc/net/dev').read_text().splitlines()[2:]}
    require(interfaces == {'lo'}, 'chat_requires_network_none')
    for path in ('/', '/repo', '/artifacts/keyzapper-helper'):
        require(bool(os.statvfs(path).f_flag & os.ST_RDONLY), 'chat_requires_readonly_mounts')


def opt_in(options):
    home = prepare_home(options.home)
    os.umask(0o077)
    for part in ('user', 'hermes', 'keyzapper', 'tmp'):
        (home / part).mkdir(mode=0o700)
    env = isolated_env(home)
    require_container_isolation()
    env.pop('HERMES_SAFE_MODE', None)
    hermes, helper = Path(options.hermes), Path(options.helper)
    require(hermes.is_absolute() and hermes.is_file() and os.access(hermes, os.X_OK), 'hermes_path')
    require(helper.is_absolute() and helper.name == 'keyzapper-helper'
            and helper.is_file() and os.access(helper, os.X_OK), 'helper_path')
    require(hashlib.sha256(helper.read_bytes()).hexdigest() == HELPER_SHA256, 'helper_fingerprint')
    results = {'harness_status': 'partial', 'helper_fingerprint_verified': True, 'fixtures': 'declared synthetic, not model output',
               'full_pass': False, 'checks': {}}
    stage = 'initialization'
    server = None
    try:
        with MockGateway() as server:
            env.update(CONTRACT_HELPER=str(helper), CONTRACT_BASE=server.base)
            # Deny proxy fallback; all intended HTTP destinations are numeric loopback.
            env.update(HTTP_PROXY=server.base, HTTPS_PROXY=server.base, ALL_PROXY=server.base,
                       NO_PROXY='127.0.0.1,localhost')
            (home / 'hermes/config.yaml').write_text(json.dumps(scratch_settings(server.base)))
            stage = 'identity'
            # Public read-only launcher boundary, before any provider/config imports.
            rc, command_json, _ = captured([str(hermes), '--print-runtime-command'], env, home / 'user')
            version_rc, version, _ = captured([str(hermes), '--version'], env, home / 'user')
            if rc == 0:
                command = json.loads(command_json)
                import ast
                tree = ast.parse(command[3])
                roots = [node.args[1].value for node in ast.walk(tree)
                         if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                         and node.func.attr == 'insert' and len(node.args) == 2
                         and isinstance(node.args[1], ast.Constant) and isinstance(node.args[1].value, str)]
                require(len(roots) == 1 and Path(roots[0]).is_absolute(), 'source_discovery')
                env['CONTRACT_SOURCE_ROOT'] = roots[0]
                env['CONTRACT_PYTHON'] = command[0]
            else:
                require(version_rc == 0, 'selected_launcher_version_failed')
                python, root = developer_runtime(hermes, version.decode())
                env['CONTRACT_SOURCE_ROOT'] = str(root)
                env['CONTRACT_PYTHON'] = str(python)
            env['PATH'] = str(Path(env['CONTRACT_PYTHON']).parent) + ':/usr/bin:/bin'
            identity = native_case(hermes, 'identity', env, home)
            root = Path(identity['source_root'])
            # Native CLI unconditionally considers its developer dotenv. Refuse it without reading.
            require(not any((root / name).exists() for name in ('.env', '.op.env')), 'source_dotenv_present')
            results['source_commit'] = None  # Archives have no git identity; hashes below bind the source.
            results['launcher_version_exit'] = version_rc
            results['version_line'] = identity['version_line']
            results['version_provenance'] = 'selected launcher and bound installed interpreter'
            results['selected_executable'] = str(hermes)
            results['source_root'] = str(root)
            results['anthropic_sdk_available'] = identity['anthropic_sdk_available']
            results['python'] = identity['python']
            results['source_hashes'] = {name: hashlib.sha256((root / name).read_bytes()).hexdigest()
                for name in ('agent/command_token_source.py', 'agent/anthropic_adapter.py',
                             'hermes_cli/config.py', 'hermes_cli/main.py')}
            results['checks']['identity'] = True
            stage = 'safe_local_capability_diagnostic'
            fixture_path = Path(__file__).parents[1] / 'core/tests/fixtures/hermes-compatibility.json'
            probe_path = Path(__file__).parents[1] / 'core/src/hermes/compatibility_probe.py'
            rc, out, _ = captured([env['CONTRACT_PYTHON'], '-I', str(probe_path),
                                   str(root), str(fixture_path)], env, home / 'user')
            probe = json.loads(out)
            require(rc == 0 and probe.get('passed') is True, 'safe_local_capability_diagnostic')
            results['local_diagnostic'] = probe
            results['entrypoint_sha256'] = hashlib.sha256(hermes.read_bytes()).hexdigest()
            results['checks']['safe_local_capability_diagnostic'] = True
            stage = 'parser'
            native_case(hermes, 'parser', env, home)
            results['checks']['native_parser'] = True
            stage = 'concurrent_helper'
            seed_sso(home, server.base)
            command = [str(helper), 'credential', '--profile', PROFILE, '--json']
            children = [subprocess.Popen(command, env=env, cwd=home / 'user', stdin=subprocess.DEVNULL,
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(2)]
            try:
                pairs = [child.communicate(timeout=20) for child in children]
                require(all(child.returncode == 0 for child in children), 'helper_exit')
                payloads = [json.loads(out) for out, _ in pairs]
                require(all(set(p) == {'access_token', 'expiry'} for p in payloads), 'helper_json')
                require(payloads[0] == payloads[1] and server.refreshes == 1, 'helper_single_refresh')
                require(all(datetime.fromisoformat(p['expiry'].replace('Z', '+00:00')).timestamp() > time.time()
                            for p in payloads), 'helper_absolute_expiry')
                stored = json.loads((home / 'keyzapper/sso-tokens.json').read_text())[PROFILE]
                require(stored['accessToken'] == payloads[0]['access_token']
                    and stored['expiryVerified'] is True
                    and stored['expiresAt'] == int(datetime.fromisoformat(
                        payloads[0]['expiry'].replace('Z', '+00:00')).timestamp()), 'helper_snapshot_expiry')
                require((home / 'keyzapper/sso-tokens.json').stat().st_mode & 0o777 == 0o600,
                        'helper_private_store')
            finally:
                for child in children:
                    if child.poll() is None:
                        child.kill()
                        child.communicate()
            rc, bare, _ = captured(command[:-1], env, home / 'user')
            require(rc == 0 and bare.decode().strip() == payloads[0]['access_token'], 'real_helper_bare')
            missing = command.copy()
            missing[3] = '00000000-0000-0000-0000-000000000000'
            rc, out, _ = captured(missing, env, home / 'user')
            require(rc == 65 and not out, 'real_helper_error')
            results['checks']['concurrent_real_helper'] = True
            results['concurrent_refresh_count'] = server.refreshes
            stage = 'same_live_source'
            native_case(hermes, 'rotation', env, home)
            require(server.refreshes == 2, 'same_source_refresh_count')
            results['checks']['same_live_source_real_helper_rotation'] = True
            stage = 'isolation'
            native_case(hermes, 'isolation', env, home)
            results['checks']['resolved_fallback_and_tools_disabled'] = True
            stage = 'adapter'
            results['blockers'] = []
            if not identity['anthropic_sdk_available']:
                results['blockers'].append('installed_interpreter_missing_anthropic_sdk_no_install_permitted')
                results['checks']['native_json_sse_tool_parser_no_execution'] = False
            else:
                native_case(hermes, 'adapter', env, home)
                server.tool = True
                native_case(hermes, 'adapter', {**env, 'CONTRACT_TOOL': '1'}, home)
                server.tool = False
                require(len(server.requests) == 4 and all(r['accepted'] for r in server.requests), 'adapter_wire')
                results['checks']['native_json_sse_tool_parser_no_execution'] = True
            stage = 'cli_roundtrip_and_chat'
            provider = {'base_url': server.base + '/v1', 'api_mode': 'anthropic_messages',
                        'key_cmd': shlex.quote(str(helper)) + f' credential --profile {PROFILE} --json'}
            # Supplemental actual CLI entrypoint exercise; NEVER substitutes for the selected
            # launcher. This uses installed Python/source, not a test double and not config mocks.
            def source_cli(args):
                return captured([env['CONTRACT_PYTHON'], '-I', str(Path(__file__).resolve()),
                                 '--native-case', 'cli', *args], env, home / 'user')
            rc, _, _ = source_cli(['config', 'set', 'model.default', json.dumps('team/mock')])
            require(rc == 0, 'quoted_scalar_probe_set')
            rc, out, _ = source_cli(['config', 'get', '--json', 'model.default'])
            require(rc == 0 and json.loads(out) == json.dumps('team/mock'), 'quoted_scalar_is_literal')
            results['checks']['native_cli_keeps_json_scalar_quotes_literal'] = True
            results['checks']['installed_python_cli_roundtrip_supplemental'] = cli_roundtrip(source_cli, provider)
            native_case(hermes, 'stored_absence', env, home)
            results['checks']['installed_python_cli_unset_stored_absence'] = True
            rc, out, err = captured([str(hermes), 'config', 'path'], env, home / 'user')
            if rc != 0:
                results['blockers'].append('selected_launcher_no_committed_dependency_environment_in_fresh_home'
                    if b'no dependency environment is committed for this install' in err
                    else 'selected_launcher_config_path_failed')
                raise ContractFailure('isolated_cli_bootstrap_failed', {'exit': rc})
            require(Path(out.decode().strip()) == home / 'hermes/config.yaml', 'cli_home')
            results['checks']['selected_launcher_roundtrip'] = cli_roundtrip(
                lambda args: captured([str(hermes), *args], env, home / 'user'), provider)
            native_case(hermes, 'stored_absence', env, home)
            results['checks']['selected_launcher_unset_stored_absence'] = True
            require_container_isolation()
            env['HERMES_ALLOW_ROOT_GATEWAY'] = '1'  # Approved only inside this ephemeral sandbox.
            # In this Hermes build SAFE_MODE bypasses profile config entirely, including the
            # provider under test. The outer sandbox + resolved empty toolsets are the boundary.
            env.pop('HERMES_SAFE_MODE', None)
            # Configure exclusively through the selected native entrypoint, then exercise its
            # real gateway-backed no-tool chat. The container owns every detached descendant.
            fields = {'providers.keyzapper': provider, 'model.provider': 'keyzapper',
                      'model.default': 'team/mock', 'model.base_url': provider['base_url']}
            for key, value in fields.items():
                rc, _, _ = captured([str(hermes), 'config', 'set', key,
                    value if isinstance(value, str) else json.dumps(value)], env, home / 'user')
                require(rc == 0, 'chat_config_set')
            before = len(server.requests)
            try:
                rc, out, err = captured([str(hermes), 'chat', '--oneshot', '-Q', '--max-turns', '4',
                    '--run-budget', '60', '-q', 'Return the synthetic contract fixture; do not use tools.'],
                    env, home / 'user', 90)
                results['chat_exit'] = rc
                if rc != 0:
                    import re
                    diagnostic = (out + err).decode(errors='replace')
                    # Scratch-only CLI diagnostics: redact every known credential before selecting
                    # error lines. Never expose helper stdout, bodies or Authorization headers.
                    for secret in (server.expected, server.refresh, 'synthetic-expired'):
                        diagnostic = diagnostic.replace(secret, '[redacted]')
                    diagnostic = re.sub(r'synthetic-(?:access|refresh)-\d+', '[redacted]', diagnostic)
                    results['chat_diagnostic_tail'] = [line[:500] for line in diagnostic.splitlines()[-18:]
                        if not any(marker in line.lower() for marker in ('bearer', 'authorization', 'key_cmd'))]
                    results['chat_error_lines'] = [line[:500] for line in diagnostic.splitlines()
                        if any(marker in line.lower() for marker in ('error', 'failed', '✗', 'unsupported', 'refus', 'gateway', 'provider', 'configur', 'install', 'cannot', 'missing'))
                        and not any(marker in line.lower() for marker in ('bearer', 'authorization', 'key_cmd'))][-8:]
                    results['chat_error_sites'] = [
                        {'file': Path(file).name, 'line': int(line), 'function': function}
                        for file, line, function in re.findall(r'File "([^"]+)", line (\d+), in ([\w<>]+)', diagnostic)]
                    results['chat_error_types'] = re.findall(r'^([\w.]+(?:Error|Exception)):', diagnostic, re.M)
                    results['chat_error_categories'] = {word: word.lower() in diagnostic.lower() for word in
                        ('unrecognized arguments', 'connection refused', 'gateway', 'permission denied',
                         'No module named', 'api key', 'provider', 'proxy', 'timeout', 'read-only')}
                    logs = home / 'hermes/logs'
                    results['gateway_log_error_types'] = []
                    results['gateway_log_diagnostic_tail'] = {}
                    for log in logs.glob('*.log'):
                        text = log.read_text(errors='replace')
                        for secret in (server.expected, server.refresh, 'synthetic-expired'):
                            text = text.replace(secret, '[redacted]')
                        text = re.sub(r'synthetic-(?:access|refresh)-\d+', '[redacted]', text)
                        results['gateway_log_diagnostic_tail'][log.name] = [line[:500] for line in text.splitlines()[-12:]
                            if not any(marker in line.lower() for marker in ('bearer', 'authorization', 'key_cmd'))]
                        results['gateway_log_error_types'].extend(re.findall(r'^([\w.]+(?:Error|Exception)):', text, re.M))
                        results['chat_error_sites'].extend([
                            {'file': Path(file).name, 'line': int(line), 'function': function}
                            for file, line, function in re.findall(r'File "([^"]+)", line (\d+), in ([\w<>]+)', text)])
                results['chat_fixture_received'] = b'SYNTHETIC CONTRACT OK' in out
                observed = server.requests[before:]
                results['chat_observations'] = observed
                require(rc == 0 and results['chat_fixture_received'] and observed
                        and all(r['accepted'] and r['tools_absent'] for r in observed), 'selected_chat_wire')
                results['checks']['selected_launcher_chat'] = True
            finally:
                stop_rc, _, _ = captured([str(hermes), 'gateway', 'stop'], env, home / 'user', 30)
                results['gateway_stop_exit'] = stop_rc
            require(stop_rc == 0, 'gateway_cleanup')
            results['full_pass'] = (version_rc == 0 and not results['blockers']
                and all(results['checks'].values()) and server.other_requests == 0)
            results['harness_status'] = 'passed' if results['full_pass'] else 'partial'
    except Exception as error:
        results['failure'] = safe_failure(stage, error)
    if server is not None:
        results['gateway_request_count'] = len(server.requests)
        results['gateway_accepted_count'] = sum(r['accepted'] for r in server.requests)
        results['idp_refresh_count'] = server.refreshes
        results['idp_discovery_count'] = server.discovery_count
        results['unexpected_http_count'] = server.other_requests
        results['local_metadata_request_count'] = server.metadata_requests
        results['unexpected_routes'] = server.other_routes
    print(json.dumps(results, indent=2))
    (home / 'result.json').write_text(json.dumps(results, indent=2) + '\n')
    return 0 if results['full_pass'] else 1


if __name__ == '__main__':
    if sys.argv[1:2] == ['--native-case']:
        try:
            sys.exit(native_main(sys.argv[2]))
        except Exception as error:
            print(json.dumps({'passed': False, **safe_failure('native_bootstrap', error)}))
            sys.exit(1)
    elif '--hermes' in sys.argv:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--hermes', required=True)
        parser.add_argument('--helper', required=True)
        parser.add_argument('--home', required=True, help='unique NEW child under trusted $TMPDIR')
        try:
            sys.exit(opt_in(parser.parse_args()))
        except Exception as error:
            print(json.dumps({'full_pass': False, **safe_failure('preflight', error)}))
            sys.exit(1)
    else:
        unittest.main()
