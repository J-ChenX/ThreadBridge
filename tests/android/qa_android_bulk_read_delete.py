"""Bulk read/delete and composer regressions in a dedicated debug app and synthetic Hub.

Requires qa_android_fixture018.py, emulator-5554 and a freshly built debug APK.
No production identities, upstream Codex, or model services are used.
"""
from qa_android_ui015 import *
from qa_android_settings021 import enter,field
from qa_android_keyentry021 import lan_toggle
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
import http.client,threading,os

PACKAGE='dev.threadbridge.debug'
PHONE='bulk-phone-'+uuid.uuid4().hex
KEY='bulk'+uuid.uuid4().hex
MODE='healthy'
DELETE_COUNT=0

class Proxy(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):self.forward('GET')
 def do_POST(self):self.forward('POST')
 def forward(self,method):
  global DELETE_COUNT
  request=self.rfile.read(int(self.headers.get('Content-Length',0)))
  if self.headers.get('Upgrade'):
   status,body=403,b'{"error":"fixture_websocket_disabled"}'
  elif self.path.endswith('/delete') and MODE=='partial':
   DELETE_COUNT+=1
   if DELETE_COUNT==2:status,body=503,b'{"error":"fixture_second_delete_failed"}'
   else:status,body=self.upstream(method,request)
  elif self.path=='/v1/threads/create' and MODE=='reject':
   status,body=409,b'{"error":"host_offline"}'
  else:
   status,body=self.upstream(method,request)
   if self.path=='/v1/threads/create' and MODE=='unknown' and status==200:
    command=json.loads(body)['id']
    with closing(sqlite3.connect(DB)) as db,db:
     db.execute("UPDATE commands SET status='unknown',error='creation_unconfirmed_no_retry' WHERE id=?",(command,))
  self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def upstream(self,method,body):
  connection=http.client.HTTPConnection('127.0.0.1',8798,timeout=10)
  try:
   connection.request(method,self.path,body if method=='POST' else None,headers={'Authorization':self.headers.get('Authorization',''),'Content-Type':'application/json'})
   response=connection.getresponse();return response.status,response.read()
  finally:connection.close()

def launch():
 adb('shell','am','force-stop',PACKAGE);adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity')
 return ui()
def read_dot(tree,label):return any(label in x.attrib.get('content-desc','') for x in tree.iter('node'))
def click_text(text):tap(node(ui(),text=text))
def select_menu():
 tap(node(ui(),desc='更多选项'));n=wait(lambda n:'多选会话' in texts(n));tap(node(n,text='多选会话'))
 return wait(lambda n:'已选 0 个' in texts(n))
def rows_in_app():
 raw=adb('shell','run-as',PACKAGE,'cat','/data/user/0/'+PACKAGE+'/shared_prefs/conversation_reads.xml')
 return {e.attrib.get('name'):e.text for e in ET.fromstring(raw) if e.tag=='string'}
def new_blank():
 tap(node(ui(),desc='新建对话'));n=wait(lambda n:'新建对话' in texts(n));tap(node(n,text='建立'))
 return wait(lambda n:'开始新对话' in texts(n))
def send_button(tree):
 parents={child:parent for parent in tree.iter('node') for child in parent}
 target=node(tree,desc='发送')
 while target in parents:
  if target.attrib.get('clickable')=='true' and target.attrib.get('enabled')=='true':return target
  target=parents[target]
 return None
def send_text(text):
 tap(field(ui(),0));adb('shell','input','text',text);adb('shell','input','keyevent','4')
 n=wait(lambda n:send_button(n) is not None);tap(send_button(n))
 return wait(lambda n:field(n,0).attrib.get('text','') in ['', '发送消息'],seconds=90)

