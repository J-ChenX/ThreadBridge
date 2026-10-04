"""Read only user.text items belonging to the explicitly completed thread/turn.

The notify input-messages array can contain previous turns and system context;
never treat it as the current human message. Opt-in through --user-turn-index.
"""
import hashlib,json,re,sqlite3,uuid
from datetime import datetime
from pathlib import Path

MAX_LINE=4*1024*1024
MAX_SCAN=256*1024*1024
MAX_INPUT=256*1024
MARKER=re.compile(r'\A\[ThreadBridge request:([0-9a-f-]{36})\]\n')

def read_turn(index,native,turn):
    uuid.UUID(native);uuid.UUID(turn)
    with sqlite3.connect('file:'+str(Path(index).resolve())+'?mode=ro',uri=True,timeout=3) as c:
        row=c.execute('SELECT rollout_path FROM threads WHERE id=?',(native,)).fetchone()
    if not row:raise ValueError('user_turn_source_missing')
    result=[];used=0;identity=False;complete=False
    with Path(row[0]).open('rb') as stream:
        while True:
            line=stream.readline(MAX_LINE+1)
            if not line:break
            used+=len(line)
            if len(line)>MAX_LINE or used>MAX_SCAN:raise ValueError('user_turn_source_limit')
            record=json.loads(line);p=record.get('payload',{})
            if record.get('type')=='session_meta':
                identity=p.get('id')==native
                if not identity:raise ValueError('user_turn_identity_mismatch')
            if not identity:continue
            if record.get('type')=='event_msg' and p.get('type')=='task_complete' and p.get('turn_id')==turn:
                complete=True;completed_at=int(datetime.fromisoformat(record['timestamp'].replace('Z','+00:00')).timestamp()*1000);break
            if record.get('type')!='response_item' or p.get('type')!='message' or p.get('role')!='user':continue
            meta=p.get('internal_chat_message_metadata_passthrough',{})
            if meta.get('turn_id')!=turn or meta.get('content_item_kinds')!=['user.text']:continue
            content=p.get('content',[])
            if not content or any(x.get('type')!='input_text' for x in content):continue
            raw=''.join(x['text'] for x in content)
            if not raw or len(raw.encode())>MAX_INPUT:raise ValueError('user_turn_input_limit')
            message=p.get('id');created=meta.get('create_time')
            if not isinstance(message,str) or len(message)>128 or not isinstance(created,(float,int)):raise ValueError('user_turn_metadata_invalid')
            result.append((message,MARKER.sub('',raw,count=1),int(created*1000),hashlib.sha256(raw.encode()).hexdigest()))
            if len(result)>100:raise ValueError('user_turn_input_limit')
    if not complete:raise ValueError('user_turn_not_complete')
    return result,completed_at

def save_turn(database,native,turn,rows,completed_at):
    with sqlite3.connect(database,timeout=3) as c:
        c.execute('PRAGMA synchronous=FULL')
        c.execute('CREATE TABLE IF NOT EXISTS captured_user_messages(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,input_digest TEXT NOT NULL,PRIMARY KEY(thread_id,message_id))')
        c.execute('CREATE TABLE IF NOT EXISTS captured_turn_order(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,completed_at INTEGER NOT NULL,PRIMARY KEY(thread_id,turn_id))')
        old=c.execute('SELECT completed_at FROM captured_turn_order WHERE thread_id=? AND turn_id=?',(native,turn)).fetchone()
        if old and old!=(completed_at,):raise ValueError('conflicting_turn_order')
        c.execute('INSERT OR IGNORE INTO captured_turn_order VALUES(?,?,?)',(native,turn,completed_at))
        for message,text,created,digest in rows:
            old=c.execute('SELECT turn_id,text,created_at,input_digest FROM captured_user_messages WHERE thread_id=? AND message_id=?',(native,message)).fetchone()
            if old and old!=(turn,text,created,digest):raise ValueError('conflicting_user_input')
            c.execute('INSERT OR IGNORE INTO captured_user_messages VALUES(?,?,?,?,?,?)',(native,turn,message,text,created,digest))

def capture_users(database,index,event):
    native=event['thread-id'];turn=event['turn-id']
    rows,completed_at=read_turn(index,native,turn)
    save_turn(database,native,turn,rows,completed_at)
