#!/usr/bin/python3
"""Dormant one-shot original-ID App Server dispatcher. No real use before approval.

A runtime owner grant must be provisioned separately after stopping competing
workers. No granting, discovery, fork, historical body reads or auto-retry here.
"""
from contextlib import closing
import argparse,fcntl,hashlib,json,os,selectors,sqlite3,subprocess,time,uuid
from pathlib import Path
FRAME=1024*1024
UNRESOLVED=('accepted','dispatching','upstream_queued','unknown')
class Stop(Exception):pass

def key(host,native):return hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest()
def grant_hash(grant):return hashlib.sha256(json.dumps(grant,sort_keys=True,separators=(',',':')).encode()).hexdigest()
def validate_grant(grant):
    required={'native_id','host_id','cwd','verified_cli_version','expires_at','tool_compatibility_confirmed','handoff_confirmed','approval_policy'}
    if not required.issubset(grant) or set(grant)-required-{'sandbox_mode'}:raise Stop('invalid_grant')
    if grant.get('sandbox_mode','read-only') not in ('read-only','workspace-write'):raise Stop('unsupported_sandbox_mode')
    uuid.UUID(grant['native_id']);uuid.UUID(grant['host_id'])
    if not Path(grant['cwd']).is_absolute() or not Path(grant['cwd']).is_dir():raise Stop('invalid_working_directory')
    if grant['approval_policy']!='on-request':raise Stop('unsupported_approval_policy')
    if grant['expires_at']<=int(time.time()):raise Stop('grant_expired')
    if not grant['tool_compatibility_confirmed']:raise Stop('tool_compatibility_not_confirmed')
    if not grant['handoff_confirmed']:raise Stop('execution_handoff_not_confirmed')
    if not isinstance(grant['verified_cli_version'],str):raise Stop('invalid_cli_version')

def connect(db):
    c=sqlite3.connect(db,timeout=3);c.execute('PRAGMA synchronous=FULL')
    c.execute('CREATE TABLE IF NOT EXISTS resume_dispatch_ledger(id TEXT PRIMARY KEY,host TEXT NOT NULL,native TEXT NOT NULL,status TEXT NOT NULL,turn TEXT,error TEXT,created INTEGER NOT NULL)')
    c.commit();return c

def receipt(c,command,status,error=None,turn=None):
    c.execute('BEGIN IMMEDIATE')
    current=c.execute('SELECT status,native_turn FROM commands WHERE id=?',(command,)).fetchone()
    if current and current[0]=='codex_accepted' and status!='codex_accepted':
        c.rollback();return
    c.execute('UPDATE commands SET status=?,error=?,native_turn=coalesce(?,native_turn) WHERE id=?',(status,error,turn,command))
    c.execute('UPDATE resume_dispatch_ledger SET status=?,error=?,turn=coalesce(?,turn) WHERE id=?',(status,error,turn,command))
    c.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,? FROM commands WHERE id=?",(int(time.time()),command))
    c.commit()

