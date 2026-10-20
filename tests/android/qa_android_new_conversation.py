"""Real APK + dedicated emulator + real Hub/bridge + mock Codex only."""
from contextlib import closing
import hashlib,json,os,socket,sqlite3,subprocess,tempfile,threading,time,uuid,sys
from pathlib import Path
from qa_android_ui015 import adb,ui,texts,node,tap,wait,shot,ROOT
from qa_android_settings021 import enter
from qa_android_keyentry021 import lan_toggle
sys.path.insert(0,str(ROOT/'tests/integration'))
from test_conversation_sync import save_turn
PACKAGE='dev.threadbridge.debug'

def main():
 assert adb('shell','getprop','ro.kernel.qemu').strip()=='1'
 if 'package:'+PACKAGE in adb('shell','pm','list','packages',PACKAGE):
  assert subprocess.run([str(ROOT/'local/android-sdk/platform-tools/adb'),'-s','emulator-5554','shell','run-as',PACKAGE,'test','!','-e','databases/threadbridge.sqlite'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0,'Fresh isolated debug data required'
 with tempfile.TemporaryDirectory(prefix='tb-new-conversation-ui-') as directory:
  root=Path(directory);db=root/'hub.sqlite';source=root/'capture.sqlite';project=root/'remote-project';project.mkdir();binary=ROOT/'target/release/threadbridge'
  env=dict(os.environ,HOME=directory,CODEX_HOME=directory,PATH=str(Path(sys.executable).parent)+os.pathsep+"/usr/bin:/bin")
  native='00000000-0000-4000-8000-000000000001';turn='00000000-0000-4000-8000-000000000009'
  mock=root/'codex';mock.symlink_to(ROOT/'tests/fixtures/mock_codex.py')
  (root/'fixture.json').write_text(json.dumps({'mode':'create','native_id':native,'reply':True,'cwd':str(project)}))
  def run(*args):return subprocess.check_output([binary,*map(str,args)],env=env,text=True)
  def register(name):
   f=root/(name+'.json');run('register-agent','--db',db,'--name',name,'--output',f);return json.loads(f.read_text())['host_id']
  owner=register('windows');executor=register('lerrem');key='synthetic-'+uuid.uuid4().hex
  baseline=str(uuid.uuid4())
  run('capture','--database',source,'--thread',baseline,json.dumps({'type':'agent-turn-complete','thread-id':baseline,'turn-id':str(uuid.uuid4()),'last-assistant-message':'baseline','cwd':str(root)}))
  with closing(sqlite3.connect(db)) as c,c:
   c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','QA',?)",(hashlib.sha256(key.encode()).hexdigest(),int(time.time())+3600))
   c.execute('INSERT INTO desktop_project_catalog VALUES(?,?,?,?)',(owner,executor,str(project),'远端首轮项目'))
  with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
  hub=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  bridge=subprocess.Popen([binary,'capture-bridge','--db',db,'--capture-db',source,'--host',executor,'--codex',mock,'--allow-queue','--verified-version','codex-cli fixture'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  stop=threading.Event()
  def heartbeat():
   while not stop.wait(.4):
    with closing(sqlite3.connect(db,timeout=3)) as c,c:c.execute('UPDATE devices SET last_seen=? WHERE id=?',(int(time.time()),owner))
  threading.Thread(target=heartbeat,daemon=True).start()
  def start():
   adb('shell','am','force-stop',PACKAGE);adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity')
   return wait(lambda n:'设备列表' in texts(n))
  def choose_remote():
   tap(node(ui(),desc='新建对话'));n=wait(lambda n:'新建对话' in texts(n));tap(node(n,text='lerrem'))
   n=wait(lambda n:'选择设备' in texts(n));tap(node(n,text='Windows'))
   n=wait(lambda n:'新建对话' in texts(n));tap(node(n,text='其他'))
   n=wait(lambda n:'选择项目' in texts(n));assert '远端首轮项目' in texts(n);assert any('lerrem' in t and str(project) in t for t in texts(n))
   shot('new-conversation-remote-projects.png');tap(node(n,text='远端首轮项目'))
   return wait(lambda n:'将在 lerrem 上创建对话。' in texts(n))
  try:
   adb('install',ROOT/'android/app/build/outputs/apk/debug/app-debug.apk');adb('reverse',f'tcp:{port}',f'tcp:{port}')
   adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity');n=wait(lambda n:'连接你的电脑' in texts(n));tap(node(n,text='连接密钥'))
   wait(lambda n:'输入地址与连接密钥，让对话继续。' in texts(n));enter(0,f'http://127.0.0.1:{port}');enter(1,key)
   n=ui()
   if '局域网测试' not in texts(n):adb('shell','input','swipe',600,1700,600,700,350);n=ui()
   tap(lan_toggle(n));wait(lambda n:lan_toggle(n).attrib.get('checked')=='true');tap(node(ui(),text='连接'));wait(lambda n:'设备列表' in texts(n),seconds=45)
   n=choose_remote();shot('new-conversation-remote-confirm.png')
   with closing(sqlite3.connect(db)) as c:assert c.execute('SELECT count(*) FROM commands').fetchone()[0]==0
   tap(node(n,text='建立'));n=wait(lambda n:'开始新对话' in texts(n));field=next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText');tap(field);adb('shell','input','text','phone%sstarts%shere');tap(node(ui(),desc='发送'))
   deadline=time.monotonic()+30
   while time.monotonic()<deadline:
    with closing(sqlite3.connect(db)) as c:rows=c.execute('SELECT id,host,status,native_turn FROM commands').fetchall()
    if rows and rows[0][2]=='dispatching' and rows[0][3]==turn:break
    time.sleep(.2)
   else:raise AssertionError('First turn did not start: '+str(rows))
   assert len(rows)==1 and rows[0][1]==executor
   assert (root/'calls').read_text()=='start\nname\nturn\n'
   with closing(sqlite3.connect(db)) as c:assert c.execute('SELECT owner,project FROM creation_context').fetchone()==(owner,str(project))
   stamp=int(time.time()*1000);save_turn(source,native,turn,[('human','phone starts here',stamp,hashlib.sha256(b'phone starts here').hexdigest())],stamp+1)
   run('capture','--thread',native,'--database',source,json.dumps({'thread-id':native,'turn-id':turn,'type':'agent-turn-complete','last-assistant-message':'first created reply','cwd':str(project)}))
   n=wait(lambda n:'first created reply' in texts(n),seconds=45);shot('new-conversation-first-reply.png')
   start();time.sleep(2);assert (root/'calls').read_text()=='start\nname\nturn\n'
   # Same selection remains usable under constrained layouts; no further sends.
   adb('shell','wm','size','1080x1920');adb('shell','wm','density','440');adb('shell','cmd','uimode','night','yes');adb('shell','settings','put','system','font_scale','1.5')
   start();n=choose_remote();shot('new-conversation-small-dark-large-font.png');tap(node(n,text='取消'))
   adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no');adb('shell','settings','put','system','accelerometer_rotation','0');adb('shell','settings','put','system','user_rotation','1')
   adb('shell','wm','size','1920x1080');adb('shell','settings','put','global','animator_duration_scale','0')
   start();n=choose_remote();shot('new-conversation-landscape.png');tap(node(n,text='取消'))
   import struct
   width,height=struct.unpack('>II',(ROOT/'artifacts/new-conversation-landscape.png').read_bytes()[16:24]);assert width>height,'Landscape screenshot must be landscape'
   with closing(sqlite3.connect(db)) as c:assert c.execute('SELECT count(*) FROM commands').fetchone()[0]==1
   (ROOT/'artifacts/new-conversation-ui.verification.json').write_text(json.dumps({'passed':True,'real_apk':True,'real_phone':False,'real_codex_send':False,'checks':['uncached desktop remote project selection','owner vs executor routing','first turn/start without queue','final reply and native ID adoption','restart no resend','small dark large font and landscape']},indent=2))
   print('PASS real Android UI -> Windows remote lerrem project -> real Hub/bridge -> mock turn/start -> captured first reply; no resend; constrained layouts',flush=True)
  finally:
   stop.set();bridge.terminate();bridge.wait(timeout=5);hub.terminate();hub.wait(timeout=5)
   adb('shell','settings','put','global','animator_duration_scale','1');adb('shell','wm','size','reset');adb('shell','wm','density','reset');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no');adb('shell','settings','put','system','user_rotation','0')
if __name__=='__main__':main()
