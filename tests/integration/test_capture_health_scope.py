"""Existing phone health interpretation distinguishes offline devices from failures."""
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


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        db = root / 'hub.sqlite'
        subprocess.run([binary, 'register-agent', '--db', db, '--name', 'fixture', '--output', root / 'agent.json'], check=True, stdout=subprocess.DEVNULL)
        with sqlite3.connect(db) as c:
            c.execute('DELETE FROM devices')
            c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)", (hashlib.sha256(b'fixture-phone').hexdigest(), int(time.time())+300))
            for host in ['local', 'remote1', 'remote2', 'remote3']:
                c.execute("INSERT INTO devices(id,role,name,expires) VALUES(?,'agent',?,?)", (host, host, int(time.time())+300))
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        process = subprocess.Popen([binary, 'hub', '--db', db, '--listen', f'127.0.0.1:{port}'], env=dict(os.environ, HOME=directory, CODEX_HOME=directory), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        def get(path):
            req = urllib.request.Request(f'http://127.0.0.1:{port}'+path, headers={'Authorization': 'Bearer fixture-phone'})
            with opener.open(req, timeout=2) as response:
                return json.load(response)

        def write(sql, args=()):
            with sqlite3.connect(db) as c:
                c.execute(sql, args)

        def ids(rows):
            return {row['host_id'] for row in rows}

        try:
            deadline = time.monotonic()+5
            while True:
                try:
                    get('/health')
                    break
                except OSError:
                    if time.monotonic() > deadline:
                        raise
                    time.sleep(.05)
            now = int(time.time())
            healthy = json.dumps({'failures': {}, 'overflow': False})
            for host in ['local', 'remote1', 'remote2', 'remote3']:
                write('INSERT INTO capture_health VALUES(?,?,?)', (host, healthy, now if host == 'local' else now-120))
            write("UPDATE devices SET last_seen=? WHERE id='local'", (now,))
            result = get('/v1/capture-health')
            assert ids(result['hosts']) == {'local'}
            assert ids(result['offline_hosts']) == {'remote1', 'remote2', 'remote3'}
            # These are exactly the conditions used by the installed Android app.
            assert result['hosts']
            assert all(result['server_time']-r['checked_at'] <= 30 for r in result['hosts'])
            assert all(not r['status']['failures'] for r in result['hosts'])
            assert not next(r for r in get('/v1/hosts')['hosts'] if r['id'] == 'remote1')['online']
            assert next(r for r in result['offline_hosts'] if r['host_id'] == 'remote1')['checked_at'] == now-120
            # A real offline failure must still reach the phone, with its original age.
            failed = {'failures': {'turn': {'thread_id': 'thread', 'reason': 'user_input_capture_failed'}}, 'overflow': False}
            write("UPDATE capture_health SET status=? WHERE host='remote1'", (json.dumps(failed),))
            result = get('/v1/capture-health')
            assert ids(result['hosts']) == {'local', 'remote1'}
            assert next(r for r in result['hosts'] if r['host_id'] == 'remote1')['status'] == failed
            for status in [{'failures': {}, 'overflow': True}, {'failures': {}, 'overflow': False, 'projection_error': 'unavailable'}]:
                write("UPDATE capture_health SET status=? WHERE host='remote1'", (json.dumps(status),))
                assert 'remote1' in ids(get('/v1/capture-health')['hosts'])
            write("UPDATE capture_health SET status=? WHERE host='remote1'", (healthy,))
            # Online computers with stale monitoring still warn.
            write("UPDATE capture_health SET checked_at=? WHERE host='local'", (now-120,))
            assert 'local' in ids(get('/v1/capture-health')['hosts'])
            write('UPDATE devices SET last_seen=0')
            result = get('/v1/capture-health')
            assert not result['hosts'] and len(result['offline_hosts']) == 4
            # A fresh read-only monitor remains valid even without a queue heartbeat.
            write("UPDATE capture_health SET checked_at=? WHERE host='remote2'", (now,))
            assert ids(get('/v1/capture-health')['hosts']) == {'remote2'}
            print('capture health scope: existing Android warning clears for healthy local + offline remotes; actual failures and stale online monitors remain visible')
        finally:
            process.terminate()
            process.wait(timeout=3)


if __name__ == '__main__':
    main()
