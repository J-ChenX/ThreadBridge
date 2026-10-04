#!/usr/bin/env python3
"""Offline health recovery: retain backups; only proven sources leave quarantine.

The fleet poller must be stopped. This module never queues a message or changes
the capture database. Work is bounded per batch, and finalization is conditional
on unchanged capture artifacts and complete coverage of discoverable targets.
"""
import hashlib
import json
import os
import shutil
import sqlite3
import uuid
from contextlib import closing
from pathlib import Path

import remote_capture as capture
from capture_health import LIMIT, read_health, update_health, health_path
from capture_user_turn import read_turn


def binding(database):
    digest=hashlib.sha256()
    # SQLite checkpoint/GC can change DB/WAL bytes without changing data.
    used=0
    with closing(sqlite3.connect(database.as_uri()+'?mode=ro',uri=True)) as db:
        db.execute('BEGIN')
        for table in ('captured_replies','captured_user_messages','captured_turn_order','captured_request_ids'):
            digest.update(table.encode())
            for row in db.execute('SELECT rowid,* FROM '+table+' ORDER BY rowid'):
                encoded=json.dumps(row,ensure_ascii=False,separators=(',',':')).encode()
                used+=len(encoded)
                if used>2*capture.DB_BYTES:raise ValueError('recovery_binding_budget')
                digest.update(encoded+b'\n')
    for path in (health_path(database),database.parent/'poll.json'):
        digest.update(path.name.encode())
        if path.exists():
            with path.open('rb') as stream:
                while chunk:=stream.read(1024*1024):digest.update(chunk)
        else:digest.update(b'missing')
    return digest.hexdigest()


def index_signature(index,targets):
    with closing(sqlite3.connect(index.as_uri()+'?mode=ro',uri=True)) as db:
        db.execute('BEGIN')
        recent=db.execute('SELECT id,rollout_path FROM threads ORDER BY updated_at DESC,id DESC LIMIT 200').fetchall()
        paths=[(native,db.execute('SELECT rollout_path FROM threads WHERE id=?',(native,)).fetchone()) for native in targets]
    return hashlib.sha256(json.dumps([recent,paths],sort_keys=True).encode()).hexdigest()


def prepare(config,database,identifier):
    uuid.UUID(identifier)
    index_file=Path(config['codex_home'])/'state_5.sqlite'
    if database.stat().st_size>capture.DB_BYTES or index_file.stat().st_size>capture.DB_BYTES:
        raise ValueError('recovery_backup_budget')
    if shutil.disk_usage(database.parent).free<64*1024*1024+2*capture.DB_BYTES:
        raise ValueError('recovery_free_space_reserve')
    directory=database.parent/('health-recovery-'+identifier)
    directory.mkdir(mode=0o700,exist_ok=False)
    cache=json.loads((database.parent/'poll.json').read_text(encoding='utf-8'))
    if len(cache)>=5000:raise ValueError('recovery_cache_coverage_unconfirmed')
    health=read_health(database)
    universe=set(cache)|{v['thread_id'] for v in health['failures'].values()}
    with closing(sqlite3.connect(database)) as source,closing(sqlite3.connect(directory/'replies.sqlite')) as backup:
        source.backup(backup)
        for table in ('captured_replies','captured_user_messages','captured_turn_order','captured_request_ids'):
            universe.update(r[0] for r in source.execute('SELECT DISTINCT thread_id FROM '+table))
    home=Path(config['codex_home'])
    with closing(sqlite3.connect((home/'state_5.sqlite').as_uri()+'?mode=ro',uri=True)) as index,closing(sqlite3.connect(directory/'index.sqlite')) as backup:
        index.backup(backup)
        universe.update(r[0] for r in backup.execute('SELECT id FROM threads ORDER BY updated_at DESC,id DESC LIMIT 200'))
    if len(universe)>5000:raise ValueError('recovery_target_limit')
    for native in universe:uuid.UUID(native)
    shutil.copy2(health_path(database),directory/'original.health')
    shutil.copy2(database.parent/'poll.json',directory/'poll.json')
    state={'binding':binding(database),'targets':sorted(universe),'results':{},'proofs':{},'codex_home':str(home),
           'index_signature':index_signature(directory/'index.sqlite',sorted(universe)),
           'prior_failures':sorted({v['thread_id'] for v in health['failures'].values()})}
    capture.atomic_json(directory/'recovery.json',state)
    return directory


