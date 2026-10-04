#!/usr/bin/env python3
"""ThreadBridge 四机运维入口；私有清单独立于 screen_control。"""
import argparse
import base64
import io
import json
import os
import re
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import zipfile
import zlib
from contextlib import closing
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT = ROOT / 'local/fleet/fleet.json'
MAX_RESPONSE = 48 * 1024 * 1024
MAX_REQUEST = 256 * 1024
LOCAL_UNITS = ['threadbridge-phone-hub.service', 'threadbridge-phone-inbox.service', 'threadbridge-phone-capture.service']
REMOTE_FILES = ['remote_capture.py', 'capture_catalog.py', 'capture_completion.py', 'capture_user_turn.py', 'capture_health.py', 'collection_policy.py']


def load(path):
    c = json.loads(Path(path).read_text())
    if not 1 <= len(c['hosts']) <= 4: raise ValueError('host_count')
    for name, host in c['hosts'].items():
        if not re.fullmatch(r'[a-z][a-z0-9-]{0,31}', name): raise ValueError('host_name')
        if host.get('local'): continue
        if not re.fullmatch(r'[a-zA-Z0-9_.-]+@100\.\d{1,3}\.\d{1,3}\.\d{1,3}', host['ssh']):
            raise ValueError('tailnet_ssh_target')
        if host['platform'] not in ('linux', 'windows'): raise ValueError('platform')
    return c


def atomic(path, data):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    tmp = path.with_suffix(path.suffix + '.next')
    with tmp.open('wb') as f: f.write(data); f.flush(); os.fsync(f.fileno())
    os.chmod(tmp, 0o600); os.replace(tmp, path)


def status_file(config_path, name):
    return Path(config_path).parent / name / 'status.json'


def command(host, code=None):
    args = [host['python'], '-c', code] if code else [host['python'], host['helper']]
    if host['platform'] == 'linux': return shlex.join(args)
    if code:
        # Windows PowerShell 5.1 的原生命令行会剥离嵌入双引号，用固定编码脚本绕过。
        args=[host['python'],'-c',"import base64;exec(base64.b64decode('"+base64.b64encode(code.encode()).decode()+"'))"]
    def ps_quote(value): return "'" + value.replace("'", "''") + "'"
    ps = "$ErrorActionPreference='Stop';$env:PYTHONUTF8='1';$OutputEncoding=New-Object System.Text.UTF8Encoding($false);$p=[Console]::In.ReadToEnd();$p | & " + ' '.join(ps_quote(a) for a in args) + ';exit $LASTEXITCODE'
    return 'powershell.exe -NoProfile -NonInteractive -EncodedCommand ' + base64.b64encode(ps.encode('utf-16le')).decode()


def ssh(host, cmd, data, timeout=12):
    args = ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes',
            '-o', 'ClearAllForwardings=yes', '-o', 'ConnectTimeout=5',
            '-o', 'ServerAliveInterval=5', '-o', 'ServerAliveCountMax=1', '--', host['ssh'], cmd]
    # 临时文件限制内存；读取时再次检查限额，绝不输出远端正文/错误内容。
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=out, stderr=err)
        try:
            os.set_blocking(p.stdin.fileno(),False)
            position=0
            deadline = time.monotonic() + timeout
            while p.poll() is None:
                if os.fstat(out.fileno()).st_size > MAX_RESPONSE or os.fstat(err.fileno()).st_size > 65536:
                    raise RuntimeError('rpc_stream_limit')
                if time.monotonic() > deadline: raise subprocess.TimeoutExpired(args, timeout)
                if p.stdin is not None:
                    try: position += os.write(p.stdin.fileno(),data[position:position+8192])
                    except BlockingIOError: pass
                    if position == len(data): p.stdin.close();p.stdin=None
                time.sleep(.05)
        except BaseException:
            if p.stdin is not None:p.stdin.close();p.stdin=None
            p.kill(); p.wait(); raise
        size = out.seek(0, os.SEEK_END)
        if p.returncode or size > MAX_RESPONSE: raise RuntimeError('ssh_rpc_failed_or_response_limit')
        out.seek(0); return out.read(MAX_RESPONSE + 1)


def rpc(host, request, timeout=12):
    # ensure_ascii 保证 Windows PowerShell stdin 管道不改变正文。
    encoded=json.dumps(request).encode('ascii')
    if len(encoded)>MAX_REQUEST:raise ValueError('rpc_request_limit')
    data = ssh(host, command(host), encoded, timeout)
    response = json.loads(data)
    if response.get('schema') != 1: raise ValueError('rpc_schema')
    return response


