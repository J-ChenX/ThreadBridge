from contextlib import closing
import json,sqlite3,tempfile,unittest,uuid,os,subprocess,sys
from pathlib import Path
from unittest.mock import patch
from capture_completion import capture,MAX_REPLY_BYTES
from capture_health import read_health,update_health,health_path,SLOT
T='00000000-0000-4000-8000-000000000031'
U='00000000-0000-4000-8000-000000000032'
class DurableCaptureTests(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.db=Path(self.temp.name)/'capture.sqlite';self.event={'type':'agent-turn-complete','thread-id':T,'turn-id':U,'last-assistant-message':'complete final','input-messages':['NEVER STORE'],'cwd':'NEVER STORE'}
 def save(self,**kw):return capture(json.dumps(self.event),T,self.db,**kw)
 def test_long_term_rows_and_bytes_have_no_test_budget(self):
  with closing(sqlite3.connect(self.db)) as c, c:
   c.execute('CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER,PRIMARY KEY(thread_id,turn_id))')
   c.executemany('INSERT INTO captured_replies VALUES(?,?,?,?,?)',[(T,str(uuid.uuid4()),'x'*17000,17000,1) for _ in range(1001)])
  self.assertEqual(self.save(),'captured')
  with closing(sqlite3.connect(self.db)) as c, c:self.assertEqual(c.execute('SELECT count(*),sum(utf8_bytes) FROM captured_replies').fetchone(),(1002,17017000+14))
 def test_budget_failure_is_durable_and_original_event_retry_resolves(self):
  with self.assertRaisesRegex(ValueError,'budget'):self.save(storage_budget=1)
  self.assertEqual(read_health(self.db)['failures'][T+':'+U]['reason'],'storage_budget_exceeded')
  with closing(sqlite3.connect(self.db)) as c, c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],0)
  self.assertEqual(self.save(),'captured');self.assertEqual(self.save(),'duplicate');self.assertEqual(read_health(self.db)['failures'],{})
  self.assertNotIn(b'NEVER STORE',self.db.read_bytes());self.assertNotIn(b'complete final',health_path(self.db).read_bytes())
 def test_disk_guard_preserves_prior_reply_and_failure_status(self):
  self.save();self.event['turn-id']=str(uuid.uuid4())
  with patch('capture_completion.shutil.disk_usage') as usage:
   usage.return_value.free=0
   with self.assertRaisesRegex(ValueError,'disk_space_low'):self.save()
  state=read_health(self.db);self.assertEqual(next(iter(state['failures'].values()))['reason'],'disk_space_low')
  with closing(sqlite3.connect(self.db)) as c, c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],1)
 def test_storage_failure_is_generic_and_intent_survives_restart(self):
  with patch('capture_completion.capture_record',side_effect=sqlite3.OperationalError('PRIVATE SQL ERROR')):
   with self.assertRaises(sqlite3.OperationalError):self.save()
  self.assertEqual(next(iter(read_health(self.db)['failures'].values()))['reason'],'storage_write_failed')
  self.assertNotIn(b'PRIVATE',health_path(self.db).read_bytes())
 def test_oversize_does_not_truncate_and_records_unsaved(self):
  self.event['last-assistant-message']='x'*(MAX_REPLY_BYTES+1)
  with self.assertRaisesRegex(ValueError,'reply_too_large'):self.save()
  self.assertFalse(self.db.exists());self.assertEqual(next(iter(read_health(self.db)['failures'].values()))['reason'],'reply_too_large')
 def test_interrupted_capture_remains_unconfirmed(self):
  update_health(self.db,T,U,'capture_not_confirmed')
  self.assertEqual(next(iter(read_health(self.db)['failures'].values()))['reason'],'capture_not_confirmed')
  self.save();self.assertEqual(read_health(self.db)['failures'],{})
 def test_corrupt_new_slot_falls_back_to_prior_durable_intent(self):
  update_health(self.db,T,U,'capture_not_confirmed');update_health(self.db,T,U,None)
  with health_path(self.db).open('r+b') as f:f.seek(0);f.write(b'corrupt');f.flush();os.fsync(f.fileno())
  self.assertEqual(next(iter(read_health(self.db)['failures'].values()))['reason'],'capture_not_confirmed')
 def test_failure_overflow_is_sticky_and_never_claims_health(self):
  for _ in range(129):update_health(self.db,T,str(uuid.uuid4()),'disk_space_low')
  self.assertTrue(read_health(self.db)['overflow']);self.assertEqual(len(read_health(self.db)['failures']),128)
 def test_concurrent_success_does_not_clear_other_attempt_failure(self):
  update_health(self.db,T,U,'capture_not_confirmed','first')
  update_health(self.db,T,U,'conflicting_reply','second')
  update_health(self.db,T,U,None,'first')
  self.assertEqual(next(iter(read_health(self.db)['failures'].values()))['reason'],'conflicting_reply')
 def test_selected_python_catalog_cli(self):
  script=Path(__file__).resolve().parents[2] / 'scripts/capture_catalog.py';argv=[sys.executable,str(script),'--all-tasks','--database',str(self.db),'--storage-budget-bytes','1',json.dumps(self.event)]
  first=subprocess.run(argv,text=True,capture_output=True);self.assertEqual(first.returncode,2);self.assertNotIn('complete final',first.stderr)
  state=read_health(self.db);self.assertEqual(next(iter(state['failures'].values()))['reason'],'storage_budget_exceeded')
  argv=argv[:-3]+[json.dumps(self.event)]
  second=subprocess.run(argv,text=True,capture_output=True);self.assertEqual(second.returncode,0,second.stderr)
if __name__=='__main__':unittest.main()
