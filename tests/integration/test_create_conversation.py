"""First-message creation through real loopback Hub/bridge, fixed mock CLI only."""
from contextlib import closing
import hashlib,json,os,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
from test_conversation_sync import save_turn

def scenario(binary,ack):
 repo=Path(__file__).resolve().parents[2]
 with tempfile.TemporaryDirectory(prefix='tb-create-') as tmp:
  root=Path(tmp);db=root/'hub.sqlite';source=root/'capture.sqlite';fake=root/'codex';fake.symlink_to(repo/'tests/fixtures/mock_codex.py');native='00000000-0000-4000-8000-000000000001';root.joinpath('fixture.json').write_text(json.dumps({'mode':'create','native_id':native,'reply':ack}))
  env={'PATH':'/usr/bin:/bin','HOME':str(root),'CODEX_HOME':str(root)}
  baseline='00000000-0000-4000-8000-000000000002';turn='00000000-0000-4000-8000-000000000003'
  event={'thread-id':baseline,'turn-id':turn,'type':'agent-turn-complete','last-assistant-message':'baseline','cwd':str(root)}
  subprocess.run([binary,'capture','--thread',baseline,'--database',source,json.dumps(event)],check=True,stdout=subprocess.DEVNULL,env=env)
  with closing(sqlite3.connect(db)) as c,c:
   c.execute('CREATE TABLE devices(id TEXT PRIMARY KEY,token TEXT UNIQUE,role TEXT NOT NULL,name TEXT NOT NULL,expires INTEGER NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,last_seen INTEGER NOT NULL DEFAULT 0)')
   c.execute("INSERT INTO devices(id,role,name,expires) VALUES('host','agent','Fixture',?)",(int(time.time())+300,))
   c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','Fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
  with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
  opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
  def api(path,body=None):
   request=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
   with opener.open(request,timeout=2) as response:return json.load(response)
  def wait(fn):
   deadline=time.monotonic()+15
   while time.monotonic()<deadline:
    try:
     result=fn()
     if result:return result
    except (OSError,KeyError,IndexError):pass
    time.sleep(.1)
   raise AssertionError('creation condition timed out')
  hub=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);worker=None
  try:
   wait(lambda:api('/health'))
   def bridge():return subprocess.Popen([binary,'capture-bridge','--db',db,'--capture-db',source,'--host','host','--codex',fake,'--allow-queue','--verified-version','codex-cli fixture'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   worker=bridge();wait(lambda:api('/v1/threads')['threads'])
   payload={'request_id':str(uuid.uuid4()),'host_id':'host','project':str(root),'text':'phone starts here','created_at':int(time.time())}
   command=api('/v1/threads/create',payload);assert api('/v1/threads/create',payload)['id']==command['id']
   # dispatching also covers the local claim, before a native thread exists.
   receipt=wait(lambda:(lambda r:r if r['status']==('dispatching' if ack else 'unknown') and r.get('result_thread_id') and (not ack or r.get('native_turn_id')) else None)(api('/v1/commands/'+command['id'])))
   tid=hashlib.sha256(('host\0default\0'+native).encode()).hexdigest();assert receipt['result_thread_id']==tid, receipt
   assert receipt['native_turn_id']==('00000000-0000-4000-8000-000000000009' if ack else None)
   row=next(r for r in api('/v1/threads')['threads'] if r['id']==tid);assert row['native_id']==native and row['project']==str(root)
   assert root.joinpath('calls').read_text()=='start\nname\nturn\n'
   final_turn='00000000-0000-4000-8000-000000000009';stamp=int(time.time()*1000)+1
   if ack:
    # An unrelated captured turn must not settle an acknowledged first turn.
    unrelated={'thread-id':native,'turn-id':str(uuid.uuid4()),'type':'agent-turn-complete','last-assistant-message':'unrelated','cwd':str(root)}
    subprocess.run([binary,'capture','--thread',native,'--database',source,json.dumps(unrelated)],check=True,stdout=subprocess.DEVNULL,env=env)
    time.sleep(2.2)
    assert api('/v1/commands/'+command['id'])['status']=='dispatching'
   save_turn(source,native,final_turn,[('new-human','phone starts here',stamp,hashlib.sha256(b'phone starts here').hexdigest())],stamp+1)
   completed={'thread-id':native,'turn-id':final_turn,'type':'agent-turn-complete','last-assistant-message':'first created reply','cwd':str(root)}
   subprocess.run([binary,'capture','--thread',native,'--database',source,'--title','phone starts here',json.dumps(completed)],check=True,stdout=subprocess.DEVNULL,env=env)
   wait(lambda:api('/v1/commands/'+command['id'])['status']=='codex_accepted')
   assert [m['role'] for m in reversed(api('/v1/threads/'+tid+'/messages')['messages'])]==(['assistant','user','assistant'] if ack else ['user','assistant'])
   worker.terminate();worker.wait(timeout=3);worker=bridge();time.sleep(2.2)
   assert root.joinpath('calls').read_text()=='start\nname\nturn\n';assert api('/v1/commands/'+payload['request_id'])['result_thread_id']==tid
   assert api('/v1/threads?q=phone%20here')['threads'][0]['id']==tid
   print('PASS real Hub -> durable creation -> existing executor turn/start; '+('authoritative turn completion' if ack else 'lost turn acknowledgement')+'; native identity, search and restart no resend')
  finally:
   if worker:worker.terminate();worker.wait(timeout=3)
   hub.terminate();hub.wait(timeout=3)
def main():
 binary=Path(sys.argv[1]).resolve()
 for ack in (True,False):scenario(binary,ack)
if __name__=='__main__':main()
