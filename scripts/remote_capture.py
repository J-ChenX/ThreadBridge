#!/usr/bin/env python3
"""四机接入的固定 SSH RPC：只读取完成轮次；队列能力默认关闭。"""
import base64
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid
import zlib
from datetime import datetime
from contextlib import closing
from pathlib import Path

from capture_catalog import capture_catalog
from capture_health import update_health

TAIL_BYTES = 16 * 1024 * 1024
TOTAL_SCAN = 128 * 1024 * 1024
DB_BYTES = 32 * 1024 * 1024
REQUEST_BYTES = 256 * 1024
ROOT = Path(__file__).resolve().parent


def atomic_json(path, value):
    tmp = path.with_suffix(path.suffix + '.next')
    with tmp.open('w', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False)
        stream.flush(); os.fsync(stream.fileno())
    os.replace(tmp, path)


def initialize(database):
    database.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with sqlite3.connect(database, timeout=3) as c:
        c.execute('PRAGMA journal_mode=WAL')
        c.execute('PRAGMA synchronous=FULL')
        c.execute('PRAGMA max_page_count=8192')
        c.executescript('''
CREATE TABLE IF NOT EXISTS captured_replies(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,reply TEXT NOT NULL,utf8_bytes INTEGER NOT NULL,captured_at INTEGER NOT NULL,title TEXT NOT NULL DEFAULT '',PRIMARY KEY(thread_id,turn_id));
CREATE TABLE IF NOT EXISTS captured_request_ids(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,request_id TEXT NOT NULL,PRIMARY KEY(thread_id,turn_id));
CREATE TABLE IF NOT EXISTS captured_user_messages(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,input_digest TEXT NOT NULL,PRIMARY KEY(thread_id,message_id));
CREATE TABLE IF NOT EXISTS captured_turn_order(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,completed_at INTEGER NOT NULL,PRIMARY KEY(thread_id,turn_id));
''')
    if not Path(str(database) + '.health').exists():
        update_health(database, 'initial', 'initial', None)


def completed_events(path, native, home):
    """只扫描已核验 session 的有界尾部，不读取任意索引路径。"""
    path = Path(path).resolve()
    roots = [(home / name).resolve() for name in ('sessions', 'archived_sessions')]
    if not any(path.is_relative_to(root) for root in roots):
        raise ValueError('rollout_path_outside_sessions')
    events = []
    with path.open('rb') as stream:
        first = stream.readline(4 * 1024 * 1024 + 1)
        if len(first) > 4 * 1024 * 1024:
            raise ValueError('session_metadata_limit')
        meta = json.loads(first)
        if meta.get('type') != 'session_meta' or meta.get('payload', {}).get('id') != native:
            raise ValueError('session_identity_mismatch')
        size = path.stat().st_size
        if size > TAIL_BYTES:
            stream.seek(size - TAIL_BYTES); stream.readline(4 * 1024 * 1024 + 1)
        else:
            stream.seek(0)
        used = 0
        while True:
            line = stream.readline(4 * 1024 * 1024 + 1)
            if not line: break
            used += len(line)
            if len(line) > 4 * 1024 * 1024 or used > TAIL_BYTES + 4 * 1024 * 1024:
                raise ValueError('rollout_scan_limit')
            item = json.loads(line); data = item.get('payload', {})
            if item.get('type') == 'event_msg' and data.get('type') == 'task_complete':
                uuid.UUID(data['turn_id'])
                reply = data.get('last_agent_message')
                if not isinstance(reply, str) or not reply: continue
                stamp = int(datetime.fromisoformat(item['timestamp'].replace('Z', '+00:00')).timestamp() * 1000)
                events.append((stamp, {'type':'agent-turn-complete', 'thread-id':native,
                                      'turn-id':data['turn_id'], 'last-assistant-message':reply}))
    return events, min(size, TAIL_BYTES)