def main():
 global MODE,DELETE_COUNT
 assert 'package:'+PACKAGE not in adb('shell','pm','list','packages',PACKAGE),'Dedicated fresh debug package required'
 OUT.mkdir(exist_ok=True)
 proxy=ThreadingHTTPServer(('127.0.0.1',8799),Proxy);threading.Thread(target=proxy.serve_forever,daemon=True).start()
 now=int(time.time());ids=[]
 with closing(sqlite3.connect(DB)) as db,db:
  for old, in db.execute("SELECT id FROM threads WHERE native LIKE 'bulk-%'").fetchall():
   for table in ('messages','thread_projects','thread_message_state'):db.execute(f'DELETE FROM {table} WHERE thread=?',(old,))
   db.execute('DELETE FROM threads WHERE id=?',(old,))
  db.execute('INSERT INTO devices VALUES(?,?,?,?,?,0,0)',(PHONE,hashlib.sha256(KEY.encode()).hexdigest(),'phone','Bulk QA',now+3600))
  for i in range(6):
   native='bulk-'+uuid.uuid4().hex;tid=hashlib.sha256(('ui-fixture-host\0default\0'+native).encode()).hexdigest();ids.append(tid)
   db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,'ui-fixture-host',native,f'Bulk QA {i}','completed','bulk-turn',now+i,1))
   db.execute('INSERT INTO thread_projects VALUES(?,?)',(tid,'/qa/bulk-a' if i<5 else '/qa/bulk-b'))
   db.execute('INSERT INTO thread_message_state VALUES(?,0,?)',(tid,(now+i)*1000))
 try:
  adb('install',ROOT/'android/app/build/outputs/apk/debug/app-debug.apk');launch()
  n=wait(lambda n:'连接你的电脑' in texts(n));tap(node(n,text='连接密钥'));enter(0,'http://10.0.2.2:8799');enter(1,KEY)
  n=ui()
  if '局域网测试' not in texts(n):adb('shell','input','swipe',600,1700,600,800,350);n=ui()
  tap(lan_toggle(n));tap(node(ui(),text='连接'));n=wait(lambda n:'Bulk QA 5' in texts(n))
  deadline=time.monotonic()+15
  while not all(tid in rows_in_app() for tid in ids):
   if time.monotonic()>deadline:raise AssertionError('Initial read baseline not persisted')
   time.sleep(.2)
  time.sleep(1)
  with closing(sqlite3.connect(DB)) as db,db:db.execute('UPDATE thread_message_state SET revision=revision+1 WHERE thread IN ('+','.join('?'*len(ids))+')',ids)
  n=wait(lambda n:read_dot(n,'未读项目 bulk-a'))
  tap(node(n,text='bulk-a'),long=True);n=wait(lambda n:'全部标为已读 · 5 个未读' in texts(n));shot('bulk-project-read.png');tap(node(n,text='全部标为已读 · 5 个未读'))
  n=wait(lambda n:not read_dot(n,'未读项目 bulk-a'));assert read_dot(n,'未读项目 bulk-b')
  marks=rows_in_app();assert all(marks[tid].endswith(':1') for tid in ids[:5])
  tap(node(n,text='echova'),long=True);n=wait(lambda n:'全部标为已读 · 1 个未读' in texts(n));tap(node(n,text='全部标为已读 · 1 个未读'))
  n=wait(lambda n:not read_dot(n,'未读设备 echova'));launch();n=wait(lambda n:'Bulk QA 5' in texts(n));assert not read_dot(n,'未读设备 echova')
  print('PASS project/device bulk read covers hidden old rows; persists across restart; scopes stay isolated',flush=True)
  with closing(sqlite3.connect(DB)) as db,db:db.execute('UPDATE thread_message_state SET revision=revision+1 WHERE thread=?',(ids[0],))
  n=wait(lambda n:read_dot(n,'未读项目 bulk-a'))
  select_menu();n=wait(lambda n:'Bulk QA 0' in texts(n));tap(node(n,text='Bulk QA 0'));tap(node(ui(),text='Bulk QA 1'));n=wait(lambda n:'已选 2 个' in texts(n));shot('bulk-selection.png')
  tap(node(n,desc='删除选中的会话'));n=wait(lambda n:'删除 2 个会话？' in texts(n));click_text('取消')
  n=wait(lambda n:'已选 2 个' in texts(n));MODE='partial';DELETE_COUNT=0;tap(node(n,desc='删除选中的会话'));wait(lambda n:'删除 2 个会话？' in texts(n));click_text('删除')
  n=wait(lambda n:'暂时无法完成' in texts(n));assert any('已删除 1 个，剩余 1 个未删除' in t for t in texts(n));click_text('知道了');n=wait(lambda n:'已选 1 个' in texts(n))
  MODE='healthy';tap(node(n,desc='删除选中的会话'));wait(lambda n:'删除 1 个会话？' in texts(n));click_text('删除');wait(lambda n:'设备列表' in texts(n))
  with closing(sqlite3.connect(DB)) as db:
   assert db.execute('SELECT count(*) FROM threads WHERE id IN (?,?)',ids[:2]).fetchone()[0]==0
   assert db.execute("SELECT count(*) FROM tombstones WHERE thread IN (?,?) AND message=''",ids[:2]).fetchone()[0]==2
  print('PASS selection, cancel confirmation, partial failure retains remaining selection, retry deletes only remaining replicas',flush=True)
  new_blank();MODE='unknown';send_text('uncertain_bulk_fixture');n=wait(lambda n:'结果未知，请勿重复发送' in texts(n),seconds=90);assert field(n,0).attrib.get('text','') in ['', '发送消息']
  launch();n=wait(lambda n:'设备列表' in texts(n));tap(node(n,text='新对话'));n=wait(lambda n:'结果未知，请勿重复发送' in texts(n),seconds=90);assert field(n,0).attrib.get('text','') in ['', '发送消息'];shot('bulk-unknown-composer-clear.png');adb('shell','input','keyevent','4');wait(lambda n:'设备列表' in texts(n))
  MODE='reject';new_blank();tap(field(ui(),0));adb('shell','input','text','rejected_bulk_fixture');adb('shell','input','keyevent','4');n=wait(lambda n:send_button(n) is not None);tap(send_button(n));n=wait(lambda n:'已拒绝' in texts(n));assert field(n,0).attrib.get('text')=='rejected_bulk_fixture'
  print('PASS unknown creation clears composer/durable draft across restart; explicit rejection keeps draft',flush=True)
  adb('shell','input','keyevent','4');wait(lambda n:'设备列表' in texts(n));MODE='healthy';new_blank();send_text('success_bulk_fixture');n=wait(lambda n:any('模拟器中的测试回复' in t for t in texts(n)),seconds=90);assert field(n,0).attrib.get('text','') in ['', '发送消息']
  print('PASS successful creation adopts native ID and composer stays clear',flush=True)
  adb('shell','input','keyevent','4');wait(lambda n:'设备列表' in texts(n))
  adb('shell','wm','size','750x1334');adb('shell','wm','density','320');adb('shell','settings','put','system','font_scale','2.0');adb('shell','cmd','uimode','night','yes')
  for setting in ['window_animation_scale','transition_animation_scale','animator_duration_scale']:adb('shell','settings','put','global',setting,'0')
  launch();wait(lambda n:'设备列表' in texts(n));n=select_menu();shot('bulk-small-dark-large-font.png');tap(node(n,text='全选'));wait(lambda n:any(t.startswith('已选 ') and t!='已选 0 个' for t in texts(n)))
  n=ui();tap(node(n,desc='删除选中的会话'));wait(lambda n:any(t.startswith('删除 ') and t.endswith('个会话？') for t in texts(n)));click_text('取消');tap(node(ui(),desc='取消多选'))
  adb('shell','settings','put','system','font_scale','1.0');adb('shell','settings','put','system','accelerometer_rotation','0');adb('shell','settings','put','system','user_rotation','1');launch();wait(lambda n:'设备列表' in texts(n));select_menu();shot('bulk-landscape.png')
  print('PASS small-phone dark mode, 2x text, animation disabled, all-selection controls, and landscape',flush=True)
 finally:
  adb('shell','am','force-stop',PACKAGE)
  if not os.environ.get('THREADBRIDGE_QA_KEEP_APP'):adb('uninstall',PACKAGE)
  adb('shell','wm','size','reset');adb('shell','wm','density','reset');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no');adb('shell','settings','put','system','user_rotation','0')
  proxy.shutdown()
  with closing(sqlite3.connect(DB)) as db,db:db.execute('DELETE FROM devices WHERE id=?',(PHONE,))

if __name__=='__main__':main()
