#!/usr/bin/env python3
"""Synthetic HTTP -> real Rust resume CLI -> mock stdio -> Rust final capture."""
from contextlib import closing
import hashlib,json,os,signal,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
HOST='00000000-0000-4000-8000-000000000051'
NATIVE='00000000-0000-4000-8000-000000000052'
PRIOR='00000000-0000-4000-8000-000000000053'
TURN='00000000-0000-4000-8000-000000000054'
OTHER='00000000-0000-4000-8000-000000000055'

# Immutable fixture program: symlink this file into each test's private directory.
# It models only public App Server messages; no production Codex is invoked.
def mock():
 root=Path(sys.argv[0]).absolute().parent
 if Path(sys.argv[0]).name=='systemctl':return int((root/'competitor').read_text())
 if sys.argv[1:]==['--version']:print('codex-cli fixture');return 0
 assert sys.argv[1:]==['app-server']
 def emit(row):print(json.dumps(row),flush=True)
 for line in sys.stdin:
  row=json.loads(line);method=row['method'];params=row.get('params',{})
  with (root/'calls.jsonl').open('a') as f:f.write(json.dumps({'method':method,'thread':params.get('threadId'),'text':params.get('input',[{}])[0].get('text')})+'\n')
  if 'id' not in row:continue
  if method=='initialize':result={}
  elif method in ('thread/read','thread/resume'):
   result={'thread':{'id':NATIVE,'cwd':str(root),'ephemeral':False,'forkedFromId':None,'status':{'type':'idle'},'canAcceptDirectInput':True},'approvalPolicy':'on-request','approvalsReviewer':'user','sandbox':{'type':'readOnly','writableRoots':[],'networkAccess':False}}
  elif method=='thread/turns/list':
   assert params['itemsView']=='summary' and params['limit']==1
   result={'data':[{'id':PRIOR,'status':'completed'}]}
  elif method=='turn/start':
   assert params['threadId']==NATIVE and params['approvalPolicy']=='on-request' and params['approvalsReviewer']=='user' and params['sandboxPolicy']=={'type':'readOnly','networkAccess':False}
   assert params['input']==[{'type':'text','text':'synthetic explicit phone action'}]
   emit({'id':row['id'],'result':{'turn':{'id':TURN,'status':'inProgress'}}})
   if (root/'pause-final').exists():
    (root/'turn-running').write_text('1')
    while not (root/'allow-final').exists():time.sleep(.005)
   emit({'method':'item/completed','params':{'threadId':NATIVE,'turnId':TURN,'item':{'type':'agentMessage','phase':'final_answer','text':'synthetic final'}}})
   emit({'method':'turn/completed','params':{'threadId':NATIVE,'turn':{'id':TURN,'status':'completed'}}});continue
  else:raise AssertionError(method)
  emit({'id':row['id'],'result':result})
 return 0

