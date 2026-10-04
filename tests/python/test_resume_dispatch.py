import fcntl,os,json,sqlite3,tempfile,time,unittest,uuid,sys
from pathlib import Path
from resume_dispatch import dispatch,key,grant_hash,Stop,receipt,worker,reconcile_completed,preflight
HOST='00000000-0000-4000-8000-000000000051'
NATIVE='00000000-0000-4000-8000-000000000052'
PRIOR='00000000-0000-4000-8000-000000000053'
TURN='00000000-0000-4000-8000-000000000054'
OTHER='00000000-0000-4000-8000-000000000055'
class Fixture:
 def __init__(self,root):
  self.root=Path(root);self.db=self.root/'hub.sqlite';self.capture=self.root/'capture.sqlite';self.binary=self.root/'mock-codex';self.calls=self.root/'calls.jsonl';self.mode=self.root/'mode';self.mode.write_text('ok');self.command=str(uuid.uuid4());self.thread=key(HOST,NATIVE)
  self.grant={'native_id':NATIVE,'host_id':HOST,'cwd':str(self.root),'verified_cli_version':'codex-cli fixture','expires_at':int(time.time())+120,'tool_compatibility_confirmed':True,'handoff_confirmed':True,'approval_policy':'on-request','sandbox_mode':'read-only'}
  with sqlite3.connect(self.db) as c:
   c.executescript("CREATE TABLE devices(id TEXT PRIMARY KEY,role TEXT,revoked INTEGER,expires INTEGER);CREATE TABLE threads(id TEXT PRIMARY KEY,revision TEXT);CREATE TABLE commands(id TEXT PRIMARY KEY,host TEXT,thread TEXT,status TEXT,payload TEXT,expires INTEGER,error TEXT,native_turn TEXT,created INTEGER NOT NULL DEFAULT 0);CREATE TABLE capture_queue_ledger(id TEXT PRIMARY KEY,status TEXT);CREATE TABLE events(kind TEXT,thread TEXT,created INTEGER);CREATE TABLE resume_owners(thread TEXT PRIMARY KEY,grant_hash TEXT,expires INTEGER);")
   c.execute('INSERT INTO devices VALUES(?,?,0,?)',(HOST,'agent',int(time.time())+120));c.execute('INSERT INTO threads VALUES(?,?)',(self.thread,PRIOR))
   payload={'id':self.command,'native_id':NATIVE,'thread_id':self.thread,'expected_revision':PRIOR,'text':'synthetic phone message','kind':'send'}
   c.execute("INSERT INTO commands(id,host,thread,status,payload,expires) VALUES(?,?,?,'accepted',?,?)",(self.command,HOST,self.thread,json.dumps(payload),int(time.time())+60))
   c.execute('UPDATE commands SET created=?',(int(time.time()),))
   c.execute('INSERT INTO resume_owners VALUES(?,?,?)',(self.thread,grant_hash(self.grant),int(time.time())+120))
  source=Path(__file__).resolve().parents[2] / 'scripts'
  self.binary.write_text('#!/usr/bin/python3\n'+"import sys,json,time\nfrom pathlib import Path\nsys.path.insert(0,"+repr(str(source))+')\nfrom capture_completion import capture\n'+
   'mode=Path('+repr(str(self.mode))+').read_text()\n'+'calls=Path('+repr(str(self.calls))+')\n'+
   "if '--version' in sys.argv:print('codex-cli fixture');sys.exit(0)\nassert sys.argv[1:] == ['app-server']\n"+
   'cwd='+repr(str(self.root))+'\nprior='+repr(PRIOR)+'\nturn='+repr(TURN)+'\n'+
   "def emit(row):print(json.dumps(row),flush=True)\n"+
   "for line in sys.stdin:\n r=json.loads(line);method=r['method'];params=r.get('params',{})\n with calls.open('a') as f:f.write(json.dumps({'method':method,'thread':params.get('threadId'),'text':params.get('input',[{}])[0].get('text')})+'\\n')\n if 'id' not in r:continue\n identity=r['id'];native=params.get('threadId')\n"+
   " if method=='initialize':result={}\n elif method in ('thread/read','thread/resume'):\n  original=native\n  if mode=='wrong_id' and method=='thread/resume':original="+repr(OTHER)+"\n  result={'thread':{'id':original,'cwd':cwd,'ephemeral':False,'forkedFromId':None,'status':{'type':'idle'},'canAcceptDirectInput':True},'cwd':cwd,'approvalPolicy':'on-request','approvalsReviewer':'user','sandbox':{'type':'readOnly','writableRoots':[],'networkAccess':False}}\n  if mode=='bad_reviewer' and method=='thread/resume':result['approvalsReviewer']='auto_review'\n  if mode=='no_direct_input' and method=='thread/resume':result['thread']['canAcceptDirectInput']=False\n  if mode=='bad_policy' and method=='thread/resume':result['sandbox']['networkAccess']=True\n elif method=='thread/turns/list':\n  assert params['itemsView']=='summary' and params['limit']==1\n  result={'data':[{'id':prior if mode!='stale' else 'stale','status':'completed'}]}\n elif method=='turn/start':\n  assert native=="+repr(NATIVE)+" and params['approvalPolicy']=='on-request' and params['approvalsReviewer']=='user' and not params['sandboxPolicy']['networkAccess']\n  text=params['input'][0]['text'];assert not text.startswith('[ThreadBridge request:')\n  if mode in ('approval','tool'):\n   emit({'id':'callback','method':'item/commandExecution/requestApproval' if mode=='approval' else 'item/tool/call','params':{'threadId':native}});continue\n  emit({'id':identity,'result':{'turn':{'id':turn,'status':'inProgress'}}})\n  if mode=='hang':time.sleep(30);continue\n  event={'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'last-assistant-message':'synthetic final','input-messages':[text]}\n  capture(json.dumps(event),native,"+repr(str(self.capture))+",'same title')\n  completed=turn if mode!='wrong_turn' else "+repr(OTHER)+"\n  emit({'method':'turn/completed','params':{'threadId':native,'turn':{'id':completed,'status':'completed'}}});continue\n else:raise RuntimeError('unexpected method')\n emit({'id':identity,'result':result})\n")
  code=self.binary.read_text()
  code=code.replace("  capture(json.dumps(event),native,", "  if mode not in ('client_final_only','legacy_phase'):capture(json.dumps(event),native,")
  marker="  completed=turn if mode!='wrong_turn'"
  extra="  if mode in ('client_final_only','legacy_phase'):\n   emit({'method':'item/completed','params':{'threadId':native,'turnId':turn,'item':{'type':'agentMessage','phase':'final_answer' if mode=='client_final_only' else None,'text':'synthetic final'}}})\n"
  code=code.replace(marker,extra+marker);code=code.replace("'input-messages':[text]","'input-messages':[] if mode=='without_marker' else [text]");self.binary.write_text(code)
  self.binary.chmod(0o700)
 def run(self,**kw):return dispatch(self.db,self.capture,str(self.binary),self.grant,self.command,execute=True,**kw)
 def methods(self):return [json.loads(x)['method'] for x in self.calls.read_text().splitlines()] if self.calls.exists() else []
