"""Native filtering and targeted replica cleanup, including stale capture health."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid


def main():
    binary = str(Path(sys.argv[1]).resolve())
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        env = dict(os.environ, HOME=directory, CODEX_HOME=directory)

        def run(*args):
            return subprocess.run([binary, *map(str, args)], env=env, check=True,
                                  capture_output=True, text=True).stdout.strip()

        hub, capture, index = [root / name for name in ['hub.sqlite', 'capture.sqlite', 'index.sqlite']]
        agent = root / 'agent.json'
        run('register-agent', '--db', hub, '--name', 'fixture', '--output', agent)
        host = json.loads(agent.read_text())['host_id']
        human, created, child = [str(uuid.uuid4()) for _ in range(3)]
        turn = str(uuid.uuid4())
        with sqlite3.connect(index) as c:
            c.execute('CREATE TABLE threads(id TEXT PRIMARY KEY,created_at INTEGER,created_at_ms INTEGER,thread_source TEXT,source TEXT,rollout_path TEXT)')
            for native, kind in [(human, 'user'), (created, 'agent_created_thread'), (child, 'subagent')]:
                c.execute('INSERT INTO threads VALUES(?,?,?,?,?,?)', (native, int(time.time()), int(time.time()*1000), kind, '{}', str(root / 'missing-rollout')))
        event = {'type': 'agent-turn-complete', 'thread-id': child, 'turn-id': turn, 'last-assistant-message': 'child reply'}
        scope = ['--all-tasks', '--database', capture, '--user-turn-index', index]
        # Filtering precedes transcript reads and health intent creation.
        assert run('capture', *scope, json.dumps(event)) == 'ignored'
        assert not capture.exists() and not Path(str(capture)+'.health').exists()
        for native in [human, created, child]:
            event['thread-id'] = native
            run('capture', '--all-tasks', '--database', capture, json.dumps(event))
            run('capture-import', '--db', hub, '--capture-db', capture, '--host', host, '--thread', native)
        policy = {'schema': 1, 'generation': str(uuid.uuid4()), 'cutoff_ms': 1, 'excluded': []}
        Path(str(capture)+'.collection.json').write_text(json.dumps(policy))
        # Recreate the existing child-only user-input failure from before the fix.
        with sqlite3.connect(index) as c:
            c.execute("UPDATE threads SET thread_source='user' WHERE id=?", (child,))
        event['thread-id'] = child
        event['turn-id'] = str(uuid.uuid4())
        failed = subprocess.run([binary, 'capture', *map(str, scope), json.dumps(event)], env=env, capture_output=True)
        assert failed.returncode == 2
        failures = json.loads(run('capture-health', '--database', capture))['failures']
        assert len(failures) == 1
        assert next(iter(failures.values()))['reason'] == 'user_input_capture_failed'
        event['thread-id'] = human
        failed = subprocess.run([binary, 'capture', *map(str, scope), json.dumps(event)], env=env, capture_output=True)
        assert failed.returncode == 2
        assert len(json.loads(run('capture-health', '--database', capture))['failures']) == 2
        event['thread-id'] = child
        with sqlite3.connect(index) as c:
            c.execute("UPDATE threads SET thread_source='subagent' WHERE id=?", (child,))
        args = ['cleanup-subagents', '--db', hub, '--capture-db', capture, '--index', index, '--host', host]
        plan = json.loads(run(*args))
        assert plan['native_ids'] == [child]
        with sqlite3.connect(hub) as c:
            assert c.execute('SELECT count(*) FROM threads').fetchone()[0] == 3
            identity_before = c.execute('SELECT id,token FROM devices ORDER BY id').fetchall()
            child_key = c.execute('SELECT id FROM threads WHERE native=?', (child,)).fetchone()[0]
            c.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) VALUES('unknown','phone','immutable-request','immutable-digest',?,?,'private child input','unknown',1,1)", (host, child_key))
        result = json.loads(run(*args, '--apply', '--backup-dir', root / 'backup'))
        assert result['subagent_threads'] == 1
        with sqlite3.connect(hub) as c:
            assert {r[0] for r in c.execute('SELECT native FROM threads')} == {human, created}
            assert c.execute('SELECT id,token FROM devices ORDER BY id').fetchall() == identity_before
            assert c.execute("SELECT count(*) FROM events WHERE kind='delete_thread'").fetchone()[0] == 1
            failures = json.loads(c.execute('SELECT status FROM capture_health WHERE host=?', (host,)).fetchone()[0])['failures']
            assert len(failures) == 1 and next(iter(failures.values()))['thread_id'] == human
            assert c.execute("SELECT request,digest,payload,status FROM commands WHERE id='unknown'").fetchone() == ('immutable-request', 'immutable-digest', '{}', 'unknown')
        with sqlite3.connect(capture) as c:
            assert {r[0] for r in c.execute('SELECT thread_id FROM captured_replies')} == {human, created}
        with sqlite3.connect(index) as c:
            assert c.execute('SELECT count(*) FROM threads').fetchone()[0] == 3
        failures = json.loads(run('capture-health', '--database', capture))['failures']
        assert len(failures) == 1 and next(iter(failures.values()))['thread_id'] == human
        assert run('capture', *scope, json.dumps(event)) == 'ignored'
        assert json.loads(run(*args))['subagent_threads'] == 0
        with sqlite3.connect(root / 'backup' / 'hub.sqlite') as c:
            assert c.execute('SELECT count(*) FROM threads').fetchone()[0] == 3
        print('subagent filtering, cleanup, health refresh and native preservation passed')


if __name__ == '__main__':
    main()
