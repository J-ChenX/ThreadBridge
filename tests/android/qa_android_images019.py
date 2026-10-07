"""Fullscreen image UI in signed APK; generated fixture, no external data or sends."""
from qa_android_ui015 import *
from PIL import Image,ImageDraw
import io,os,zipfile

def state(tree):return next(x.attrib.get('state-description','') for x in tree.iter('node') if x.attrib.get('content-desc')=='图片预览')
def button(tree,desc):
 return next(p for p in tree.iter('node') if p.attrib.get('clickable')=='true' and any(c.attrib.get('content-desc')==desc for c in p.iter('node')))
def prepare_gestures():
 target=ROOT/'local/image-gestures';target.mkdir(exist_ok=True);jdk=ROOT/'local/toolchains/jdk17';env=os.environ.copy();env['JAVA_HOME']=str(jdk);env['PATH']=str(jdk/'bin')+os.pathsep+env['PATH']
 subprocess.run([str(jdk/'bin/javac'),'-source','8','-target','8','-classpath',str(ROOT/'local/android-sdk/platforms/android-35/android.jar'),'-d',str(target),str(ROOT/'tests/android/ImageGestures.java')],check=True)
 subprocess.run([str(ROOT/'local/android-sdk/build-tools/35.0.0/d8'),'--min-api','26','--output',str(target),str(target/'dev/threadbridge/qa/ImageGestures.class')],check=True,env=env)
 with zipfile.ZipFile(target/'gestures.jar','w') as z:z.write(target/'classes.dex','classes.dex')
 adb('push',target/'gestures.jar','/data/local/tmp/threadbridge-image-gestures.jar')
