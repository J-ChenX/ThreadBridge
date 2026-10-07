"""Project expansion and local retention in signed APK, synthetic emulator/Hub only."""
from qa_android_ui015 import *
import tempfile

PRIVATE='/data/user/0/dev.threadbridge'
OWNER='fixture-windows'
def prefs(name):
 return ET.fromstring(adb('shell','su','0','cat',f'{PRIVATE}/shared_prefs/{name}.xml'))
def put(root,name,kind,value):
 for old in list(root):
  if old.attrib.get('name')==name:root.remove(old)
 n=ET.SubElement(root,kind,{'name':name})
 if kind=='string':n.text=str(value)
 else:n.set('value',str(value))
def save_prefs(name,root):
 path=OUT/f'qa019-{name}.xml';path.write_bytes(ET.tostring(root,encoding='utf-8',xml_declaration=True))
 adb('push',path,'/sdcard/qa019-prefs.xml');adb('shell','su','0','cp','/sdcard/qa019-prefs.xml',f'{PRIVATE}/shared_prefs/{name}.xml')
 adb('shell','su','0','rm','-f',f'{PRIVATE}/shared_prefs/{name}.xml.bak');path.unlink()
def snapshot():
 adb('shell','am','force-stop','dev.threadbridge')
 directory=Path(tempfile.mkdtemp(prefix='qa019-',dir=ROOT/'local'))
 for suffix in ('','-wal','-shm'):
  data=subprocess.check_output([str(ADB),'-s','emulator-5554','exec-out','su','0','cat',f'{PRIVATE}/databases/threadbridge.sqlite{suffix}'])
  (directory/f'threadbridge.sqlite{suffix}').write_bytes(data)
 return sqlite3.connect(directory/'threadbridge.sqlite')
def find(text):
 n=ui()
 for _ in range(12):
  if text in texts(n):return n
  adb('shell','input','swipe',600,2100,600,750,350);n=ui()
 raise AssertionError('Not found '+text+': '+str(texts(n)))
