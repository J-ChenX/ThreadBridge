#!/usr/bin/env python3
"""Reversible local project rollout. Never resets collection, pairs or sends a message."""
import argparse,hashlib,json,os,shutil,sqlite3,subprocess,time
from pathlib import Path

def run(*argv):return subprocess.check_output(list(map(str,argv)),text=True,stderr=subprocess.STDOUT)
def ro(path):return sqlite3.connect('file:'+Path(path).resolve().as_posix()+'?mode=ro',uri=True)
def protected(path):
 with ro(path) as db:
  tables={r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
  names=['threads','messages','devices','pairings','commands','capture_queue_ledger','capture_queue_send_times','tombstones','settings']
  return {t:hashlib.sha256(json.dumps(db.execute('SELECT * FROM '+t+' ORDER BY rowid').fetchall(),ensure_ascii=False,default=str).encode()).hexdigest() for t in names if t in tables}
def guard(path):
 with ro(path) as db:
  assert db.execute("SELECT count(*) FROM commands WHERE status IN ('accepted','dispatching')").fetchone()[0]==0,'active send: defer rollout'
  assert db.execute("SELECT count(*) FROM capture_queue_ledger WHERE status='intent'").fetchone()[0]==0,'active intent: defer rollout'
def start_all(units):
 failures=[]
 for unit in units:
  try:run('systemctl','--user','start',unit)
  except Exception as error:failures.append(unit+': '+str(error))
 # Verify every unit even if an earlier start failed.
 for unit in units:
  try:
   if run('systemctl','--user','show',unit,'--property=ActiveState','--value').strip()!='active':failures.append(unit+': not active')
  except Exception as error:failures.append(unit+': '+str(error))
 return failures

def main():
 p=argparse.ArgumentParser();p.add_argument('--fleet',type=Path,required=True);p.add_argument('--binary',type=Path,required=True);p.add_argument('--capture-db',type=Path,required=True);p.add_argument('--native-index',type=Path,required=True);p.add_argument('--host',required=True);p.add_argument('--apply',action='store_true');a=p.parse_args();cfg=json.loads(a.fleet.read_text());hub=Path(cfg['hub_db']);assert any(h.get('local') and h['host_id']==a.host for h in cfg['hosts'].values()),'host mismatch'
 units=['threadbridge-phone-hub.service','threadbridge-phone-inbox.service','threadbridge-phone-capture.service']+[f'threadbridge-remote@{n}.service' for n,h in cfg['hosts'].items() if not h.get('local')]
 active=[u for u in units if run('systemctl','--user','show',u,'--property=ActiveState','--value').strip()=='active'];guard(hub)
 print(run(a.binary,'project-align','--capture-db',a.capture_db,'--native-index',a.native_index,'--db',hub,'--host',a.host).strip(),flush=True)
 if not a.apply:return
 root=Path.cwd();backup=root/'local/backups'/time.strftime('project-ui016-%Y%m%dT%H%M%S');backup.mkdir(parents=True,mode=0o700);shutil.copy2(a.fleet,backup/'fleet.json')
 originals={};dropins=[]
 for u in active:
  definition=run('systemctl','--user','cat',u);(backup/(u+'.definition')).write_text(definition);originals[u]=next(line.removeprefix('ExecStart=') for line in reversed(definition.splitlines()) if line.startswith('ExecStart=') and line!='ExecStart=')
 (backup/'originals.json').write_text(json.dumps(originals));os.chmod(backup/'originals.json',0o600)
 # Stop ingress first, then stop readers/queue workers; a second guard closes the race.
 stopped=[]
 try:
  for u in active:run('systemctl','--user','stop',u);stopped.append(u)
  guard(hub)
  for source,name in [(hub,'hub.sqlite'),(a.capture_db,'capture.sqlite')]:
   with ro(source) as c,sqlite3.connect(backup/name) as dst:c.backup(dst)
   os.chmod(backup/name,0o600)
  before=protected(hub)
  target=root/'local/runtime/threadbridge-ui016-20261006';shutil.copy2(a.binary,target);os.chmod(target,0o700)
  # Change only the executable, preserving every existing service argument.
  for u in active:
   argv=originals[u];_,rest=argv.split(' ',1)
   directory=Path.home()/'.config/systemd/user'/f'{u}.d';directory.mkdir(parents=True,exist_ok=True);dest=directory/'zz-project-ui016.conf'
   assert not dest.exists(),'rollout already staged';dest.write_text('[Service]\nExecStart=\nExecStart='+str(target)+' '+rest+'\n');dropins.append(dest)
  cfg['binary']=str(target)
  for h in cfg['hosts'].values():
   if not h.get('local'):h['remote_binary']=str(root/'local/runtime/threadbridge-ui016-windows-20261006.exe') if h['platform']=='windows' else str(target)
  a.fleet.write_text(json.dumps(cfg,ensure_ascii=False,indent=2)+'\n');os.chmod(a.fleet,0o600)
  run(target,'project-align','--capture-db',a.capture_db,'--native-index',a.native_index,'--db',hub,'--host',a.host,'--apply')
  assert protected(hub)==before,'protected data changed'
  run('systemctl','--user','daemon-reload')
  failures=start_all(active)
  if failures:raise RuntimeError('new services failed: '+'; '.join(failures))
 except BaseException as error:
  # Keep aligned metadata; restoring whole DBs could overwrite concurrent work.
  # Backups permit explicit metadata-only recovery if the mapping itself is wrong.
  recovery=[]
  for unit in stopped:
   try:run('systemctl','--user','stop',unit)
   except Exception as failure:recovery.append(unit+': stop during rollback: '+str(failure))
  for file in dropins:
   try:file.unlink(missing_ok=True)
   except Exception as failure:recovery.append(str(file)+': '+str(failure))
  try:shutil.copy2(backup/'fleet.json',a.fleet)
  except Exception as failure:recovery.append('fleet restore: '+str(failure))
  try:run('systemctl','--user','daemon-reload')
  except Exception as failure:recovery.append('daemon-reload: '+str(failure))
  recovery.extend(start_all(active))
  raise RuntimeError('rollout failed: '+str(error)+'; original service recovery: '+('; '.join(recovery) if recovery else 'all active')) from error
 print('PASS local program/project alignment; protected tables unchanged; backup '+str(backup),flush=True)
if __name__=='__main__':main()
