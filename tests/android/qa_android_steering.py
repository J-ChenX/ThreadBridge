"""Explicit guidance in a real debug APK, isolated Hub/bridge and fixed mock CLI."""
from contextlib import closing
from pathlib import Path
import hashlib,json,os,socket,sqlite3,subprocess,tempfile,time,uuid,sys,xml.etree.ElementTree as ET
from qa_android_ui015 import adb,ui,texts,node,tap,wait,shot,ROOT
from qa_android_settings021 import enter
from qa_android_keyentry021 import lan_toggle
sys.path.insert(0,str(ROOT/'tests/integration'))
from test_conversation_sync import save_turn
PACKAGE='dev.threadbridge.debug'

def main():
 assert adb('shell','getprop','ro.kernel.qemu').strip()=='1','Dedicated emulator required'
 if 'package:'+PACKAGE in adb('shell','pm','list','packages',PACKAGE):
  # Only discard the preceding synthetic debug fixture, never the release app.
  xml=adb('shell','run-as',PACKAGE,'cat','shared_prefs/connection.xml')
  assert next(e.text for e in ET.fromstring(xml) if e.get('name')=='server').startswith('http://127.0.0.1:')
  adb('shell','pm','clear',PACKAGE)
 with tempfile.TemporaryDirectory(prefix='tb-steer-ui-') as directory:
  root=Path(directory);db=root/'hub.sqlite';source=root/'capture.sqlite';binary=ROOT/'target/release/threadbridge';env=dict(os.environ,HOME=directory,CODEX_HOME=directory,PATH=str(Path(sys.executable).parent)+os.pathsep+'/usr/bin:/bin')
  native,turn,prior,queue=[str(uuid.uuid4()) for _ in range(4)];mock=root/'codex';mock.symlink_to(ROOT/'tests/fixtures/mock_codex.py');root.joinpath('fixture.json').write_text(json.dumps({'mode':'steer','native_id':native,'turn_id':turn,'queue_id':queue}))
  def run(*args):return subprocess.check_output([binary,*map(str,args)],env=env,text=True)
  run('register-agent','--db',db,'--name','lerrem','--output',root/'agent.json');host=json.loads(root.joinpath('agent.json').read_text())['host_id'];key='synthetic-'+uuid.uuid4().hex
  run('capture','--database',source,'--thread',native,'--title','手机引导测试',json.dumps({'type':'agent-turn-complete','thread-id':native,'turn-id':prior,'last-assistant-message':'prior final'}))
  with closing(sqlite3.connect(source)) as c,c:
   c.execute('CREATE TABLE IF NOT EXISTS captured_task_state(thread_id TEXT PRIMARY KEY,turn_id TEXT NOT NULL,status TEXT NOT NULL)');c.execute("INSERT OR REPLACE INTO captured_task_state VALUES(?,?,'running')",(native,turn))
  with closing(sqlite3.connect(db)) as c,c:c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','QA',?)",(hashlib.sha256(key.encode()).hexdigest(),int(time.time())+3600))
  with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
  hub=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  bridge=subprocess.Popen([binary,'capture-bridge','--db',db,'--capture-db',source,'--host',host,'--codex',mock,'--allow-queue','--verified-version','codex-cli fixture'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   adb('install','-r',ROOT/'android/app/build/outputs/apk/debug/app-debug.apk');adb('reverse',f'tcp:{port}',f'tcp:{port}');adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity')
   n=wait(lambda n:'连接你的电脑' in texts(n));tap(node(n,text='连接密钥'));wait(lambda n:'输入地址与连接密钥，让对话继续。' in texts(n));enter(0,f'http://127.0.0.1:{port}');enter(1,key);n=ui()
   if '局域网测试' not in texts(n):adb('shell','input','swipe',600,1700,600,700,350);n=ui()
   tap(lan_toggle(n));wait(lambda n:lan_toggle(n).attrib.get('checked')=='true');tap(node(ui(),text='连接'));n=wait(lambda n:'手机引导测试' in texts(n),seconds=45);tap(node(n,text='手机引导测试'));n=wait(lambda n:'prior final' in texts(n))
   field=next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText');tap(field);adb('shell','input','text','explicit%sphone%sguidance');tap(node(ui(),desc='发送'))
   n=wait(lambda n:'作为引导发送' in texts(n),seconds=45);shot('steering-queued-light.png')
   assert root.joinpath('calls').read_text()=='queue\n'
   adb('shell','wm','size','1080x1920');adb('shell','wm','density','440');adb('shell','cmd','uimode','night','yes');adb('shell','settings','put','system','font_scale','1.5')
   n=wait(lambda n:'作为引导发送' in texts(n) or '手机引导测试' in texts(n))
   if '作为引导发送' not in texts(n):tap(node(n,text='手机引导测试'))
   n=wait(lambda n:'作为引导发送' in texts(n));shot('steering-queued-small-dark-large-font.png')
   tap(node(n,text='作为引导发送'));n=wait(lambda n:'引导已提交到当前轮次' in texts(n),seconds=45);shot('steering-accepted.png')
   assert '作为引导发送' not in texts(n);assert root.joinpath('calls').read_text()=='queue\ndelete\nsteer\n'
   field=next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText');tap(field);adb('shell','input','text','next%sdraft');n=wait(lambda n:node(n,desc='发送').attrib.get('enabled')=='true');shot('steering-composer-unblocked.png');adb('shell','input','keyevent','4')
   with closing(sqlite3.connect(db)) as c:assert c.execute('SELECT status FROM commands').fetchall()==[('steer_accepted',)]
   stamp=int(time.time()*1000);save_turn(source,native,turn,[('steered-human','explicit phone guidance',stamp,hashlib.sha256(b'explicit phone guidance').hexdigest())],stamp+1)
   run('capture','--database',source,'--thread',native,'--title','手机引导测试',json.dumps({'type':'agent-turn-complete','thread-id':native,'turn-id':turn,'last-assistant-message':'steered final reply'}))
   with closing(sqlite3.connect(source)) as c,c:c.execute("UPDATE captured_task_state SET status='completed' WHERE thread_id=?",(native,))
   n=wait(lambda n:'steered final reply' in texts(n),seconds=45);assert any(x.attrib.get('text')=='next draft' for x in n.iter('node'));shot('steering-final-reply.png')
   adb('shell','am','force-stop',PACKAGE);adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity');n=wait(lambda n:'手机引导测试' in texts(n));tap(node(n,text='手机引导测试'));n=wait(lambda n:'steered final reply' in texts(n));assert root.joinpath('calls').read_text()=='queue\ndelete\nsteer\n'
   with closing(sqlite3.connect(db)) as c:assert c.execute('SELECT status FROM commands').fetchall()==[('codex_accepted',)]
   report={'passed':True,'real_phone':False,'real_codex_send':False,'checks':['explicit mobile steer button for queued message','one queue removal and one same-turn steer','accepted guidance unlocks composer before final','acknowledgement distinct from final capture','draft preserved','restart no resend','small dark large font']}
   (ROOT/'artifacts/steering-ui.verification.json').write_text(json.dumps(report,indent=2)+'\n');print('PASS real Android queued message -> explicit guide button -> same active turn accepted -> composer unlocked -> final capture; restart no resend',flush=True)
  finally:
   bridge.terminate();bridge.wait(timeout=5);hub.terminate();hub.wait(timeout=5);adb('shell','wm','size','reset');adb('shell','wm','density','reset');adb('shell','cmd','uimode','night','no');adb('shell','settings','put','system','font_scale','1.0')
if __name__=='__main__':main()
