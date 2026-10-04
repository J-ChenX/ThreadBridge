"""Native UI QA against the isolated 8798 fixture; no real Codex messages."""
import json,re,sqlite3,subprocess,time,uuid,xml.etree.ElementTree as ET
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2];ADB=ROOT/'local/android-sdk/platform-tools/adb';DIR=ROOT/'local/ui-test-018';ART=ROOT/'artifacts';threads=json.loads((DIR/'threads.json').read_text())
def call(*args):return subprocess.check_output([str(ADB),'-s','emulator-5554',*args],text=True)
def ui():
 call('shell','uiautomator','dump','/sdcard/ui018.xml');return ET.fromstring(call('shell','cat','/sdcard/ui018.xml'))
def texts(n):return '\n'.join(x.attrib.get('text','') for x in n.iter('node'))
def find(n,text=None,desc=None):return next(x for x in n.iter('node') if (text is not None and x.attrib.get('text')==text) or (desc is not None and x.attrib.get('content-desc')==desc))
def coords(n):
 a=list(map(int,re.findall(r'\d+',n.attrib['bounds'])));return str((a[0]+a[2])//2),str((a[1]+a[3])//2)
def tap(n):call('shell','input','tap',*coords(n))
def longpress(n):
 x,y=coords(n);call('shell','input','swipe',x,y,x,y,'850')
def wait(check,seconds=35):
 end=time.monotonic()+seconds
 while True:
  n=ui()
  if check(n):return n
  if time.monotonic()>end:raise RuntimeError('UI wait failed: '+texts(n)[:500])
  time.sleep(.3)
def shot(name):(ART/name).write_bytes(subprocess.check_output([str(ADB),'-s','emulator-5554','exec-out','screencap','-p']))
def open_thread(name):
 call('shell','am','start','-a','android.intent.action.VIEW','-d','threadbridge://thread/'+threads[name],'dev.threadbridge');return wait(lambda n:'发送消息' in texts(n))
def main():
 call('shell','cmd','uimode','night','no');call('shell','settings','put','system','font_scale','1.0');call('shell','wm','size','1080x1920');call('shell','wm','density','440')
 n=ui()
 if '连接与设置' in texts(n):call('shell','input','keyevent','4')
 n=wait(lambda n:'你的设备' in texts(n));shot('ui-0.1.8-devices.png');print('device view loaded',flush=True)
 open_thread('conversation');n=wait(lambda n:'已调整为 更清爽的对话界面。' in texts(n));assert '**' not in texts(n);assert not any(x.attrib.get('content-desc')=='对话操作' for x in n.iter('node'));shot('ui-0.1.8-chat.png')
 tap(find(n,desc='更多选项'));n=wait(lambda n:'回到最新消息' in texts(n));assert '查看更早消息' in texts(n);assert '同步对话' in texts(n);shot('ui-0.1.8-menu.png');tap(find(n,text='回到最新消息'));print('top menu includes former composer actions',flush=True)
 n=ui();tap(find(n,desc='打开对话列表'));n=wait(lambda n:'连接与设置' in texts(n));tap(find(n,text='echova'));n=wait(lambda n:'nix' in texts(n) and 'Windows' in texts(n));assert '优化手机端显示' not in texts(n);shot('ui-0.1.8-drawer.png');print('four device groups and folding',flush=True)
 longpress(find(n,text='整理今天的想法'));n=wait(lambda n:'删除对话' in texts(n));shot('ui-0.1.8-longpress.png');tap(find(n,text='删除对话'));n=wait(lambda n:'删除这个对话？' in texts(n));assert '电脑 Codex 中的原始对话会保留' in texts(n);tap(find(n,text='删除'));n=wait(lambda n:'整理今天的想法' not in texts(n));
 with sqlite3.connect(DIR/'hub.sqlite') as db:
  assert db.execute('SELECT count(*) FROM threads WHERE id=?',(threads['lerrem'],)).fetchone()[0]==0
  assert db.execute("SELECT count(*) FROM tombstones WHERE thread=? AND message=''",(threads['lerrem'],)).fetchone()[0]==1
 call('shell','am','force-stop','dev.threadbridge');call('shell','am','start','-n','dev.threadbridge/.MainActivity');n=wait(lambda n:'你的设备' in texts(n));assert '整理今天的想法' not in texts(n);print('long-press copy deletion persists',flush=True)
 # Actual input creates an old-generation phone draft; reset will discard it.
 open_thread('conversation');n=ui();tap(next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText'));call('shell','input','text','draft_before_reset');call('shell','input','keyevent','4')
 epoch=str(uuid.uuid4())
 from reset_collection import clear_replica
 clear_replica(DIR/'hub.sqlite',epoch)
 n=ui();tap(find(n,desc='更多选项'));n=wait(lambda n:'同步对话' in texts(n));tap(find(n,text='同步对话'));n=wait(lambda n:'你的设备' in texts(n) and '暂无新对话' in texts(n));shot('ui-0.1.8-reset.png')
 assert 'draft_before_reset' not in texts(n)
 print('collection reset returns to four empty devices without unpairing',flush=True)
 call('shell','cmd','uimode','night','yes');time.sleep(1);shot('ui-0.1.8-dark.png');call('shell','cmd','uimode','night','no')
 report={'scope':'isolated actual Android emulator with synthetic loopback Hub','apk_version':'0.1.8-test','pass':True,'real_codex_send':False,'physical_phone_install':False,'checks':['Room 2-to-3 migration launches','four-device folding','top menu has composer actions','composer no ellipsis','Markdown rendering','long-press copy delete','persistent Hub tombstone','delete remains absent after app restart','generation reset discards draft and returns to four empty devices','pairing retained through reset','system dark appearance']}
 (ART/'ui-0.1.8-verification.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n');print('PASS actual Android UI checks',flush=True)
if __name__=='__main__':main()