def deploy(c, names, config_path):
    for name in names:
        host = c['hosts'][name]
        if host.get('local'): continue
        buffer = io.BytesIO()
        remote = {'codex':host['codex'], 'codex_home':host['codex_home'], 'verified_version':host.get('verified_version')}
        with zipfile.ZipFile(buffer, 'w', zipfile.ZIP_DEFLATED) as z:
            for item in REMOTE_FILES: z.write(ROOT / 'scripts' / item, item)
            z.writestr('remote.json', json.dumps(remote))
        code = "import sys,base64,io,zipfile,pathlib,os;os.umask(0o077);p=pathlib.Path(" + repr(str(Path(host['helper']).parent) if host['platform']=='linux' else host['helper'].rsplit('\\',1)[0]) + ");p.mkdir(parents=True,exist_ok=True);z=zipfile.ZipFile(io.BytesIO(base64.b64decode(sys.stdin.buffer.read())));names=z.namelist();assert all('/' not in n and '\\\\' not in n and n not in ('.','..') for n in names);[(p/n).write_bytes(z.read(n)) for n in names];print('installed')"
        ssh(host, command(host, code), base64.b64encode(buffer.getvalue()), timeout=20)
        response = rpc(host, {'op':'version'})
        print(name + ': installed; ' + response['stdout'].strip() + '; queue=' + ('verified' if host.get('verified_version') else 'read-only'))


def validate_snapshot(raw, destination, prefix):
    compressed = base64.b64decode(raw, validate=True)
    decoder = zlib.decompressobj(); data = decoder.decompress(compressed, 32 * 1024 * 1024 + 1)
    if len(data) > 32 * 1024 * 1024 or not decoder.eof or decoder.unused_data:
        raise ValueError('snapshot_uncompressed_limit')
    temp = destination.with_suffix('.download')
    atomic(temp, data)
    try:
        with sqlite3.connect(temp.as_uri() + '?mode=rw', uri=True) as db:
            db.execute('PRAGMA journal_mode=DELETE')
            if db.execute('PRAGMA quick_check').fetchone() != ('ok',): raise ValueError('snapshot_corrupt')
            required = {'captured_replies','captured_user_messages','captured_turn_order','captured_request_ids'}
            actual = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if not required <= actual: raise ValueError('snapshot_schema')
            for native, turn, body in db.execute('SELECT thread_id,turn_id,reply FROM captured_replies'):
                import uuid
                uuid.UUID(native); uuid.UUID(turn)
                if len(body.encode()) > 512 * 1024: raise ValueError('snapshot_reply_limit')
            db.execute("UPDATE captured_replies SET title=? || substr(title,1,160)", (prefix + ' · ',))
        os.replace(temp, destination)
    finally:
        temp.unlink(missing_ok=True)


def offline(c, host):
    # 仅更新该主机就绪/心跳，不触碰其不可变发送账本。
    with sqlite3.connect(c['hub_db'], timeout=3) as db:
        db.execute('UPDATE devices SET last_seen=0 WHERE id=?', (host['host_id'],))
        db.execute('UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE host=?', (host['host_id'],))
        db.execute('UPDATE threads SET can_send=0 WHERE host=?', (host['host_id'],))


def queue_snapshot(source,destination,blocked):
    """写桥只发现健康会话；完整读取副本仍保留被隔离会话的回复。"""
    tmp=destination.with_suffix('.next')
    with closing(sqlite3.connect(source.as_uri()+'?mode=ro',uri=True)) as origin,closing(sqlite3.connect(tmp)) as target:
        origin.backup(target);target.execute('PRAGMA journal_mode=DELETE')
        for table in ('captured_replies','captured_user_messages','captured_turn_order','captured_request_ids'):
            target.executemany('DELETE FROM '+table+' WHERE thread_id=?',[(native,) for native in blocked])
        target.commit()
    os.chmod(tmp,0o600);os.replace(tmp,destination)


def quarantine(c,host,blocked):
    with sqlite3.connect(c['hub_db'],timeout=3) as db:
        for native in blocked:
            db.execute('UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE host=? AND native=?',(host['host_id'],native))
            db.execute('UPDATE threads SET can_send=0 WHERE host=? AND native=?',(host['host_id'],native))


