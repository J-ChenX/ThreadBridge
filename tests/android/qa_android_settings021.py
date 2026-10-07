"""Manual endpoint/key configuration in signed APK, synthetic Hub/emulator only."""
from qa_android_ui015 import *
from qa_android_connection020 import prefs_bytes,save_connection,cached_state
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
import http.client,threading

MODE='healthy'
KEY='qa021'+uuid.uuid4().hex
PHONE='qa021-phone-'+uuid.uuid4().hex
SEEN_KEYS=set()

class Proxy(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  if self.headers.get('Upgrade'):
   status,body=403,b'<html>fixture blocked WebSocket</html>'
  elif MODE=='different' and self.path=='/v1/hosts':
   status,body=200,json.dumps({'collection_generation':'different-fixture-collection','hosts':[]}).encode()
  else:
   authorization=self.headers.get('Authorization','')
   SEEN_KEYS.add(hashlib.sha256(authorization.removeprefix('Bearer ').encode()).hexdigest())
   connection=http.client.HTTPConnection('127.0.0.1',8798,timeout=15)
   try:
    connection.request('GET',self.path,headers={'Authorization':authorization});response=connection.getresponse();status=response.status;body=response.read()
   finally:connection.close()
  self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)

def settings():
 n=wait(lambda n:'设备列表' in texts(n));tap(node(n,desc='更多选项'))
 n=wait(lambda n:'设置' in texts(n));tap(node(n,text='设置'))
 return wait(lambda n:'验证并保存' in texts(n))

def field(tree,number):return [x for x in tree.iter('node') if x.attrib.get('class')=='android.widget.EditText'][number]
def enter(number,value):
 n=ui();tap(field(n,number));wait(lambda tree:field(tree,number).attrib.get('focused')=='true')
 # adb keycombination does not consistently deliver CTRL meta state to Compose.
 current=field(ui(),number).attrib.get('text','')
 adb('shell','input','keyevent','123');adb('shell','input','keyevent',*(['67']*(len(current)+8)+['112']*(len(current)+8)))
 adb('shell','input','text',value);adb('shell','input','keyevent','4')
 return wait(lambda tree:field(tree,0).attrib.get('text')==value) if number==0 else ui()
def save():
 n=ui();tap(node(n,text='验证并保存'))
 return wait(lambda n:'验证并保存' not in texts(n) and '正在验证…' not in texts(n))
def server_value():return next(e.text for e in ET.fromstring(prefs_bytes()) if e.attrib.get('name')=='server')
def library_bytes():return adb('shell','su','0','cat','/data/user/0/dev.threadbridge/shared_prefs/conversation_library.xml')

