from contextlib import closing
import json
from pathlib import Path
import sqlite3
import tempfile
import subprocess
import sys
import unittest
from capture_reply import capture, MAX_REPLY_BYTES

THREAD = '00000000-0000-4000-8000-000000000001'
TURN = '00000000-0000-4000-8000-000000000002'

class CaptureTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = Path(self.tmp.name) / 'capture.sqlite'
        self.event = {'type': 'agent-turn-complete', 'thread-id': THREAD, 'turn-id': TURN, 'last-assistant-message': '中文回复\n' * 200, 'input-messages': ['DO NOT STORE THIS'], 'cwd': '/do-not-store'}
    def run_capture(self):
        return capture(json.dumps(self.event), THREAD, self.db)
    def test_marker_stores_only_uuid_and_ambiguous_markers_do_not_ack(self):
        self.event['input-messages']=['[ThreadBridge request:'+THREAD+']\nPRIVATE INPUT']
        self.run_capture()
        with closing(sqlite3.connect(self.db)) as db, db:
            self.assertEqual(db.execute('SELECT request_id FROM captured_request_ids').fetchall(),[(THREAD,)])
            self.assertNotIn(b'PRIVATE INPUT',self.db.read_bytes())
        self.event['turn-id']='00000000-0000-4000-8000-000000000003'
        self.event['input-messages'].append('[ThreadBridge request:'+TURN+']\nOTHER INPUT')
        self.run_capture()
        with closing(sqlite3.connect(self.db)) as db, db:self.assertEqual(db.execute('SELECT count(*) FROM captured_request_ids').fetchone()[0],1)
    def test_title_identity_and_recorded_time_survive_reopen(self):
        self.assertEqual(capture(json.dumps(self.event),THREAD,self.db,'原任务标题'),'captured')
        with closing(sqlite3.connect(self.db)) as db, db:
            row=db.execute('SELECT title,thread_id,turn_id,captured_at,reply FROM captured_replies').fetchone()
            self.assertEqual(row[:3],('原任务标题',THREAD,TURN));self.assertGreater(row[3],0);self.assertEqual(row[4],self.event['last-assistant-message'])
    def test_legacy_database_migrates_without_replaying_or_losing_reply(self):
        with closing(sqlite3.connect(self.db)) as db, db:
            db.execute('CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER,PRIMARY KEY(thread_id,turn_id))')
            db.execute('INSERT INTO captured_replies VALUES(?,?,?,?,?)',(THREAD,TURN,'legacy',6,1))
        self.event['turn-id']='00000000-0000-4000-8000-000000000003'
        capture(json.dumps(self.event),THREAD,self.db,'registered')
        with closing(sqlite3.connect(self.db)) as db, db:
            self.assertEqual(db.execute('SELECT reply FROM captured_replies WHERE turn_id=?',(TURN,)).fetchone()[0],'legacy')
            self.assertEqual(db.execute('SELECT title FROM captured_replies WHERE turn_id=?',(self.event['turn-id'],)).fetchone()[0],'registered')
    def test_allowlist_filters_before_storage(self):
        self.event['thread-id'] = TURN
        self.assertEqual(self.run_capture(), 'ignored')
        self.assertFalse(self.db.exists())
    def test_exact_reply_survives_reopen_without_input(self):
        self.assertEqual(self.run_capture(), 'captured')
        with closing(sqlite3.connect(self.db)) as db, db:
            row = db.execute('SELECT thread_id,turn_id,reply FROM captured_replies').fetchone()
            self.assertEqual(row, (THREAD, TURN, self.event['last-assistant-message']))
            self.assertNotIn('input-messages', [r[1] for r in db.execute('PRAGMA table_info(captured_replies)')])
        self.assertEqual(self.run_capture(), 'duplicate')
    def test_actual_cli_reopens_durable_reply(self):
        script = Path(__file__).resolve().parents[2] / 'scripts/capture_reply.py'
        argv = [sys.executable, str(script), '--thread', THREAD, '--database', str(self.db), json.dumps(self.event)]
        first = subprocess.run(argv, text=True, capture_output=True, check=True)
        second = subprocess.run(argv, text=True, capture_output=True, check=True)
        self.assertEqual(first.stdout.strip(), 'captured')
        self.assertEqual(second.stdout.strip(), 'duplicate')
        with closing(sqlite3.connect(self.db)) as db, db:
            self.assertEqual(db.execute('SELECT count(*) FROM captured_replies').fetchone()[0], 1)
    def test_other_event_type_is_ignored(self):
        self.event['type'] = 'unrelated'
        self.assertEqual(self.run_capture(), 'ignored')
        self.assertFalse(self.db.exists())
    def test_conflicting_duplicate_does_not_replace(self):
        self.run_capture()
        self.event['last-assistant-message'] = 'changed'
        with self.assertRaisesRegex(ValueError, 'conflicting'): self.run_capture()
        with closing(sqlite3.connect(self.db)) as db, db:
            self.assertNotEqual(db.execute('SELECT reply FROM captured_replies').fetchone()[0], 'changed')
    def test_oversize_rejected_without_truncation_or_storage(self):
        self.event['last-assistant-message'] = 'x' * (MAX_REPLY_BYTES + 1)
        with self.assertRaisesRegex(ValueError, 'too_large'): self.run_capture()
        self.assertFalse(self.db.exists())
    def test_missing_turn_or_reply_rejected(self):
        del self.event['turn-id']
        with self.assertRaises(ValueError): self.run_capture()
        self.assertFalse(self.db.exists())

if __name__ == '__main__': unittest.main()
