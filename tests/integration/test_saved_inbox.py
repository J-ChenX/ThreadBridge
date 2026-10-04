#!/usr/bin/python3
"""No real Codex or phone: completion catalog -> independent PC inbox -> HTTP."""
from contextlib import closing
import hashlib,json,os,socket,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
def main():
    binary=Path(sys.argv[1]).resolve();all_mode='--all' in sys.argv[2:]
    ids=['00000000-0000-4000-8000-000000000011','00000000-0000-4000-8000-000000000012']
    with tempfile.TemporaryDirectory(prefix='tb-saved-inbox-') as tmp:
     root=Path(tmp);db=root/'hub.sqlite';capture=root/'capture.sqlite';catalog=root/'catalog.json';catalog.write_text(json.dumps(dict.fromkeys(ids,'同名任务'),ensure_ascii=False))
     index=root/'session_index.jsonl';index.write_text(''.join(json.dumps({'id':native,'thread_name':'同名任务','updated_at':'2026-10-03'})+'\n' for native in ids))
     receiver_scope=['--all-tasks','--title-index',str(index)] if all_mode else ['--catalog',str(catalog)]
     sync_scope=['--all-captured'] if all_mode else ['--catalog',str(catalog)]
     env={'PATH':'/usr/bin:/bin','HOME':str(root)}
     for i,native in enumerate(ids):
      event={'type':'agent-turn-complete','thread-id':native,'turn-id':native,'last-assistant-message':'saved final '+str(i),'input-messages':['PRIVATE INPUT'],'cwd':'PRIVATE PATH'}
      captured=subprocess.run([binary,'capture',*receiver_scope,'--database',capture,json.dumps(event)],env=env,check=True,capture_output=True,text=True)
      assert captured.stdout.strip()=='captured' and not captured.stderr
     with closing(sqlite3.connect(db)) as c, c:
      c.execute('CREATE TABLE devices(id TEXT PRIMARY KEY,token TEXT UNIQUE,role TEXT NOT NULL,name TEXT NOT NULL,expires INTEGER NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,last_seen INTEGER NOT NULL DEFAULT 0)')
      c.execute("INSERT INTO devices(id,role,name,expires) VALUES('host','agent','fixture',?)",(int(time.time())+300,))
      c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','fixture',?)",(hashlib.sha256(b'fixture-phone').hexdigest(),int(time.time())+300))
     with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
     opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
     def api(path,body=None):
      req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer fixture-phone','Content-Type':'application/json'})
      with opener.open(req,timeout=2) as r:return json.load(r)
     def wait(fn):
      until=time.monotonic()+8
      while time.monotonic()<until:
       try:
        value=fn()
        if value:return value
       except (OSError,IndexError,KeyError):pass
       time.sleep(.1)
      raise AssertionError('condition not reached')
     def start():
      p=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);wait(lambda:api('/health'));return p
     hub=start();sync=None
     try:
      sync=subprocess.Popen([binary,'capture-sync','--db',db,'--capture-db',capture,'--host','host',*sync_scope],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
      rows=wait(lambda:api('/v1/threads')['threads'] if len(api('/v1/threads')['threads'])==2 else None)
      assert all(r['title']=='同名任务' and not r['can_send'] for r in rows)
      assert len({r['id'] for r in rows})==2 and {r['native_id'] for r in rows}==set(ids)
      for row in rows:
       m=api('/v1/threads/'+row['id']+'/messages')['messages'][0]
       assert m['completion_title']=='同名任务' and m['turn_id']==row['native_id'] and m['recorded_at']>0
       assert m['text']=='saved final '+str(ids.index(row['native_id']))
      if all_mode:
       # Actual native CLI failure, polling visibility, exact event retry, duplicates.
       failed_event={'type':'agent-turn-complete','thread-id':ids[0],'turn-id':'00000000-0000-4000-8000-000000000013','last-assistant-message':'retry final'}
       argv=[binary,'capture',*receiver_scope,'--database',capture]
       failed=subprocess.run([*argv,'--storage-budget-bytes','1',json.dumps(failed_event)],env=env,capture_output=True);assert failed.returncode==2
       assert failed.stdout==b'' and failed.stderr==b'completion capture failed; no event content logged\n'
       health=wait(lambda:api('/v1/capture-health')['hosts'][0]['status'] if api('/v1/capture-health')['hosts'] and api('/v1/capture-health')['hosts'][0]['status'].get('failures') else None)
       assert next(iter(health['failures'].values()))['reason']=='storage_budget_exceeded'
       assert len(api('/v1/threads/'+next(r['id'] for r in rows if r['native_id']==ids[0])+'/messages')['messages'])==1
       # Failure remains readable after Hub restart before retry.
       hub.terminate();hub.wait(timeout=3);hub=start();assert api('/v1/capture-health')['hosts'][0]['status']['failures']
       for expected in ['captured','duplicate']:
        retry=subprocess.run([*argv,json.dumps(failed_event)],env=env,check=True,capture_output=True,text=True)
        assert retry.stdout.strip()==expected and not retry.stderr
       wait(lambda:api('/v1/capture-health')['hosts'] and not api('/v1/capture-health')['hosts'][0]['status'].get('failures'))
       wait(lambda:len(api('/v1/threads/'+next(r['id'] for r in rows if r['native_id']==ids[0])+'/messages')['messages'])==2)
      # Stop projection/sending side entirely; only PC HTTP storage remains.
      sync.terminate();sync.wait(timeout=3);sync=None
      hub.terminate();hub.wait(timeout=3);hub=start()
      with closing(sqlite3.connect(db)) as c, c:c.execute("UPDATE devices SET last_seen=0,expires=? WHERE id='host'",(int(time.time())-1,))
      offline=api('/v1/threads')['threads'];assert len(offline)==2 and all(not r['host_online'] and not r['can_send'] for r in offline)
      assert all(len(api('/v1/threads/'+r['id']+'/messages')['messages'])==(2 if all_mode and r['native_id']==ids[0] else 1) for r in offline)
      # Same-title selection is ID-bound; no executable is invoked.
      with closing(sqlite3.connect(db)) as c, c:
       c.execute("UPDATE devices SET last_seen=?,expires=? WHERE id='host'",(int(time.time()),int(time.time())+300))
       c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=?',(int(time.time()),));c.execute("UPDATE threads SET can_send=1,status='queue_ready'")
      chosen=next(r for r in api('/v1/threads')['threads'] if r['native_id']==ids[1])
      command=api('/v1/commands',{'request_id':str(uuid.uuid4()),'thread_id':chosen['id'],'text':'explicit mock selection','expected_revision':chosen['revision'],'created_at':int(time.time()),'kind':'send'})
      with closing(sqlite3.connect(db)) as c, c:
       payload=json.loads(c.execute('SELECT payload FROM commands WHERE id=?',(command['id'],)).fetchone()[0]);assert payload['native_id']==ids[1] and payload['thread_id']==chosen['id']
       assert c.execute('SELECT count(*) FROM capture_queue_ledger').fetchone()[0]==0
       assert c.execute('SELECT count(*) FROM outbox').fetchone()[0]==0
      print(('ALL MODE: ' if all_mode else 'CATALOG MODE: ')+'PASS: titled final records; independent inbox with no Codex; sync stopped + Hub restart + sender offline remain readable; duplicate names route by unique ID; no sends or third-party body')
     finally:
      if sync:sync.terminate();sync.wait(timeout=3)
      hub.terminate();hub.wait(timeout=3)

if __name__=="__main__":main()
