"""Public-source and real SDK diagnostic. No credentials, helpers, tools or network.

Invoked with an allowlisted manifest by the native compatibility verifier in a fresh home.
The full native CLI/gateway acceptance lives in test_hermes_contract.py, not this probe.
"""
import hashlib
import json
import os
from pathlib import Path
import sys


def source_fingerprint(root, files):
    root = root.resolve(strict=True)
    digest = hashlib.sha256()
    for name in sorted(files):
        relative = Path(name)
        path = root / relative
        if relative.is_absolute() or '..' in relative.parts or not path.resolve(strict=True).is_relative_to(root):
            raise ValueError('non-public source path')
        digest.update(name.encode() + b'\0' + hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def diagnose(root, manifest):
    # Audit before importing any installed modules. MockTransport never opens a socket.
    def audit(event, args):
        if event in ('socket.connect', 'socket.getaddrinfo', 'subprocess.Popen'):
            raise PermissionError('network and subprocesses forbidden in compatibility diagnostic')
        if event == 'open' and isinstance(args[0], (str, bytes)):
            path = Path(os.fsdecode(args[0]))
            if path.name in ('.env', '.op.env', 'auth.json', 'sso-tokens.json', 'keys.json'):
                raise PermissionError('private files forbidden in compatibility diagnostic')
    sys.addaudithook(audit)
    if any((root / name).exists() for name in ('.env', '.op.env')):
        raise ValueError('developer dotenv present')
    tree = source_fingerprint(root, manifest['publicSourceFiles'])
    if not any(build.get('passed') is True and build.get('fullPass') is True
               and build.get('sourceTreeSha256') == tree for build in manifest.get('supportedBuilds', [])):
        raise ValueError('unverified public source')
    # Never execute an existing installation's bytecode instead of verified source bytes.
    sys.pycache_prefix = str(Path(os.environ['TMPDIR']) / 'verified-bytecode')
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(root))
    import hermes_bootstrap
    if Path(hermes_bootstrap.__file__).resolve().parent != root:
        raise ValueError('runtime source mismatch')
    import anthropic
    import httpx
    from agent.command_token_source import CommandTokenSource
    from agent.anthropic_adapter import create_anthropic_message
    assert callable(CommandTokenSource)
    seen = []
    tool = {'type': 'tool_use', 'id': 'probe-call', 'name': 'contract_echo', 'input': {'text': 'harmless fixture'}}
    message = {'id': 'msg_probe', 'type': 'message', 'role': 'assistant', 'model': 'team/mock',
               'content': [tool], 'stop_reason': 'tool_use', 'stop_sequence': None,
               'usage': {'input_tokens': 1, 'output_tokens': 1}}

    def handle(request):
        body = json.loads(request.content)
        assert request.url.path == '/v1/messages'
        assert request.headers.get('authorization') == 'Bearer synthetic-probe'
        assert body['model'] == 'team/mock' and not body.get('tools')
        seen.append(bool(body.get('stream')))
        if not body.get('stream'):
            return httpx.Response(200, json=message)
        events = [
            {'type': 'message_start', 'message': {**message, 'content': [], 'stop_reason': None}},
            {'type': 'content_block_start', 'index': 0, 'content_block': {**tool, 'input': {}}},
            {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'input_json_delta', 'partial_json': json.dumps(tool['input'])}},
            {'type': 'content_block_stop', 'index': 0},
            {'type': 'message_delta', 'delta': {'stop_reason': 'tool_use', 'stop_sequence': None}, 'usage': {'output_tokens': 1}},
            {'type': 'message_stop'},
        ]
        data = ''.join('event: ' + e['type'] + '\ndata: ' + json.dumps(e) + '\n\n' for e in events)
        return httpx.Response(200, text=data, headers={'content-type': 'text/event-stream'})

    with anthropic.Anthropic(auth_token='synthetic-probe', base_url='http://127.0.0.1',
            max_retries=0, http_client=httpx.Client(transport=httpx.MockTransport(handle))) as client:
        for stream in (False, True):
            parsed = create_anthropic_message(client, {'model': 'team/mock', 'max_tokens': 32,
                'messages': [{'role': 'user', 'content': 'declared fixture'}]}, prefer_stream=stream)
            assert parsed.stop_reason == 'tool_use' and parsed.content[0].name == 'contract_echo'
            assert parsed.content[0].input == {'text': 'harmless fixture'}
    assert seen == [False, True]
    return {'passed': True, 'sourceTreeSha256': tree, 'anthropicVersion': anthropic.__version__,
            'pythonVersion': sys.version.split()[0], 'sdkJsonSseToolParsing': True}


if __name__ == '__main__':
    try:
        root = Path(sys.argv[1]).resolve(strict=True)
        manifest = json.loads(Path(sys.argv[2]).read_text())
        print(json.dumps(diagnose(root, manifest)))
    except BaseException as error:
        # Do not reflect source, argv, library diagnostics or any credential output.
        print(json.dumps({'passed': False, 'errorType': type(error).__name__}))
        sys.exit(1)