class ResumeTests(unittest.TestCase):
 def setUp(self):self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.f=Fixture(self.temp.name)
 def test_preflight_restores_same_id_without_generation(self):
  with sqlite3.connect(self.f.db) as c:c.execute("UPDATE commands SET status='cancelled'")
  result=preflight(self.f.db,str(self.f.binary),self.f.grant,execute=True)
  self.assertTrue(result['no_turn_started']);self.assertEqual(result['native_id'],NATIVE)
  self.assertNotIn('turn/start',self.f.methods());self.assertNotIn('thread/fork',self.f.methods())
 def test_late_verified_final_resolves_without_relaunch(self):
  self.f.run()
  with sqlite3.connect(self.f.db) as c:c.execute("UPDATE commands SET status='unknown'");c.execute("UPDATE resume_dispatch_ledger SET status='unknown'")
  calls=len(self.f.methods());reconcile_completed(self.f.db,self.f.capture,self.f.grant)
  with sqlite3.connect(self.f.db) as c:self.assertEqual(c.execute('SELECT status,native_turn FROM commands').fetchone(),('codex_accepted',TURN))
  self.assertEqual(len(self.f.methods()),calls)
 def test_late_reconcile_cannot_guess_a_legacy_queue_turn(self):
  with sqlite3.connect(self.f.db) as c:c.execute("UPDATE commands SET status='upstream_queued'");c.execute("INSERT INTO capture_queue_ledger VALUES(?,'upstream_queued')",(self.f.command,))
  reconcile_completed(self.f.db,self.f.capture,self.f.grant)
  with sqlite3.connect(self.f.db) as c:self.assertEqual(c.execute('SELECT status,native_turn FROM commands').fetchone(),('upstream_queued',None))
  self.assertFalse(self.f.calls.exists())
 def test_worker_refuses_active_competitor_before_new_process(self):
  with self.assertRaisesRegex(Stop,'old_queue_worker_not_stopped'):worker(self.f.db,self.f.capture,str(self.f.binary),self.f.grant,True,competitor_check=lambda:False,stop_after=1)
  self.assertFalse(self.f.calls.exists())
 def test_worker_exclusive_lock_rejects_second_owner(self):
  fd=os.open(str(self.f.db)+'.'+self.f.thread+'.resume.lock',os.O_CREAT|os.O_RDWR,0o600)
  try:
   fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
   with self.assertRaisesRegex(Stop,'resume_owner_running'):worker(self.f.db,self.f.capture,str(self.f.binary),self.f.grant,True,competitor_check=lambda:True,stop_after=1)
  finally:os.close(fd)
  self.assertFalse(self.f.calls.exists())
 def test_worker_never_provisions_over_unresolved_request(self):
  with sqlite3.connect(self.f.db) as c:c.execute("UPDATE commands SET status='unknown'")
  with self.assertRaisesRegex(Stop,'unresolved_request_before_handoff'):worker(self.f.db,self.f.capture,str(self.f.binary),self.f.grant,True,competitor_check=lambda:True,stop_after=1)
  self.assertFalse(self.f.calls.exists())
 def test_original_id_turn_and_unmodified_input_match(self):
  self.assertEqual(self.f.run(),('codex_accepted',None,TURN));self.assertEqual(self.f.run(),('codex_accepted',None,TURN))
  self.assertEqual(self.f.methods().count('turn/start'),1);self.assertNotIn('thread/start',self.f.methods());self.assertNotIn('thread/fork',self.f.methods())
  with sqlite3.connect(self.f.capture) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_request_ids').fetchone()[0],0)
  starts=[json.loads(line) for line in self.f.calls.read_text().splitlines() if json.loads(line)['method']=='turn/start']
  self.assertEqual(starts[0]['text'],'synthetic phone message')
  with sqlite3.connect(self.f.db) as c:self.assertEqual(c.execute('SELECT native_turn FROM commands WHERE id=?',(self.f.command,)).fetchone()[0],TURN)
 def test_authoritative_turn_correlates_even_when_notify_omits_input_marker(self):
  self.f.mode.write_text('without_marker');self.assertEqual(self.f.run(),('codex_accepted',None,TURN))
  with sqlite3.connect(self.f.capture) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_request_ids').fetchone()[0],0)
 def test_verified_final_phase_saves_without_official_notify(self):
  self.f.mode.write_text('client_final_only');self.assertEqual(self.f.run(),('codex_accepted',None,TURN))
  with sqlite3.connect(self.f.capture) as c:self.assertEqual(c.execute('SELECT reply FROM captured_replies').fetchone()[0],'synthetic final')
 def test_unknown_message_phase_is_not_promoted_to_final(self):
  self.f.mode.write_text('legacy_phase');self.assertEqual(self.f.run(capture_timeout=.1)[:2],('unknown','final_capture_not_confirmed'))
  self.assertFalse(self.f.capture.exists())
 def test_original_id_and_policy_mismatch_never_start_turn(self):
  for mode,reason in [('wrong_id','original_id_mismatch'),('bad_policy','sandbox_policy_mismatch'),('bad_reviewer','approval_policy_mismatch'),('no_direct_input','direct_input_capability_unconfirmed'),('stale','original_context_revision_mismatch')]:
   with self.subTest(mode=mode),tempfile.TemporaryDirectory() as temp:
    f=Fixture(temp);f.mode.write_text(mode);self.assertEqual(f.run()[:2],('rejected',reason));self.assertNotIn('turn/start',f.methods())
 def test_approval_or_missing_dynamic_tools_require_user_and_never_retry(self):
  for mode,reason in [('approval','user_action_required'),('tool','desktop_dynamic_tool_unavailable')]:
   with self.subTest(mode=mode),tempfile.TemporaryDirectory() as temp:
    f=Fixture(temp);f.mode.write_text(mode);self.assertEqual(f.run()[:2],('unknown',reason));f.run();self.assertEqual(f.methods().count('turn/start'),1)
 def test_wrong_completion_turn_is_unknown(self):
  self.f.mode.write_text('wrong_turn');self.assertEqual(self.f.run()[:2],('unknown','completion_turn_mismatch'))
 def test_timeout_then_restart_does_not_send_again(self):
  self.f.mode.write_text('hang');self.assertEqual(self.f.run(turn_timeout=.1)[:2],('unknown','upstream_timeout'));self.f.run();self.assertEqual(self.f.methods().count('turn/start'),1)
 def test_prior_queue_unresolved_blocks_before_any_process(self):
  with sqlite3.connect(self.f.db) as c:c.execute('INSERT INTO capture_queue_ledger VALUES(?,?)',(self.f.command,'upstream_queued'))
  with self.assertRaisesRegex(Stop,'legacy_queue_unresolved'):self.f.run()
  self.assertFalse(self.f.calls.exists())
 def test_disabled_or_unconfirmed_tools_never_spawn(self):
  with self.assertRaisesRegex(Stop,'execution_not_enabled'):dispatch(self.f.db,self.f.capture,str(self.f.binary),self.f.grant,self.f.command)
  self.f.grant['tool_compatibility_confirmed']=False
  with self.assertRaisesRegex(Stop,'tool_compatibility'):self.f.run()
  self.assertFalse(self.f.calls.exists())
 def test_other_pending_command_blocks_single_thread(self):
  with sqlite3.connect(self.f.db) as c:c.execute('INSERT INTO commands(id,host,thread,status) VALUES(?,?,?,?)',(str(uuid.uuid4()),HOST,self.f.thread,'unknown'))
  with self.assertRaisesRegex(Stop,'thread_command_pending'):self.f.run()
  self.assertFalse(self.f.calls.exists())
 def test_interrupted_intent_reopens_unknown_without_spawn(self):
  with sqlite3.connect(self.f.db) as c:
   c.execute('CREATE TABLE resume_dispatch_ledger(id TEXT PRIMARY KEY,host TEXT,native TEXT,status TEXT,turn TEXT,error TEXT,created INTEGER)');c.execute('INSERT INTO resume_dispatch_ledger(id,status) VALUES(?,?)',(self.f.command,'intent'))
  self.assertEqual(self.f.run()[:2],('unknown','interrupted_resume_no_retry'));self.assertFalse(self.f.calls.exists())
 def test_owner_grant_is_required_and_title_never_routes(self):
  with sqlite3.connect(self.f.db) as c:c.execute('DELETE FROM resume_owners')
  with self.assertRaisesRegex(Stop,'owner_not_authorized'):self.f.run()
  self.assertFalse(self.f.calls.exists())
if __name__=='__main__':unittest.main()
