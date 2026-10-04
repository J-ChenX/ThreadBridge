#!/usr/bin/python3
"""Ambiguous notify marker reproduction and candidate fail-closed disambiguation."""
from contextlib import closing
import hashlib,json,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
from capture_completion import capture as old_capture
from capture_completion_v014 import capture as candidate_capture
from test_resume_dispatch import HOST,NATIVE,PRIOR,TURN

def run_case(binary,mode):
 with tempfile.TemporaryDirectory(prefix='tb-correlation-') as temp:
  root=Path(temp);db=root/'hub.sqlite';source=root/'capture.sqlite';old=str(uuid.uuid4());receiver=old_capture if mode=='legacy' else candidate_capture
  old_capture(json.dumps({'type':'agent-turn-complete','thread-id':NATIVE,'turn-id':PRIOR,'last-assistant-message':'prior final'}),NATIVE,source,'same title')
  env={'PATH':'/usr/bin:/bin','HOME':temp}
  with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
  opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
  def api(path,body=None):
   request=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
   with opener.open(request,timeout=2) as response:return json.load(response)
  process=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   deadline=time.monotonic()+5
   while True:
    try:api('/health');break
    except OSError:
     if time.monotonic()>deadline:raise
     time.sleep(.02)
   with closing(sqlite3.connect(db)) as c, c:
    c.execute("INSERT INTO devices(id,role,name,expires,last_seen) VALUES(?,'agent','fixture',?,?)",(HOST,int(time.time())+300,int(time.time())))
    c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
   args=[binary,'capture-import','--db',db,'--capture-db',source,'--host',HOST,'--thread',NATIVE]
   subprocess.run(args,env=env,check=True,stdout=subprocess.DEVNULL)
   row=api('/v1/threads')['threads'][0]
   with closing(sqlite3.connect(db)) as c, c:
    c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=?',(int(time.time()),));c.execute("UPDATE threads SET can_send=1,status='queue_ready'")
    c.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires,native_turn) VALUES(?,'phone',?,'fixture',?,?,?,'codex_accepted',?,?,?)",(old,old,HOST,row['id'],json.dumps({'expected_revision':'prior-history'}),int(time.time())-60,int(time.time())-1,PRIOR))
    c.execute("INSERT INTO capture_queue_ledger(id,status) VALUES(?,'codex_accepted')",(old,))
   new=api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':row['id'],'text':'synthetic explicit phone click','expected_revision':PRIOR,'created_at':int(time.time()),'kind':'send'})['id']
   candidates=[old,new]
   with closing(sqlite3.connect(db)) as c, c:
    c.execute("UPDATE commands SET status='upstream_queued' WHERE id=?",(new,));c.execute("INSERT INTO capture_queue_ledger(id,status) VALUES(?,'upstream_queued')",(new,))
    if mode=='two_active':
     second=str(uuid.uuid4());payload=json.loads(c.execute('SELECT payload FROM commands WHERE id=?',(new,)).fetchone()[0]);payload['id']=second
     c.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) VALUES(?,'phone',?,'fixture',?,?,?,'unknown',?,?)",(second,second,HOST,row['id'],json.dumps(payload),int(time.time()),int(time.time())+60));c.execute("INSERT INTO capture_queue_ledger(id,status) VALUES(?,'unknown')",(second,));candidates.append(second)
    if mode=='wrong_revision':
     payload=json.loads(c.execute('SELECT payload FROM commands WHERE id=?',(new,)).fetchone()[0]);payload['expected_revision']='wrong';c.execute('UPDATE commands SET payload=? WHERE id=?',(json.dumps(payload),new))
   inputs=[] if mode=='no_marker' else ['[ThreadBridge request:'+request+']\nPRIVATE INPUT' for request in candidates]
   receiver(json.dumps({'type':'agent-turn-complete','thread-id':NATIVE,'turn-id':TURN,'last-assistant-message':'new final','input-messages':inputs}),NATIVE,source,'same title')
   subprocess.run(args,env=env,check=True,stdout=subprocess.DEVNULL)
   receipt=api('/v1/commands/'+new)
   if mode=='unique':assert receipt['status']=='codex_accepted' and receipt['native_turn_id']==TURN,receipt
   else:assert receipt['status']=='upstream_queued' and receipt['native_turn_id'] is None,(mode,receipt)
   with closing(sqlite3.connect(source)) as c, c:
    assert c.execute('SELECT count(*) FROM captured_replies').fetchone()[0]==2
    if mode=='legacy':assert c.execute('SELECT count(*) FROM captured_request_ids WHERE turn_id=?',(TURN,)).fetchone()[0]==0
    else:assert c.execute('SELECT count(*) FROM captured_request_candidates WHERE turn_id=?',(TURN,)).fetchone()[0]==len(inputs)
    assert b'PRIVATE INPUT' not in source.read_bytes()
   assert api('/v1/commands/'+old)['native_turn_id']==PRIOR
  finally:process.terminate();process.wait(timeout=3)

def main():
 candidate=Path(sys.argv[1]).resolve();old=Path(sys.argv[2]).resolve()
 run_case(old,'legacy')
 for mode in ['unique','two_active','no_marker','wrong_revision']:run_case(candidate,mode)
 print('PASS: reproduced old multi-marker loss; candidate UUID-only active-request disambiguation; ambiguous/no marker/wrong revision refuse ack; prior final untouched; no real data or sends')
if __name__=='__main__':main()