def run_host(c, name, config_path):
    host = c['hosts'][name]; directory = Path(config_path).parent / name
    source = directory / 'replies.sqlite'; writable = directory/'queue.sqlite'; directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    revision = None; children = []; stopping = False; queue_mode = None; blocked_threads = None
    def stop(*_):
        nonlocal stopping
        stopping = True
    for sig in (signal.SIGTERM, signal.SIGINT): signal.signal(sig, stop)
    def close():
        for child in children:
            if child.poll() is None: child.terminate()
        for child in children:
            try: child.wait(timeout=2)
            except subprocess.TimeoutExpired: child.kill(); child.wait()
        children.clear(); offline(c, host)
    try:
        while not stopping:
            started = time.monotonic()
            try:
                with sqlite3.connect(c['hub_db'],timeout=3) as db:
                    excluded=[r[0] for r in db.execute("SELECT native FROM capture_targets ct JOIN tombstones t ON t.thread=ct.thread AND t.message='' WHERE ct.host=?",(host['host_id'],))]
                    local_host=next(h['host_id'] for h in c['hosts'].values() if h.get('local'))
                    local_excluded=[r[0] for r in db.execute("SELECT native FROM capture_targets ct JOIN tombstones t ON t.thread=ct.thread AND t.message='' WHERE ct.host=?",(local_host,))]
                if local_excluded:
                    from collection_policy import purge
                    purge(ROOT/'local/notify-capture/replies.sqlite',local_excluded)
                response = rpc(host, {'op':'snapshot', 'revision':revision,'excluded':excluded}, timeout=45 if revision is None else 10)
                verified = host.get('verified_version')
                enabled = bool(verified and response['version'] == verified and not response.get('overflow',True))
                if queue_mode is not None and enabled != queue_mode: close()
                queue_mode = enabled
                if response['changed']:
                    blocked=response.get('blocked_threads',[])
                    # 旧写桥可能已缓存目标列表，先等待退出才撤销故障目标就绪。
                    if blocked_threads is not None and set(blocked)!=blocked_threads: close()
                    # 已打开的只读句柄读完旧快照，下次打开读取新文件；不重启队列进程。
                    validate_snapshot(response['database'], source, name)
                    health = base64.b64decode(response['health'], validate=True)
                    if len(health) != 131072: raise ValueError('health_size')
                    atomic(Path(str(source)+'.health'), health)
                    queue_snapshot(source,writable,blocked)
                    quarantine(c,host,blocked)
                    blocked_threads=set(blocked)
                    revision = response['revision']
                atomic(status_file(config_path, name), json.dumps({'online':True,'last_success':time.time(),
                       'version':response['version'],'queue':enabled,
                       'blocked_threads':response.get('blocked_threads',[]),
                       'revision':revision}).encode())
                if any(child.poll() is not None for child in children): close()
                if not children:
                    shared = [c['binary'], '--db', c['hub_db'], '--capture-db', str(source), '--host', host['host_id']]
                    children.append(subprocess.Popen([shared[0], 'capture-sync', *shared[1:], '--all-captured']))
                    args = [shared[0], 'capture-bridge', *shared[1:]]
                    if enabled:
                        args[args.index('--capture-db')+1]=str(writable)
                        args += ['--codex', str(directory/'codex-proxy'), '--allow-queue', '--verified-version', host['verified_version']]
                    children.append(subprocess.Popen(args))
            except Exception:
                close()
                atomic(status_file(config_path, name), json.dumps({'online':False,'checked_at':time.time(),'error':'connection_or_capture_unavailable'}).encode())
                print(name + ': unavailable; cached replies retained, queue disabled', flush=True)
                revision = None
            delay = max(0.5, 5 - (time.monotonic() - started))
            end = time.monotonic() + delay
            while not stopping and time.monotonic() < end: time.sleep(.2)
    finally:
        close()
        atomic(status_file(config_path, name), json.dumps({'online':False,'checked_at':time.time(),'stopped':True}).encode())


def proxy(c, name, config_path, args):
    host = c['hosts'][name]
    if args == ['--version']:
        sys.stdout.write(rpc(host, {'op':'version'}, timeout=4)['stdout']); return
    if len(args) != 5 or args[:2] != ['queue','--thread'] or args[3] != '--message':
        raise ValueError('proxy_arguments')
    state = json.loads(status_file(config_path, name).read_text())
    if not state.get('online') or time.time() - state['last_success'] > 10:
        raise ValueError('remote_not_fresh')
    if args[2] in state.get('blocked_threads',[]):raise ValueError('target_capture_unconfirmed')
    sys.stdout.write(rpc(host, {'op':'queue', 'thread':args[2], 'text':args[4],
                              'expected_version':host.get('verified_version')}, timeout=12)['stdout'])