def main():
 assert 'versionName=0.1.19-test' in adb('shell','dumpsys','package','dev.threadbridge')
 adb('shell','am','force-stop','dev.threadbridge')
 now=int(time.time());recent=now+7200;old=now-8*86400;rows=[]
 with closing(sqlite3.connect(DB)) as db,db:

  for project in ('A','B'):
   path='/qa/019/'+project;db.execute('INSERT OR REPLACE INTO host_projects VALUES(?,?)',(OWNER,path));db.execute('INSERT OR REPLACE INTO project_names VALUES(?,?,?)',(OWNER,path,'Folder '+project))
   for index in range(1,7):
    title=f'Folder {project} {index}' if index<6 else ('Retention old' if project=='A' else 'Legacy old')
    tid=hashlib.sha256((OWNER+'\0default\0'+title).encode()).hexdigest();updated=(now-600 if project=='B' and index==1 else recent+(100 if project=='A' else 0)+index) if index<6 else old
    db.execute('INSERT OR REPLACE INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,OWNER,title,title,'completed','fixture',updated,1));db.execute('INSERT OR REPLACE INTO thread_projects VALUES(?,?)',(tid,path));db.execute('INSERT OR REPLACE INTO thread_message_state VALUES(?,1,?)',(tid,updated*1000))
    body='Restored body '+title;db.execute('INSERT OR REPLACE INTO messages VALUES(?,?,?,?,?,?,?)',(tid,'notify:final','turn','assistant',body,hashlib.sha256(body.encode()).hexdigest(),updated*1000))
    rows.append((tid,title,updated))
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
 reads=prefs('conversation_reads');library=prefs('conversation_library')
 for tid,title,updated in rows:put(reads,tid,'string',f'{updated}:fixture:'+('0' if title=='Folder B 1' else '1'))
 for element in list(library):
  if element.attrib.get('name') in {'expired:'+r[0] for r in rows}:library.remove(element)
 legacy=next(row for row in rows if row[1]=='Legacy old');retention=next(row for row in rows if row[1]=='Retention old')


 put(library,'cleanup_at','long',int(time.time()*1000))
 save_prefs('conversation_reads',reads);save_prefs('conversation_library',library)
 n=home();refresh();n=wait(lambda n:'Folder A 5' in texts(n))
 assert all(f'Folder A {i}' in texts(n) for i in (3,4,5));assert all(f'Folder A {i}' not in texts(n) for i in (1,2))
 assert '展开全部 · 另 3 个对话' in texts(n)
 shot('ui-0.1.19-project-preview.png')
 tap(node(n,text='展开全部 · 另 3 个对话'));n=wait(lambda n:'Folder A 1' in texts(n));assert 'Folder A 2' in texts(n)
 shot('ui-0.1.19-project-expanded.png')
 n=find('收起到最近与未读');tap(node(n,text='收起到最近与未读'));n=wait(lambda n:'Folder A 1' not in texts(n))
 tap(node(n,desc='折叠项目 Folder A'));n=find('Folder B 1');assert all(f'Folder B {i}' in texts(n) for i in (1,3,4,5));assert 'Folder B 2' not in texts(n)
 tap(node(n,desc='折叠项目 Folder B'));n=wait(lambda n:'Folder B 5' not in texts(n));tap(node(n,desc='展开项目 Folder B'));n=wait(lambda n:'Folder B 5' in texts(n))
 tap(node(n,text='Folder B 1'));n=wait(lambda n:'Restored body Folder B 1' in texts(n));tap(node(n,desc='打开对话列表'));n=wait(lambda n:'主页' in texts(n));tap(node(n,text='主页'));n=wait(lambda n:'设备列表' in texts(n));n=find('Folder B 5');assert 'Folder B 1' not in texts(n)
 with closing(snapshot()) as cache:
  recent_read=next(r[0] for r in rows if r[1]=='Folder B 1');assert cache.execute('SELECT title FROM threads WHERE id=?',(recent_read,)).fetchone()==('Folder B 1',)
 n=home();tap(node(n,desc='折叠项目 Folder A'));n=find('展开全部 · 另 3 个对话');tap(node(n,text='展开全部 · 另 3 个对话'));n=wait(lambda n:'Folder B 1' in texts(n));assert 'Folder B 2' in texts(n)
 print('PASS ten-minute-old unread becomes read, remains cached and accessible via project show-all',flush=True)
 n=find('Legacy old');shot('ui-0.1.19-all-read.png')
 tap(node(n,text='Legacy old'));n=wait(lambda n:'Restored body Legacy old' in texts(n))
 tap(node(n,desc='打开对话列表'));n=wait(lambda n:'主页' in texts(n));assert 'Folder B 2' in texts(n) or 'Legacy old' in texts(n);shot('ui-0.1.19-drawer-expanded.png');tap(node(n,text='主页'));n=wait(lambda n:'设备列表' in texts(n))
 with closing(snapshot()) as cache:
  assert cache.execute('SELECT count(*) FROM threads WHERE id IN ('+','.join('?' for _ in rows)+')',tuple(r[0] for r in rows)).fetchone()[0]==12
  assert cache.execute('SELECT count(*) FROM messages WHERE threadId=?',(legacy[0],)).fetchone()[0]==1
 assert not any(e.attrib.get('name')=='expired:'+legacy[0] for e in prefs('conversation_library'))
 assert next(e.text for e in prefs('conversation_reads') if e.attrib.get('name')==legacy[0])==f'{old}:fixture:1'
 print('PASS older read messages stay in Room and show-all; shared drawer expansion',flush=True)
 # Read an old remote body through the app, then make local cleanup due.
 n=home();tap(node(n,desc='搜索对话标题'));n=ui();field=next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText');tap(field);adb('shell','input','text','Retention');adb('shell','input','keyevent','4');n=wait(lambda n:'Retention old' in texts(n));tap(node(n,text='Retention old'));n=wait(lambda n:'Restored body Retention old' in texts(n))
 with closing(snapshot()) as cache:assert cache.execute('SELECT count(*) FROM messages WHERE threadId=?',(retention[0],)).fetchone()[0]==1
 library=prefs('conversation_library');put(library,'cleanup_at','long',0);save_prefs('conversation_library',library)
 n=home();refresh();time.sleep(2)
 with closing(snapshot()) as cache:
  assert cache.execute('SELECT title FROM threads WHERE id=?',(retention[0],)).fetchone() is None
  assert cache.execute('SELECT count(*) FROM messages WHERE threadId=?',(retention[0],)).fetchone()[0]==0
  assert cache.execute('SELECT count(*) FROM chunks WHERE threadId=?',(retention[0],)).fetchone()[0]==0
 assert any(e.attrib.get('name')=='expired:'+retention[0] for e in prefs('conversation_library'))
 n=home();refresh();time.sleep(2)
 with closing(snapshot()) as cache:assert cache.execute('SELECT title FROM threads WHERE id=?',(retention[0],)).fetchone() is None
 n=home();tap(node(n,desc='搜索对话标题'));n=ui();tap(next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText'));adb('shell','input','text','Retention');adb('shell','input','keyevent','4');n=wait(lambda n:'Retention old' in texts(n));tap(node(n,text='Retention old'));n=wait(lambda n:'Restored body Retention old' in texts(n));shot('ui-0.1.19-retention-restored.png')
 with closing(sqlite3.connect(DB)) as db:
  assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
  assert db.execute('SELECT body FROM messages WHERE thread=?',(retention[0],)).fetchone()[0]=='Restored body Retention old'
  assert db.execute('SELECT count(*) FROM threads WHERE id IN ('+','.join('?' for _ in rows)+')',tuple(r[0] for r in rows)).fetchone()[0]==12
 print('PASS seven-day cleanup removes local copy, unchanged sync does not refill, explicit search reloads body; Hub data untouched, no commands',flush=True)
 home()
if __name__=='__main__':main()
