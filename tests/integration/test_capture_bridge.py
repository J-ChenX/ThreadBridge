#!/usr/bin/python3
"""Temporary loopback Hub, synthetic phone and mock CLI only; no real sends."""
from contextlib import closing
import hashlib,json,os,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from capture_user_turn import save_turn
from pathlib import Path
def main():
    binary=Path(sys.argv[1]).resolve()
    receiver=Path(sys.argv[2]).resolve()
    thread='00000000-0000-4000-8000-000000000001'
    turn='00000000-0000-4000-8000-000000000002'
    thread2='00000000-0000-4000-8000-000000000006'
    all_threads='--all' in sys.argv[3:]
    with tempfile.TemporaryDirectory(prefix='tb-mock-bridge-') as tmp:
     root=Path(tmp);db=root/'hub.sqlite';capture=root/'capture.sqlite';calls=root/'calls';mode=root/'mode';mode.write_text('ack')
     env={'PATH':'/usr/bin:/bin','HOME':str(root)}
     event={'thread-id':thread,'turn-id':turn,'type':'agent-turn-complete','last-assistant-message':'baseline'}
     def emit(e):subprocess.run([sys.executable,receiver,'--thread',e['thread-id'],'--database',capture,json.dumps(e)],check=True,stdout=subprocess.DEVNULL,env=env)
     clock=int(time.time())*1000
     save_turn(capture,thread,turn,[('desktop-input','desktop hello',clock-500,hashlib.sha256(b'desktop hello').hexdigest())],clock-100)
     emit(event)
     with closing(sqlite3.connect(db)) as c, c:
      c.execute('CREATE TABLE devices(id TEXT PRIMARY KEY,token TEXT UNIQUE,role TEXT NOT NULL,name TEXT NOT NULL,expires INTEGER NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,last_seen INTEGER NOT NULL DEFAULT 0)')
      c.execute("INSERT INTO devices(id,role,name,expires) VALUES('host','agent','fixture',?)",(int(time.time())+300,))
      c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
     fake=root/'mock-codex'
     fake.write_text(f"""#!{sys.executable}
import json,sys
from pathlib import Path
if sys.argv[1]=='--version':
 print('codex-cli fixture');sys.exit()
assert sys.argv[1]=='queue'
native=sys.argv[sys.argv.index('--thread')+1]
assert native in {[thread,thread2]!r}
with open({str(calls)!r},'a') as f:f.write(json.dumps(sys.argv)+'\\n')
if Path({str(mode)!r}).read_text()=='ack':
 print('Queued message 00000000-0000-4000-8000-000000000004 for thread '+native+'.')
else:print('uncertain')
""");fake.chmod(0o700)
     with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
     opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
     def api(path,body=None):
      data=None if body is None else json.dumps(body).encode()
      req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=data,headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
      with opener.open(req,timeout=2) as r:return json.load(r)
     def wait(fn):
      deadline=time.monotonic()+12
      while time.monotonic()<deadline:
       try:
        v=fn()
        if v:return v
       except (OSError,KeyError,IndexError):pass
       time.sleep(.1)
      raise AssertionError('mock condition timed out')
     def worker():return subprocess.Popen([binary,'capture-bridge','--db',db,'--capture-db',capture,'--host','host',* ([] if all_threads else ['--thread',thread]),'--codex',fake,'--allow-queue','--verified-version','codex-cli fixture'],env=env,stdout=subprocess.DEVNULL)
     hub=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);bridge=None
     try:
      wait(lambda:api('/health'))
      bridge=worker()
      row=wait(lambda:next((r for r in api('/v1/threads')['threads'] if r['can_send']),None));tid=row['id']
      def submit(text):
       r=api('/v1/threads')['threads'][0]
       return api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':tid,'text':text,'expected_revision':r['revision'],'created_at':int(time.time()),'kind':'send'})['id']
      cmd=submit('explicit mock phone click')
      wait(lambda:api('/v1/commands/'+cmd)['status']=='upstream_queued')
      assert api('/v1/commands/'+cmd)['native_turn_id'] is None
      args=json.loads(calls.read_text().splitlines()[0]);text=args[args.index('--message')+1]
      assert text=='explicit mock phone click'
      complete=dict(event,**{'turn-id':'00000000-0000-4000-8000-000000000005','last-assistant-message':'mock final','input-messages':[text]});save_turn(capture,thread,complete['turn-id'],[('phone-input',text,int(time.time()*1000),hashlib.sha256(text.encode()).hexdigest())],int(time.time()*1000)+1);emit(complete)
      wait(lambda:api('/v1/commands/'+cmd)['status']=='codex_accepted')
      assert api('/v1/commands/'+cmd)['native_turn_id']==complete['turn-id']
      msgs=api('/v1/threads/'+tid+'/messages')['messages'];assert len(msgs)==4
      chronological=list(reversed(msgs));assert [m['role'] for m in chronological]==['user','assistant','user','assistant'];assert chronological[0]['text']=='desktop hello';assert chronological[2]['text']==text
      cursor=api('/v1/events?after=0')['cursor'];emit(complete)
      bridge.terminate();bridge.wait(timeout=3);bridge=worker();time.sleep(3)
      assert api('/v1/events?after=0')['cursor']==cursor
      assert len(calls.read_text().splitlines())==1
      mode.write_text('unknown');cmd2=submit('uncertain mock click')
      wait(lambda:api('/v1/commands/'+cmd2)['status']=='unknown')
      bridge.terminate();bridge.wait(timeout=3);bridge=worker();time.sleep(3)
      assert len(calls.read_text().splitlines())==2
      assert api('/v1/commands/'+cmd2)['status']=='unknown'
      assert api('/v1/threads/'+tid+'/messages')['messages']==msgs
      with closing(sqlite3.connect(db)) as c, c:assert c.execute('SELECT count(*) FROM outbox').fetchone()[0]==0
      if all_threads:
       # A conversation created after worker startup must become writable without restart.
       emit(dict(event, **{'thread-id':thread2,'last-assistant-message':'second baseline'}))
       second=wait(lambda:next((r for r in api('/v1/threads')['threads'] if r['native_id']==thread2 and r['can_send']),None))
       mode.write_text('ack')
       cmd3=api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':second['id'],'text':'second conversation phone click','expected_revision':second['revision'],'created_at':int(time.time()),'kind':'send'})['id']
       wait(lambda:api('/v1/commands/'+cmd3)['status']=='upstream_queued')
       lines=calls.read_text().splitlines();assert len(lines)==3
       args=json.loads(lines[-1]);assert args[args.index('--thread')+1]==thread2
       text=args[args.index('--message')+1]
       save_turn(capture,thread2,'00000000-0000-4000-8000-000000000007',[('second-input',text,int(time.time()*1000),hashlib.sha256(text.encode()).hexdigest())],int(time.time()*1000)+1)
       emit(dict(event, **{'thread-id':thread2,'turn-id':'00000000-0000-4000-8000-000000000007','last-assistant-message':'second final','input-messages':[text]}))
       wait(lambda:api('/v1/commands/'+cmd3)['status']=='codex_accepted')
       assert api('/v1/threads/'+tid+'/messages')['messages']==msgs
       bridge.terminate();bridge.wait(timeout=3);bridge=worker()
       wait(lambda:all(r['can_send'] for r in api('/v1/threads')['threads']))
       time.sleep(3);assert len(calls.read_text().splitlines())==3
       print('PASS: default all conversations; dynamic discovery; native thread routing; restart without resend')
      print('PASS: mock phone HTTP -> scoped queue -> notify capture -> final reply; restart/replay and uncertain no resend; no real Codex/phone/credentials')
     finally:
      if bridge:bridge.terminate();bridge.wait(timeout=3)
      hub.terminate();hub.wait(timeout=3)

if __name__=="__main__":main()