def main():
 binary=Path(sys.argv[1]).resolve();is_worker='--worker' in sys.argv
 with tempfile.TemporaryDirectory(prefix='tb-resume-phone-') as temp:
  root=Path(temp);db=root/'hub.sqlite';capture_db=root/'capture.sqlite';codex=root/'mock-codex';calls=root/'calls.jsonl';grant_path=root/'grant.json'
  codex.symlink_to(Path(__file__).resolve());(root/'systemctl').symlink_to(Path(__file__).resolve());(root/'competitor').write_text('3')
  env={**os.environ,'PATH':temp+os.pathsep+str(Path(sys.executable).parent)+os.pathsep+os.environ.get('PATH',''),'HOME':temp}
  grant={'native_id':NATIVE,'host_id':HOST,'cwd':temp,'verified_cli_version':'codex-cli fixture','expires_at':int(time.time())+120,'tool_compatibility_confirmed':True,'handoff_confirmed':True,'approval_policy':'on-request','sandbox_mode':'read-only'}
  grant_path.write_text(json.dumps(grant));digest=hashlib.sha256(json.dumps(grant,sort_keys=True,separators=(',',':')).encode()).hexdigest()
  resume=[binary,'resume','--db',db,'--capture-db',capture_db,'--codex',codex,'--grant',grant_path,'--execute']
  for native,turn in [(OTHER,OTHER),(NATIVE,PRIOR)]:
   event={'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'last-assistant-message':'synthetic baseline'}
   subprocess.run([binary,'capture','--thread',native,'--database',capture_db,'--title','same title',json.dumps(event)],env=env,check=True,stdout=subprocess.DEVNULL)
  with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
  opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
  def api(path,body=None):
   request=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
   with opener.open(request,timeout=2) as response:return json.load(response)
  def sql(statement,args=()):
   with closing(sqlite3.connect(db)) as c,c:c.execute(statement,args)
  worker=None;process=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  def stop_worker():
   if worker is not None and worker.poll() is None:worker.send_signal(signal.SIGINT);worker.wait(timeout=5)
  try:
   deadline=time.monotonic()+5
   while True:
    try:api('/health');break
    except OSError:
     if time.monotonic()>deadline:raise
     time.sleep(.05)
   sql("INSERT INTO devices(id,role,name,expires,last_seen) VALUES(?,'agent','fixture',?,?)",(HOST,int(time.time())+300,int(time.time())))
   sql("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
   for native in [OTHER,NATIVE]:subprocess.run([binary,'capture-import','--db',db,'--capture-db',capture_db,'--host',HOST,'--thread',native],env=env,check=True,stdout=subprocess.DEVNULL)
   rows=api('/v1/threads')['threads'];assert len(rows)==2 and all(r['title']=='same title' for r in rows)
   chosen=next(r for r in rows if r['native_id']==NATIVE)
   sql('UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?',(int(time.time()),chosen['id']))
   sql("UPDATE threads SET can_send=1,status='queue_ready' WHERE id=?",(chosen['id'],))
   sql('INSERT INTO resume_owners(thread,grant_hash,expires) VALUES(?,?,?)',(chosen['id'],digest,int(time.time())+120))
   if is_worker:
    (root/'competitor').write_text('0')
    refused=subprocess.run(resume+['--worker'],env=env,capture_output=True,text=True,timeout=5)
    assert refused.returncode!=0 and 'old_queue_worker_not_stopped' in refused.stderr
    assert not calls.exists();(root/'competitor').write_text('3')
    sql('UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE thread=?',(chosen['id'],));sql("UPDATE threads SET can_send=0,status='capture_only' WHERE id=?",(chosen['id'],))
    worker=subprocess.Popen(resume+['--worker'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True)
    deadline=time.monotonic()+5
    while True:
     visible=next(r for r in api('/v1/threads')['threads'] if r['native_id']==NATIVE)
     if visible['can_send'] and visible['status']=='resume_ready':break
     if worker.poll() is not None or time.monotonic()>deadline:raise AssertionError('Rust worker did not advertise its lease')
     time.sleep(.02)
    subprocess.run([binary,'capture-import','--db',db,'--capture-db',capture_db,'--host',HOST,'--thread',NATIVE],env=env,check=True,stdout=subprocess.DEVNULL)
    assert next(r for r in api('/v1/threads')['threads'] if r['native_id']==NATIVE)['status']=='resume_ready'
    (root/'pause-final').write_text('1')
    # Let the initial idle wait register the process signal receiver.
    time.sleep(.05)
   command=api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':chosen['id'],'text':'synthetic explicit phone action','expected_revision':chosen['revision'],'created_at':int(time.time()),'kind':'send'})
   if is_worker:
    deadline=time.monotonic()+7
    while not (root/'turn-running').exists():
     if worker.poll() is not None or time.monotonic()>deadline:raise AssertionError('Mock turn did not start')
     time.sleep(.01)
    # Signal while dispatch awaits the mock final. Finish that turn before
    # releasing ownership; a signal receiver recreated later loses this signal.
    worker.send_signal(signal.SIGINT);time.sleep(.05)
    assert worker.poll() is None,'Worker must finish the in-flight turn before shutdown'
    (root/'allow-final').write_text('1')
    deadline=time.monotonic()+7
    while api('/v1/commands/'+command['id'])['status']!='codex_accepted':
     if time.monotonic()>deadline:raise AssertionError('Rust worker did not complete')
     time.sleep(.02)
    worker.wait(timeout=5);assert worker.returncode==0
    with closing(sqlite3.connect(db)) as c,c:
     assert c.execute('SELECT can_send,status FROM threads WHERE id=?',(chosen['id'],)).fetchone()==(0,'capture_only')
     assert c.execute('SELECT last_seen FROM resume_owners WHERE thread=?',(chosen['id'],)).fetchone()[0]==0
   else:
    result=subprocess.run(resume+['--command',command['id']],env=env,check=True,capture_output=True,text=True,timeout=5)
    assert json.loads(result.stdout)['receipt']==['codex_accepted',None,TURN]
   subprocess.run(resume+['--command',command['id']],env=env,check=True,stdout=subprocess.DEVNULL,timeout=5)
   events=[json.loads(line) for line in calls.read_text().splitlines()];assert sum(e['method']=='turn/start' for e in events)==1
   for event in events:
    if event['thread'] is not None:assert event['thread']==NATIVE
    if event['method']=='turn/start':assert event['text']=='synthetic explicit phone action'
   subprocess.run([binary,'capture-import','--db',db,'--capture-db',capture_db,'--host',HOST,'--thread',NATIVE],env=env,check=True,stdout=subprocess.DEVNULL)
   receipt=api('/v1/commands/'+command['id']);assert receipt['status']=='codex_accepted' and receipt['native_turn_id']==TURN
   messages=api('/v1/threads/'+chosen['id']+'/messages')['messages'];assert len(messages)==2 and any(m['turn_id']==TURN and m['text']=='synthetic final' for m in messages)
   other=next(r for r in rows if r['native_id']==OTHER);assert len(api('/v1/threads/'+other['id']+'/messages')['messages'])==1
   with closing(sqlite3.connect(db)) as c,c:assert c.execute('SELECT count(*) FROM capture_queue_ledger').fetchone()[0]==0;assert c.execute('SELECT count(*) FROM outbox').fetchone()[0]==0
   if is_worker:
    sql('UPDATE resume_owners SET expires=?,last_seen=?',(int(time.time())-1,int(time.time())))
    sql('UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?',(int(time.time()),chosen['id']));sql("UPDATE threads SET can_send=1,status='resume_ready' WHERE id=?",(chosen['id'],))
    current=next(r for r in api('/v1/threads')['threads'] if r['native_id']==NATIVE);assert not current['can_send']
    try:api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':chosen['id'],'text':'synthetic expired lease rejection','expected_revision':current['revision'],'created_at':int(time.time()),'kind':'send'})
    except urllib.error.HTTPError as error:
     with error:assert error.code==409 and json.load(error)['error']=='not_ready'
    else:raise AssertionError('expired resume lease accepted a message')
   print('PASS: Rust resume CLI preserves same-title original ID, unmodified input, one authoritative turn, final capture, duplicate no resend, worker lease and release, and task isolation')
  finally:
   (root/'allow-final').write_text('1')
   stop_worker()
   if worker is not None and worker.stderr is not None:worker.stderr.close()
   process.terminate();process.wait(timeout=3)
if __name__=='__main__':
 if Path(sys.argv[0]).name in ('mock-codex','systemctl'):raise SystemExit(mock())
 main()
