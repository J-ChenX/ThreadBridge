"""Actual signed APK, dedicated emulator and synthetic loopback Hub only."""
from contextlib import closing
from pathlib import Path
import hashlib,json,re,sqlite3,subprocess,time,uuid,xml.etree.ElementTree as ET
ROOT=Path(__file__).resolve().parents[2];ADB=ROOT/'local/android-sdk/platform-tools/adb';DB=ROOT/'local/ui-test-018/hub.sqlite';OUT=ROOT/'artifacts'
def adb(*args):return subprocess.check_output([str(ADB),'-s','emulator-5554',*map(str,args)],timeout=15,text=True)
def ui():
 adb('shell','uiautomator','dump','/sdcard/ui015.xml')
 return ET.fromstring(adb('shell','cat','/sdcard/ui015.xml'))
def texts(tree):return [n.attrib.get('text','') for n in tree.iter('node')]
def node(tree,text=None,desc=None):
 return next(n for n in tree.iter('node') if (text is None or n.attrib.get('text')==text) and (desc is None or n.attrib.get('content-desc')==desc))
def tap(n,long=False):
 x1,y1,x2,y2=map(int,re.findall(r'\d+',n.attrib['bounds']));x=(x1+x2)//2;y=(y1+y2)//2
 if long:adb('shell','input','swipe',x,y,x,y,800)
 else:adb('shell','input','tap',x,y)
def wait(fn,seconds=35):
 end=time.monotonic()+seconds
 while time.monotonic()<end:
  n=ui()
  if fn(n):return n
  time.sleep(.3)
 raise AssertionError('UI condition timed out: '+str(texts(n)))
def shot(name):
 data=subprocess.check_output([str(ADB),'-s','emulator-5554','exec-out','screencap','-p'],timeout=15);(OUT/name).write_bytes(data)
def home():
 adb('shell','am','force-stop','dev.threadbridge');adb('shell','am','start','-n','dev.threadbridge/.MainActivity');return wait(lambda n:'设备列表' in texts(n))