def main():
 global MODE
 assert 'versionName=0.1.21-test' in adb('shell','dumpsys','package','dev.threadbridge')
 adb('shell','am','force-stop','dev.threadbridge');original=prefs_bytes()
 assert server_value()=='http://10.0.2.2:8798','Only the synthetic fixture is allowed'
 with closing(sqlite3.connect(DB)) as db,db:
  assert db.execute("SELECT count(*) FROM devices WHERE id='fixture-windows'").fetchone()[0]==1
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
  db.execute('INSERT INTO devices VALUES(?,?,?,?,?,0,0)',(PHONE,hashlib.sha256(KEY.encode()).hexdigest(),'phone','Settings QA',int(time.time())+3600))
 proxy=ThreadingHTTPServer(('127.0.0.1',8799),Proxy);worker=threading.Thread(target=proxy.serve_forever,daemon=True);worker.start()
 try:
  home();n=settings();shot('ui-0.1.21-settings.png')
  assert field(n,1).attrib.get('text','') in ('','留空使用当前密钥')
  n=enter(0,'http://example.com');assert '正式连接需要 HTTPS；局域网测试仅允许私人 IP' in '\n'.join(texts(n))
  assert any(x.attrib.get('enabled')=='false' and '验证并保存' in texts(x) for x in n.iter('node'))
  n=enter(0,'http://10.0.2.2:8799');tap(node(n,text='取消'));n=wait(lambda n:'连接与设置' not in texts(n));assert server_value()=='http://10.0.2.2:8798'
  baseline=cached_state();assert baseline['drafts'],'Must exercise draft preservation'
  home();settings();enter(0,'http://10.0.2.2:8799');save();assert server_value()=='http://10.0.2.2:8799'
  n=wait(lambda n:'设备列表' in texts(n));settings();n=ui();assert field(n,1).attrib.get('text','') in ('','留空使用当前密钥');tap(node(n,text='取消'))
  print('PASS unsafe addresses blocked, cancellation unchanged, blank key validates and retains pairing',flush=True)
  settings();MODE='different';n=enter(0,'http://10.0.2.2:8799');tap(node(n,text='验证并保存'));n=wait(lambda n:'该地址对应不同的同步库，请通过重新配对切换' in texts(n));assert server_value()=='http://10.0.2.2:8799';MODE='healthy'
  n=enter(1,'invalid-fixture-key');tap(node(n,text='验证并保存'));n=wait(lambda n:'连接密钥无效或没有手机访问权限' in texts(n));assert server_value()=='http://10.0.2.2:8799';shot('ui-0.1.21-settings-error.png')
  # Key reveal is explicit, saved keys remain hidden and absent from the form.
  n=enter(1,KEY);assert KEY not in texts(n);tap(node(n,desc='显示连接密钥'));n=wait(lambda n:KEY in texts(n));tap(node(n,desc='隐藏连接密钥'));n=wait(lambda n:KEY not in texts(n));save()
  assert KEY.encode() not in prefs_bytes();assert hashlib.sha256(KEY.encode()).hexdigest() in SEEN_KEYS
  after=cached_state();assert after['threads']==baseline['threads'];assert after['drafts']==baseline['drafts']
  print('PASS different collection and bad key preserve state; explicit key replacement validated and encrypted, drafts/cache retained',flush=True)
  home();settings();enter(0,'http://10.0.2.2:8798');save();assert server_value()=='http://10.0.2.2:8798'
  home();settings();n=ui();tap(node(n,text='清除并重新配对'));n=wait(lambda n:'重新配对？' in texts(n));assert '清除本机并重新配对' in texts(n);tap(node(n,text='取消'));n=wait(lambda n:'重新配对？' not in texts(n));tap(node(n,text='取消'))
  # Consistent dark mode, small display/large text, landscape and reduced motion.
  adb('shell','cmd','uimode','night','yes');home();settings();shot('ui-0.1.21-settings-dark.png');tap(node(ui(),text='取消'))
  adb('shell','cmd','uimode','night','no');adb('shell','wm','density','512');adb('shell','settings','put','system','font_scale','2.0');home();settings();n=ui();assert '验证并保存' in texts(n);shot('ui-0.1.21-settings-large.png');tap(node(n,text='取消'))
  adb('shell','settings','put','system','font_scale','1.0');adb('shell','wm','density','450');adb('shell','settings','put','system','accelerometer_rotation','0');adb('shell','wm','size','2670x1200');home();settings();n=ui();assert '验证并保存' in texts(n);shot('ui-0.1.21-settings-landscape.png');tap(node(n,text='取消'));adb('shell','wm','size','1200x2670')
  adb('shell','settings','put','system','user_rotation','0')
  for name in ('animator_duration_scale','transition_animation_scale','window_animation_scale'):adb('shell','settings','put','global',name,'0')
  home();settings();n=ui();assert '验证并保存' in texts(n);tap(node(n,text='取消'))
  with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
  print('PASS protected reset, dark/small-display/large-font/landscape/reduced-motion layout; no remote commands',flush=True)
 finally:
  MODE='healthy';adb('shell','am','force-stop','dev.threadbridge');save_connection(original)
  proxy.shutdown();proxy.server_close();worker.join(timeout=3)
  with closing(sqlite3.connect(DB)) as db,db:db.execute('DELETE FROM devices WHERE id=?',(PHONE,))
  adb('shell','cmd','uimode','night','no');adb('shell','wm','size','1200x2670');adb('shell','wm','density','450');adb('shell','settings','put','system','font_scale','1.0');adb('shell','settings','put','system','user_rotation','0')
  for name in ('animator_duration_scale','transition_animation_scale','window_animation_scale'):adb('shell','settings','put','global',name,'1')
  home()

if __name__=='__main__':main()
