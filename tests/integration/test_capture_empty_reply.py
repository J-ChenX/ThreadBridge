"""Empty native completions do not poison health; old false alarms need proof."""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import uuid


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        env = dict(os.environ, HOME=directory, CODEX_HOME=directory)
        capture, index, rollout = [root / n for n in ('capture.sqlite', 'index.sqlite', 'rollout.jsonl')]
        native = str(uuid.uuid4())
        turns = [str(uuid.uuid4()) for _ in range(9)]
        with sqlite3.connect(index) as db:
            db.execute('CREATE TABLE threads(id TEXT,rollout_path TEXT,thread_source TEXT,archived INTEGER)')
            db.execute('INSERT INTO threads VALUES(?,?,?,0)', (native, str(rollout), 'user'))

        def run(*args):
            result = subprocess.run([binary, *map(str, args)], env=env, capture_output=True, text=True)
            assert result.returncode == 0, result.stderr
            return result.stdout.strip()

        for reply in (None, ''):
            event = {'type': 'agent-turn-complete', 'thread-id': native,
                     'turn-id': turns[0], 'last-assistant-message': reply}
            assert run('capture', '--all-tasks', '--database', capture,
                       '--user-turn-index', index, json.dumps(event)) == 'ignored_empty_reply'
            assert not capture.exists() and not Path(str(capture)+'.health').exists()

        # Establish a real saved reply; its empty duplicate must not erase it.
        event['turn-id'], event['last-assistant-message'] = turns[7], 'saved final'
        run('capture', '--all-tasks', '--database', capture, json.dumps(event))
        failures = {f'{native}:{turn}': {'thread_id': native, 'turn_id': turn,
                    'reason': 'storage_write_failed' if n == 6 else 'missing_reply_identity',
                    'attempt': f'attempt-{n}'} for n, turn in enumerate(turns)}
        data = json.dumps({'failures': failures, 'overflow': False})
        slot = json.dumps({'generation': 100, 'sha256': hashlib.sha256(data.encode()).hexdigest(), 'data': data}).encode()
        Path(str(capture)+'.health').write_bytes(slot.ljust(65536, b' ')*2)

        rows = [{'type': 'session_meta', 'payload': {'id': native}}]
        for n, turn in enumerate(turns):
            if n != 2:
                rows.append({'type': 'response_item', 'timestamp': '1970-01-01T00:01:40Z',
                             'payload': {'type': 'message', 'id': f'final-{n}', 'role': 'assistant',
                                         'phase': 'final_answer',
                                         'content': [{'type': 'output_text', 'text': 'real final' if n == 4 else ''}],
                                         'internal_chat_message_metadata_passthrough': {'turn_id': turn}}})
            if n != 3:
                payload = {'type': 'task_complete', 'turn_id': turn, 'last_agent_message': '' if n == 1 else None}
                if n == 5:
                    payload['error'] = 'failed'
                if n == 8:
                    del payload['last_agent_message']
                rows.append({'type': 'event_msg', 'timestamp': '1970-01-01T00:01:41Z', 'payload': payload})
        rollout.write_text(''.join(json.dumps(r)+'\n' for r in rows))
        for _ in range(2):
            run('capture-backfill', '--database', capture, '--index', index, '--thread', native)
            state = json.loads(run('capture-health', '--database', capture))
            assert set(state['failures']) == {f'{native}:{t}' for t in turns[2:]}, state
            with sqlite3.connect(capture) as db:
                assert db.execute('SELECT reply FROM captured_replies').fetchall() == [('saved final',)]
        print('empty completion: no persistent warning; history clears only proven false alarms and preserves real/ambiguous failures')


if __name__ == '__main__':
    main()
