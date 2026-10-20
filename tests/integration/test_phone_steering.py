"""Real Hub/bridge and fixed CLI fixture: explicit queued guidance, no native tasks."""
from pathlib import Path
from contextlib import closing
import hashlib,json,os,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,urllib.error,uuid

def scenario(binary,lose):
 with tempfile.TemporaryDirectory(prefix='tb-phone-steer-') as temp:
  root=Path(temp);db=root/'hub.sqlite';source=root/'capture.sqlite';native,turn,prior,queue=[str(uuid.uuid4()) for _ in range(4)];fake=root/'codex';fake.symlink_to(Path(__file__).resolve().parents[1]/'fixtures/mock_codex.py')
  root.joinpath('fixture.json').write_text(json.dumps({'mode':'steer','native_id':native,'turn_id':turn,'queue_id':queue,'lose_steer':lose}))
  env={'PATH':'/usr/bin:/bin','HOME':temp,'CODEX_HOME':temp}
  def run(*args):return subprocess.check_output([binary,*map(str,args)],env=env,text=True)
  run('register-agent','--db',db,'--name','fixture','--output',root/'agent.json');host=json.loads((root/'agent.json').read_text())['host_id']
  run('capture','--database',source,'--thread',native,json.dumps({'type':'agent-turn-complete','thread-id':native,'turn-id':prior,'last-assistant-message':'prior final'}))
  run('capture-import','--db',db,'--capture-db',source,'--host',host,'--thread',native)
  tid=hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest();now=int(time.time())
  with closing(sqlite3.connect(source)) as c,c:
   c.execute('CREATE TABLE IF NOT EXISTS captured_task_state(thread_id TEXT PRIMARY KEY,turn_id TEXT NOT NULL,status TEXT NOT NULL)')
   c.execute("INSERT OR REPLACE INTO captured_task_state VALUES(?,?,'running')",(native,turn))
  with closing(sqlite3.connect(db)) as c,c:
   c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),now+300));c.execute('UPDATE devices SET last_seen=? WHERE id=?',(now,host));c.execute("UPDATE threads SET can_send=1,status='queue_ready'");c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=?',(now,))
  with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
  opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
  def api(path,body=None):
   r=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
   try:
    with opener.open(r,timeout=3) as response:return json.load(response)
   except urllib.error.HTTPError as e:raise AssertionError(path+': '+e.read().decode()) from e
  def wait(fn):
   end=time.monotonic()+20
   while time.monotonic()<end:
    try:
     r=fn()
     if r:return r
    except (OSError,KeyError):pass
    time.sleep(.05)
   raise AssertionError('steering condition timed out')
  hub=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);worker=None
  def bridge():return subprocess.Popen([binary,'capture-bridge','--db',db,'--capture-db',source,'--host',host,'--codex',fake,'--allow-queue','--verified-version','codex-cli fixture'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   wait(lambda:api('/health'))
   with closing(sqlite3.connect(db)) as c,c:
    c.execute('UPDATE devices SET last_seen=? WHERE id=?',(int(time.time()),host));c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=?',(int(time.time()),));c.execute("UPDATE threads SET can_send=1,status='queue_ready'")
   request={'request_id':str(uuid.uuid4()),'thread_id':tid,'text':'explicit phone guidance','expected_revision':prior,'created_at':now,'kind':'send'}
   command=api('/v1/commands',request)['id']
   with closing(sqlite3.connect(db)) as c,c:
    c.execute("UPDATE commands SET status='upstream_queued' WHERE id=?",(command,));c.execute("INSERT INTO capture_queue_ledger VALUES(?,'upstream_queued',?)",(command,queue));c.execute('INSERT INTO capture_queue_send_times VALUES(?,?)',(command,now*1000))
   worker=bridge();wait(lambda:api('/v1/threads')['threads'][0]['task_status']=='running')
   first=api('/v1/commands/'+command+'/steer',{});assert first['error']=='steer_in_progress'
   api('/v1/commands/'+command+'/steer',{})
   status='unknown' if lose else 'steer_accepted';wait(lambda:api('/v1/commands/'+command)['status']==status)
   assert root.joinpath('calls').read_text()=='delete\nsteer\n'
   assert api('/v1/commands/'+command+'/steer',{})['status']==status
   if not lose:
    assert api('/v1/commands/'+command)['native_turn_id']==turn
    worker.terminate();worker.wait(timeout=5);worker=None
    another=dict(request,request_id=str(uuid.uuid4()),text='next explicit human message',created_at=int(time.time()))
    second=api('/v1/commands',another);assert second['status']=='accepted';api('/v1/commands/'+second['id']+'/cancel',{})
    worker=bridge()
    assert api('/v1/commands/'+command)['status']=='steer_accepted' # No false final.
    run('capture','--database',source,'--thread',native,json.dumps({'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'last-assistant-message':'actual completed guidance reply'}))
    wait(lambda:api('/v1/commands/'+command)['status']=='codex_accepted')
   worker.terminate();worker.wait(timeout=5);worker=bridge();time.sleep(2.2)
   assert root.joinpath('calls').read_text()=='delete\nsteer\n'
   assert api('/v1/commands',request)['id']==command
   print('PASS phone queued guidance -> original item removed -> same active turn accepted; '+('lost receipt stays unknown' if lose else 'composer can send again before final; exact final settles')+'; duplicate and restart never resend')
  finally:
   if worker:worker.terminate();worker.wait(timeout=5)
   hub.terminate();hub.wait(timeout=5)
def main():
 binary=Path(sys.argv[1]).resolve()
 for lose in (False,True):scenario(binary,lose)
if __name__=='__main__':main()
