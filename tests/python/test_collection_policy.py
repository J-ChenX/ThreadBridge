import json,sqlite3,tempfile,unittest,uuid,multiprocessing,time
from unittest.mock import patch
from pathlib import Path
from collection_policy import allowed,baseline,write_policy,purge,policy_path
from reset_collection import clear_replica

def delayed_capture(database,index,native,started,resume):
 import capture_catalog
 def write(*args,**kwargs):
  started.set();resume.wait(10)
  with sqlite3.connect(database) as c:c.execute('INSERT INTO captured_replies VALUES(?,?)',(native,'late reply'))
  return 'captured'
 with patch.object(capture_catalog,'capture',write):
  capture_catalog.capture_catalog(json.dumps({'thread-id':native}),None,database,index,True)

def delayed_purge(database,native,done):
 purge(database,[native]);done.set()

class CollectionTests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.index=self.root/'state.sqlite';self.db=self.root/'replies.sqlite';self.old=str(uuid.uuid4());self.new=str(uuid.uuid4())
  with sqlite3.connect(self.index) as c:
   c.execute('CREATE TABLE threads(id TEXT,created_at INTEGER,created_at_ms INTEGER)');c.execute('INSERT INTO threads VALUES(?,?,?)',(self.old,1,1000))
  self.policy=baseline(self.index,str(uuid.uuid4()));write_policy(self.db,self.policy)
 def tearDown(self):self.tmp.cleanup()
 def test_old_continued_and_late_restored_are_excluded(self):
  self.assertFalse(allowed(self.db,self.index,self.old))
  with sqlite3.connect(self.index) as c:c.execute('INSERT INTO threads VALUES(?,?,?)',(self.new,2,2000))
  self.assertFalse(allowed(self.db,self.index,self.new))
 def test_fresh_unknown_and_same_second(self):
  self.assertFalse(allowed(self.db,self.index,self.new))
  with sqlite3.connect(self.index) as c:c.execute('INSERT INTO threads VALUES(?,?,?)',(self.new,self.policy['cutoff_ms']//1000,self.policy['cutoff_ms']+1))
  self.assertTrue(allowed(self.db,self.index,self.new))
 def test_missing_or_corrupt_policy_is_closed(self):
  policy_path(self.db).unlink()
  with self.assertRaises(ValueError):allowed(self.db,self.index,self.old)
  policy_path(self.db).write_text('{}')
  with self.assertRaises(ValueError):allowed(self.db,self.index,self.old)
 def test_delete_blocks_reimport_and_removes_only_replica(self):
  from capture_health import update_health,read_health
  update_health(self.db,self.old,'failed-turn','capture_not_confirmed')
  with sqlite3.connect(self.db) as c:
   c.execute('CREATE TABLE captured_replies(thread_id TEXT,reply TEXT)');c.execute('INSERT INTO captured_replies VALUES(?,?)',(self.old,'private content'))
  purge(self.db,[self.old])
  with sqlite3.connect(self.db) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],0)
  with sqlite3.connect(self.index) as c:self.assertEqual(c.execute('SELECT count(*) FROM threads').fetchone()[0],1)
  self.assertFalse(allowed(self.db,self.index,self.old))
  self.assertEqual(read_health(self.db)['failures'],{})
 def test_reset_preserves_credentials_and_dedup_but_erases_body(self):
  with sqlite3.connect(self.db) as c:
   c.executescript("CREATE TABLE devices(id TEXT,token TEXT);INSERT INTO devices VALUES('phone','credential');CREATE TABLE commands(id TEXT,request TEXT,digest TEXT,payload TEXT,status TEXT,error TEXT);INSERT INTO commands VALUES('command','immutable-request','digest','secret','unknown',NULL);CREATE TABLE messages(thread TEXT,body TEXT);INSERT INTO messages VALUES('thread','private');CREATE TABLE settings(k TEXT PRIMARY KEY,v TEXT);")
  clear_replica(self.db,self.policy['generation'])
  with sqlite3.connect(self.db) as c:
   self.assertEqual(c.execute('SELECT token FROM devices').fetchone()[0],'credential');self.assertEqual(c.execute('SELECT request,digest,payload,status FROM commands').fetchone(),('immutable-request','digest','{}','unknown'));self.assertEqual(c.execute('SELECT count(*) FROM messages').fetchone()[0],0)
  self.assertNotIn(b'private',self.db.read_bytes());self.assertNotIn(b'secret',self.db.read_bytes())
 def test_capture_and_delete_share_cross_process_boundary(self):
  with sqlite3.connect(self.index) as c:c.execute('INSERT INTO threads VALUES(?,?,?)',(self.new,self.policy['cutoff_ms']//1000+1,self.policy['cutoff_ms']+1000))
  with sqlite3.connect(self.db) as c:c.execute('CREATE TABLE captured_replies(thread_id TEXT,reply TEXT)')
  ctx=multiprocessing.get_context('spawn');started=ctx.Event();resume=ctx.Event();done=ctx.Event()
  writer=ctx.Process(target=delayed_capture,args=(self.db,self.index,self.new,started,resume));writer.start();self.assertTrue(started.wait(5))
  deleter=ctx.Process(target=delayed_purge,args=(self.db,self.new,done));deleter.start();self.assertFalse(done.wait(.3));resume.set();writer.join(5);deleter.join(5)
  self.assertEqual(writer.exitcode,0);self.assertEqual(deleter.exitcode,0);self.assertTrue(done.is_set())
  with sqlite3.connect(self.db) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],0)
  self.assertFalse(allowed(self.db,self.index,self.new))
 def test_disabled_collection_ignores_new_threads_during_reset(self):
  self.policy['enabled']=False;write_policy(self.db,self.policy)
  with sqlite3.connect(self.index) as c:c.execute('INSERT INTO threads VALUES(?,?,?)',(self.new,self.policy['cutoff_ms']//1000+1,self.policy['cutoff_ms']+1000))
  self.assertFalse(allowed(self.db,self.index,self.new))
  final=baseline(self.index,self.policy['generation']);write_policy(self.db,final)
  self.assertFalse(allowed(self.db,self.index,self.new))

if __name__=='__main__':unittest.main()
