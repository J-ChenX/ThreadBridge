"""Loopback interactive preview of emulator-5554. Pixels are the actual Android APK.
Start the isolated UI fixture and emulator first. No real desktop Codex calls.
"""
import argparse,json,secrets,subprocess,threading,time
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
ADB=ROOT/'local/android-sdk/platform-tools/adb'
LOCK=threading.Lock()
TOKEN=secrets.token_urlsafe(32)
WIDTH,HEIGHT,DENSITY=1200,2670,480
HTML='''<!doctype html><html lang="zh"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>续桥 · 手机界面预览</title>
<style>*{box-sizing:border-box}body{margin:0;background:#f1f1f1;color:#171717;font:14px system-ui,sans-serif}main{max-width:1120px;margin:auto;padding:24px;display:flex;gap:36px;align-items:flex-start}aside{max-width:290px;padding-top:16px}h1{font-size:24px;letter-spacing:-.6px}p{line-height:1.8;color:#666}button,a{font:inherit;color:#171717;border:1px solid #ddd;background:white;border-radius:12px;padding:10px 14px;cursor:pointer;text-decoration:none}nav{display:flex;flex-wrap:wrap;gap:10px;margin:24px 0}#phone{width:min(390px,100%);border:1px solid #dedede;box-shadow:0 12px 44px #0001;background:#fff;line-height:0;border-radius:22px;overflow:hidden}img{width:100%;touch-action:none;user-select:none}#state{font-size:12px;color:#777}@media(max-width:750px){main{padding:12px;display:block}aside{max-width:none;padding:0}h1{font-size:19px}aside p{display:none}nav{margin:10px 0}#phone{margin:auto}}
</style><main><div id="phone"><img id="screen" draggable="false" alt="实际 Android 手机界面"></div><aside><h1>续桥 · 手机界面</h1><p>这里显示同一份 Android 应用的实际界面，使用独立演示对话。你可以直接点击、长按、滑动，或保存截图指出修改位置。</p><nav><button data-action="back">返回</button><button data-action="light">浅色</button><button data-action="dark">深色</button><a id="save" href="/snapshot.png" download="ThreadBridge-phone.png">保存截图</a></nav><span id="state">连接预览中…</span></aside></main>
<script>const token="TOKEN",img=document.querySelector('#screen'),state=document.querySelector('#state');let busy=false,start=null;async function frame(){if(!busy){busy=true;try{let r=await fetch('/frame.png',{cache:'no-store'});if(!r.ok)throw Error();let blob=await r.blob();let prior=img.src;img.src=URL.createObjectURL(blob);if(prior.startsWith('blob:'))URL.revokeObjectURL(prior);state.textContent='实际 Android 界面 · 演示数据'}catch(e){state.textContent='预览连接中…'}finally{busy=false}}setTimeout(frame,650)}frame();function point(e){let r=img.getBoundingClientRect(),w=img.naturalWidth,h=img.naturalHeight;return{x:Math.max(0,Math.min(w-1,Math.round((e.clientX-r.left)*w/r.width))),y:Math.max(0,Math.min(h-1,Math.round((e.clientY-r.top)*h/r.height)))}}async function action(data){await fetch('/action',{method:'POST',headers:{'Content-Type':'application/json','X-Preview-Token':token},body:JSON.stringify(data)})}img.onpointerdown=e=>{start={...point(e),at:Date.now()};img.setPointerCapture(e.pointerId)};img.onpointerup=e=>{if(!start)return;let end=point(e),duration=Date.now()-start.at;action({action:Math.hypot(end.x-start.x,end.y-start.y)>30?'swipe':duration>500?'longpress':'tap',x:start.x,y:start.y,x2:end.x,y2:end.y});start=null};img.onpointercancel=()=>start=null;img.oncontextmenu=e=>e.preventDefault();document.querySelectorAll('[data-action]').forEach(b=>b.onclick=()=>action({action:b.dataset.action}));</script></html>'''

def adb(*args):
 return subprocess.check_output([str(ADB),'-s','emulator-5554',*args],timeout=8,stderr=subprocess.DEVNULL)

class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def reply(self,status,body=b'',kind='text/plain',download=False):
  self.send_response(status)
  if download:self.send_header('Content-Disposition','attachment; filename=ThreadBridge-phone.png')
  self.send_header('Content-Type',kind);self.send_header('Content-Length',str(len(body)));self.send_header('Cache-Control','no-store');self.send_header('X-Frame-Options','DENY');self.send_header('Cross-Origin-Resource-Policy','same-origin');self.send_header('X-Content-Type-Options','nosniff');self.end_headers();self.wfile.write(body)
 def valid_host(self):return self.headers.get('Host') in (f'127.0.0.1:{self.server.server_port}',f'localhost:{self.server.server_port}')
 def do_GET(self):
  if not self.valid_host():return self.reply(403)
  if self.path=='/':return self.reply(200,HTML.replace('TOKEN',TOKEN).encode(),'text/html; charset=utf-8')
  if self.path in ('/frame.png','/snapshot.png'):
   try:
    with LOCK:picture=adb('exec-out','screencap','-p')
    return self.reply(200,picture,'image/png',download=self.path=='/snapshot.png')
   except (subprocess.SubprocessError,OSError):return self.reply(503)
  self.reply(404)
 def do_POST(self):
  origin=self.headers.get('Origin')
  if not self.valid_host() or origin not in (f'http://127.0.0.1:{self.server.server_port}',f'http://localhost:{self.server.server_port}') or self.headers.get('X-Preview-Token')!=TOKEN:return self.reply(403)
  if self.path!='/action':return self.reply(404)
  try:
   size=int(self.headers.get('Content-Length','0'))
   if not 0<size<=1024:return self.reply(400)
   data=json.loads(self.rfile.read(size));operation=data['action']
   def coord(name,limit):
    value=data[name]
    if type(value) is not int or not 0<=value<limit:raise ValueError('coordinate')
    return str(value)
   if operation=='tap':args=['input','tap',coord('x',WIDTH),coord('y',HEIGHT)]
   elif operation=='longpress':args=['input','swipe',coord('x',WIDTH),coord('y',HEIGHT),coord('x',WIDTH),coord('y',HEIGHT),'800']
   elif operation=='swipe':args=['input','swipe',coord('x',WIDTH),coord('y',HEIGHT),coord('x2',WIDTH),coord('y2',HEIGHT),'400']
   elif operation=='back':args=['input','keyevent','4']
   elif operation in ('light','dark'):args=['cmd','uimode','night','yes' if operation=='dark' else 'no']
   else:return self.reply(400)
   with LOCK:adb('shell',*args)
   self.reply(200,b'{}','application/json')
  except (ValueError,KeyError,AssertionError,subprocess.SubprocessError,OSError):self.reply(400)

if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--port',type=int,default=8898);args=parser.parse_args()
 adb('shell','wm','size',f'{WIDTH}x{HEIGHT}')
 adb('shell','wm','density',str(DENSITY))
 adb('shell','cmd','overlay','enable-exclusive','--category','com.android.internal.systemui.navbar.gestural')
 print(f'Android preview: http://127.0.0.1:{args.port}',flush=True);ThreadingHTTPServer(('127.0.0.1',args.port),Handler).serve_forever()
