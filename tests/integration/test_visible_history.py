"""Rebuild visible chat, including photos and live commentary, without tool cards."""
import base64
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
import urllib.error
import uuid


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        env = dict(os.environ, HOME=directory, CODEX_HOME=directory)
        hub, capture, index, rollout = [root / n for n in ['hub.sqlite', 'capture.sqlite', 'index.sqlite', 'rollout.jsonl']]
        def run(*args):
            r=subprocess.run([binary, *map(str, args)], env=env, capture_output=True, text=True)
            assert r.returncode==0,r.stderr
            return r.stdout.strip()
        run('register-agent', '--db', hub, '--name', 'fixture', '--output', root/'agent.json')
        host = json.loads((root/'agent.json').read_text())['host_id']
        native, turn, live = [str(uuid.uuid4()) for _ in range(3)]
        png = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jC1sAAAAASUVORK5CYII=')
        image = hashlib.sha256(png).hexdigest()
        def item(payload, stamp='1970-01-01T00:01:40.000Z'):
            return {'type':'response_item', 'timestamp':stamp, 'payload':payload}
        def user(mid, content, kinds, tid=turn, at=100):
            return item({'type':'message','role':'user','id':mid,'content':content,'internal_chat_message_metadata_passthrough':{'turn_id':tid,'create_time':at,'content_item_kinds':kinds}})
        def assistant(mid, text, phase='commentary', tid=turn, stamp='1970-01-01T00:01:41.000Z'):
            return item({'type':'message','role':'assistant','id':mid,'phase':phase,'content':[{'type':'output_text','text':text}],'internal_chat_message_metadata_passthrough':{'turn_id':tid}},stamp)
        rows = [{'type':'session_meta','payload':{'id':native}},
                user('photo',[{'type':'input_text','text':'\n# Files mentioned by the user:\nignored path\n## My request:\n照片问题'},{'type':'input_text','text':'<image name="a" path="/tmp/private.png">'},{'type':'input_image','image_url':'data:image/png;base64,'+base64.b64encode(png).decode()},{'type':'input_text','text':'</image>'}],['user.text','user.text','user.image','user.text']),
                user('env',[{'type':'input_text','text':'ENVIRONMENT_SECRET'}],['environments.environment_context']),
                assistant('thinking','正在检查来源'),
                item({'type':'function_call','name':'edited_9_files','arguments':'TOOL_SECRET'}),
                item({'type':'function_call_output','output':'TOOL_SECRET'}),
                assistant('reasoning','REASONING_SECRET','analysis'),
                assistant('final','已找到原因','final_answer',stamp='1970-01-01T00:01:42.000Z'),
                {'type':'event_msg','timestamp':'1970-01-01T00:01:42.100Z','payload':{'type':'task_complete','turn_id':turn,'last_agent_message':'已找到原因'}},
                user('live',[{'type':'input_text','text':'继续检查'},{'type':'input_text','text':'，不要显示文件卡片'}],['user.text','user.text'],live,103),
                assistant('live-comment','继续检查中',tid=live,stamp='1970-01-01T00:01:44.000Z')]
        rollout.write_text(''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in rows))
        with sqlite3.connect(index) as c:
            c.execute('CREATE TABLE threads(id TEXT,rollout_path TEXT,thread_source TEXT,archived INTEGER)')
            c.execute('INSERT INTO threads VALUES(?,?,?,0)',(native,str(rollout),'user'))
        event={'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'last-assistant-message':'已找到原因'}
        run('capture','--all-tasks','--database',capture,'--title','fixture',json.dumps(event))
        for _ in range(2):
            assert run('capture-backfill','--database',capture,'--index',index,'--thread',native)=='4'
            run('capture-import','--db',hub,'--capture-db',capture,'--host',host,'--thread',native)
        with rollout.open('a') as f:f.write('{"partial_live_record":')
        assert run('capture-backfill','--database',capture,'--index',index,'--thread',native)=='4'
        with sqlite3.connect(hub) as c:
            tid=c.execute('SELECT id FROM threads').fetchone()[0]
            messages=c.execute('SELECT id,role,body,ordinal FROM messages ORDER BY ordinal,id').fetchall()
            assert [r[0] for r in messages]==['input:photo','native:thinking','notify:'+turn,'input:live','native:live-comment'],messages
            assert messages[0][2]=='照片问题\n![图片](threadbridge-image:'+image+')\n'
            assert messages[2][3]==102100
            assert len(messages)==5
            c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
        with socket.socket() as s:
            s.bind(('127.0.0.1',0));port=s.getsockname()[1]
        process=subprocess.Popen([binary,'hub','--db',hub,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
        def get(path,token='fixture-phone'):
            req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,headers={'Authorization':'Bearer '+token})
            with opener.open(req,timeout=2) as response:return json.load(response)
        path=f'/v1/threads/{tid}/messages/input%3Aphoto/images/{image}'
        try:
            deadline=time.monotonic()+5
            while True:
                try:get('/health');break
                except OSError:
                    if time.monotonic()>deadline:raise
                    time.sleep(.05)
            data=get(path)
            assert data['mime']=='image/png' and base64.b64decode(data['base64'])==png
            assert len(get(f'/v1/threads/{tid}/messages')['messages'])==5
            before=get('/v1/threads')['threads'][0]
            assert before['revision']==turn and before['message_activity_at']==104000
            # Same-second (and same-timestamp) running-turn updates change the
            # read marker without replacing the last completed turn ID.
            new_comment=assistant('live-comment-2','仍在检查',tid=live,stamp='1970-01-01T00:01:44.000Z')
            rows.append(new_comment)
            rollout.write_text(''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in rows))
            assert run('capture-backfill','--database',capture,'--index',index,'--thread',native)=='5'
            run('capture-import','--db',hub,'--capture-db',capture,'--host',host,'--thread',native)
            after=get('/v1/threads')['threads'][0]
            assert after['revision']==turn and after['updated_at']==before['updated_at']
            assert after['message_revision']>before['message_revision']
            run('capture-import','--db',hub,'--capture-db',capture,'--host',host,'--thread',native)
            assert get('/v1/threads')['threads'][0]['message_revision']==after['message_revision']
            try:get(path,'wrong');raise AssertionError('missing auth')
            except urllib.error.HTTPError as e:assert e.code==401
            with sqlite3.connect(hub) as c:
                c.execute('DELETE FROM messages WHERE id=?',('input:photo',))
            try:get(path);raise AssertionError('deleted message asset readable')
            except urllib.error.HTTPError as e:assert e.code==404
        finally:
            process.terminate();process.wait(timeout=3)
        # Older final inserted last must not move the completed revision backwards.
        older=str(uuid.uuid4())
        old_rows=[user('older',[{'type':'input_text','text':'较早问题'}],['user.text'],older,90),
                  assistant('older-final','较早回复','final_answer',older,'1970-01-01T00:01:31.000Z'),
                  {'type':'event_msg','timestamp':'1970-01-01T00:01:31.100Z','payload':{'type':'task_complete','turn_id':older,'last_agent_message':'较早回复'}}]
        rollout.write_text(''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in [rows[0],*old_rows,*rows[1:]]))
        assert run('capture-backfill','--database',capture,'--index',index,'--thread',native)=='6'
        run('capture-import','--db',hub,'--capture-db',capture,'--host',host,'--thread',native)
        with sqlite3.connect(hub) as c:
            assert c.execute('SELECT revision FROM threads').fetchone()[0]==turn
            assert c.execute("SELECT body FROM messages WHERE id=?",('notify:'+older,)).fetchone()[0]=='较早回复'
        with sqlite3.connect(capture) as c:
            assert c.execute('SELECT captured_at FROM captured_replies WHERE turn_id=?',(older,)).fetchone()[0]==91
        args=['cleanup-archived','--db',hub,'--capture-db',capture,'--index',index,'--host',host,'--apply','--backup-dir',root/'backup']
        with sqlite3.connect(index) as c:c.execute('UPDATE threads SET archived=1')
        Path(str(capture)+'.collection.json').write_text(json.dumps({'schema':1,'generation':str(uuid.uuid4()),'cutoff_ms':1,'excluded':[]}))
        run(*args)
        for database,table in [(capture,'captured_images'),(capture,'captured_visible_messages'),(hub,'attachments'),(hub,'thread_message_state')]:
            with sqlite3.connect(database) as c:assert c.execute(f'SELECT count(*) FROM {table}').fetchone()[0]==0
        print('visible history: photos, repeated human text, live commentary, ordered canonical finals, replay, auth and cleanup pass; tools and reasoning excluded')


if __name__=='__main__':main()