class Protocol:
    def __init__(self,child):
        self.child=child;self.selector=selectors.DefaultSelector();self.selector.register(child.stdout,selectors.EVENT_READ);self.buffer=b'';self.ident=0;self.completed=[];self.finals=[];self.thread=None;self.turn=None;self.bytes=0
    def send(self,value):
        encoded=(json.dumps(value,separators=(',',':'))+'\n').encode()
        if len(encoded)>FRAME:raise Stop('request_limit')
        self.child.stdin.write(encoded);self.child.stdin.flush()
    def receive(self,deadline):
        while b'\n' not in self.buffer:
            if time.monotonic()>=deadline or not self.selector.select(max(0,deadline-time.monotonic())):raise Stop('upstream_timeout')
            chunk=os.read(self.child.stdout.fileno(),65536)
            if not chunk:raise Stop('upstream_disconnected')
            self.buffer+=chunk;self.bytes+=len(chunk)
            if len(self.buffer)>FRAME or self.bytes>8*FRAME:raise Stop('upstream_output_limit')
        line,self.buffer=self.buffer.split(b'\n',1)
        return json.loads(line)
    def handle(self,row):
        method=row.get('method');params=row.get('params',{})
        if 'id' in row and method:
            # Never approve filesystem/network/tool permissions or answer user input.
            if method=='item/tool/call':reason='desktop_dynamic_tool_unavailable'
            elif 'requestApproval' in method or method in ('tool/requestUserInput','mcpServer/elicitation/request'):reason='user_action_required'
            else:reason='unsupported_server_request'
            raise Stop(reason)
        if method=='item/completed' and params.get('item',{}).get('type')=='agentMessage' and params['item'].get('phase')=='final_answer':
            if params.get('threadId')!=self.thread:raise Stop('completion_thread_mismatch')
            text=params['item'].get('text')
            if isinstance(text,str) and text:self.finals.append((params.get('turnId'),text))
        if method=='turn/completed':
            if params.get('threadId')!=self.thread:raise Stop('completion_thread_mismatch')
            self.completed.append(params.get('turn',{}))
    def rpc(self,method,params,timeout=12):
        self.ident+=1;identity=self.ident;self.send({'id':identity,'method':method,'params':params});deadline=time.monotonic()+timeout
        while True:
            row=self.receive(deadline)
            if row.get('id')==identity and 'method' not in row:
                if 'error' in row:raise Stop('upstream_rejected')
                if 'result' not in row:raise Stop('upstream_response_invalid')
                return row['result']
            self.handle(row)
    def wait_completed(self,timeout):
        deadline=time.monotonic()+timeout
        while not self.completed:self.handle(self.receive(deadline))
        turn=self.completed.pop(0)
        if turn.get('id')!=self.turn:raise Stop('completion_turn_mismatch')
        if turn.get('status')!='completed':raise Stop('turn_not_completed')
    def close(self):self.selector.close()

def require_thread(row,native,cwd):
    t=row.get('thread',{})
    if t.get('id')!=native:raise Stop('original_id_mismatch')
    if t.get('cwd')!=cwd:raise Stop('working_directory_mismatch')
    if t.get('ephemeral') or t.get('forkedFromId'):raise Stop('unsupported_thread_identity')
    if t.get('status',{}).get('type') not in ('idle','notLoaded'):raise Stop('thread_not_idle')
    return t