def refresh():adb('shell','input','swipe',600,450,600,1700,500)
def main():
 assert 'versionName=0.1.15-test' in adb('shell','dumpsys','package','dev.threadbridge')
 now=int(time.time())+120;rows=[]
 with closing(sqlite3.connect(DB)) as db,db:
  assert {r[0] for r in db.execute("SELECT id FROM devices WHERE role='agent'")}=={'ui-fixture-host','fixture-lerrem','fixture-nix','fixture-windows'}
  for old, in db.execute("SELECT id FROM threads WHERE native LIKE 'ui015-%'").fetchall():
   for table in ('messages','thread_projects','thread_message_state'):db.execute(f'DELETE FROM {table} WHERE thread=?',(old,))
   db.execute('DELETE FROM threads WHERE id=?',(old,))
  for i in range(1,9):
   native='ui015-'+uuid.uuid4().hex;tid=hashlib.sha256(('fixture-windows\0default\0'+native).encode()).hexdigest();title=f'QA Conversation {i:02d}';project=['','/qa/alpha','/qa/beta'][i%3]
   db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,'fixture-windows',native,title,'completed','qa-turn',now+i,1));db.execute('INSERT INTO thread_projects VALUES(?,?)',(tid,project));db.execute('INSERT INTO thread_message_state VALUES(?,0,?)',(tid,(now+i)*1000))
   body='可见助手回复：标题、项目和标准时间戳。';db.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(tid,'qa-message','qa-turn','assistant',body,hashlib.sha256(body.encode()).hexdigest(),(now+i)*1000));rows.append((tid,title,project))
  db.execute("INSERT OR REPLACE INTO host_projects VALUES('fixture-nix','/qa/empty-project')")
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
 adb('shell','am','force-stop','dev.threadbridge');adb('shell','su','0','rm','-f','/data/user/0/dev.threadbridge/shared_prefs/conversation_reads.xml','/data/user/0/dev.threadbridge/shared_prefs/conversation_reads.xml.bak')
 adb('shell','cmd','uimode','night','no');adb('shell','settings','put','system','font_scale','1.0')
 n=home();refresh();n=wait(lambda n:'QA Conversation 08' in texts(n));assert sorted(t for t in texts(n) if t.startswith('QA Conversation'))==['QA Conversation 06','QA Conversation 07','QA Conversation 08'];assert 'alpha' in texts(n) and 'beta' in texts(n) and '其他' in texts(n)
 shot('ui-0.1.15-home.png');print('PASS recent-three union baseline and project hierarchy',flush=True)
 tap(node(n,desc='折叠项目 alpha'));n=wait(lambda n:'QA Conversation 07' not in texts(n));tap(node(n,desc='展开项目 alpha'));n=wait(lambda n:'QA Conversation 07' in texts(n))
 with closing(sqlite3.connect(DB)) as db,db:db.execute('UPDATE thread_message_state SET revision=1 WHERE thread=?',(rows[0][0],))
 refresh();n=wait(lambda n:'QA Conversation 01' in texts(n));assert len([t for t in texts(n) if t.startswith('QA Conversation')])==4
 print('PASS older unread added without removing recent three',flush=True)
 tap(node(n,desc='搜索对话标题'));n=ui();field=next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText');tap(field);adb('shell','input','text','QA05');adb('shell','input','keyevent','4');n=wait(lambda n:'QA Conversation 05' in texts(n));assert 'QA Conversation 08' not in texts(n)
 shot('ui-0.1.15-search.png');tap(node(n,text='QA Conversation 05'),long=True);n=wait(lambda n:'置顶对话' in texts(n));tap(node(n,text='置顶对话'));n=wait(lambda n:'置顶对话' not in texts(n));tap(node(n,text='QA Conversation 05'),long=True);n=wait(lambda n:'收藏对话' in texts(n));tap(node(n,text='收藏对话'));n=wait(lambda n:'收藏对话' not in texts(n));tap(node(n,desc='清除搜索'));n=wait(lambda n:'QA Conversation 08' in texts(n));tap(node(n,desc='搜索对话标题'))
 n=home();refresh();n=wait(lambda n:'QA Conversation 05' in texts(n));assert [t for t in texts(n) if t.startswith('QA Conversation')][0]=='QA Conversation 05'
 tap(node(n,desc='更多选项'));n=wait(lambda n:'收藏对话' in texts(n));tap(node(n,text='收藏对话'));n=wait(lambda n:'QA Conversation 05' in texts(n));shot('ui-0.1.15-favorites.png');tap(node(n,text='QA Conversation 05'));n=wait(lambda n:any(re.match(r'\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}',t) for t in texts(n)));assert not any(x.attrib.get('content-desc')=='更多选项' for x in n.iter('node'));assert '连接状态' not in texts(n);shot('ui-0.1.15-chat.png')
 print('PASS fuzzy search beyond preview, pin/favorite persistence, timestamp and chat menu removal',flush=True)
 tap(node(n,desc='打开对话列表'));n=wait(lambda n:'主页' in texts(n));assert '续桥' not in texts(n);assert '连接与设置' not in texts(n);assert 'beta' in texts(n);shot('ui-0.1.15-drawer.png');tap(node(n,text='主页'));n=wait(lambda n:'设备列表' in texts(n))
 tap(node(n,desc='新建对话'));n=wait(lambda n:'新建对话' in texts(n));assert '设备' in texts(n) and '项目' in texts(n);shot('ui-0.1.15-new-dialog.png');tap(node(n,text='echova'));n=ui();tap(node(n,text='nix'));n=ui();tap(node(n,text='其他'));n=wait(lambda n:'empty-project' in texts(n));tap(node(n,text='empty-project'));n=ui();tap(node(n,text='建立'));n=wait(lambda n:'开始新对话' in texts(n));shot('ui-0.1.15-blank.png')
 field=next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText');tap(field);adb('shell','input','text','first_phone_test');n=ui();tap(node(n,desc='发送'));n=wait(lambda n:'模拟器中的测试回复' in '\n'.join(texts(n)),seconds=45);shot('ui-0.1.15-created-chat.png')
 print('PASS blank first message adopts native thread and receives reply through real APK and synthetic Hub',flush=True)
 adb('shell','input','keyevent','4');n=wait(lambda n:'设备列表' in texts(n));tap(node(n,desc='更多选项'));n=wait(lambda n:'连接状态' in texts(n));tap(node(n,text='连接状态'));n=wait(lambda n:'已配对电脑：4 台' in '\n'.join(texts(n)));adb('shell','input','keyevent','4')
 print('PASS drawer hierarchy, no brand header, device/project blank dialog and home-only connection',flush=True)
 adb('shell','cmd','uimode','night','yes');n=home();shot('ui-0.1.15-dark.png');adb('shell','settings','put','system','font_scale','1.5');n=home();shot('ui-0.1.15-large.png');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no');home()
 with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands+1
 print('PASS dark/large-font screenshots; only one explicit synthetic creation command dispatched',flush=True)
if __name__=='__main__':main()
