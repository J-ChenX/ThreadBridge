#!/usr/bin/python3
"""Synthetic phone HTTP -> dormant dispatcher -> mock stdio -> final capture."""
from contextlib import closing
import hashlib,json,threading,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
from test_resume_dispatch import Fixture,HOST,NATIVE,OTHER,PRIOR,TURN
from resume_dispatch import dispatch,grant_hash,worker,Stop
from capture_completion import capture

def main():
 binary=Path(sys.argv[1]).resolve()
 with tempfile.TemporaryDirectory(prefix='tb-resume-phone-') as temp:
  f=Fixture(temp);f.db.unlink()
  env={'PATH':'/usr/bin:/bin','HOME':temp}
  for native,turn in [(OTHER,OTHER),(NATIVE,PRIOR)]:
   capture(json.dumps({'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'last-assistant-message':'synthetic baseline'}),native,f.capture,'same title')
  with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
  opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
  def api(path,body=None):
   request=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
   with opener.open(request,timeout=2) as response:return json.load(response)
  thread_worker=None;worker_errors=[];stop_event=threading.Event()
  process=subprocess.Popen([binary,'hub','--db',f.db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   deadline=time.monotonic()+5
   while True:
    try:api('/health');break
    except OSError:
     if time.monotonic()>deadline:raise
     time.sleep(.05)
   with closing(sqlite3.connect(f.db)) as c, c:
    c.execute("INSERT INTO devices(id,role,name,expires,last_seen) VALUES(?,'agent','fixture',?,?)",(HOST,int(time.time())+300,int(time.time())))
    c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
   for native in [OTHER,NATIVE]:subprocess.run([binary,'capture-import','--db',f.db,'--capture-db',f.capture,'--host',HOST,'--thread',native],env=env,check=True,stdout=subprocess.DEVNULL)
   rows=api('/v1/threads')['threads'];assert len(rows)==2 and all(r['title']=='same title' for r in rows)
   chosen=next(r for r in rows if r['native_id']==NATIVE)
   with closing(sqlite3.connect(f.db)) as c, c:
    # Explicit synthetic handoff/owner approval; no real service or credential change.
    c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?',(int(time.time()),chosen['id']))
    c.execute("UPDATE threads SET can_send=1,status='queue_ready' WHERE id=?",(chosen['id'],))
    c.execute('CREATE TABLE IF NOT EXISTS resume_owners(thread TEXT PRIMARY KEY,grant_hash TEXT,expires INTEGER,last_seen INTEGER NOT NULL DEFAULT 0)')
    c.execute('INSERT INTO resume_owners(thread,grant_hash,expires) VALUES(?,?,?)',(chosen['id'],grant_hash(f.grant),int(time.time())+120))
   if '--worker' in sys.argv:
    try:worker(f.db,f.capture,str(f.binary),f.grant,execute=True,competitor_check=lambda:False,stop_after=1)
    except Stop as e:assert str(e)=='old_queue_worker_not_stopped'
    else:raise AssertionError('competing old worker was ignored')
   if '--worker' in sys.argv:
    # Start with read-only target: the worker must advertise its own live lease.
    with closing(sqlite3.connect(f.db)) as c, c:
     c.execute('UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE thread=?',(chosen['id'],));c.execute("UPDATE threads SET can_send=0,status='capture_only' WHERE id=?",(chosen['id'],))
    def run_worker():
     try:worker(f.db,f.capture,str(f.binary),f.grant,execute=True,competitor_check=lambda:True,poll_interval=.1,stop_event=stop_event)
     except Exception as error:worker_errors.append(error)
    thread_worker=threading.Thread(target=run_worker);thread_worker.start()
    deadline=time.monotonic()+4
    while True:
     visible=next(r for r in api('/v1/threads')['threads'] if r['native_id']==NATIVE)
     if visible['can_send'] and visible['status']=='resume_ready':break
     if time.monotonic()>deadline:raise AssertionError('worker did not advertise eligible original ID')
     time.sleep(.02)
    # Independent read projection must retain sender identity while lease is active.
    subprocess.run([binary,'capture-import','--db',f.db,'--capture-db',f.capture,'--host',HOST,'--thread',NATIVE],env=env,check=True,stdout=subprocess.DEVNULL)
    assert next(r for r in api('/v1/threads')['threads'] if r['native_id']==NATIVE)['status']=='resume_ready'
   command=api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':chosen['id'],'text':'synthetic explicit phone action','expected_revision':chosen['revision'],'created_at':int(time.time()),'kind':'send'})
   if '--worker' in sys.argv:
    deadline=time.monotonic()+5
    while api('/v1/commands/'+command['id'])['status']!='codex_accepted':
     if worker_errors or time.monotonic()>deadline:raise AssertionError(('worker did not complete',worker_errors))
     time.sleep(.02)
    stop_event.set();thread_worker.join(timeout=5);assert not thread_worker.is_alive() and not worker_errors,worker_errors
    with closing(sqlite3.connect(f.db)) as c, c:
     assert c.execute('SELECT can_send,status FROM threads WHERE id=?',(chosen['id'],)).fetchone()==(0,'capture_only')
     assert c.execute('SELECT last_seen FROM resume_owners WHERE thread=?',(chosen['id'],)).fetchone()[0]==0
    result=api('/v1/commands/'+command['id']);assert result['status']=='codex_accepted' and result['native_turn_id']==TURN
   else:
    result=dispatch(f.db,f.capture,str(f.binary),f.grant,command['id'],execute=True);assert result==('codex_accepted',None,TURN),result
   dispatch(f.db,f.capture,str(f.binary),f.grant,command['id'],execute=True)
   assert f.methods().count('turn/start')==1
   for line in f.calls.read_text().splitlines():
    event=json.loads(line)
    if event['thread'] is not None:assert event['thread']==NATIVE
    if event['method']=='turn/start':assert event['text']=='synthetic explicit phone action'
   subprocess.run([binary,'capture-import','--db',f.db,'--capture-db',f.capture,'--host',HOST,'--thread',NATIVE],env=env,check=True,stdout=subprocess.DEVNULL)
   receipt=api('/v1/commands/'+command['id']);assert receipt['status']=='codex_accepted' and receipt['native_turn_id']==TURN
   messages=api('/v1/threads/'+chosen['id']+'/messages')['messages'];assert len(messages)==2 and any(m['turn_id']==TURN and m['text']=='synthetic final' for m in messages)
   other=next(r for r in rows if r['native_id']==OTHER);assert len(api('/v1/threads/'+other['id']+'/messages')['messages'])==1
   with closing(sqlite3.connect(f.db)) as c, c:assert c.execute('SELECT count(*) FROM capture_queue_ledger').fetchone()[0]==0;assert c.execute('SELECT count(*) FROM outbox').fetchone()[0]==0
   if '--worker' in sys.argv:
    with closing(sqlite3.connect(f.db)) as c, c:
     c.execute('UPDATE resume_owners SET expires=?,last_seen=?',(int(time.time())-1,int(time.time())));c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?',(int(time.time()),chosen['id']));c.execute("UPDATE threads SET can_send=1,status='resume_ready' WHERE id=?",(chosen['id'],))
    current=next(r for r in api('/v1/threads')['threads'] if r['native_id']==NATIVE);assert not current['can_send']
    try:api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':chosen['id'],'text':'synthetic expired lease rejection','expected_revision':current['revision'],'created_at':int(time.time()),'kind':'send'})
    except urllib.error.HTTPError as error:
     with error:assert error.code==409 and json.load(error)['error']=='not_ready'
    else:raise AssertionError('expired resume lease accepted a message')
   print('PASS: synthetic phone selects same-title original ID; mock stdio resumes identical ID without fork; one turn; authoritative turn + final capture; duplicate no resend; other task unchanged; no real Codex/phone/send credentials')
  finally:
   if thread_worker is not None:stop_event.set();thread_worker.join(timeout=5)
   process.terminate();process.wait(timeout=3)
if __name__=='__main__':main()
