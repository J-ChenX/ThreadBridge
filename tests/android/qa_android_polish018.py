"""Real APK visual/motion QA on synthetic Hub only; no requests submitted."""
from qa_android_ui015 import *
import statistics

def bounds(n):return list(map(int,re.findall(r'\d+',n.attrib['bounds'])))
def open_short():
 with closing(sqlite3.connect(DB)) as db,db:
  now=int(time.time())+1000;tid=db.execute("SELECT id FROM threads WHERE native='ui017-short'").fetchone()[0]
  db.execute("UPDATE threads SET updated=? WHERE id=?",(now,tid));db.execute("UPDATE thread_message_state SET revision=revision+1,activity_at=? WHERE thread=?",(now*1000,tid))
 n=home();refresh();n=wait(lambda n:'Downward short' in texts(n));tap(node(n,text='Downward short'));return wait(lambda n:any(x.attrib.get('content-desc')=='展开过程' for x in n.iter('node')))
def motion_video(n):
 control=node(n,desc='展开过程');x1,y1,x2,y2=bounds(control);assert x2-x1<600 and y2-y1>=140
 shot('ui-0.1.18-motion-before.png')
 recording=subprocess.Popen([str(ADB),'-s','emulator-5554','shell','screenrecord','--time-limit','3','/sdcard/ui018-motion.mp4'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 time.sleep(.5);tap(control);recording.wait(timeout=8);adb('pull','/sdcard/ui018-motion.mp4',OUT/'ui-0.1.18-process-motion.mp4')
 info=json.loads(subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-show_entries','stream=width,height','-of','json',str(OUT/'ui-0.1.18-process-motion.mp4')],text=True))['streams'][0]
 height=info['height'];scale=min(info['width']/1200,height/2670);left=(info['width']-1200*scale)/2;top=(height-2670*scale)/2
 width=round(90*scale);x=round(left+(x1+36)*scale);control_y=round(top+y1*scale);control_height=round((y2-y1)*scale)
 data=subprocess.check_output(['ffmpeg','-v','error','-i',str(OUT/'ui-0.1.18-process-motion.mp4'),'-vf',f'format=gray,crop={width}:{height}:{x}:0','-vsync','0','-f','rawvideo','pipe:1']);frame=width*height
 translate=bytes(1 if i<180 else 0 for i in range(256));first=data[:frame].translate(translate)
 profile=[first[r*width:(r+1)*width].count(1) for r in range(control_y,control_y+control_height)];nonzero=[i for i,v in enumerate(profile) if v];lo,hi=min(nonzero),max(nonzero)+1;profile=profile[lo:hi];expected=control_y+lo
 observed=[];scores=[]
 for offset in range(0,len(data),frame):
  b=data[offset:offset+frame].translate(translate);rows=[b[r*width:(r+1)*width].count(1) for r in range(height)]
  candidates=range(max(0,expected-240),min(height-len(profile),expected+240))
  error,y=min((sum(abs(a-b) for a,b in zip(profile,rows[pos:pos+len(profile)])),pos) for pos in candidates)
  observed.append(y);scores.append(error/max(1,sum(profile)))
 assert len(observed)>=12,(len(observed),height)
 assert max(scores)<0.2,('control disappears during transition',max(scores))
 assert max(abs(y-expected) for y in observed)<=2,('frame drift',expected,min(observed),max(observed))
 # Sample the empty center-top of the capsule. Its light grey background and
 # 7.5% state layer cannot legitimately produce the former ~206 dark flash.
 sample_x=round(left+(x1+x2)/2*scale);sample_y=round(top+(y1+18)*scale)
 pixels=subprocess.check_output(['ffmpeg','-v','error','-i',str(OUT/'ui-0.1.18-process-motion.mp4'),'-vf',f'format=rgb24,crop=1:1:{sample_x}:{sample_y}','-vsync','0','-f','rawvideo','pipe:1'])
 assert min(pixels)>=220,('capsule darkens unexpectedly between frames',min(pixels))
 n=ui();assert any(x.attrib.get('content-desc')=='收起过程' for x in n.iter('node'));shot('ui-0.1.18-process-polished.png')
 print(f'PASS motion video: {len(observed)} sampled frames; no dark flash, disappearance or shift >2 encoded pixels',flush=True)
 # Hold the capsule to inspect its pressed state, without triggering any send.
 control=node(n,desc='收起过程');x1,y1,x2,y2=bounds(control);x=(x1+x2)//2;y=(y1+y2)//2
 hold=subprocess.Popen([str(ADB),'-s','emulator-5554','shell','input','swipe',str(x),str(y),str(x),str(y),'900']);time.sleep(.4);shot('ui-0.1.18-process-pressed.png');hold.wait(timeout=3)
 wait(lambda n:any(x.attrib.get('content-desc')=='展开过程' for x in n.iter('node')))

def library_marks():
 tree=ET.fromstring(adb('shell','su','0','cat','/data/user/0/dev.threadbridge/shared_prefs/conversation_library.xml'))
 return {key:{x.text for x in tree.find(f"set[@name='{key}']")} for key in ('pinned','favorites')}
def icon_interactions():
 before=library_marks()
 with closing(sqlite3.connect(DB)) as db:tid=db.execute("SELECT id FROM threads WHERE native='ui017-short'").fetchone()[0]
 assert all(tid not in values for values in before.values()),'test transcript needs unmarked baseline'
 n=home();tap(node(n,desc='更多选项'));n=wait(lambda n:all(t in texts(n) for t in ['收藏对话','连接状态','设置']));shot('ui-0.1.18-menu-icons.png');adb('shell','input','keyevent','4')
 n=home();tap(node(n,text='Downward short'),long=True);n=wait(lambda n:'置顶对话' in texts(n));shot('ui-0.1.18-action-icons.png');tap(node(n,text='置顶对话'))
 n=wait(lambda n:'置顶对话' not in texts(n));tap(node(n,text='Downward short'),long=True);n=wait(lambda n:'收藏对话' in texts(n));tap(node(n,text='收藏对话'))
 n=wait(lambda n:'收藏对话' not in texts(n));assert any(x.attrib.get('content-desc')=='已置顶' for x in n.iter('node')) and any(x.attrib.get('content-desc')=='已收藏' for x in n.iter('node'));shot('ui-0.1.18-selected-icons.png')
 tap(node(n,text='Downward short'),long=True);n=wait(lambda n:'取消收藏' in texts(n));assert '取消置顶' in texts(n);shot('ui-0.1.18-selected-action-icons.png');tap(node(n,text='取消收藏'));n=wait(lambda n:'取消收藏' not in texts(n));tap(node(n,text='Downward short'),long=True);n=wait(lambda n:'取消置顶' in texts(n));tap(node(n,text='取消置顶'));wait(lambda n:'取消置顶' not in texts(n))
 n=home();tap(node(n,desc='搜索对话标题'));n=wait(lambda n:any(x.attrib.get('class')=='android.widget.EditText' for x in n.iter('node')));shot('ui-0.1.18-search-active-icon.png');tap(node(n,desc='搜索对话标题'));home()
 assert library_marks()==before,'icon QA must restore exactly the original local marks'
 print('PASS menu/action icon family, pin/favorite selected states and active search feedback; marks restored',flush=True)

def main():
 assert 'versionName=0.1.18-test' in adb('shell','dumpsys','package','dev.threadbridge')
 with closing(sqlite3.connect(DB)) as db:commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
 n=open_short();motion_video(n)
 # Rapid reversal must settle back at the same position without a stale scroll correction.
 n=ui();button=node(n,desc='展开过程');position=bounds(button)[1];x1,y1,x2,y2=bounds(button)
 adb('shell','input','tap',(x1+x2)//2,(y1+y2)//2);adb('shell','input','tap',(x1+x2)//2,(y1+y2)//2)
 n=wait(lambda n:any(x.attrib.get('content-desc')=='展开过程' for x in n.iter('node')));assert abs(bounds(node(n,desc='展开过程'))[1]-position)<=6
 print('PASS interrupted disclosure settles closed at its original control position',flush=True)
 # Native animation duration scale=0 must keep disclosure and gestures functional.
 adb('shell','settings','put','global','animator_duration_scale','0');n=open_short();tap(node(n,desc='展开过程'));n=wait(lambda n:'最终回复 short' in texts(n) and any(x.attrib.get('content-desc')=='收起过程' for x in n.iter('node')));shot('ui-0.1.18-reduced-motion.png');tap(node(n,desc='收起过程'));wait(lambda n:any(x.attrib.get('content-desc')=='展开过程' for x in n.iter('node')));adb('shell','settings','put','global','animator_duration_scale','1')
 print('PASS system animations disabled: reveal/collapse remains functional',flush=True)
 # Do not hide controls on compact phones, landscape, large text or tablets.
 modes=[('small','1125x2436','480','1.0',False),('large-text','1200x2670','480','2.0',True),('landscape','2436x1125','480','1.0',False),('tablet','2400x1800','320','1.0',False),('tablet-portrait','1800x2400','320','1.0',False)]
 for name,size,density,font,dark in modes:
  adb('shell','wm','size',size);adb('shell','wm','density',density);adb('shell','settings','put','system','font_scale',font);adb('shell','cmd','uimode','night','yes' if dark else 'no')
  n=home();shot('ui-0.1.18-'+name+'-home.png');tap(node(n,desc='新建对话'));n=wait(lambda n:'新建对话' in texts(n));assert '取消' in texts(n) and '建立' in texts(n);shot('ui-0.1.18-'+name+'-dialog.png');tap(node(n,text='取消'))
  n=open_short();button=node(n,desc='展开过程');tap(button);n=wait(lambda n:any(x.attrib.get('content-desc')=='收起过程' for x in n.iter('node')));shot('ui-0.1.18-'+name+'-chat.png');tap(node(n,desc='收起过程'))
  print('PASS '+name+': home/dialog/chat controls reachable; no send',flush=True)
 adb('shell','wm','size','1200x2670');adb('shell','wm','density','480');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no')
 n=open_short()
 def composer(n):return next(x for x in n.iter('node') if x.attrib.get('resource-id','').endswith('conversation_composer'))
 def height(n):b=bounds(composer(n));return b[3]-b[1]
 assert abs(height(n)-156)<=3,('compact composer',height(n))
 field=next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText');tap(field);draft='polish_draft_check';adb('shell','input','text',draft)
 n=wait(lambda n:draft in texts(n));assert height(n)>=300
 parents={child:parent for parent in n.iter() for child in parent};send_node=node(n,desc='发送')
 while send_node.attrib.get('clickable')!='true':send_node=parents[send_node]
 send=bounds(send_node);assert send[2]-send[0]>=144 and send[3]-send[1]>=144;shot('ui-0.1.18-keyboard.png')
 adb('shell','input','keyevent','4');n=wait(lambda n:height(n)<=159);assert draft in texts(n)
 n=open_short();assert draft in texts(n),('draft restoration',texts(n))
 tap(next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText'));adb('shell','input','keyevent','123');adb('shell','input','keyevent',*(['67']*len(draft)));adb('shell','input','keyevent','4');home()
 print('PASS 52dp/100dp composer, 48dp send target and draft survives keyboard dismissal/restart; no send',flush=True)
 adb('shell','input','keyevent','61');n=ui();assert any(x.attrib.get('focused')=='true' and x.attrib.get('package')=='dev.threadbridge' for x in n.iter('node'));shot('ui-0.1.18-keyboard-focus.png');home()
 print('PASS keyboard navigation focus remains visible',flush=True)
 icon_interactions()
 with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
if __name__=='__main__':
 try:main()
 finally:
  adb('shell','wm','size','1200x2670');adb('shell','wm','density','480');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no');adb('shell','settings','put','global','animator_duration_scale','1')