def batch(config,database,directory):
    state=json.loads((directory/'recovery.json').read_text())
    if state['binding']!=binding(database):raise ValueError('recovery_capture_changed')
    cache=json.loads((directory/'poll.json').read_text())
    home=Path(config['codex_home']);index=directory/'index.sqlite';budget=capture.TOTAL_SCAN
    with closing(sqlite3.connect(index.as_uri()+'?mode=ro',uri=True)) as source_index:
        paths={native:row[0] for native in state['targets'] if (row:=source_index.execute('SELECT rollout_path FROM threads WHERE id=?',(native,)).fetchone())}
    with closing(sqlite3.connect((directory/'replies.sqlite').as_uri()+'?mode=ro',uri=True)) as db:
        for native in state['targets']:
            if native in state['results']:continue
            reason=None
            try:
                if native in state['prior_failures']:raise ValueError('prior_failure_retained')
                old=cache.get(native,{})
                if old.get('error_at') or not old.get('turn_id'):raise ValueError('watermark_unconfirmed')
                path=Path(paths[native]);before=path.stat();fingerprint=[before.st_size,before.st_mtime_ns]
                if old.get('file')!=fingerprint:raise ValueError('pending_source_change')
                rows=db.execute('SELECT turn_id,reply FROM captured_replies WHERE thread_id=? ORDER BY rowid',(native,)).fetchall()
                if not rows or rows[-1][0]!=old['turn_id']:raise ValueError('capture_watermark_mismatch')
                reply_turns={turn for turn,_ in rows}
                for table in ('captured_user_messages','captured_turn_order','captured_request_ids'):
                    turns={r[0] for r in db.execute('SELECT DISTINCT turn_id FROM '+table+' WHERE thread_id=?',(native,))}
                    if not turns<=reply_turns or (table!='captured_request_ids' and turns!=reply_turns):
                        raise ValueError('partial_turn_records_unconfirmed')
                cost=min(before.st_size,capture.TAIL_BYTES)+before.st_size*len(rows)
                if cost>capture.TOTAL_SCAN:raise ValueError('source_revalidation_budget')
                if cost>budget:break
                budget-=cost
                events,_=capture.completed_events(path,native,home)
                if not events or events[-1][1]['turn-id']!=old['turn_id']:raise ValueError('completion_chain_unconfirmed')
                turn_ids=[event['turn-id'] for _,event in events]
                first=turn_ids.index(rows[0][0])
                if turn_ids[first:]!=[turn for turn,_ in rows]:raise ValueError('stored_completion_chain_gap')
                completions={event['turn-id']:(stamp,event['last-assistant-message']) for stamp,event in events}
                for turn,reply in rows:
                    users,stamp=read_turn(index,native,turn)
                    stored=db.execute('SELECT message_id,text,created_at,input_digest FROM captured_user_messages WHERE thread_id=? AND turn_id=?',(native,turn)).fetchall()
                    order=db.execute('SELECT completed_at FROM captured_turn_order WHERE thread_id=? AND turn_id=?',(native,turn)).fetchone()
                    if not users or sorted(users)!=sorted(stored) or order!=(stamp,) or completions.get(turn)!=(stamp,reply):
                        raise ValueError('stored_turn_unconfirmed')
                after=path.stat()
                if [after.st_size,after.st_mtime_ns]!=fingerprint:raise ValueError('source_changed_during_revalidation')
                state['proofs'][native]={'path':str(path),'resolved':str(path.resolve()),'fingerprint':fingerprint}
            except Exception as error:
                reason=str(error) if isinstance(error,ValueError) else 'source_unavailable'
            state['results'][native]=reason
    capture.atomic_json(directory/'recovery.json',state)
    return {'covered':len(state['results']),'total':len(state['targets'])}


def finalize(database,directory):
    state=json.loads((directory/'recovery.json').read_text())
    if set(state['results'])!=set(state['targets']):raise ValueError('recovery_incomplete')
    blocked=[native for native,reason in state['results'].items() if reason]
    if len(blocked)>LIMIT:raise ValueError('recovery_failure_capacity')
    rebuilt=directory/'rebuilt.sqlite'
    if health_path(rebuilt).exists():raise ValueError('recovery_already_finalized')
    update_health(rebuilt,'initial','initial',None)
    for native in blocked:update_health(rebuilt,native,'recovery','recovery_pending')
    new=read_health(rebuilt)
    if new['overflow'] or len(new['failures'])!=len(blocked):raise ValueError('recovery_status_unconfirmed')
    if state['binding']!=binding(database):raise ValueError('recovery_capture_changed')
    if state['index_signature']!=index_signature(Path(state['codex_home'])/'state_5.sqlite',state['targets']):
        raise ValueError('recovery_index_changed')
    for native,proof in state['proofs'].items():
        path=Path(proof['path']);stat=path.stat()
        if str(path.resolve())!=proof['resolved'] or [stat.st_size,stat.st_mtime_ns]!=proof['fingerprint']:
            raise ValueError('recovery_source_changed')
    os.replace(health_path(rebuilt),health_path(database))
    if os.name!='nt':
        fd=os.open(database.parent,os.O_RDONLY|os.O_DIRECTORY)
        try:os.fsync(fd)
        finally:os.close(fd)
    return {'covered':len(state['targets']),'blocked':len(blocked),'healthy':len(state['targets'])-len(blocked),'overflow':False}
