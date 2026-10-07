"""Isolated rollout failure recovery; never calls real systemctl or native Codex."""
import importlib.util,json,sqlite3,sys,tempfile
from pathlib import Path
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('alignment',ROOT/'scripts/align_runtime.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def main():
 with tempfile.TemporaryDirectory() as td:
  root=Path(td);(root/'local/runtime').mkdir(parents=True);hub=root/'hub.sqlite';capture=root/'capture.sqlite';binary=root/'binary';binary.write_text('fixture');fleet=root/'fleet.json'
  with sqlite3.connect(hub) as db:
   db.executescript("CREATE TABLE commands(id,status);INSERT INTO commands VALUES('unknown','unknown');CREATE TABLE capture_queue_ledger(id,status);INSERT INTO capture_queue_ledger VALUES('unknown','upstream_queued');CREATE TABLE messages(body);INSERT INTO messages VALUES('preserve');")
  with sqlite3.connect(capture) as db:db.execute('CREATE TABLE metadata(value)')
  config={'hub_db':str(hub),'binary':'old','hosts':{'local':{'local':True,'host_id':'host'},'peer':{'local':False,'platform':'linux'},'windows':{'local':False,'platform':'windows'}}};fleet.write_text(json.dumps(config));original=fleet.read_bytes();before=m.protected(hub)
  active=['threadbridge-phone-hub.service','threadbridge-phone-inbox.service','threadbridge-phone-capture.service','threadbridge-remote@peer.service'];states={u:'active' for u in active};events=[];injected=False
  def fake(*args):
   nonlocal injected
   args=list(map(str,args))
   if args[0]!='systemctl':return 'fixture metadata plan'
   action=args[2];unit=args[3] if len(args)>3 else ''
   events.append((action,unit))
   if action=='show':return states.get(unit,'inactive')
   if action=='cat':return '[Service]\nExecStart=/old/threadbridge fixture --verified-version "codex-cli fixture"\n'
   if action=='stop':states[unit]='inactive'
   if action=='start':
    if not injected and list(root.glob('.config/systemd/user/*.d/zz-project-ui016.conf')):injected=True;raise RuntimeError('injected first start failure')
    states[unit]='active'
   return ''
  argv=['align','--fleet',str(fleet),'--binary',str(binary),'--capture-db',str(capture),'--native-index',str(root/'native'),'--host','host','--apply']
  with patch.object(m,'run',fake),patch.object(m.Path,'cwd',return_value=root),patch.object(m.Path,'home',return_value=root),patch.object(sys,'argv',argv):
   try:m.main();raise AssertionError('injection did not fail')
   except RuntimeError as error:assert 'original service recovery: all active' in str(error),str(error)
  assert injected and all(states[u]=='active' for u in active)
  assert all(sum(action=='start' and unit==u for action,unit in events)==2 for u in active),events
  assert not any(unit=='threadbridge-remote@windows.service' and action in ('start','stop') for action,unit in events)
  assert fleet.read_bytes()==original and not list(root.glob('.config/systemd/user/*.d/zz-project-ui016.conf'))
  assert m.protected(hub)==before
  with sqlite3.connect(hub) as db:db.execute("UPDATE commands SET status='accepted'")
  events.clear()
  with patch.object(m,'run',fake),patch.object(sys,'argv',argv):
   try:m.main();raise AssertionError('active-send guard bypassed')
   except AssertionError as error:assert 'active send' in str(error)
  assert not any(action in ('start','stop') for action,_ in events)
 print('PASS first-start failure attempts every service, restores original config/active set; inactive peer and unknown request preserved; active sends defer')
if __name__=='__main__':main()
