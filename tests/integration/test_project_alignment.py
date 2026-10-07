"""Metadata-only alignment preserves existing bodies, identities, policy and requests."""
from pathlib import Path
import hashlib,json,os,sqlite3,subprocess,sys,tempfile,time

def main():
 binary=Path(sys.argv[1]).resolve()
 with tempfile.TemporaryDirectory(prefix='tb-project-align-') as tmp:
  root=Path(tmp);capture=root/'capture.sqlite';hub=root/'hub.sqlite';index=root/'index.sqlite';native='00000000-0000-4000-8000-000000000001';turn='00000000-0000-4000-8000-000000000002';host='fixture';env={'PATH':'/usr/bin:/bin','HOME':tmp}
  def run(*args):return subprocess.check_output([binary,*map(str,args)],env=env,text=True)
  run('enable-writes','--db',hub,'--reconciled')
  with sqlite3.connect(hub) as c:
   c.execute("INSERT INTO devices(id,role,name,expires) VALUES(?,'agent','fixture',?)",(host,int(time.time())+3600))
  run('capture','--database',capture,'--thread',native,json.dumps({'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'cwd':'/registered/root/sub','last-assistant-message':'body must stay exact'}))
  run('capture-import','--db',hub,'--capture-db',capture,'--host',host,'--thread',native)
  with sqlite3.connect(index) as c:
   c.executescript("CREATE TABLE threads(id TEXT,cwd TEXT,project_id TEXT);CREATE TABLE projects(id TEXT,name TEXT);CREATE TABLE project_roots(project_id TEXT,position INTEGER,path TEXT);INSERT INTO projects VALUES('p','我的注册项目');INSERT INTO project_roots VALUES('p',0,'/registered/root');")
   c.execute('INSERT INTO threads VALUES(?,?,NULL)',(native,'/registered/root/sub'));c.execute("INSERT INTO threads VALUES('excluded-old','/registered/root',NULL)")
  def invariant():
   with sqlite3.connect(hub) as c:return {table:c.execute('SELECT * FROM '+table+' ORDER BY rowid').fetchall() for table in ['threads','messages','devices','commands','tombstones','settings','capture_queue_ledger']}
  before=invariant();plan=json.loads(run('project-align','--capture-db',capture,'--native-index',index,'--db',hub,'--host',host));assert not plan['applied'];assert invariant()==before
  run('project-align','--capture-db',capture,'--native-index',index,'--db',hub,'--host',host,'--apply');assert invariant()==before
  with sqlite3.connect(hub) as c:
   assert c.execute('SELECT project FROM thread_projects').fetchall()==[('/registered/root',)];assert c.execute('SELECT name FROM project_names').fetchall()==[('我的注册项目',)];assert c.execute('SELECT count(*) FROM threads').fetchone()==(1,)
  with sqlite3.connect(index) as c:c.execute("UPDATE project_roots SET path='/moved'");c.execute("DELETE FROM threads WHERE id='excluded-old'")
  run('project-align','--capture-db',capture,'--native-index',index,'--db',hub,'--host',host,'--apply');assert invariant()==before
  with sqlite3.connect(hub) as c:assert c.execute('SELECT project FROM thread_projects').fetchone()==('',);assert c.execute('SELECT project FROM host_projects').fetchall()==[('/moved',)]
  print('PASS metadata-only native projects -> capture -> existing Hub IDs; names, dry-run, rename/removal, body/credential/policy/request invariants; excluded history never imported')
if __name__=='__main__':main()