def install(c, config_path):
    units = Path.home()/'.config/systemd/user'; units.mkdir(parents=True, exist_ok=True)
    backup = ROOT/'local/backups'/('fleet-'+time.strftime('%Y%m%dT%H%M%S'))
    backup.mkdir(parents=True, exist_ok=False, mode=0o700)
    with sqlite3.connect(c['hub_db']) as source, sqlite3.connect(backup/'hub.sqlite') as target: source.backup(target)
    for unit in LOCAL_UNITS+['threadbridge.target','threadbridge-remote@.service']:
        if (units/unit).exists(): shutil.copy2(units/unit,backup/unit)
        if (units/(unit+'.d')).exists(): shutil.copytree(units/(unit+'.d'),backup/(unit+'.d'))
        if unit in LOCAL_UNITS:
            fragment=subprocess.check_output(['systemctl','--user','show',unit,'-p','FragmentPath','--value'],text=True).strip()
            shutil.copy2(fragment,backup/(unit+'.fragment'))
    for unit in LOCAL_UNITS:
        # 保存真实当前片段，原有 drop-in 继续生效；不从日志推断启动配置。
        existing = subprocess.check_output(['systemctl','--user','show',unit,'-p','FragmentPath','--value'],text=True).strip()
        if not (units/unit).exists(): (units/unit).write_text(Path(existing).read_text())
        directory = units/(unit+'.d'); directory.mkdir(exist_ok=True)
        (directory/'80-fleet.conf').write_text('[Unit]\nPartOf=threadbridge.target\n')
    names = [n for n,h in c['hosts'].items() if not h.get('local')]
    target = '[Unit]\nDescription=ThreadBridge four-computer service\nWants='+' '.join(LOCAL_UNITS+['threadbridge-remote@'+n+'.service' for n in names])+'\nAfter=network-online.target\n\n[Install]\nWantedBy=default.target\n'
    (units/'threadbridge.target').write_text(target)
    template = '[Unit]\nDescription=ThreadBridge remote capture %i\nPartOf=threadbridge.target\nAfter=threadbridge-phone-hub.service\n\n[Service]\nType=simple\nWorkingDirectory='+str(ROOT)+'\nExecStart=/usr/bin/python3 '+str(ROOT/'scripts/fleet.py')+' --config '+str(Path(config_path).resolve())+' run %i\nRestart=on-failure\nRestartSec=5\nUMask=0077\nMemoryHigh=128M\nMemoryMax=256M\nKillMode=control-group\n'
    (units/'threadbridge-remote@.service').write_text(template)
    for name in names:
        directory = Path(config_path).parent/name; directory.mkdir(parents=True,exist_ok=True)
        wrapper = '#!/bin/sh\nexec /usr/bin/python3 '+shlex.quote(str(ROOT/'scripts/fleet.py'))+' --config '+shlex.quote(str(Path(config_path).resolve()))+' proxy '+shlex.quote(name)+' -- "$@"\n'
        (directory/'codex-proxy').write_text(wrapper); (directory/'codex-proxy').chmod(0o700)
    subprocess.run(['systemctl','--user','daemon-reload'],check=True)
    subprocess.run(['systemctl','--user','enable','threadbridge.target'],check=True)
    print('service definitions and consistent Hub backup: '+str(backup))


def main():
    os.umask(0o077)
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument('--config',default=str(DEFAULT))
    parser.add_argument('action',choices=['deploy','install','start','stop','restart','status','run','proxy','probe'])
    parser.add_argument('name',nargs='?'); args,extra=parser.parse_known_args()
    c=load(args.config)
    if args.action=='proxy': proxy(c,args.name,args.config,extra)
    elif args.action=='run': run_host(c,args.name,args.config)
    elif args.action=='deploy': deploy(c,[args.name] if args.name else list(c['hosts']),args.config)
    elif args.action=='install': install(c,args.config)
    elif args.action in ('start','stop','restart'):
        subprocess.run(['systemctl','--user',args.action,'threadbridge.target'],check=True)
    elif args.action=='probe':
        for name,host in c['hosts'].items():
            if not host.get('local'):
                try: print(name+': '+rpc(host,{'op':'version'})['stdout'].strip())
                except Exception: print(name+': unavailable')
    else:
        for name,host in c['hosts'].items():
            if host.get('local'):
                active=all(subprocess.run(['systemctl','--user','is-active','--quiet',u]).returncode==0 for u in LOCAL_UNITS)
                print(name+': '+('online; existing local queue' if active else 'stopped or partial'))
            else:
                path=status_file(args.config,name);state=json.loads(path.read_text()) if path.exists() else {}
                fresh=state.get('online') and time.time()-state.get('last_success',0)<15
                print(name+': '+('online' if fresh else 'offline')+'; '+('queue verified' if fresh and state.get('queue') else 'read-only')+'; '+state.get('version','not yet checked'))
    return 0


if __name__=='__main__':
    try: sys.exit(main())
    except Exception: sys.stderr.write('ThreadBridge operation failed; check service status (message contents omitted)\n');sys.exit(2)
