"""Native names and metadata-only renames reach the phone API without new messages."""
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
import zlib


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix='tb-thread-titles-') as directory:
        root = Path(directory)
        home = root / 'native'
        rollout = home / 'sessions' / 'fixture.jsonl'
        rollout.parent.mkdir(parents=True)
        index = home / 'state_5.sqlite'
        native, turn = [str(uuid.uuid4()) for _ in range(2)]
        env = dict(os.environ, HOME=directory, CODEX_HOME=str(home))
        mock = root / 'codex'
        mock.symlink_to(Path(__file__).resolve().parents[1] / 'fixtures' / 'mock_codex.py')
        (root / 'fixture.json').write_text(json.dumps({'mode': 'create', 'native_id': native}))
        config = root / 'remote.json'
        config.write_text(json.dumps({'codex': str(mock), 'codex_home': str(home)}))
        records = [
            {'type': 'session_meta', 'payload': {'id': native}},
            {'type': 'response_item', 'payload': {
                'type': 'message', 'role': 'user', 'id': 'human',
                'content': [{'type': 'input_text', 'text': 'human input'}],
                'internal_chat_message_metadata_passthrough': {
                    'turn_id': turn, 'create_time': 1700000000,
                    'content_item_kinds': ['user.text']}}},
            {'type': 'response_item', 'timestamp': '2023-11-14T22:13:21Z', 'payload': {
                'type': 'message', 'role': 'assistant', 'id': 'final', 'phase': 'final_answer',
                'content': [{'type': 'output_text', 'text': 'exact final reply'}],
                'internal_chat_message_metadata_passthrough': {'turn_id': turn}}},
            {'type': 'event_msg', 'timestamp': '2023-11-14T22:13:22Z', 'payload': {
                'type': 'task_complete', 'turn_id': turn, 'last_agent_message': 'exact final reply'}}]
        rollout.write_text(''.join(json.dumps(record) + '\n' for record in records))
        with sqlite3.connect(index) as c:
            c.execute('CREATE TABLE threads(id TEXT,rollout_path TEXT,title TEXT,name TEXT,updated_at INTEGER)')
            c.execute('INSERT INTO threads VALUES(?,?,?,?,1)',
                      (native, str(rollout), '# Files mentioned by the user:\nattachment wrapper', '正确的对话名称'))

        def run(*args, input=None):
            result = subprocess.run([binary, *map(str, args)], input=input, env=env,
                                    capture_output=True, text=True, check=True)
            return result.stdout

        def snapshot(revision=None):
            return json.loads(run('remote-capture', '--config', config,
                                  input=json.dumps({'op': 'snapshot', 'revision': revision})))

        notify = root / 'notify.sqlite'
        run('capture', '--database', notify, '--all-tasks', '--user-turn-index', index,
            json.dumps({'type': 'agent-turn-complete', 'thread-id': native, 'turn-id': turn,
                        'last-assistant-message': 'exact final reply'}))
        with sqlite3.connect(notify) as c:
            assert c.execute('SELECT title FROM captured_replies').fetchone()[0] == '正确的对话名称'
            assert c.execute('SELECT text FROM captured_user_messages').fetchone()[0] == 'human input'

        hubdb = root / 'hub.sqlite'
        run('register-agent', '--db', hubdb, '--name', 'fixture', '--output', root / 'agent.json')
        host = json.loads((root / 'agent.json').read_text())['host_id']
        with sqlite3.connect(hubdb) as c:
            c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",
                      (hashlib.sha256(b'fixture-phone').hexdigest(), int(time.time()) + 300))
        source = root / 'replica.sqlite'

        def import_snapshot(result):
            source.write_bytes(zlib.decompress(base64.b64decode(result['database'])))
            run('capture-import', '--db', hubdb, '--capture-db', source, '--host', host, '--thread', native)

        first = snapshot()
        import_snapshot(first)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        process = subprocess.Popen([binary, 'hub', '--db', hubdb, '--listen', f'127.0.0.1:{port}'],
                                   env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        sync = None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        def get(path):
            request = urllib.request.Request(f'http://127.0.0.1:{port}' + path,
                                             headers={'Authorization': 'Bearer fixture-phone'})
            with opener.open(request, timeout=2) as response:
                return json.load(response)

        def wait_for(predicate):
            deadline = time.monotonic() + 8
            while True:
                try:
                    if predicate():
                        return
                except OSError:
                    pass
                assert time.monotonic() < deadline, 'fixture synchronization timed out'
                time.sleep(.05)

        try:
            wait_for(lambda: get('/health'))
            row = get('/v1/threads')['threads'][0]
            assert row['title'] == '正确的对话名称', row
            tid = row['id']
            messages = get(f'/v1/threads/{tid}/messages')
            cursor = get('/v1/events?after=0')['cursor']
            with sqlite3.connect(index) as c:
                c.execute("UPDATE threads SET name='仅修改名称'")
            renamed = snapshot(first['revision'])
            assert renamed['changed'] and renamed['revision'] != first['revision']
            import_snapshot(renamed)
            current = get('/v1/threads')['threads'][0]
            assert current['id'] == tid and current['title'] == '仅修改名称'
            for field in ['revision', 'updated_at', 'message_revision', 'message_activity_at']:
                assert current[field] == row[field], field
            assert get(f'/v1/threads/{tid}/messages') == messages
            assert get('/v1/events?after=0')['cursor'] == cursor + 1
            repeated = snapshot(renamed['revision'])
            assert not repeated['changed'] and 'database' not in repeated
            run('capture-import', '--db', hubdb, '--capture-db', source, '--host', host, '--thread', native)
            assert get('/v1/events?after=0')['cursor'] == cursor + 1
            # Local capture-sync must also refresh names independently of rollout changes.
            sync = subprocess.Popen([binary, 'capture-sync', '--db', hubdb, '--capture-db', root / 'data/replies.sqlite',
                                     '--host', host, '--all-captured', '--native-index', index],
                                    env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            wait_for(lambda: get('/v1/threads')['threads'][0]['title'] == '仅修改名称')
            with sqlite3.connect(index) as c:
                c.execute("UPDATE threads SET name='本机再次改名'")
            wait_for(lambda: get('/v1/threads')['threads'][0]['title'] == '本机再次改名')
            with sqlite3.connect(hubdb) as c:
                assert c.execute('SELECT count(*) FROM outbox').fetchone()[0] == 0
                assert c.execute('SELECT count(*) FROM commands').fetchone()[0] == 0
            assert not (root / 'calls').exists(), 'no native send or creation is allowed'
            print('PASS native name -> snapshot -> Hub/phone API; rename without messages; local metadata refresh; stable IDs/read state; replay; no send or notification')
        finally:
            if sync is not None:
                sync.terminate()
                sync.wait(timeout=5)
            process.terminate()
            process.wait(timeout=5)


if __name__ == '__main__':
    main()
