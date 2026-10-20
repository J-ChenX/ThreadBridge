"""Consumed desktop steering settles old queued receipts without resending anything."""
from pathlib import Path
from contextlib import closing
import hashlib,json,os,sqlite3,subprocess,sys,tempfile,time,uuid
from test_conversation_sync import save_turn

def scenario(binary,mode):
 with tempfile.TemporaryDirectory(prefix='tb-steer-receipt-') as temp:
  root=Path(temp);hub=root/'hub.sqlite';source=root/'capture.sqlite';host,native,turn,command,queue=[str(uuid.uuid4()) for _ in range(5)];text='explicit phone guidance';stamp=int(time.time()*1000)-2000
  env=dict(os.environ,HOME=temp,CODEX_HOME=temp)
  def run(*args):return subprocess.check_output([binary,*map(str,args)],env=env,text=True)
  run('register-agent','--db',hub,'--name','fixture','--output',root/'agent.json');registered=json.loads((root/'agent.json').read_text())['host_id'];host=registered
  tid=hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest()
  raw=text+ ('\n\n' if mode=='two_newlines' else '\n')
  rows=[('initial','initial desktop request',stamp-1000,hashlib.sha256(b'initial desktop request').hexdigest()),('steered',raw,stamp+1,hashlib.sha256(raw.encode()).hexdigest())]
  if mode=='duplicate':rows.append(('duplicate',raw,stamp+2,hashlib.sha256(raw.encode()).hexdigest()))
  save_turn(source,native,turn,rows,stamp+500)
  run('capture','--database',source,'--thread',native,json.dumps({'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'last-assistant-message':'completed after guidance'}))
  args=['capture-import','--db',hub,'--capture-db',source,'--host',host,'--thread',native]
  run(*args) # Final reply and inputs are already imported before the repair runs.
  with closing(sqlite3.connect(hub)) as c,c:
   c.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) VALUES(?,'phone',?,'fixture',?,?,?,'upstream_queued',?,?)",(command,command,host,tid,json.dumps({'kind':'send','text':text,'expected_revision':'wrong' if mode=='wrong_revision' else ''}),stamp//1000,stamp//1000+60))
   c.execute("INSERT INTO capture_queue_ledger VALUES(?,'upstream_queued',?)",(command,None if mode=='no_queue_ack' else queue))
   c.execute('INSERT INTO capture_queue_send_times VALUES(?,?)',(command,stamp))
   before=c.execute('SELECT count(*) FROM messages').fetchone()[0]
  run(*args);run(*args)
  with closing(sqlite3.connect(hub)) as c:
   receipt=c.execute('SELECT status,native_turn FROM commands WHERE id=?',(command,)).fetchone()
   assert receipt==(('codex_accepted',turn) if mode=='consumed' else ('upstream_queued',None)),(mode,receipt)
   assert c.execute('SELECT count(*) FROM messages').fetchone()[0]==before
   assert c.execute('SELECT count(*) FROM commands').fetchone()[0]==1
   assert c.execute('SELECT queue_id FROM capture_queue_ledger WHERE id=?',(command,)).fetchone()[0]==(None if mode=='no_queue_ack' else queue)

def main():
 binary=Path(sys.argv[1]).resolve()
 for mode in ['consumed','no_queue_ack','wrong_revision','duplicate','two_newlines']:scenario(binary,mode)
 print('PASS consumed first-turn desktop guidance with one appended newline reconciles an already-imported final; no queue ack, wrong revision, duplicates or extra whitespace stay pending; no sends')
if __name__=='__main__':main()
