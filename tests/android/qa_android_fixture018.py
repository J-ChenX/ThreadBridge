"""Local synthetic phone UI data and fake receipts. Never invokes Codex."""
from contextlib import closing
import hashlib,json,os,sqlite3,subprocess,time,uuid
from pathlib import Path
root=Path(__file__).resolve().parents[2];directory=root/'local/ui-test-018';directory.mkdir(parents=True,exist_ok=True)
db=directory/'hub.sqlite';binary=root/'target/release/threadbridge';port=8798
fresh=not db.exists()
env={'PATH':'/usr/bin:/bin','HOME':str(directory)}
log=(directory/'hub.log').open('w')
server=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=log,stderr=log)
def digest(s):return hashlib.sha256(s.encode()).hexdigest()
try:
 end=time.monotonic()+5
 while not db.exists():
  if time.monotonic()>end:raise RuntimeError('fixture failed to start')
  time.sleep(.05)
 time.sleep(.1)
 host='ui-fixture-host';now=int(time.time());threads={};hosts=[('ui-fixture-host','echova'),('fixture-lerrem','lerrem'),('fixture-nix','nix'),('fixture-windows','Windows')]
 if fresh:
  with closing(sqlite3.connect(db)) as c, c:
   for hid,name in hosts:
    c.execute("INSERT INTO devices(id,role,name,expires,last_seen) VALUES(?,'agent',?,?,?)",(hid,name,now+86400,now))
    c.execute('INSERT INTO capture_health VALUES(?,?,?)',(hid,json.dumps({'failures':{}}),now))
   c.execute("UPDATE settings SET v=? WHERE k='collection_generation'",(str(uuid.uuid4()),))
   for native,title,count in [('conversation','优化手机端显示',2),('markdown','Markdown 排版',2),('history','长对话与历史',84),('lerrem','整理今天的想法',2),('nix','检查服务运行',2),('windows','继续我的项目',2)]:
    owner={'lerrem':'fixture-lerrem','nix':'fixture-nix','windows':'fixture-windows'}.get(native,host)
    tid=digest(owner+'\0default\0'+native);threads[native]=tid
    c.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,owner,native,title,'completed','turn-'+str(count),now,1))
    for index in range(count):
     role='user' if index%2==0 else 'assistant'
     if native=='conversation':text='让手机端更清爽一些。' if role=='user' else '已调整为 **更清爽的对话界面**。\n\n用户消息靠右，回复直接阅读。\n\n- 顶部只保留对话标题\n- 操作移入菜单\n- 草稿与配对保持不变\n\n你可以继续发送消息。'
     elif native=='markdown':text='检查一下正文排版。' if role=='user' else '**格式正常显示**，不再看到多余的标记。\n\n1. 列表按正文排版\n2. 链接直接显示标题\n\n```kotlin\nval message = "继续对话"\n```\n\n[项目说明](https://example.com)'
     else:text=('历史问题 ' if role=='user' else '历史回复 ')+str(index+1)
     c.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(tid,'fixture-'+str(index),'turn-'+str(index+1),role,text,digest(text),(now-200+index)*1000))
    c.execute("INSERT INTO events(kind,thread,created) VALUES('thread',?,?)",(tid,now))
  pairing=json.loads(subprocess.check_output([binary,'pair','--db',db,'--url',f'http://10.0.2.2:{port}'],env=env,text=True))
  (directory/'pairing.json').write_text(json.dumps(pairing));os.chmod(directory/'pairing.json',0o600)
  (directory/'threads.json').write_text(json.dumps(threads))
 print('synthetic UI fixture ready on loopback; pairing saved privately',flush=True)
 while True:
  with closing(sqlite3.connect(db,timeout=3)) as c, c:
   now=int(time.time());[c.execute("INSERT OR REPLACE INTO creation_hosts VALUES(?,?)",(hid,now)) for hid,_ in hosts];c.execute("UPDATE devices SET last_seen=?,expires=? WHERE role='agent'",(now,now+86400));[c.execute('INSERT OR REPLACE INTO capture_health VALUES(?,?,?)',(hid,json.dumps({'failures':{}}),now)) for hid,_ in hosts]
   for command,thread,payload in c.execute("SELECT id,thread,payload FROM commands WHERE status='accepted'").fetchall():
    value=json.loads(payload)
    if value.get('kind')=='create':
     native=str(uuid.uuid4());owner=c.execute('SELECT host FROM commands WHERE id=?',(command,)).fetchone()[0];thread=digest(owner+'\0default\0'+native)
     c.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(thread,owner,native,'新对话 · '+value['text'][:32],'completed','fixture-new',now,1));c.execute('INSERT INTO thread_projects VALUES(?,?)',(thread,value.get('cursor') or ''));c.execute('INSERT INTO creation_results VALUES(?,?)',(command,thread));c.execute('UPDATE commands SET thread=? WHERE id=?',(thread,command))
    text=value['text'];turn=str(uuid.uuid4());reply='已收到 **'+text+'**。\n\n这是模拟器中的测试回复。'
    for role,body,offset in [('user',text,0),('assistant',reply,1)]:c.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(thread,role+':'+command,turn,role,body,digest(body),now*1000+offset))
    c.execute("UPDATE commands SET status='codex_accepted',native_turn=? WHERE id=?",(turn,command));c.execute("UPDATE threads SET revision=?,updated=? WHERE id=?",(turn,now,thread))
    for kind in ['thread','command']:c.execute('INSERT INTO events(kind,thread,created) VALUES(?,?,?)',(kind,thread,now))
  time.sleep(.2)
finally:server.terminate();server.wait(timeout=3);log.close()
