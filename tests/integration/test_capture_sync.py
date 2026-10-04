#!/usr/bin/env python3
"""Isolated loopback Hub/phone API verification; synthetic identities, no Codex calls."""
from contextlib import closing
import hashlib
import json
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request

def main():
    binary = Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/threadbridge').resolve()
    thread = '00000000-0000-4000-8000-000000000001'
    turn = '00000000-0000-4000-8000-000000000002'
    with tempfile.TemporaryDirectory(prefix='threadbridge-capture-sync-') as tmp:
        root = Path(tmp)
        capture = root/'capture.sqlite'
        hubdb = root/'hub.sqlite'
        with closing(sqlite3.connect(capture)) as db, db:
            db.execute('CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER)')
            db.execute('INSERT INTO captured_replies VALUES (?,?,?,?,?)', (thread,turn,'TB-NOTIFY-OK',12,1))
        # Fixed synthetic test identities, no new real credentials.
        with closing(sqlite3.connect(hubdb)) as db, db:
            db.execute('CREATE TABLE devices(id TEXT PRIMARY KEY,token TEXT UNIQUE,role TEXT NOT NULL,name TEXT NOT NULL,expires INTEGER NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,last_seen INTEGER NOT NULL DEFAULT 0)')
            db.execute("INSERT INTO devices(id,role,name,expires) VALUES ('fixture-host','agent','fixture',?)", (int(time.time())+60,))
            db.execute("INSERT INTO devices(id,token,role,name,expires) VALUES ('fixture-phone',?,'phone','fixture',?)", (hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+60))
        env = {'PATH':'/usr/bin:/bin','HOME':str(root)}
        with socket.socket() as sock:
            sock.bind(('127.0.0.1',0)); port = sock.getsockname()[1]
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        def get(path):
            req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,headers={'Authorization':'Bearer fixture-phone'})
            with opener.open(req,timeout=2) as r:return json.load(r)
        def start():
            p=subprocess.Popen([binary,'hub','--db',hubdb,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            deadline=time.monotonic()+5
            while time.monotonic()<deadline:
                try:get('/health');return p
                except OSError:time.sleep(.05)
            p.terminate();p.wait();raise AssertionError('isolated Hub did not start')
        argv=[binary,'capture-import','--db',hubdb,'--capture-db',capture,'--host','fixture-host','--thread',thread]
        process=start()
        try:
            subprocess.run(argv,env=env,check=True,stdout=subprocess.DEVNULL)
            rows=get('/v1/threads')['threads'];assert len(rows)==1 and not rows[0]['can_send']
            tid=rows[0]['id']
            messages=get(f'/v1/threads/{tid}/messages')['messages']
            assert len(messages)==1 and messages[0]['text']=='TB-NOTIFY-OK' and messages[0]['turn_id']==turn
            events=get('/v1/events?after=0')['cursor']
            process.terminate();process.wait(timeout=3)
            process=start()
            subprocess.run(argv,env=env,check=True,stdout=subprocess.DEVNULL)
            assert get('/v1/events?after=0')['cursor']==events
            assert get(f'/v1/threads/{tid}/messages')['messages']==messages
            with closing(sqlite3.connect(hubdb)) as db, db:assert db.execute('SELECT count(*) FROM outbox').fetchone()[0]==0
            print(json.dumps({'result':'pass','scope':'isolated loopback Hub and phone HTTP API','checks':['captured turn/text exact','read-only target','event sync','Hub restart persistence','replay idempotency','no extra notifications'],'real_phone':False,'real_codex_send':False}))
        finally:
            process.terminate();process.wait(timeout=3)

if __name__=="__main__":main()
