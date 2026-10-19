"""Actual signed upgrade/startup regression on a rooted dedicated emulator, offline synthetic data.

Usage: python3 tests/android/qa_android_startup_upgrade.py --serial emulator-5554 CANDIDATE.apk
Never runs on a physical phone, never uninstalls, pairs, or sends an upstream command.
The synthetic cache replaces only this test emulator's database. Network is disabled
for the test and restored afterward; existing connection preferences are preserved.
"""
import argparse
import hashlib
import json
import shlex
import sqlite3
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ADB = ROOT/'local/android-sdk/platform-tools/adb'
PACKAGE = 'dev.threadbridge'
DATA = '/data/user/0/'+PACKAGE
DATABASE = DATA+'/databases/threadbridge.sqlite'
SCHEMAS = ROOT/'android/app/schemas/dev.threadbridge.BridgeDatabase'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--require-paired', action='store_true')
    parser.add_argument('candidate', type=Path)
    parser.add_argument('--previous', type=Path, help='Actual most recently delivered APK')
    parser.add_argument('--published-only', action='store_true', help='Test only the specified actual previous APK')
    parser.add_argument('--previous-schema', type=Path, default=SCHEMAS/'9.json')
    args = parser.parse_args()
    assert args.serial.startswith('emulator-'), 'Dedicated emulator required'
    candidate = args.candidate.resolve()
    output = ROOT/'artifacts'/('startup-upgrade-'+candidate.stem)/args.serial
    output.mkdir(parents=True, exist_ok=True)

    def adb(*command):
        return subprocess.check_output([str(ADB), '-s', args.serial, *map(str, command)], text=True, timeout=35)

    def shell(*command):
        return adb('shell', shlex.join(map(str, command))).strip()

    assert shell('getprop', 'ro.kernel.qemu') == '1'
    adb('root')
    adb('wait-for-device')
    sdk = int(shell('getprop', 'ro.build.version.sdk'))
    adb('install', '-r', str(candidate))
    shell('am', 'start', '-W', '-n', PACKAGE+'/.MainActivity')
    time.sleep(3)
    assert shell('pidof', PACKAGE), 'Fresh candidate startup failed'
    shell('am', 'force-stop', PACKAGE)
    uid = shell('stat', '-c', '%u:%g', DATA)
    wifi = shell('settings', 'get', 'global', 'wifi_on')
    mobile = shell('settings', 'get', 'global', 'mobile_data')
    shell('svc', 'wifi', 'disable')
    shell('svc', 'data', 'disable')

    def query(sql):
        return json.loads(shell('sqlite3', '-json', DATABASE, sql) or '[]')

    def connection_hash():
        path = DATA+'/shared_prefs/connection.xml'
        exists = shell('sh', '-c', 'test -f '+shlex.quote(path)+' && echo yes || true')
        if not exists:
            return None
        raw = subprocess.check_output([str(ADB), '-s', args.serial, 'exec-out', 'cat', path], timeout=10)
        return hashlib.sha256(raw).hexdigest()

    def seed(schema_path, destination):
        schema = json.loads(schema_path.read_text())['database']
        with sqlite3.connect(destination) as c:
            for entity in schema['entities']:
                c.execute(entity['createSql'].replace('${TABLE_NAME}', entity['tableName']))
                for index in entity.get('indices', []):
                    c.execute(index['createSql'].replace('${TABLE_NAME}', entity['tableName']))
            c.execute('CREATE TABLE room_master_table (id INTEGER PRIMARY KEY,identity_hash TEXT)')
            c.execute('INSERT INTO room_master_table VALUES(42,?)', (schema['identityHash'],))
            c.execute('PRAGMA user_version='+str(schema['version']))
            thread = 'a'*64
            data = dict(id=thread, title='Upgrade preserved task', hostId='upgrade-host', status='completed',
                        revision='stable', updated=int(time.time()), canSend=0, online=0, historyCursor=None,
                        messageRevision=7, messageActivityAt=int(time.time()*1000), project='/qa/startup',
                        localOnly=0, projectName='Upgrade project', projectKnown=1,
                        taskStatus='active', taskTurnId='preserve-turn', displayHostId='upgrade-host')
            columns = {r[1] for r in c.execute('PRAGMA table_info(threads)')}
            data = {k: v for k, v in data.items() if k in columns}
            c.execute('INSERT INTO threads ('+','.join(data)+') VALUES ('+','.join('?' for _ in data)+')', list(data.values()))
            text = 'Keep cached reply exactly'
            c.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?,?,?)',
                      (thread, 'message', 'turn', 'assistant', text, 'same', int(time.time()*1000), len(text), int(time.time()*1000)))
            c.execute('INSERT INTO drafts VALUES(?,?)', (thread, 'Keep unsent draft exactly'))
            c.execute('INSERT INTO pending VALUES(?,?,?,?,?,?)',
                      ('pending-request', thread, '{"text":"do not resend"}', 'unknown', 'unchanged-command', None))
            c.execute('INSERT INTO hosts VALUES(?,?,?)', ('upgrade-host', 'Upgrade fixture', 0))
            c.execute('INSERT INTO projects VALUES(?,?,?)', ('upgrade-host', '/qa/startup', 'Upgrade project'))
            c.execute('INSERT INTO sync VALUES(?,?,?)', (1, 123, 'offline-upgrade-collection'))
        return schema

    def snapshot(schema):
        rows = {}
        for entity in schema['entities']:
            names = ','.join('`'+f['columnName']+'`' for f in entity['fields'])
            rows[entity['tableName']] = query('SELECT '+names+' FROM `'+entity['tableName']+'` ORDER BY rowid')
        return rows

    def launch(case):
        adb('logcat', '-c')
        shell('am', 'start', '-W', '-n', PACKAGE+'/.MainActivity')
        time.sleep(3)
        crash = adb('logcat', '-d', '-b', 'crash')
        (output/(case+'.log')).write_text(crash)
        assert shell('pidof', PACKAGE), 'Startup crashed: '+case+'; '+crash[:500]
        assert 'FATAL EXCEPTION' not in crash, case
        shell('uiautomator', 'dump', '/sdcard/startup-qa.xml')
        tree = shell('cat', '/sdcard/startup-qa.xml')
        assert 'package="dev.threadbridge"' in tree, 'Activity not visible: '+case
        if args.require_paired:
            assert '设备列表' in tree and 'Upgrade preserved task' in tree, 'Paired cache not visible: '+case
        (output/(case+'.xml')).write_text(tree)
        with (output/(case+'.png')).open('wb') as out:
            subprocess.run([str(ADB), '-s', args.serial, 'exec-out', 'screencap', '-p'], stdout=out, check=True, timeout=20)
        shell('am', 'force-stop', PACKAGE)

    cases = [('from-1.0.2-room8', ROOT/'artifacts/ThreadBridge-1.0.2.apk', SCHEMAS/'8.json'),
             ('from-1.0.3-room7', ROOT/'artifacts/ThreadBridge-1.0.3.apk', SCHEMAS/'7.json'),
             ('from-1.0.1-task-room7', ROOT/'artifacts/ThreadBridge-1.0.1.apk', ROOT/'tests/fixtures/android/room-7-task-state.json')]
    if args.previous:
        cases.insert(0, ('from-published-release', args.previous.resolve(), args.previous_schema.resolve()))
    if args.published_only:
        assert args.previous, '--published-only requires --previous'
        cases=cases[:1]
    results = []
    try:
        for name, prior, schema_path in cases:
            shell('am', 'force-stop', PACKAGE)
            adb('install', '-r', '-d', str(prior))
            with tempfile.TemporaryDirectory(prefix='tb-upgrade-') as temporary:
                path = Path(temporary)/'cache.sqlite'
                schema = seed(schema_path, path)
                adb('push', path, '/data/local/tmp/startup-qa.sqlite')
                shell('cp', '/data/local/tmp/startup-qa.sqlite', DATABASE)
                shell('rm', '-f', DATABASE+'-wal', DATABASE+'-shm')
                shell('chown', uid, DATABASE)
                shell('chmod', '660', DATABASE)
            before = snapshot(schema)
            connection = connection_hash()
            assert not args.require_paired or connection is not None, 'Pair this test emulator with a synthetic fixture first'
            launch(name+'-old')
            assert snapshot(schema) == before, 'Old app changed offline cache: '+name
            # Actual signed cover upgrade: keep application data and Android Keystore.
            adb('install', '-r', str(candidate))
            launch(name+'-new')
            assert query('PRAGMA user_version')[0]['user_version'] == 9
            assert snapshot(schema) == before, 'Upgrade lost cache/draft/pending state: '+name
            assert connection_hash() == connection, 'Connection preferences changed: '+name
            launch(name+'-restart')
            assert snapshot(schema) == before
            results.append({'case': name, 'startup': True, 'restart': True, 'preserved_old_columns_and_rows': True,
                            'connection_preferences_preserved': True, 'paired_connection_present': connection is not None, 'room_version': 9})
            print('PASS', args.serial, name, 'signed upgrade/start/restart/cache/draft/pending/connection preservation', flush=True)
    finally:
        if wifi == '1':
            shell('svc', 'wifi', 'enable')
        if mobile == '1':
            shell('svc', 'data', 'enable')
    report = {'emulator': args.serial, 'android_sdk': sdk, 'candidate': str(candidate), 'cases': results,
              'physical_phone_tested': False, 'upstream_commands_sent': 0}
    (output/'report.json').write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    main()