def gesture(kind):adb('shell','env','CLASSPATH=/data/local/tmp/threadbridge-image-gestures.jar','app_process','/system/bin','dev.threadbridge.qa.ImageGestures',kind,600,1200)
def open_image():
 n=home();tap(node(n,desc='搜索对话标题'));n=ui();tap(next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText'));adb('shell','input','text','Image%sViewer%sQA');adb('shell','input','keyevent','4');n=wait(lambda n:any(x.attrib.get('text')=='Image Viewer QA' and x.attrib.get('class')=='android.widget.TextView' for x in n.iter('node')));tap(next(x for x in n.iter('node') if x.attrib.get('text')=='Image Viewer QA' and x.attrib.get('class')=='android.widget.TextView'));n=wait(lambda n:any(x.attrib.get('content-desc')=='查看图片附件' for x in n.iter('node')));tap(node(n,desc='查看图片附件'));return wait(lambda n:'100%' in texts(n))
def main():
 assert 'versionName=0.1.19-test' in adb('shell','dumpsys','package','dev.threadbridge')
 adb('shell','wm','size','1200x2670');adb('shell','wm','density','480');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no')
 prepare_gestures()
 fixture=Image.new('RGB',(2400,1600),'#f4f4f4');draw=ImageDraw.Draw(fixture)
 for x in range(0,2400,100):draw.line((x,0,x,1600),fill='#c7d3df',width=2)
 for y in range(0,1600,100):draw.line((0,y,2400,y),fill='#c7d3df',width=2)
 draw.rounded_rectangle((250,230,2150,1370),radius=60,fill='#ffffff',outline='#2383e2',width=12)
 draw.text((450,500),'ThreadBridge image viewer - synthetic QA',fill='#171717',font_size=64)
 draw.rectangle((450,780,1000,1100),fill='#e4eefb');draw.ellipse((1250,720,1680,1150),fill='#2383e2')
 buffer=io.BytesIO();fixture.save(buffer,format='PNG');data=buffer.getvalue();digest=hashlib.sha256(data).hexdigest()
 owner='fixture-windows';tid=hashlib.sha256((owner+'\0default\0Image Viewer QA').encode()).hexdigest();now=int(time.time())+10000
 with closing(sqlite3.connect(DB)) as db,db:
  db.execute('INSERT OR REPLACE INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,owner,'Image Viewer QA','Image Viewer QA','completed','fixture',now,1));db.execute('INSERT OR REPLACE INTO thread_projects VALUES(?,?)',(tid,'/qa/images'));db.execute('INSERT OR REPLACE INTO thread_message_state VALUES(?,1,?)',(tid,now*1000));db.execute('INSERT OR REPLACE INTO project_names VALUES(?,?,?)',(owner,'/qa/images','图片检查'))
  text=f'![图片](threadbridge-image:{digest})\n图片预览测试';db.execute('INSERT OR REPLACE INTO messages VALUES(?,?,?,?,?,?,?)',(tid,'input:photo','turn','user',text,hashlib.sha256(text.encode()).hexdigest(),now*1000));db.execute('INSERT OR REPLACE INTO attachments VALUES(?,?,?,?,?)',(tid,'input:photo',digest,'image/png',data));commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
 n=home();refresh();n=wait(lambda n:any(x.attrib.get('text')=='Image Viewer QA' and x.attrib.get('class')=='android.widget.TextView' for x in n.iter('node')));tap(next(x for x in n.iter('node') if x.attrib.get('text')=='Image Viewer QA' and x.attrib.get('class')=='android.widget.TextView'));n=wait(lambda n:any(x.attrib.get('content-desc')=='查看图片附件' for x in n.iter('node')))
 shot('ui-0.1.19-image-thumbnail.png');tap(node(n,desc='查看图片附件'));n=wait(lambda n:'双指缩放 · 双击放大' in texts(n));assert '100%' in texts(n)
 preview=node(n,desc='图片预览');x1,y1,x2,y2=map(int,re.findall(r'\d+',preview.attrib['bounds']));assert x2-x1>=1100 and y2-y1>=1600,preview.attrib
 shot('ui-0.1.19-image-fullscreen.png');tap(node(n,desc='放大图片'));n=wait(lambda n:'150%' in texts(n));tap(node(n,desc='放大图片'));n=wait(lambda n:'225%' in texts(n));shot('ui-0.1.19-image-zoomed.png')
 adb('shell','input','swipe',600,1200,1050,1000,400);n=ui();assert '225%' in texts(n);tap(node(n,text='复位'));n=wait(lambda n:'100%' in texts(n))
 gesture('double');n=wait(lambda n:'250%' in texts(n));gesture('double');n=wait(lambda n:'100%' in texts(n))
 gesture('pinch');n=ui();assert any(re.fullmatch(r'\d+%',t) and 150<int(t[:-1])<250 for t in texts(n)),texts(n);shot('ui-0.1.19-image-pinched.png');tap(node(n,text='复位'));n=wait(lambda n:'100%' in texts(n))
 for _ in range(8):tap(node(n,desc='放大图片'));n=ui()
 n=wait(lambda n:'500%' in texts(n));assert button(n,'放大图片').attrib.get('enabled')=='false';tap(node(n,text='复位'));n=wait(lambda n:'100%' in texts(n));tap(node(n,desc='关闭图片预览'));n=wait(lambda n:'图片预览测试' in texts(n))
 tap(node(n,desc='查看图片附件'));n=wait(lambda n:'100%' in texts(n));adb('shell','input','keyevent','4');n=wait(lambda n:'图片预览测试' in texts(n));assert not any(x.attrib.get('content-desc')=='图片预览' for x in n.iter('node'))
 print('PASS fullscreen, zoom buttons, bounds, pan, real two-pointer pinch, double-tap/reset, reopen and system back',flush=True)
 adb('shell','cmd','uimode','night','yes');n=home();tap(next(x for x in n.iter('node') if x.attrib.get('text')=='Image Viewer QA' and x.attrib.get('class')=='android.widget.TextView'));n=wait(lambda n:any(x.attrib.get('content-desc')=='查看图片附件' for x in n.iter('node')));tap(node(n,desc='查看图片附件'));n=wait(lambda n:'100%' in texts(n));shot('ui-0.1.19-image-dark.png');tap(node(n,desc='关闭图片预览'));adb('shell','cmd','uimode','night','no')
 for name,size,font in [('large','1125x2436','2.0'),('landscape','2670x1200','1.0')]:
  adb('shell','wm','size',size);adb('shell','settings','put','system','font_scale',font);n=open_image();shot('ui-0.1.19-image-'+name+'.png')
  preview=node(n,desc='图片预览');control=button(n,'放大图片');pb=list(map(int,re.findall(r'\d+',preview.attrib['bounds'])));cb=list(map(int,re.findall(r'\d+',control.attrib['bounds'])));assert pb[3]<=cb[1],(name,pb,cb)
  tap(control);n=wait(lambda n:'150%' in texts(n));tap(node(n,text='复位'));wait(lambda n:'100%' in texts(n));adb('shell','input','keyevent','4')
 adb('shell','wm','size','1200x2670');adb('shell','settings','put','system','font_scale','1.0')
 with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
 print('PASS dark, 375dp/2x font and landscape; toolbar stays outside image; no commands sent',flush=True);home()
if __name__=='__main__':
 try:main()
 finally:
  adb('shell','wm','size','1200x2670');adb('shell','wm','density','480');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no')
