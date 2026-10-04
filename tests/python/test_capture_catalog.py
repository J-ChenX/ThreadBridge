import json,sqlite3,tempfile,unittest
from pathlib import Path
from capture_catalog import capture_catalog
class CatalogTests(unittest.TestCase):
 def test_approved_all_mode_reads_only_name_index_and_survives_codex_absence(self):
  with tempfile.TemporaryDirectory() as temp:
   root=Path(temp);index=root/'session_index.jsonl';db=root/'capture.sqlite';native='00000000-0000-4000-8000-000000000021'
   index.write_text(json.dumps({'id':native,'thread_name':'indexed title','updated_at':'2026-10-03'})+'\n')
   event={'type':'agent-turn-complete','thread-id':native,'turn-id':native,'last-assistant-message':'final'}
   self.assertEqual(capture_catalog(json.dumps(event),None,db,index,True),'captured')
   with sqlite3.connect(db) as c:self.assertEqual(c.execute('SELECT title FROM captured_replies').fetchone()[0],'indexed title')
   index.unlink();event['turn-id']='00000000-0000-4000-8000-000000000022'
   self.assertEqual(capture_catalog(json.dumps(event),None,db,index,True),'captured')
   with sqlite3.connect(db) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],2)
   with self.assertRaisesRegex(ValueError,'scope'):capture_catalog(json.dumps(event),None,db)
 def test_equal_titles_keep_separate_ids_and_unauthorized_tasks_never_persist(self):
  with tempfile.TemporaryDirectory() as temp:
   root=Path(temp);catalog=root/'catalog.json';db=root/'capture.sqlite';ids=['00000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000002'];catalog.write_text(json.dumps(dict.fromkeys(ids,'同名任务')))
   for native in ids:
    event={'type':'agent-turn-complete','thread-id':native,'turn-id':native,'last-assistant-message':native,'input-messages':['DO NOT STORE'],'cwd':'DO NOT STORE'}
    self.assertEqual(capture_catalog(json.dumps(event),catalog,db),'captured');self.assertEqual(capture_catalog(json.dumps(event),catalog,db),'duplicate')
   event['thread-id']='00000000-0000-4000-8000-000000000003';self.assertEqual(capture_catalog(json.dumps(event),catalog,db),'ignored')
   with sqlite3.connect(db) as c:
    rows=c.execute('SELECT thread_id,title,reply FROM captured_replies ORDER BY thread_id').fetchall();self.assertEqual(rows,[(i,'同名任务',i) for i in ids])
    self.assertNotIn(b'DO NOT STORE',db.read_bytes())
if __name__=='__main__':unittest.main()
