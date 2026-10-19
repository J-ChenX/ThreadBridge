"""Question-only capture, phone API, exact queued replies, reconciliation and cleanup."""
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
import urllib.error
import urllib.request
import uuid


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        env = dict(os.environ, HOME=directory, CODEX_HOME=directory)
        hub, capture, index, rollout = [root/n for n in ('hub.sqlite','capture.sqlite','index.sqlite','rollout.jsonl')]
        def run(*args):
            return subprocess.check_output([binary,*map(str,args)],env=env,text=True).strip()
        run('register-agent','--db',hub,'--name','question-fixture','--output',root/'agent.json')
        host=json.loads((root/'agent.json').read_text())['host_id']
        native,turn=str(uuid.uuid4()),str(uuid.uuid4())
        tid=hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest()
        call='call_question'
        question='路由器是否启用了双 WAN？'
        question_id=json.dumps(['request_user_input_async',call,0],separators=(',',':'))
        mid='question:'+hashlib.sha256(question_id.encode()).hexdigest()
        def record(payload):
            return {'type':'response_item','timestamp':'2026-10-19T01:02:03Z','payload':payload}
        rows=[{'type':'session_meta','payload':{'id':native}},
              record({'type':'function_call','name':'request_user_input_async','call_id':call,'arguments':json.dumps({'questions':[{'title':question,'options':['已启用','未启用']}]}),'internal_chat_message_metadata_passthrough':{'turn_id':turn}}),
              record({'type':'function_call_output','call_id':call,'output':'{"accepted":true}'}),
              record({'type':'function_call','name':'other_tool','arguments':'TOOL_SECRET'})]
        def backfill():
            rollout.write_text(''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in rows))
            run('capture-backfill','--database',capture,'--index',index,'--thread',native)
            run('capture-import','--db',hub,'--capture-db',capture,'--host',host,'--thread',native)
        with sqlite3.connect(index) as c:
            c.execute('CREATE TABLE threads(id TEXT,rollout_path TEXT,thread_source TEXT,archived INTEGER)')
            c.execute('INSERT INTO threads VALUES(?,?,?,0)',(native,str(rollout),'user'))
        backfill()
        with sqlite3.connect(hub) as c:
            assert c.execute('SELECT count(*) FROM messages').fetchone()[0]==1
            assert c.execute('SELECT role FROM messages').fetchone()[0]=='question'
            before=c.execute('SELECT revision FROM thread_message_state WHERE thread=?',(tid,)).fetchone()[0]
            c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
            c.execute('UPDATE devices SET last_seen=? WHERE id=?',(int(time.time()),host))
            c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?',(int(time.time()),tid))
            c.execute("UPDATE threads SET can_send=1,status='queue_ready' WHERE id=?",(tid,))
        with socket.socket() as sock:
            sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
        process=subprocess.Popen([binary,'hub','--db',hub,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
        def api(path,payload=None,method=None):
            data=None if payload is None else json.dumps(payload).encode()
            req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=data,method=method,headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
            with opener.open(req,timeout=3) as response:return json.load(response)
        def ready():
            # Hub startup invalidates stale device liveness; only the synthetic host is refreshed.
            with sqlite3.connect(hub) as c:
                c.execute('UPDATE devices SET last_seen=? WHERE id=?',(int(time.time()),host))
                c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?',(int(time.time()),tid))
                c.execute("UPDATE threads SET can_send=1,status='queue_ready' WHERE id=?",(tid,))
        reply='<send_user_message_question_reply>\n'+json.dumps([{'questionItemId':question_id,'question':question,'answer':'未启用\n自定义说明'}],ensure_ascii=False)+'\n</send_user_message_question_reply>\n'
        try:
            for _ in range(100):
                try:api('/health');break
                except OSError:time.sleep(.05)
            ready()
            data=api(f'/v1/threads/{tid}/messages')['messages']
            assert len(data)==1 and data[0]['id']==mid
            q=json.loads(data[0]['text'])
            assert q['title']==question and q['status']=='pending' and len(q['options'])==2
            # Replay cannot generate extra unread changes or duplicates.
            backfill();ready()
            with sqlite3.connect(hub) as c:
                assert c.execute('SELECT revision FROM thread_message_state WHERE thread=?',(tid,)).fetchone()[0]==before
            body={'request_id':str(uuid.uuid4()),'thread_id':tid,'text':reply,'expected_revision':'','created_at':int(time.time()),'kind':'send'}
            result=api('/v1/commands',body)
            assert result['status']=='accepted'
            assert api('/v1/commands',body)['id']==result['id']
            with sqlite3.connect(hub) as c:
                queued=json.loads(c.execute('SELECT payload FROM commands').fetchone()[0])
                assert queued['native_id']==native and queued['text']==reply
                c.execute("UPDATE commands SET status='codex_accepted' WHERE id=?",(result['id'],))
            # An actual human question-reply source record, rather than queue ACK, resolves the card.
            rows.append(record({'type':'message','role':'user','id':'reply','content':[{'type':'input_text','text':reply}],'internal_chat_message_metadata_passthrough':{'turn_id':turn,'create_time':1792371724,'content_item_kinds':['user.text']}}))
            backfill();ready()
            q=next(json.loads(m['text']) for m in api(f'/v1/threads/{tid}/messages')['messages'] if m['id']==mid)
            assert q['status']=='answered' and q['answer']=='未启用\n自定义说明'
            user=next(m for m in api(f'/v1/threads/{tid}/messages')['messages'] if m['role']=='user')
            assert user['text']==question+'\n\n回答：未启用\n自定义说明'
            assert 'questionItemId' not in user['text']
            body['request_id']=str(uuid.uuid4())
            try:api('/v1/commands',body);raise AssertionError('stale answer accepted')
            except urllib.error.HTTPError as error:
                assert error.code in (400,409)
                assert 'question_no_longer_pending' in error.read().decode()
            # Restarting the collector keeps the state and one stable question message.
            backfill()
            with sqlite3.connect(hub) as c:
                assert c.execute("SELECT count(*) FROM messages WHERE role='question'").fetchone()[0]==1
                assert c.execute('SELECT count(*) FROM commands').fetchone()[0]==1
            assert b'TOOL_SECRET' not in capture.read_bytes() and b'TOOL_SECRET' not in hub.read_bytes()
            api(f'/v1/threads/{tid}/delete',{})
            backfill()
            with sqlite3.connect(hub) as c:assert c.execute('SELECT count(*) FROM messages WHERE thread=?',(tid,)).fetchone()[0]==0
            assert rollout.exists()
        finally:
            process.terminate();process.wait(timeout=3)
    print('PASS: questions visible before any completion; exact reply queue, replay, stale answer rejection and cleanup')


if __name__=='__main__':main()
