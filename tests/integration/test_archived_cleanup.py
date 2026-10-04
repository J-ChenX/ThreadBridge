"""Archived replicas are removed by native ID; active equal titles stay independent."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import uuid


def main():
    binary = str(Path(sys.argv[1]).resolve())
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        env = dict(os.environ, HOME=directory, CODEX_HOME=directory)

        def run(*args):
            return subprocess.run([binary, *map(str, args)], check=True, env=env, capture_output=True, text=True).stdout.strip()

        hub, capture, index = [root / name for name in ['hub.sqlite', 'capture.sqlite', 'index.sqlite']]
        agent = root / 'agent.json'
        run('register-agent', '--db', hub, '--name', 'fixture', '--output', agent)
        host = json.loads(agent.read_text())['host_id']
        active1, active2, archived = [str(uuid.uuid4()) for _ in range(3)]
        with sqlite3.connect(index) as c:
            c.execute('CREATE TABLE threads(id TEXT PRIMARY KEY,archived INTEGER,thread_source TEXT)')
            c.executemany('INSERT INTO threads VALUES(?,?,?)', [(active1, 0, 'user'), (active2, 0, 'agent_created_thread'), (archived, 1, 'agent_created_thread')])
        for native in [active1, active2, archived]:
            event = {'type': 'agent-turn-complete', 'thread-id': native, 'turn-id': str(uuid.uuid4()), 'last-assistant-message': 'saved '+native}
            run('capture', '--all-tasks', '--database', capture, '--title', 'same title', json.dumps(event))
            run('capture-import', '--db', hub, '--capture-db', capture, '--host', host, '--thread', native)
        args = ['cleanup-archived', '--db', hub, '--capture-db', capture, '--index', index, '--host', host]
        # Missing/corrupt policy is refused during planning and before apply
        # creates a backup or mutates either replica.
        policy_path = Path(str(capture)+'.collection.json')
        for policy in [None, '{invalid-json']:
            if policy is not None:
                policy_path.write_text(policy)
            for command in [args, [*args, '--apply', '--backup-dir', root/'refused-backup']]:
                result = subprocess.run([binary, *map(str, command)], env=env, capture_output=True, text=True)
                assert result.returncode != 0
                if policy is None:
                    assert 'collection_policy_required' in result.stderr
                assert not (root/'refused-backup').exists()
                with sqlite3.connect(hub) as c:
                    assert c.execute('SELECT count(*) FROM threads').fetchone()[0] == 3
                with sqlite3.connect(capture) as c:
                    assert c.execute('SELECT count(*) FROM captured_replies').fetchone()[0] == 3
        policy_path.write_text(json.dumps({'schema': 1, 'generation': str(uuid.uuid4()), 'cutoff_ms': 1, 'excluded': []}))
        plan = json.loads(run(*args))
        assert plan['archived_threads'] == 1 and plan['native_ids'] == [archived]
        with sqlite3.connect(hub) as c:
            before = c.execute('SELECT m.* FROM messages m JOIN threads t ON t.id=m.thread WHERE t.native!=? ORDER BY m.thread,m.id', (archived,)).fetchall()
        assert json.loads(run(*args, '--apply', '--backup-dir', root / 'backup'))['archived_threads'] == 1
        with sqlite3.connect(hub) as c:
            assert {r[0] for r in c.execute('SELECT native FROM threads')} == {active1, active2}
            assert c.execute('SELECT m.* FROM messages m JOIN threads t ON t.id=m.thread ORDER BY m.thread,m.id').fetchall() == before
            assert c.execute("SELECT count(*) FROM threads WHERE title='same title'").fetchone()[0] == 2
        with sqlite3.connect(capture) as c:
            assert {r[0] for r in c.execute('SELECT thread_id FROM captured_replies')} == {active1, active2}
        with sqlite3.connect(index) as c:
            assert c.execute('SELECT archived FROM threads WHERE id=?', (archived,)).fetchone()[0] == 1
        event['thread-id'] = archived
        assert run('capture', '--all-tasks', '--database', capture, '--user-turn-index', index, json.dumps(event)) == 'ignored'
        assert json.loads(run(*args))['archived_threads'] == 0
        print('archived cleanup: retired replica removed, active equal-title IDs and messages preserved, native archive unchanged')


if __name__ == '__main__':
    main()
