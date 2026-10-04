import json,sqlite3,tempfile,unittest
from pathlib import Path
from capture_user_turn import read_turn,save_turn
N='00000000-0000-4000-8000-000000000031';T='00000000-0000-4000-8000-000000000032'
class UserTurnTests(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.root=Path(self.temp.name);self.index=self.root/'index.sqlite';self.rollout=self.root/'rollout.jsonl';self.capture=self.root/'capture.sqlite'
  with sqlite3.connect(self.index) as c:c.execute('CREATE TABLE threads(id TEXT,rollout_path TEXT)');c.execute('INSERT INTO threads VALUES(?,?)',(N,str(self.rollout)))
 def source(self,complete=True,native=N):
  def user(id,text,kind='user.text',turn=T):return {'type':'response_item','payload':{'type':'message','id':id,'role':'user','content':[{'type':'input_text','text':text}],'internal_chat_message_metadata_passthrough':{'turn_id':turn,'create_time':100.6,'content_item_kinds':[kind]}}}
  rows=[{'type':'session_meta','payload':{'id':native}},user('old','previous human','user.text',N),user('env','SYSTEM CONTEXT','additional_content.environment'),user('human','[ThreadBridge request:'+N+']\nphone text')]
  if complete:rows.append({'type':'event_msg','timestamp':'1970-01-01T00:01:40.900Z','payload':{'type':'task_complete','turn_id':T}})
  self.rollout.write_text('\n'.join(json.dumps(x) for x in rows)+'\n')
 def test_current_human_only_and_marker_compatibility(self):
  self.source();rows,completed=read_turn(self.index,N,T)
  self.assertEqual([(r[0],r[1],r[2]) for r in rows],[('human','phone text',100600)]);self.assertEqual(completed,100900)
  save_turn(self.capture,N,T,rows,completed);save_turn(self.capture,N,T,rows,completed)
  with sqlite3.connect(self.capture) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_user_messages').fetchone()[0],1)
  self.assertNotIn(b'SYSTEM CONTEXT',self.capture.read_bytes());self.assertNotIn(b'previous human',self.capture.read_bytes())
 def test_wrong_identity_and_unfinished_turn_refused(self):
  self.source(native=T)
  with self.assertRaisesRegex(ValueError,'identity'):read_turn(self.index,N,T)
  self.source(complete=False)
  with self.assertRaisesRegex(ValueError,'not_complete'):read_turn(self.index,N,T)
 def test_catalog_opt_in_stores_both_sides_and_replay_is_idempotent(self):
  from capture_catalog import capture_catalog
  self.source();event=json.dumps({'type':'agent-turn-complete','thread-id':N,'turn-id':T,'last-assistant-message':'final'})
  self.assertEqual(capture_catalog(event,None,self.capture,all_tasks=True,user_turn_index=self.index),'captured')
  self.assertEqual(capture_catalog(event,None,self.capture,all_tasks=True,user_turn_index=self.index),'duplicate')
  with sqlite3.connect(self.capture) as c:
   self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],1)
   self.assertEqual(c.execute('SELECT text FROM captured_user_messages').fetchone()[0],'phone text')
 def test_user_source_failure_keeps_reply_and_exposes_missing_input(self):
  from capture_catalog import capture_catalog
  from capture_health import read_health
  self.source(complete=False);event=json.dumps({'type':'agent-turn-complete','thread-id':N,'turn-id':T,'last-assistant-message':'final'})
  with self.assertRaisesRegex(ValueError,'not_complete'):capture_catalog(event,None,self.capture,all_tasks=True,user_turn_index=self.index)
  with sqlite3.connect(self.capture) as c:self.assertEqual(c.execute('SELECT reply FROM captured_replies').fetchone()[0],'final')
  self.assertEqual(read_health(self.capture)['failures'][N+':'+T]['reason'],'user_input_capture_failed')
 def test_conflicting_replay_does_not_replace_input(self):
  self.source();rows,completed=read_turn(self.index,N,T);save_turn(self.capture,N,T,rows,completed)
  changed=[(rows[0][0],'changed',rows[0][2],rows[0][3])]
  with self.assertRaisesRegex(ValueError,'conflicting'):save_turn(self.capture,N,T,changed,completed)
if __name__=='__main__':unittest.main()
