"""Desktop remote metadata reaches the real phone API without body snapshot changes."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix='tb-desktop-projects-') as directory:
        root = Path(directory)
        hub = root / 'hub.sqlite'
        env = dict(os.environ, HOME=directory, CODEX_HOME=str(root / 'native'))

        def run(*args):
            return subprocess.check_output([binary, *map(str, args)], env=env, text=True)

        def agent(name):
            path = root / (name + '.json')
            run('register-agent', '--db', hub, '--name', name, '--output', path)
            return json.loads(path.read_text())['host_id']

        executor, desktop = agent('executor'), agent('desktop')
        native = str(uuid.uuid4())
        # Use the exact host/native identity format computed by the production model.
        tid = hashlib.sha256(f'{executor}\0default\0{native}'.encode()).hexdigest()
        with sqlite3.connect(hub) as c:
            c.execute("INSERT INTO threads VALUES(?,?,?,'真实协议合成对话','completed','stable',42,0,NULL)",
                      (tid, executor, native))
            c.execute("INSERT INTO thread_projects VALUES(?,'')", (tid,))
            c.execute("INSERT INTO thread_message_state VALUES(?,7,123)", (tid,))
            c.execute("INSERT INTO messages VALUES(?,'m','turn','assistant','正文保持原样','v',123)", (tid,))
            c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",
                      (hashlib.sha256(b'fixture-phone').hexdigest(), int(time.time()) + 300))
        response_path = root / 'response.json'
        alias = 'remote-ssh-discovered:fixture-server'
        response = {'schema': 1, 'version': 'fixture', 'overflow': False, 'changed': False,
                    'blocked_threads': [], 'revision': 'same-body',
                    'desktop_remote_threads': {native: alias},
                    'desktop_projects': {
                        'projects': [{'host': alias, 'path': '/code/screen_control', 'name': '控屏'}],
                        'assignments': [{'host': alias, 'native': native, 'path': '/code/screen_control'}]}}

        def save():
            temporary = response_path.with_suffix('.next')
            temporary.write_text(json.dumps(response))
            temporary.replace(response_path)

        save()
        ssh = root / 'ssh'
        ssh.write_text('#!' + sys.executable + '\nimport os,sys\nfrom pathlib import Path\n'
                       'sys.stdin.read()\nprint(Path(os.environ["TB_FIXTURE_RESPONSE"]).read_text())\n')
        ssh.chmod(0o700)
        env['PATH'] = str(root) + os.pathsep + env['PATH']
        env['TB_FIXTURE_RESPONSE'] = str(response_path)
        fleet_path = root / 'fleet.json'
        fleet_path.write_text(json.dumps({
            'hub_db': str(hub), 'binary': '/bin/true',
            'hosts': {
                'executor': {'local': True, 'host_id': executor},
                'desktop': {'host_id': desktop, 'ssh': 'fixture@100.64.0.1',
                            'platform': 'linux', 'helper': '/fixture/threadbridge',
                            'desktop_remote_hosts': {alias: 'executor'}}}}))
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        def get():
            request = urllib.request.Request(f'http://127.0.0.1:{port}/v1/threads',
                                             headers={'Authorization': 'Bearer fixture-phone'})
            with opener.open(request, timeout=2) as result:
                return json.load(result)

        def wait_for(predicate):
            deadline = time.monotonic() + 12
            while True:
                try:
                    result = get()
                    if predicate(result):
                        return result
                except OSError:
                    pass
                assert time.monotonic() < deadline, 'metadata synchronization timed out'
                time.sleep(.05)

        processes = []
        try:
            processes.append(subprocess.Popen([binary, 'hub', '--db', hub, '--listen', f'127.0.0.1:{port}'],
                                              env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            before = wait_for(lambda r: bool(r['threads']))['threads'][0]
            processes.append(subprocess.Popen([binary, 'fleet', '--config', fleet_path, 'run', 'desktop'],
                                              env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            current = wait_for(lambda r: r['threads'][0]['project_name'] == '控屏')['threads'][0]
            assert current['project'] == '/code/screen_control' and current['project_known']
            for field in ['id', 'host_id', 'native_id', 'revision', 'updated_at', 'message_revision', 'message_activity_at']:
                assert current[field] == before[field], field
            response['desktop_projects']['projects'][0]['name'] = '控屏改名'
            save()
            wait_for(lambda r: r['threads'][0]['project_name'] == '控屏改名')
            # A reachable legacy helper with no new metadata field must preserve the last projection.
            del response['desktop_projects']
            save()
            time.sleep(5.5)
            assert get()['threads'][0]['project_name'] == '控屏改名'
            response['desktop_projects'] = {'projects': [], 'assignments': []}
            save()
            wait_for(lambda r: r['threads'][0]['project'] == '')
            with sqlite3.connect(hub) as c:
                assert c.execute('SELECT body FROM messages').fetchone() == ('正文保持原样',)
                assert c.execute('SELECT count(*) FROM commands').fetchone() == (0,)
        finally:
            for process in reversed(processes):
                process.send_signal(signal.SIGINT)
                process.wait(timeout=10)
        print('PASS desktop remote project -> fixed SSH RPC -> fleet -> real phone API; no body change, rename, legacy retention, removal, execution identity/activity/body preserved, no commands')


if __name__ == '__main__':
    main()