def poll(config, database):
    home = Path(config['codex_home']).resolve(); index = home / 'state_5.sqlite'
    cache_path = database.parent / 'poll.json'
    cache = json.loads(cache_path.read_text(encoding='utf-8')) if cache_path.exists() else {}
    from capture_health import read_health
    quarantined={v['thread_id'] for v in read_health(database)['failures'].values() if v.get('turn_id')=='recovery'}
    with sqlite3.connect(index.as_uri() + '?mode=ro', uri=True, timeout=3) as c:
        rows = c.execute('SELECT id,rollout_path,title FROM threads ORDER BY updated_at DESC LIMIT 200').fetchall()
    total = 0
    for native, raw_path, title in rows:
        from collection_policy import allowed
        if not allowed(database,index,native):continue
        # 离线恢复保守保留的故障须显式修复；普通重试不能堆积重复故障占满容量。
        if native in quarantined:continue
        fingerprint=None
        try:
            uuid.UUID(native)
            path = Path(raw_path); stat = path.stat(); fingerprint = [stat.st_size, stat.st_mtime_ns]
            old = cache.get(native, {})
            if native not in cache and len(cache)>=5000:raise ValueError('poll_cache_budget')
            if old.get('file') == fingerprint and (not old.get('error_at') or time.time()-old['error_at']<300):continue
            if total + min(stat.st_size, TAIL_BYTES) > TOTAL_SCAN:
                continue  # 本轮预算耗尽，下一轮从尚未处理项继续，不制造源故障。
            events, scanned = completed_events(path, native, home); total += scanned
            confirmed = old.get('turn_id')
            if old.get('completed_at') and not confirmed:
                with sqlite3.connect(database) as c:
                    row=c.execute('SELECT turn_id FROM captured_turn_order WHERE thread_id=? AND completed_at=?',
                                  (native,old['completed_at'])).fetchone()
                    confirmed=row[0] if row else None
                if not confirmed:raise ValueError('poll_watermark_unconfirmed')
            if confirmed and not any(event['turn-id']==confirmed for _,event in events):
                raise ValueError('completed_turn_gap')
            if confirmed:
                anchor=max(i for i,(_,event) in enumerate(events) if event['turn-id']==confirmed)
                candidates = events[anchor+1:]
            else:
                candidates = events[-1:]  # 初次仅接入最后完成轮次，不冒充完整历史。
            latest = old.get('completed_at', 0)
            processed = 0
            for stamp, event in candidates:
                # 用户精确轮次读取会从文件开头扫描，将其成本也算入单次预算。
                if min(stat.st_size,TAIL_BYTES)+stat.st_size>TOTAL_SCAN:raise ValueError('poll_scan_budget')
                if total + stat.st_size > TOTAL_SCAN:break
                total += stat.st_size
                result = capture_catalog(json.dumps(event), None, str(database), None, True,
                                         DB_BYTES, 64 * 1024 * 1024, str(index))
                if result not in ('captured', 'duplicate'): raise ValueError('capture_unconfirmed')
                with sqlite3.connect(database, timeout=3) as c:
                    display = title.strip()[:150] if isinstance(title,str) and title.strip() else '会话 · '+native[:8]
                    c.execute('UPDATE captured_replies SET title=? WHERE thread_id=? AND turn_id=?',
                              (display,native,event['turn-id']))
                latest = max(latest, stamp)
                confirmed=event['turn-id']
                processed += 1
            finished = processed == len(candidates)
            cache[native] = {'file':fingerprint if finished else None, 'completed_at':latest,'turn_id':confirmed}
            update_health(database, native, 'poll', None)
        except Exception:
            # 失败不推进该会话水位，下次有限重试；日志不包含正文或路径。
            update_health(database, native, 'poll', 'capture_not_confirmed')
            if native in cache or len(cache)<5000:
                preserved=cache.get(native,{}).copy()
                preserved['file']=fingerprint
                preserved['error_at']=time.time();cache[native]=preserved
    atomic_json(cache_path, cache)


def codex_output(config,args,timeout,limit):
    """运行期间限制 stdout/stderr 与时长，不无限收集子进程输出。"""
    with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
        p=subprocess.Popen([config['codex'],*args],stdin=subprocess.DEVNULL,stdout=out,stderr=err)
        try:
            end=time.monotonic()+timeout
            while p.poll() is None:
                if os.fstat(out.fileno()).st_size>limit or os.fstat(err.fileno()).st_size>65536:raise ValueError('codex_output_limit')
                if time.monotonic()>end:raise TimeoutError('codex_timeout')
                time.sleep(.02)
            if p.returncode or os.fstat(out.fileno()).st_size>limit or os.fstat(err.fileno()).st_size>65536:
                raise ValueError('codex_result_unconfirmed')
            out.seek(0);return out.read(limit+1).decode('utf-8')
        except BaseException:
            if p.poll() is None:p.kill();p.wait()
            raise


