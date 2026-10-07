"""Fresh direct-key entry in an isolated debug package on the synthetic emulator."""
from qa_android_ui015 import *
from qa_android_settings021 import enter

PACKAGE='dev.threadbridge.debug'
PHONE='qa021-first-'+uuid.uuid4().hex
KEY='firstqa021'+uuid.uuid4().hex

def launch():
 adb('shell','am','force-stop',PACKAGE)
 adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity')
 return ui()

def lan_toggle(tree):
 def middle_y(n):
  b=list(map(int,re.findall(r'\d+',n.attrib['bounds'])))
  return (b[1]+b[3])/2
 label_y=middle_y(node(tree,text='局域网测试'))
 choices=[x for x in tree.iter('node') if x.attrib.get('checkable')=='true']
 toggle=min(choices,key=lambda x:abs(middle_y(x)-label_y))
 assert abs(middle_y(toggle)-label_y)<100,'LAN switch must share the label row'
 return toggle

def main():
 assert 'package:'+PACKAGE not in adb('shell','pm','list','packages',PACKAGE),'Fresh test package must not already exist'
 with closing(sqlite3.connect(DB)) as db,db:
  assert db.execute("SELECT count(*) FROM devices WHERE id='fixture-windows'").fetchone()[0]==1
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
  db.execute('INSERT INTO devices VALUES(?,?,?,?,?,0,0)',(PHONE,hashlib.sha256(KEY.encode()).hexdigest(),'phone','Fresh entry QA',int(time.time())+3600))
 try:
  adb('install',ROOT/'android/app/build/outputs/apk/debug/app-debug.apk')
  launch();n=wait(lambda n:'连接你的电脑' in texts(n));tap(node(n,text='连接密钥'))
  n=wait(lambda n:'输入地址与连接密钥，让对话继续。' in texts(n))
  enter(0,'http://10.0.2.2:8798');enter(1,KEY);n=ui()
  if '局域网测试' not in texts(n):
   adb('shell','input','swipe',600,1700,600,800,350)
   n=wait(lambda tree:'局域网测试' in texts(tree))
  tap(lan_toggle(n));n=ui();assert lan_toggle(n).attrib.get('checked')=='true'
  n=ui();assert KEY not in texts(n);shot('ui-0.1.21-first-key-entry.png');tap(node(n,text='连接'))
  n=wait(lambda n:'设备列表' in texts(n),seconds=45)
  assert '连接你的电脑' not in texts(n) and '主页' not in texts(n),'First connection must land on the home list'
  print('PASS fresh direct-key authentication lands on home with drawer closed',flush=True)
  launch();n=wait(lambda n:'设备列表' in texts(n));assert '连接你的电脑' not in texts(n)
  raw=subprocess.check_output([str(ADB),'-s','emulator-5554','exec-out','su','0','cat','/data/user/0/'+PACKAGE+'/shared_prefs/connection.xml'])
  assert KEY.encode() not in raw
  root=ET.fromstring(raw);assert next(e.text for e in root if e.attrib.get('name')=='server')=='http://10.0.2.2:8798'
  assert next(e.text for e in root if e.attrib.get('name')=='token')
  with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
  print('PASS fresh address/key connection without pair exchange; encrypted key survives relaunch; no remote commands',flush=True)
 finally:
  adb('shell','am','force-stop',PACKAGE)
  if 'package:'+PACKAGE in adb('shell','pm','list','packages',PACKAGE):adb('uninstall',PACKAGE)
  with closing(sqlite3.connect(DB)) as db,db:db.execute('DELETE FROM devices WHERE id=?',(PHONE,))
  home()

if __name__=='__main__':main()
