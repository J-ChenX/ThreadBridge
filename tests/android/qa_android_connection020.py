"""Signed APK transport regression against an isolated loopback proxy/fixture Hub.

No production credentials, pairing changes, remote commands or real Codex writes.
"""
from qa_android_ui015 import *
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client
import tempfile
import threading

PRIVATE='/data/user/0/dev.threadbridge'
MODE='healthy'
REQUESTS=[]
SECRET='private-fixture-response-must-not-appear'

class Proxy(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  REQUESTS.append((time.monotonic(),self.path,'ws' if self.headers.get('Upgrade') else 'http'))
  if self.headers.get('Upgrade'):
   status,body=403,b'<html>WebSocket blocked by fixture proxy</html>'
  elif MODE!='healthy':
   status={'expired':404,'malformed':200,'unauthorized':401}[MODE]
   body=('<html>'+SECRET+'</html>').encode()
  else:
   connection=http.client.HTTPConnection('127.0.0.1',8798,timeout=15)
   headers={key:value for key,value in self.headers.items() if key.lower() not in ('host','connection','upgrade')}
   try:
    connection.request('GET',self.path,headers=headers)
    response=connection.getresponse();status=response.status;body=response.read()
   finally:connection.close()
  self.send_response(status);self.send_header('Content-Type','application/json' if MODE=='healthy' and status==200 else 'text/html')
  self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)

def prefs_bytes():
 return subprocess.check_output([str(ADB),'-s','emulator-5554','exec-out','su','0','cat',f'{PRIVATE}/shared_prefs/connection.xml'])

def save_connection(data):
 # Private credentials never enter artifacts or terminal output.
 with tempfile.TemporaryDirectory(prefix='qa020-prefs-',dir=ROOT/'local') as directory:
  path=Path(directory)/'connection.xml';path.write_bytes(data);path.chmod(0o600)
  adb('push',path,'/data/local/tmp/threadbridge-connection-qa.xml')
  adb('shell','su','0','cp','/data/local/tmp/threadbridge-connection-qa.xml',f'{PRIVATE}/shared_prefs/connection.xml')
  adb('shell','su','0','rm','-f','/data/local/tmp/threadbridge-connection-qa.xml',f'{PRIVATE}/shared_prefs/connection.xml.bak')

def cached_state():
 adb('shell','am','force-stop','dev.threadbridge')
 with tempfile.TemporaryDirectory(prefix='qa020-cache-',dir=ROOT/'local') as directory:
  for suffix in ('','-wal','-shm'):
   data=subprocess.check_output([str(ADB),'-s','emulator-5554','exec-out','su','0','cat',f'{PRIVATE}/databases/threadbridge.sqlite{suffix}'])
   (Path(directory)/('threadbridge.sqlite'+suffix)).write_bytes(data)
  with closing(sqlite3.connect(Path(directory)/'threadbridge.sqlite')) as cache:
   return {'threads':dict(cache.execute('SELECT id,title FROM threads')),
           'drafts':dict(cache.execute('SELECT threadId,text FROM drafts'))}

def details():
 n=wait(lambda n:'设备列表' in texts(n));tap(node(n,desc='更多选项'))
 n=wait(lambda n:'连接状态' in texts(n));tap(node(n,text='连接状态'))
 return wait(lambda n:'服务器：http://10.0.2.2:8799' in texts(n))

def check_safe(n):assert SECRET not in '\n'.join(texts(n))

def main():
 global MODE
 assert 'versionName=0.1.20-test' in adb('shell','dumpsys','package','dev.threadbridge')
 adb('shell','am','force-stop','dev.threadbridge')
 original=prefs_bytes();root=ET.fromstring(original)
 server_element=next(e for e in root if e.attrib.get('name')=='server')
 assert server_element.text=='http://10.0.2.2:8798','Only the isolated synthetic Hub is allowed'
 token=next(e.text for e in root if e.attrib.get('name')=='token')
 with closing(sqlite3.connect(DB)) as db:
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
  assert db.execute("SELECT count(*) FROM devices WHERE id='fixture-windows' AND role='agent'").fetchone()[0]==1
 server=ThreadingHTTPServer(('127.0.0.1',8799),Proxy)
 worker=threading.Thread(target=server.serve_forever,daemon=True);worker.start()
 tid=hashlib.sha256(('fixture-windows\0default\0qa020-http-poll').encode()).hexdigest()
 inserted=False
 try:
  server_element.text='http://10.0.2.2:8799';save_connection(ET.tostring(root,encoding='utf-8',xml_declaration=True))
  home();details();n=wait(lambda n:any(t.startswith('已连接 ·') and 'HTTP 轮询' in t for t in texts(n)))
  assert any(kind=='ws' for _,_,kind in REQUESTS)
  shot('ui-0.1.20-http-fallback.png')
  print('PASS rejected WebSocket preserves authenticated HTTP connection health',flush=True)
  baseline=cached_state();home();details()
  started=time.monotonic();now=int(time.time())+14400
  with closing(sqlite3.connect(DB)) as db,db:
   db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,'fixture-windows','qa020-http-poll','HTTP polling recovery','completed','fixture020',now,1))
   db.execute('INSERT INTO thread_projects VALUES(?,?)',(tid,'/qa/020'))
   db.execute('INSERT INTO thread_message_state VALUES(?,1,?)',(tid,now*1000))
   inserted=True
  expected=len(baseline['threads'])+1
  n=wait(lambda n:f'已保存对话：{expected} 个' in '\n'.join(texts(n)),seconds=35)
  assert any(stamp>started and path=='/v1/threads' for stamp,path,kind in REQUESTS)
  print('PASS new conversations arrive via automatic HTTP polling without a push event',flush=True)
  for mode,issue in [('expired','服务器地址已失效（HTTP 404），请检查公网入口'),
                     ('malformed','服务器未返回有效数据，请检查地址和服务版本'),
                     ('unauthorized','凭证已失效，请重新配对')]:
   MODE=mode;n=wait(lambda n:issue in texts(n),seconds=35);check_safe(n)
   assert f'已保存对话：{expected} 个' in '\n'.join(texts(n))
   if mode=='expired':shot('ui-0.1.20-endpoint-expired.png')
   MODE='healthy';n=wait(lambda n:any(t.startswith('已连接 ·') and 'HTTP 轮询' in t for t in texts(n)),seconds=35)
   assert issue not in texts(n)
  print('PASS HTML 404, malformed success and auth failures show safe diagnostics; recover without re-pairing',flush=True)
  shot('ui-0.1.20-connection-recovered.png')
  after=cached_state()
  assert all(after['threads'].get(key)==value for key,value in baseline['threads'].items())
  assert after['drafts']==baseline['drafts']
  assert next(e.text for e in ET.fromstring(prefs_bytes()) if e.attrib.get('name')=='token')==token
  with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
  print('PASS cached conversations, drafts and encrypted credentials preserved; no remote commands',flush=True)
 finally:
  adb('shell','am','force-stop','dev.threadbridge');save_connection(original)
  MODE='healthy';server.shutdown();server.server_close();worker.join(timeout=3)
  if inserted:
   with closing(sqlite3.connect(DB)) as db,db:
    for table in ('messages','thread_projects','thread_message_state'):db.execute(f'DELETE FROM {table} WHERE thread=?',(tid,))
    db.execute('DELETE FROM threads WHERE id=?',(tid,))
  home()

if __name__=='__main__':main()