def version(config):
    return codex_output(config,['--version'],4,512).strip()


def snapshot(config, request):
    database = ROOT / 'data' / 'replies.sqlite'
    if request.get('excluded'):
        from collection_policy import purge
        excluded=request['excluded']
        if not isinstance(excluded,list) or len(excluded)>5000:raise ValueError('excluded_limit')
        for native in excluded:uuid.UUID(native)
        purge(database,excluded)
    initialize(database); poll(config, database)
    with sqlite3.connect(database, timeout=3) as c:
        identity = [c.execute('SELECT count(*),coalesce(max(rowid),0) FROM ' + table).fetchone()
                    for table in ('captured_replies', 'captured_user_messages', 'captured_turn_order')]
    health = Path(str(database) + '.health').read_bytes()
    # health 每次正常轮询不更新；内容变化才传输 SQLite 备份。
    from capture_health import read_health
    failures = read_health(database)
    identity.append([failures.get('failures'), failures.get('overflow')])
    revision = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    result = {'schema':1, 'version':version(config), 'revision':revision,
              'capture_ok':not failures.get('failures') and not failures.get('overflow'),
              'blocked_threads':sorted({v['thread_id'] for v in failures.get('failures',{}).values()}),
              'overflow':bool(failures.get('overflow')),
              'changed':revision != request.get('revision')}
    if result['changed']:
        with tempfile.TemporaryDirectory(dir=database.parent) as directory:
            backup = Path(directory) / 'snapshot.sqlite'
            with closing(sqlite3.connect(database)) as source, closing(sqlite3.connect(backup)) as target:
                source.backup(target)
            if backup.stat().st_size > DB_BYTES: raise ValueError('snapshot_budget')
            result['database'] = base64.b64encode(zlib.compress(backup.read_bytes(), 3)).decode('ascii')
        result['health'] = base64.b64encode(health).decode('ascii')
    return result


def dispatch(config, request):
    native = request['thread']; uuid.UUID(native)
    text = request['text']
    if not isinstance(text, str) or not text.strip() or len(text.encode()) > 32000:
        raise ValueError('queue_input_invalid')
    expected = config.get('verified_version')
    if not expected or request.get('expected_version') != expected or version(config) != expected:
        raise ValueError('queue_version_unverified')
    database = ROOT / 'data' / 'replies.sqlite'
    from capture_health import read_health
    health=read_health(database)
    if health.get('overflow') or any(v.get('thread_id')==native for v in health['failures'].values()):
        raise ValueError('target_capture_unconfirmed')
    with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as c:
        if not c.execute('SELECT 1 FROM captured_replies WHERE thread_id=? LIMIT 1', (native,)).fetchone():
            raise ValueError('queue_target_not_captured')
    output=codex_output(config,['queue','--thread',native,'--message='+text],10,4096)
    return {'schema':1, 'stdout':output}


def main():
    os.umask(0o077)
    try:
        raw = sys.stdin.buffer.read(REQUEST_BYTES+1)
        if len(raw) > REQUEST_BYTES: raise ValueError('request_limit')
        request = json.loads(raw)
        config = json.loads((ROOT / 'remote.json').read_text(encoding='utf-8'))
        if request.get('op') == 'snapshot': result = snapshot(config, request)
        elif request.get('op') == 'version': result = {'schema':1, 'stdout':version(config) + '\n'}
        elif request.get('op') == 'queue': result = dispatch(config, request)
        else: raise ValueError('rpc_operation_invalid')
        sys.stdout.write(json.dumps(result, ensure_ascii=False))
        return 0
    except Exception:
        sys.stderr.write('ThreadBridge RPC failed; no message content logged\n')
        return 2


if __name__ == '__main__': sys.exit(main())
