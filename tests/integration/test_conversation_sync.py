"""Synthetic same-second desktop/phone history; no upstream or real messages."""
from contextlib import closing
import hashlib,json,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
def save_turn(database,native,turn,rows,completed_at):
 # Synthetic source fixture only; production user-turn parsing is tested in Rust.
 with closing(sqlite3.connect(database)) as c, c:
  c.execute('CREATE TABLE IF NOT EXISTS captured_user_messages(thread_id TEXT,turn_id TEXT,message_id TEXT,text TEXT,created_at INTEGER,input_digest TEXT,PRIMARY KEY(thread_id,message_id))')
  c.execute('CREATE TABLE IF NOT EXISTS captured_turn_order(thread_id TEXT,turn_id TEXT,completed_at INTEGER,PRIMARY KEY(thread_id,turn_id))')
  c.execute('INSERT OR IGNORE INTO captured_turn_order VALUES(?,?,?)',(native,turn,completed_at))
  c.executemany('INSERT OR IGNORE INTO captured_user_messages VALUES(?,?,?,?,?,?)',[(native,turn,*row) for row in rows])
N='00000000-0000-4000-8000-000000000001';A='ffffffff-ffff-4fff-8fff-ffffffffffff';B='00000000-0000-4000-8000-000000000002'
def run(binary,mode):
 with tempfile.TemporaryDirectory() as temp:
  root=Path(temp);db=root/'hub.sqlite';capture=root/'capture.sqlite';now=int(time.time());thread=hashlib.sha256(('host\0default\0'+N).encode()).hexdigest();command=str(uuid.uuid4())
  subprocess.run([binary,'hub','--help'],stdout=subprocess.DEVNULL,check=True)
  with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
  p=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
  def api(path):
   req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,headers={'Authorization':'Bearer fixture'})
   with opener.open(req,timeout=2) as r:return json.load(r)
  try:
   end=time.monotonic()+5
   while True:
    try:api('/health');break
    except OSError:
     if time.monotonic()>end:raise
     time.sleep(.02)
   with closing(sqlite3.connect(db)) as c, c:
    c.execute("INSERT INTO devices(id,role,name,expires) VALUES('host','agent','fixture',?)",(now+60,));c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture').hexdigest(),now+60))
   with closing(sqlite3.connect(capture)) as c, c:
    c.execute('CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER)')
    c.execute('INSERT INTO captured_replies VALUES(?,?,?,?,?)',(N,A,'desktop reply',13,now))
   save_turn(capture,N,A,[('desktop','desktop input',now*1000-800,hashlib.sha256(b'desktop input').hexdigest())],now*1000-600)
   args=[binary,'capture-import','--db',db,'--capture-db',capture,'--host','host','--thread',N]
   def sync():subprocess.run(args,stdout=subprocess.DEVNULL,check=True)
   sync()
   with closing(sqlite3.connect(db)) as c, c:
    payload=json.dumps({'expected_revision':A if mode!='wrong_revision' else B,'text':'phone input'})
    c.execute("INSERT INTO commands VALUES(?,'phone',?,'fixture','host',?,?,'upstream_queued',?,?,NULL,NULL)",(command,command,thread,payload,now,now+60));c.execute("INSERT INTO capture_queue_ledger VALUES(?,'upstream_queued','queue-id')",(command,))
    c.execute('INSERT INTO capture_queue_send_times VALUES(?,?)',(command,now*1000+(101 if mode=='too_early' else 0)))
    if mode=='ambiguous':
     other=str(uuid.uuid4());c.execute("INSERT INTO commands VALUES(?,'phone',?,'fixture','host',?,?,'unknown',?,?,NULL,NULL)",(other,other,thread,payload,now,now+60));c.execute("INSERT INTO capture_queue_ledger VALUES(?,'unknown','queue-id-2')",(other,));c.execute('INSERT INTO capture_queue_send_times VALUES(?,?)',(other,now*1000))
   # Reproduce reply projection winning the race against user-input capture.
   with closing(sqlite3.connect(capture)) as c, c:c.execute('INSERT INTO captured_replies VALUES(?,?,?,?,?)',(N,B,'phone reply',11,now))
   sync();assert api('/v1/commands/'+command)['status']=='upstream_queued'
   save_turn(capture,N,B,[('phone','phone input',now*1000+100,hashlib.sha256(b'phone input').hexdigest())],now*1000+200)
   sync();r=api('/v1/commands/'+command);assert r['status']==('codex_accepted' if mode=='valid' else 'upstream_queued'),r
   msgs=list(reversed(api('/v1/threads/'+thread+'/messages')['messages']))
   assert [(m['role'],m['text']) for m in msgs]==[('user','desktop input'),('assistant','desktop reply'),('user','phone input'),('assistant','phone reply')],msgs
   assert api('/v1/threads')['threads'][0]['revision']==B
   cursor=api('/v1/events?after=0')['cursor'];sync();assert api('/v1/events?after=0')['cursor']==cursor
   with closing(sqlite3.connect(db)) as c, c:assert c.execute('SELECT count(*) FROM outbox').fetchone()[0]==0
  finally:p.terminate();p.wait(timeout=3)
if __name__=='__main__':
 for mode in ['valid','wrong_revision','ambiguous','too_early']:run(str(Path(sys.argv[1]).resolve()),mode)
 print('PASS: user/assistant/user/assistant; same-second ordering; latest revision; late-input race recovery; replay idempotency; wrong revision/ambiguous requests refuse acknowledgement')