def dispatch(database,capture_db,codex,grant,command_id,execute=False,turn_timeout=900,capture_timeout=10):
    validate_grant(grant)
    if not execute:raise Stop('execution_not_enabled')
    uuid.UUID(command_id);native=grant['native_id'];host=grant['host_id'];thread=key(host,native)
    c=connect(database);child=None;p=None;started=False;intent=False;authoritative_turn=None
    try:
        c.execute('BEGIN IMMEDIATE')
        old=c.execute('SELECT status FROM resume_dispatch_ledger WHERE id=?',(command_id,)).fetchone()
        if old:
            c.rollback()
            if old[0] in ('intent','dispatching'):receipt(c,command_id,'unknown','interrupted_resume_no_retry')
            elif old[0]=='preparing':receipt(c,command_id,'rejected','interrupted_preparation_no_generation')
            return c.execute('SELECT status,error,native_turn FROM commands WHERE id=?',(command_id,)).fetchone()
        valid=c.execute("SELECT count(*) FROM devices WHERE id=? AND role='agent' AND revoked=0 AND expires>?",(host,int(time.time()))).fetchone()[0]
        if not valid:raise Stop('host_not_authorized')
        # Existing queue intents/unknown/pending take precedence; never drain/replay them.
        if c.execute('SELECT count(*) FROM capture_queue_ledger q JOIN commands c ON c.id=q.id WHERE c.thread=? AND q.status<>?',(thread,'codex_accepted')).fetchone()[0]:raise Stop('legacy_queue_unresolved')
        row=c.execute('SELECT host,thread,status,payload,expires FROM commands WHERE id=?',(command_id,)).fetchone()
        if not row or row[:3]!=(host,thread,'accepted'):raise Stop('command_scope_or_state_mismatch')
        if row[4]<=int(time.time()):raise Stop('command_expired')
        command=json.loads(row[3])
        if command.get('native_id')!=native or command.get('thread_id')!=thread or command.get('id')!=command_id or command.get('kind')!='send':raise Stop('command_identity_mismatch')
        if c.execute("SELECT count(*) FROM commands WHERE thread=? AND id<>? AND status IN ('accepted','dispatching','upstream_queued','unknown')",(thread,command_id)).fetchone()[0]:raise Stop('thread_command_pending')
        revision=c.execute('SELECT revision FROM threads WHERE id=?',(thread,)).fetchone()
        if not revision or revision[0]!=command.get('expected_revision'):raise Stop('revision_conflict')
        # Separate, explicit, short-lived owner grant; this dispatcher never provisions it.
        try:owner=c.execute('SELECT grant_hash,expires FROM resume_owners WHERE thread=?',(thread,)).fetchone()
        except sqlite3.OperationalError:raise Stop('resume_owner_not_provisioned')
        if not owner or owner[0]!=grant_hash(grant) or owner[1]<=int(time.time()):raise Stop('resume_owner_not_authorized')
        c.execute('INSERT INTO resume_dispatch_ledger(id,host,native,status,created) VALUES(?,?,?,?,?)',(command_id,host,native,'preparing',int(time.time())))
        c.execute("UPDATE commands SET status='dispatching' WHERE id=?",(command_id,));c.execute("INSERT INTO events(kind,thread,created) VALUES('command',?,?)",(thread,int(time.time())));c.commit()
        version=subprocess.run([codex,'--version'],capture_output=True,timeout=5,check=True).stdout.decode().strip()
        if version!=grant['verified_cli_version']:raise Stop('cli_version_changed')
        child=subprocess.Popen([codex,'app-server'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,bufsize=0)
        p=Protocol(child);p.thread=native
        p.rpc('initialize',{'clientInfo':{'name':'threadbridge-resume-candidate','version':'0.1'},'capabilities':{'experimentalApi':True}});p.send({'method':'initialized','params':{}})
        require_thread(p.rpc('thread/read',{'threadId':native,'includeTurns':False}),native,grant['cwd'])
        response=p.rpc('thread/resume',{'threadId':native,'excludeTurns':True,'approvalPolicy':'on-request','approvalsReviewer':'user','sandbox':grant.get('sandbox_mode','read-only'),'cwd':grant['cwd']})
        require_thread(response,native,grant['cwd'])
        if response.get('approvalPolicy')!='on-request' or response.get('approvalsReviewer')!='user':raise Stop('approval_policy_mismatch')
        if response.get('thread',{}).get('canAcceptDirectInput') is not True:raise Stop('direct_input_capability_unconfirmed')
        sandbox=response.get('sandbox',{})
        expected_sandbox='workspaceWrite' if grant.get('sandbox_mode','read-only')=='workspace-write' else 'readOnly'
        if sandbox.get('type')!=expected_sandbox or sandbox.get('networkAccess',False):raise Stop('sandbox_policy_mismatch')
        if any(root!=grant['cwd'] for root in sandbox.get('writableRoots',[])):raise Stop('writable_scope_mismatch')
        page=p.rpc('thread/turns/list',{'threadId':native,'limit':1,'itemsView':'summary','sortDirection':'desc'})
        turns=page.get('data')
        if not isinstance(turns,list) or not turns or turns[0].get('id')!=command['expected_revision'] or turns[0].get('status') not in ('completed','failed','interrupted'):raise Stop('original_context_revision_mismatch')
        # Durable send intent precedes the turn/start write. Every uncertain outcome locks the command.
        validate_grant(grant)
        if command.get('expires_at',row[4])<=int(time.time()):raise Stop('command_expired')
        c.execute("UPDATE resume_dispatch_ledger SET status='intent' WHERE id=?",(command_id,));c.commit();intent=True;started=True
        text=command['text']
        reply=p.rpc('turn/start',{'threadId':native,'input':[{'type':'text','text':text}],'cwd':grant['cwd'],'approvalPolicy':'on-request','approvalsReviewer':'user','sandboxPolicy':({'type':'workspaceWrite','writableRoots':[grant['cwd']],'networkAccess':False} if grant.get('sandbox_mode','read-only')=='workspace-write' else {'type':'readOnly','networkAccess':False})})
        authoritative_turn=reply.get('turn',{}).get('id');uuid.UUID(authoritative_turn)
        c.execute("UPDATE resume_dispatch_ledger SET status='dispatching',turn=? WHERE id=?",(authoritative_turn,command_id));c.execute('UPDATE commands SET native_turn=? WHERE id=?',(authoritative_turn,command_id));c.commit();p.turn=authoritative_turn
        p.wait_completed(turn_timeout)
        # Public final-phase item + verified completed turn is an independent
        # capture source. Never promote commentary/unknown phase or old history.
        finals=[text for turn,text in p.finals if turn==authoritative_turn]
        if len(finals)==1:
            from capture_completion_v014 import capture
            names={x[1] for x in c.execute('PRAGMA table_info(threads)')}
            title=c.execute('SELECT title FROM threads WHERE id=?',(thread,)).fetchone()[0] if 'title' in names else None
            event={'type':'agent-turn-complete','thread-id':native,'turn-id':authoritative_turn,'last-assistant-message':finals[0],'input-messages':[]}
            capture(json.dumps(event),native,capture_db,title)
        deadline=time.monotonic()+capture_timeout
        while True:
            try:
                with closing(sqlite3.connect('file:'+str(Path(capture_db).resolve())+'?mode=ro',uri=True)) as source, source:
                    captured=source.execute('SELECT reply FROM captured_replies WHERE thread_id=? AND turn_id=?',(native,authoritative_turn)).fetchone()
                    marker=source.execute('SELECT request_id FROM captured_request_ids WHERE thread_id=? AND turn_id=?',(native,authoritative_turn)).fetchone()
                if captured and marker in (None,(command_id,)):break
            except sqlite3.Error:pass
            if time.monotonic()>=deadline:raise Stop('final_capture_not_confirmed')
            time.sleep(.1)
        receipt(c,command_id,'codex_accepted',None,authoritative_turn)
    except Exception as error:
        c.rollback()
        known={'command_expired','grant_expired','cli_version_changed','original_id_mismatch','working_directory_mismatch','unsupported_thread_identity','thread_not_idle','approval_policy_mismatch','direct_input_capability_unconfirmed','sandbox_policy_mismatch','writable_scope_mismatch','original_context_revision_mismatch','user_action_required','desktop_dynamic_tool_unavailable','unsupported_server_request','upstream_timeout','upstream_disconnected','upstream_rejected','upstream_response_invalid','upstream_output_limit','completion_thread_mismatch','completion_turn_mismatch','turn_not_completed','final_capture_not_confirmed'}
        reason=str(error) if isinstance(error,Stop) and str(error) in known else 'resume_dispatch_failed'
        exists=c.execute('SELECT count(*) FROM resume_dispatch_ledger WHERE id=?',(command_id,)).fetchone()[0]
        if exists:receipt(c,command_id,'unknown' if intent else 'rejected',reason,authoritative_turn)
        else:raise
    finally:
        if p:p.close()
        if child:
            if child.poll() is None:child.terminate()
            try:child.wait(timeout=3)
            except subprocess.TimeoutExpired:child.kill();child.wait(timeout=3)
            if child.stdin:child.stdin.close()
            if child.stdout:child.stdout.close()
        c.close()
    with closing(sqlite3.connect(database)) as c, c:return c.execute('SELECT status,error,native_turn FROM commands WHERE id=?',(command_id,)).fetchone()

def preflight(database,codex,grant,execute=False):
    """Explicitly approved original-ID compatibility check; never starts a turn."""
    validate_grant(grant)
    if not execute:raise Stop('execution_not_enabled')
    native=grant['native_id'];host=grant['host_id'];thread=key(host,native)
    with closing(sqlite3.connect('file:'+str(Path(database).resolve())+'?mode=ro',uri=True)) as c, c:
        if c.execute("SELECT count(*) FROM commands WHERE thread=? AND status IN ('accepted','dispatching','upstream_queued','unknown')",(thread,)).fetchone()[0]:raise Stop('unresolved_request_before_handoff')
        saved=c.execute('SELECT revision FROM threads WHERE id=?',(thread,)).fetchone()
        if not saved:raise Stop('existing_captured_target_required')
    version=subprocess.run([codex,'--version'],capture_output=True,timeout=5,check=True).stdout.decode().strip()
    if version!=grant['verified_cli_version']:raise Stop('cli_version_changed')
    child=subprocess.Popen([codex,'app-server'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,bufsize=0);p=Protocol(child);p.thread=native
    try:
        p.rpc('initialize',{'clientInfo':{'name':'threadbridge-resume-preflight','version':'0.1'},'capabilities':{'experimentalApi':True}});p.send({'method':'initialized','params':{}})
        require_thread(p.rpc('thread/read',{'threadId':native,'includeTurns':False}),native,grant['cwd'])
        result=p.rpc('thread/resume',{'threadId':native,'excludeTurns':True,'approvalPolicy':'on-request','approvalsReviewer':'user','sandbox':grant.get('sandbox_mode','read-only'),'cwd':grant['cwd']})
        require_thread(result,native,grant['cwd'])
        if result.get('approvalPolicy')!='on-request' or result.get('approvalsReviewer')!='user':raise Stop('approval_policy_mismatch')
        if result.get('thread',{}).get('canAcceptDirectInput') is not True:raise Stop('direct_input_capability_unconfirmed')
        policy=result.get('sandbox',{});expected='workspaceWrite' if grant.get('sandbox_mode','read-only')=='workspace-write' else 'readOnly'
        if policy.get('type')!=expected or policy.get('networkAccess',False):raise Stop('sandbox_policy_mismatch')
        if any(root!=grant['cwd'] for root in policy.get('writableRoots',[])):raise Stop('writable_scope_mismatch')
        page=p.rpc('thread/turns/list',{'threadId':native,'limit':1,'itemsView':'summary','sortDirection':'desc'})
        if not page.get('data') or page['data'][0].get('id')!=saved[0] or page['data'][0].get('status') not in ('completed','failed','interrupted'):raise Stop('original_context_revision_mismatch')
        return {'native_id':native,'cwd':grant['cwd'],'revision':saved[0],'no_turn_started':True,'instruction_sources':result.get('instructionSources',[])}
    finally:
        p.close()
        if child.poll() is None:child.terminate()
        try:child.wait(timeout=3)
        except subprocess.TimeoutExpired:child.kill();child.wait(timeout=3)
        child.stdin.close();child.stdout.close()

def reconcile_completed(database,capture_db,grant):
    """Settle only authoritative stored turn IDs; never infer an old queue turn."""
    native=grant['native_id'];host=grant['host_id'];c=connect(database)
    try:
        rows=c.execute("SELECT l.id,l.turn,x.created FROM resume_dispatch_ledger l JOIN commands x ON x.id=l.id WHERE l.host=? AND l.native=? AND l.turn IS NOT NULL AND x.status IN ('dispatching','unknown')",(host,native)).fetchall()
        try:
            with closing(sqlite3.connect('file:'+str(Path(capture_db).resolve())+'?mode=ro',uri=True)) as source, source:
                for command,turn,created in rows:
                    final=source.execute('SELECT captured_at FROM captured_replies WHERE thread_id=? AND turn_id=?',(native,turn)).fetchone()
                    marker=source.execute('SELECT request_id FROM captured_request_ids WHERE thread_id=? AND turn_id=?',(native,turn)).fetchone()
                    if final and final[0]>=created and marker in (None,(command,)):receipt(c,command,'codex_accepted',None,turn)
        except sqlite3.Error:pass
    finally:c.close()

def old_queue_worker_inactive():
    result=subprocess.run(['systemctl','--user','is-active','--quiet','threadbridge-phone-capture.service'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=3)
    return result.returncode==3

def worker(database,capture_db,codex,grant,execute=False,competitor_check=old_queue_worker_inactive,stop_after=None,poll_interval=2,stop_event=None):
    """Candidate polling worker; only an explicit per-ID grant can publish send lease."""
    validate_grant(grant)
    if not execute:raise Stop('execution_not_enabled')
    if not competitor_check():raise Stop('old_queue_worker_not_stopped')
    native=grant['native_id'];host=grant['host_id'];thread=key(host,native);digest=grant_hash(grant)
    lock_path=Path(str(database)+'.'+thread+'.resume.lock')
    lock_fd=os.open(lock_path,os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW,0o600)
    try:fcntl.flock(lock_fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
    except BlockingIOError:os.close(lock_fd);raise Stop('resume_owner_running')
    try:c=connect(database)
    except Exception:os.close(lock_fd);raise
    iterations=0
    try:
        reconcile_completed(database,capture_db,grant)
        c.execute('BEGIN IMMEDIATE')
        if not c.execute("SELECT count(*) FROM devices WHERE id=? AND role='agent' AND revoked=0 AND expires>?",(host,int(time.time()))).fetchone()[0]:raise Stop('host_not_authorized')
        if c.execute("SELECT count(*) FROM capture_queue_ledger q JOIN commands x ON x.id=q.id WHERE x.thread=? AND q.status<>'codex_accepted'",(thread,)).fetchone()[0]:raise Stop('legacy_queue_unresolved')
        if c.execute("SELECT count(*) FROM commands WHERE thread=? AND status IN ('dispatching','upstream_queued','unknown')",(thread,)).fetchone()[0]:raise Stop('unresolved_request_before_handoff')
        if not c.execute('SELECT count(*) FROM capture_targets WHERE thread=? AND host=? AND native=?',(thread,host,native)).fetchone()[0]:raise Stop('existing_captured_target_required')
        c.execute("CREATE TABLE IF NOT EXISTS resume_owners(thread TEXT PRIMARY KEY,grant_hash TEXT NOT NULL,expires INTEGER NOT NULL,last_seen INTEGER NOT NULL DEFAULT 0)")
        owner=c.execute('SELECT grant_hash,expires FROM resume_owners WHERE thread=?',(thread,)).fetchone()
        if owner and owner[0]!=digest and owner[1]>int(time.time()):raise Stop('another_resume_owner_active')
        c.execute('INSERT INTO resume_owners(thread,grant_hash,expires,last_seen) VALUES(?,?,?,?) ON CONFLICT(thread) DO UPDATE SET grant_hash=excluded.grant_hash,expires=excluded.expires,last_seen=excluded.last_seen',(thread,digest,grant['expires_at'],int(time.time())));c.commit()
        while True:
            if stop_event is not None and stop_event.is_set():return
            validate_grant(grant)
            reconcile_completed(database,capture_db,grant)
            if not competitor_check():raise Stop('competing_queue_worker_started')
            # Publish only this preapproved original ID, not all inbox tasks.
            with c:
                c.execute('UPDATE devices SET last_seen=? WHERE id=?',(int(time.time()),host))
                c.execute('UPDATE resume_owners SET last_seen=? WHERE thread=? AND grant_hash=?',(int(time.time()),thread,digest))
                c.execute('UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?',(int(time.time()),thread))
                c.execute("UPDATE threads SET can_send=1,status='resume_ready' WHERE id=?",(thread,))
            pending=c.execute("SELECT id FROM commands WHERE thread=? AND status='accepted' AND expires>? ORDER BY created LIMIT 1",(thread,int(time.time()))).fetchone()
            if pending:dispatch(database,capture_db,codex,grant,pending[0],execute=True,turn_timeout=min(900,max(1,grant['expires_at']-int(time.time()))))
            iterations+=1
            if stop_after is not None and iterations>=stop_after:return
            if stop_event is not None:stop_event.wait(poll_interval)
            else:time.sleep(poll_interval)
    finally:
        c.rollback()
        try:
            with c:
                owned=c.execute('SELECT grant_hash FROM resume_owners WHERE thread=?',(thread,)).fetchone()
                if owned and owned[0]==digest:
                    c.execute('UPDATE resume_owners SET last_seen=0 WHERE thread=?',(thread,))
                    c.execute('UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE thread=?',(thread,))
                    c.execute("UPDATE threads SET can_send=0,status='capture_only' WHERE id=?",(thread,))
        except sqlite3.Error:pass
        c.close();os.close(lock_fd)

def main():
    a=argparse.ArgumentParser(description=__doc__);a.add_argument('--db',required=True);a.add_argument('--capture-db',required=True);a.add_argument('--codex',required=True);a.add_argument('--grant',required=True);mode=a.add_mutually_exclusive_group(required=True);mode.add_argument('--command');mode.add_argument('--worker',action='store_true');mode.add_argument('--preflight',action='store_true');a.add_argument('--execute',action='store_true');args=a.parse_args()
    if not args.execute:a.error('candidate is disabled; real executor activation requires approval')
    try:
        grant=json.loads(Path(args.grant).read_text())
        if args.preflight:print(json.dumps(preflight(args.db,args.codex,grant,execute=True)))
        elif args.worker:worker(args.db,args.capture_db,args.codex,grant,execute=True)
        else:print(json.dumps({'receipt':dispatch(args.db,args.capture_db,args.codex,grant,args.command,True)}))
        return 0
    except Exception:print('resume dispatch refused; no input or upstream content logged',file=__import__('sys').stderr);return 2
if __name__=='__main__':raise SystemExit(main())
