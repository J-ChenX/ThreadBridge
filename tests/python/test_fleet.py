"""有界四机捕获与 SSH RPC 的隔离合同测试，不连接真实设备。"""
import base64
import json
import io
import gc
import subprocess
import sys
import time
from contextlib import redirect_stdout
from types import SimpleNamespace
import sqlite3
import tempfile
import unittest
import uuid
import zlib
from pathlib import Path
from unittest.mock import patch
import capture_health
import fleet
import remote_capture as remote
import recover_capture_health as recovery


class FleetTest(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name); self.home=self.root/'.codex'; self.home.mkdir()
        self.native=str(uuid.uuid4()); self.path=self.home/'sessions'/'session.jsonl'; self.path.parent.mkdir()
        self.index=self.home/'state_5.sqlite'; self.database=self.root/'data'/'replies.sqlite'
        with sqlite3.connect(self.index) as c:
            c.execute('CREATE TABLE threads(id TEXT,rollout_path TEXT,title TEXT,updated_at INTEGER)')
            c.execute('INSERT INTO threads VALUES(?,?,?,?)',(self.native,str(self.path),'测试对话',1))
        self.path.write_text(json.dumps({'type':'session_meta','payload':{'id':self.native}})+'\n')
        self.config={'codex_home':str(self.home),'codex':'unused','verified_version':None}
        remote.initialize(self.database)

    def append(self,text='人类原文',reply='**最终回复**'):
        with self.path.open() as stream:line_count=sum(1 for _ in stream)
        turn=str(uuid.uuid4()); stamp=1700000000+line_count
        user={'type':'response_item','payload':{'type':'message','role':'user','id':str(uuid.uuid4()),
              'content':[{'type':'input_text','text':text}],
              'internal_chat_message_metadata_passthrough':{'turn_id':turn,'content_item_kinds':['user.text'],'create_time':stamp}}}
        commentary={'type':'response_item','payload':{'type':'message','role':'assistant','content':[{'text':'过程不应同步'}]}}
        complete={'type':'event_msg','timestamp':f'2023-11-14T22:{13+(stamp-1700000000)//60:02d}:{20+(stamp-1700000000)%60:02d}Z',
                  'payload':{'type':'task_complete','turn_id':turn,'last_agent_message':reply}}
        from datetime import datetime,timezone
        complete['timestamp']=datetime.fromtimestamp(stamp,timezone.utc).isoformat()
        with self.path.open('a') as f:
            for item in (user,commentary,complete): f.write(json.dumps(item)+'\n')
        return turn

    def test_latest_seed_then_new_turns_idempotent_and_human_order(self):
        old=self.append(); latest=self.append('最新用户'); remote.poll(self.config,self.database)
        with sqlite3.connect(self.database) as c:
            self.assertEqual(c.execute('SELECT turn_id FROM captured_replies').fetchall(),[(latest,)])
        third=self.append('a; $(shell) `quoted`\n中文'); remote.poll(self.config,self.database); remote.poll(self.config,self.database)
        with sqlite3.connect(self.database) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],2)
            self.assertEqual(c.execute('SELECT text FROM captured_user_messages WHERE turn_id=?',(third,)).fetchone()[0],'a; $(shell) `quoted`\n中文')
            self.assertNotIn('过程不应同步',str(c.execute('SELECT reply FROM captured_replies').fetchall()))

    def test_path_and_identity_are_checked(self):
        self.append()
        with self.assertRaises(ValueError): remote.completed_events(self.path,str(uuid.uuid4()),self.home)
        outside=self.root/'other.jsonl'; outside.write_bytes(self.path.read_bytes())
        with self.assertRaises(ValueError): remote.completed_events(outside,self.native,self.home)

    def test_queue_readonly_and_scope(self):
        with self.assertRaisesRegex(ValueError,'queue_version_unverified'):
            remote.dispatch(self.config,{'thread':self.native,'text':'hello','expected_version':None})
        cfg=dict(self.config,verified_version='fixture')
        with patch.object(remote,'version',return_value='fixture'),patch.object(remote,'ROOT',self.root):
            with self.assertRaisesRegex(ValueError,'not_captured'):
                remote.dispatch(cfg,{'thread':self.native,'text':'hello','expected_version':'fixture'})

    def test_version_gate_prevents_queue_process(self):
        cfg=dict(self.config,verified_version='one')
        with patch.object(remote,'version',return_value='two'),patch.object(remote.subprocess,'run') as run:
            with self.assertRaises(ValueError): remote.dispatch(cfg,{'thread':self.native,'text':'hello','expected_version':'one'})
            run.assert_not_called()

    def test_failed_thread_is_quarantined_without_blocking_healthy_thread(self):
        self.append();remote.poll(self.config,self.database)
        bad=str(uuid.uuid4())
        capture_health.update_health(self.database,bad,'turn','capture_not_confirmed')
        cfg=dict(self.config,verified_version='fixture')
        with patch.object(remote,'ROOT',self.root),patch.object(remote,'version',return_value='fixture'),patch.object(remote,'codex_output',return_value='accepted') as queue:
            with self.assertRaisesRegex(ValueError,'target_capture_unconfirmed'):
                remote.dispatch(cfg,{'thread':bad,'text':'hello','expected_version':'fixture'})
            queue.assert_not_called()
            remote.dispatch(cfg,{'thread':self.native,'text':'--config=evil','expected_version':'fixture'})
            self.assertEqual(queue.call_args.args[1][-1],'--message=--config=evil')
        destination=self.root/'queue.sqlite'
        fleet.queue_snapshot(self.database,destination,[self.native])
        with sqlite3.connect(destination) as c:
            for table in ('captured_replies','captured_user_messages','captured_turn_order','captured_request_ids'):
                self.assertEqual(c.execute('SELECT count(*) FROM '+table).fetchone()[0],0)
        with sqlite3.connect(self.database) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],1)

    def test_quarantine_keeps_other_threads_and_ledger(self):
        hub=self.root/'hub.sqlite'
        with sqlite3.connect(hub) as c:
            c.executescript("CREATE TABLE capture_targets(host,native,queue_enabled,last_seen);CREATE TABLE threads(host,native,can_send);CREATE TABLE commands(id,status);INSERT INTO capture_targets VALUES('host','bad',1,10),('host','good',1,10);INSERT INTO threads VALUES('host','bad',1),('host','good',1);INSERT INTO commands VALUES('sent','unknown');")
        fleet.quarantine({'hub_db':str(hub)},{'host_id':'host'},['bad'])
        with sqlite3.connect(hub) as c:
            self.assertEqual(c.execute('SELECT native,can_send FROM threads ORDER BY native').fetchall(),[('bad',0),('good',1)])
            self.assertEqual(c.execute('SELECT status FROM commands').fetchone()[0],'unknown')

    def test_proxy_message_flags_are_data(self):
        config=self.root/'fleet.json'
        with patch.object(sys,'argv',['fleet.py','--config',str(config),'proxy','windows','--','queue','--thread',self.native,'--message','--config=evil']),patch.object(fleet,'load',return_value={}) as load,patch.object(fleet,'proxy') as proxy:
            fleet.main()
        load.assert_called_once_with(str(config))
        self.assertEqual(proxy.call_args.args[-1][-1],'--config=evil')

    def test_snapshot_changed_only_when_content_changes(self):
        self.append()
        with patch.object(remote,'ROOT',self.root),patch.object(remote,'version',return_value='fixture'):
            first=remote.snapshot(self.config,{})
            second=remote.snapshot(self.config,{'revision':first['revision']})
            self.assertTrue(first['changed']);self.assertFalse(second['changed']);self.assertNotIn('database',second)
            self.assertTrue(zlib.decompress(base64.b64decode(first['database'])).startswith(b'SQLite format 3'))

    def test_snapshot_validate_before_replace_and_prefix(self):
        self.append();remote.poll(self.config,self.database)
        backup=self.root/'backup.sqlite'
        with sqlite3.connect(self.database) as source,sqlite3.connect(backup) as target:source.backup(target)
        destination=self.root/'phone.sqlite'; destination.write_bytes(b'previous')
        with self.assertRaises(Exception):fleet.validate_snapshot(base64.b64encode(zlib.compress(b'invalid')).decode(),destination,'host')
        self.assertEqual(destination.read_bytes(),b'previous')
        fleet.validate_snapshot(base64.b64encode(zlib.compress(backup.read_bytes())).decode(),destination,'host')
        with sqlite3.connect(destination) as c:self.assertEqual(c.execute('SELECT title FROM captured_replies').fetchone()[0],'host · 测试对话')

    def test_snapshot_expansion_limit(self):
        data=base64.b64encode(zlib.compress(b'a'*(32*1024*1024+1))).decode()
        with self.assertRaises(ValueError):fleet.validate_snapshot(data,self.root/'large.sqlite','host')

    def test_offline_keeps_immutable_ledger(self):
        hub=self.root/'hub.sqlite'
        with sqlite3.connect(hub) as c:
            c.executescript("CREATE TABLE devices(id,last_seen);CREATE TABLE capture_targets(host,queue_enabled,last_seen);CREATE TABLE threads(host,can_send);CREATE TABLE commands(id,status);INSERT INTO devices VALUES('host',10);INSERT INTO capture_targets VALUES('host',1,10);INSERT INTO threads VALUES('host',1);INSERT INTO commands VALUES('sent','unknown');")
        fleet.offline({'hub_db':str(hub)},{'host_id':'host'})
        with sqlite3.connect(hub) as c:
            self.assertEqual(c.execute('SELECT can_send FROM threads').fetchone()[0],0)
            self.assertEqual(c.execute('SELECT status FROM commands').fetchone()[0],'unknown')

    def test_windows_command_fixed_arguments_and_stdin(self):
        host={'platform':'windows','python':r'C:\Program Files\Python312\python.exe','helper':r'C:\Users\operator\.local\lib\threadbridge\fleet\remote_capture.py'}
        cmd=fleet.command(host);ps=base64.b64decode(cmd.split()[-1]).decode('utf-16le')
        self.assertIn('[Console]::In.ReadToEnd()',ps);self.assertIn(host['python'],ps)
        self.assertNotIn('--message',ps)

    def test_health_portable_offsets_and_format(self):
        capture_health.update_health(self.database,self.native,'turn','capture_not_confirmed')
        self.assertIn(self.native+':turn',capture_health.read_health(self.database)['failures'])
        capture_health.update_health(self.database,self.native,'turn',None)
        self.assertEqual(capture_health.read_health(self.database)['failures'],{})

    def test_tail_gap_keeps_watermark_and_exposes_failure(self):
        first=self.append();remote.poll(self.config,self.database)
        self.append('不能丢的中间轮次')
        with self.path.open('a') as stream:stream.write(json.dumps({'type':'tool','payload':{'log':'x'*10000}})+'\n')
        self.append('最新轮次')
        with patch.object(remote,'TAIL_BYTES',4096):remote.poll(self.config,self.database)
        cache=json.loads((self.database.parent/'poll.json').read_text())
        self.assertEqual(cache[self.native]['turn_id'],first)
        with sqlite3.connect(self.database) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],1)
        self.assertTrue(capture_health.read_health(self.database)['failures'])

    def test_watermark_survives_discovery_window_absence(self):
        first=self.append();remote.poll(self.config,self.database)
        with sqlite3.connect(self.index) as c:c.execute('DELETE FROM threads')
        remote.poll(self.config,self.database)
        self.assertEqual(json.loads((self.database.parent/'poll.json').read_text())[self.native]['turn_id'],first)
        self.append();self.append()
        with sqlite3.connect(self.index) as c:c.execute('INSERT INTO threads VALUES(?,?,?,1)',(self.native,str(self.path),'测试'))
        remote.poll(self.config,self.database)
        with sqlite3.connect(self.database) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],3)

    def test_equal_or_reversed_timestamps_do_not_lose_turns(self):
        self.append();remote.poll(self.config,self.database)
        second=self.append();third=self.append()
        lines=[json.loads(line) for line in self.path.read_text().splitlines()]
        for line in lines:
            if line.get('payload',{}).get('type')=='task_complete' and line['payload']['turn_id'] in (second,third):
                line['timestamp']='2023-11-14T22:13:20Z'
        self.path.write_text(''.join(json.dumps(line)+'\n' for line in lines))
        remote.poll(self.config,self.database);remote.poll(self.config,self.database)
        with sqlite3.connect(self.database) as c:self.assertEqual(c.execute('SELECT count(*) FROM captured_replies').fetchone()[0],3)

    def test_impossible_capture_budget_is_visible_and_cools_down(self):
        self.append()
        size=self.path.stat().st_size
        with patch.object(remote,'TOTAL_SCAN',size+100),patch.object(remote,'TAIL_BYTES',size):
            remote.poll(self.config,self.database)
            self.assertTrue(capture_health.read_health(self.database)['failures'])
            with patch.object(remote,'completed_events') as scan:
                remote.poll(self.config,self.database);scan.assert_not_called()

    def test_unicode_legal_body_fits_rpc_limit(self):
        request={'op':'version','text':'é'*16000};encoded=json.dumps(request).encode('ascii')
        self.assertGreater(len(encoded),65536);self.assertLess(len(encoded),remote.REQUEST_BYTES)
        (self.root/'remote.json').write_text(json.dumps(self.config))
        out=io.StringIO()
        with patch.object(remote,'ROOT',self.root),patch.object(remote,'version',return_value='fixture'),patch.object(sys,'stdin',SimpleNamespace(buffer=io.BytesIO(encoded))),redirect_stdout(out):
            self.assertEqual(remote.main(),0)
        self.assertEqual(json.loads(out.getvalue())['stdout'],'fixture\n')

    def test_stalled_ssh_input_obeys_deadline(self):
        real=subprocess.Popen
        def blocked(*args,**kwargs):return real([sys.executable,'-c','import time;time.sleep(10)'],**kwargs)
        start=time.monotonic()
        with patch.object(fleet.subprocess,'Popen',side_effect=blocked):
            with self.assertRaises(subprocess.TimeoutExpired):fleet.ssh({'ssh':'fixture'},'unused',b'x'*1000000,timeout=.15)
        self.assertLess(time.monotonic()-start,2)

    def test_codex_output_is_bounded_during_execution(self):
        code="import os,time;os.write(1,b'x'*10000);time.sleep(10)"
        with self.assertRaises(ValueError):remote.codex_output({'codex':sys.executable},['-c',code],2,512)

    def test_overflow_recovery_preserves_failures_and_proves_healthy_source(self):
        self.append();remote.poll(self.config,self.database)
        bad=str(uuid.uuid4())
        for n in range(129):capture_health.update_health(self.database,bad,str(n),'capture_not_confirmed')
        self.assertTrue(capture_health.read_health(self.database)['overflow'])
        directory=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        original=(directory/'original.health').read_bytes()
        self.assertEqual(recovery.batch(self.config,self.database,directory)['covered'],2)
        result=recovery.finalize(self.database,directory)
        self.assertEqual(result['healthy'],1);self.assertEqual(result['blocked'],1)
        state=capture_health.read_health(self.database)
        self.assertFalse(state['overflow']);self.assertIn(bad+':recovery',state['failures'])
        self.assertEqual((directory/'original.health').read_bytes(),original)
        capture_health.update_health(self.database,bad,'poll',None)
        self.assertIn(bad+':recovery',capture_health.read_health(self.database)['failures'])

    def test_recovery_refuses_changed_capture_and_incomplete_coverage(self):
        self.append();remote.poll(self.config,self.database)
        directory=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        gc.collect()
        with self.assertRaisesRegex(ValueError,'incomplete'):recovery.finalize(self.database,directory)
        recovery.batch(self.config,self.database,directory)
        capture_health.update_health(self.database,self.native,'new','capture_not_confirmed')
        with self.assertRaisesRegex(ValueError,'capture_changed'):recovery.finalize(self.database,directory)

    def test_recovery_blocks_missing_human_and_uncaptured_completion_chain(self):
        first=self.append();remote.poll(self.config,self.database)
        directory=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        self.append('新轮次尚未捕获')
        recovery.batch(self.config,self.database,directory)
        result=recovery.finalize(self.database,directory)
        self.assertEqual(result['healthy'],0)
        self.assertIn(self.native+':recovery',capture_health.read_health(self.database)['failures'])

    def test_recovery_budget_exhaustion_keeps_source_quarantined(self):
        self.append();remote.poll(self.config,self.database)
        directory=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        with patch.object(remote,'TOTAL_SCAN',1):recovery.batch(self.config,self.database,directory)
        result=recovery.finalize(self.database,directory)
        self.assertEqual(result['healthy'],0)

    def test_recovery_finalization_rechecks_sources_and_index(self):
        self.append();remote.poll(self.config,self.database)
        directory=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        recovery.batch(self.config,self.database,directory)
        self.append('源核验后新增轮次')
        with self.assertRaisesRegex(ValueError,'source_changed'):recovery.finalize(self.database,directory)
        other=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        recovery.batch(self.config,self.database,other)
        with sqlite3.connect(self.index) as db:db.execute('INSERT INTO threads VALUES(?,?,?,99)',(str(uuid.uuid4()),str(self.path),'new'))
        with self.assertRaisesRegex(ValueError,'index_changed'):recovery.finalize(self.database,other)

    def test_recovery_keeps_partial_turn_records_quarantined(self):
        self.append();remote.poll(self.config,self.database)
        with sqlite3.connect(self.database) as db:
            db.execute('INSERT INTO captured_turn_order VALUES(?,?,?)',(self.native,str(uuid.uuid4()),1))
        directory=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        recovery.batch(self.config,self.database,directory)
        result=recovery.finalize(self.database,directory)
        self.assertEqual(result['healthy'],0)

    def test_recovery_keeps_missing_human_quarantined(self):
        self.append();remote.poll(self.config,self.database)
        with sqlite3.connect(self.database) as db:db.execute('DELETE FROM captured_user_messages')
        directory=recovery.prepare(self.config,self.database,str(uuid.uuid4()))
        recovery.batch(self.config,self.database,directory)
        result=recovery.finalize(self.database,directory)
        self.assertEqual(result['healthy'],0)
        before=capture_health.read_health(self.database)
        with patch.object(remote,'completed_events') as scan:
            remote.poll(self.config,self.database);scan.assert_not_called()
        self.assertEqual(capture_health.read_health(self.database),before)


if __name__=='__main__':unittest.main()
