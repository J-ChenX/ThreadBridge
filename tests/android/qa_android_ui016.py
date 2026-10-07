"""Signed APK against synthetic loopback fixture; no native Codex sends."""
from qa_android_ui015 import *
import os
QA_VERSION=os.environ.get("THREADBRIDGE_QA_VERSION","0.1.16-test")
QA_PREFIX="ui-"+QA_VERSION.removesuffix("-test")
def main():
 assert ('versionName='+QA_VERSION) in adb('shell','dumpsys','package','dev.threadbridge')
 now=int(time.time())+300;owner='fixture-windows';rows=[]
 with closing(sqlite3.connect(DB)) as db,db:
  for tid, in db.execute("SELECT id FROM threads WHERE native LIKE 'ui016-%'").fetchall():
   for table in ('messages','thread_projects','thread_message_state'):db.execute(f'DELETE FROM {table} WHERE thread=?',(tid,))
   db.execute('DELETE FROM threads WHERE id=?',(tid,))
  db.execute("INSERT OR REPLACE INTO host_projects VALUES(?,?)",(owner,'/qa/project-a'));db.execute('INSERT OR REPLACE INTO project_names VALUES(?,?,?)',(owner,'/qa/project-a','我的项目'))
  for i in range(18):
   native='ui016-'+str(i);tid=hashlib.sha256((owner+'\0default\0'+native).encode()).hexdigest();title='UI16 Transcript' if i==17 else f'UI16 Header {i:02d}'
   db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,owner,native,title,'completed','ui16-final',now+i,1));db.execute('INSERT INTO thread_projects VALUES(?,?)',(tid,'/qa/project-a'));db.execute('INSERT INTO thread_message_state VALUES(?,0,?)',(tid,(now+i)*1000));rows.append(tid)
  base=1700000000000
  for index,(mid,role,text) in enumerate([('user1','user','连续提问一'),('user2','user','连续提问二'),('native:process1','assistant','第一条过程说明'),('native:process2','assistant','第二条过程说明'),('notify:final','assistant','明确的最终回复')]):
   db.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(rows[-1],mid,'turn',role,text,hashlib.sha256(text.encode()).hexdigest(),base+index*1000))
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
 n=home();refresh();n=wait(lambda n:'UI16 Transcript' in texts(n));assert '我的项目' in texts(n)
 with closing(sqlite3.connect(DB)) as db,db:db.execute('UPDATE thread_message_state SET revision=1 WHERE thread in (%s)'%','.join('?' for _ in rows),rows)
 refresh();n=wait(lambda n:'UI16 Header 10' in texts(n))
 adb('shell','input','swipe',600,1800,600,700,550);n=ui();assert any(x.attrib.get('content-desc')=='固定项目 我的项目' for x in n.iter('node'));assert 'Windows' in texts(n);shot(QA_PREFIX+'-project-sticky.png')
 print('PASS registered project label and simultaneous device/project sticky header',flush=True)
 n=home();tap(node(n,text='UI16 Transcript'));n=wait(lambda n:'明确的最终回复' in texts(n));assert '第一条过程说明' not in texts(n)
 stamps=[t for t in texts(n) if re.match(r'\d{4}-\d{2}-\d{2}',t)];assert len(stamps)==2 and all(' — ' in t for t in stamps),stamps;shot(QA_PREFIX+'-process-collapsed.png')
 tap(node(n,desc='展开过程'));n=wait(lambda n:'第一条过程说明' in texts(n));assert '第二条过程说明' in texts(n);shot(QA_PREFIX+'-process-expanded.png');tap(node(n,desc='收起过程'));n=wait(lambda n:'第一条过程说明' not in texts(n))
 print('PASS process disclosure keeps final visible; consecutive roles share two time ranges',flush=True)
 tap(node(n,desc='打开对话列表'));n=wait(lambda n:'主页' in texts(n));adb('shell','input','swipe',350,1800,350,700,550);n=ui();assert any(x.attrib.get('content-desc')=='固定项目 我的项目' for x in n.iter('node'));shot(QA_PREFIX+'-drawer-sticky.png');tap(node(n,text='主页'));n=wait(lambda n:'设备列表' in texts(n))
 tap(node(n,desc='新建对话'));n=wait(lambda n:'新建对话' in texts(n));shot(QA_PREFIX+'-new-dialog.png');tap(node(n,text='echova'));n=wait(lambda n:'选择设备' in texts(n));assert 'Windows' in texts(n);shot(QA_PREFIX+'-device-picker.png');tap(node(n,text='Windows'));n=wait(lambda n:'新建对话' in texts(n));tap(node(n,text='其他'));n=wait(lambda n:'选择项目' in texts(n));assert '我的项目' in texts(n);shot(QA_PREFIX+'-project-picker.png');tap(node(n,text='我的项目'));n=wait(lambda n:'新建对话' in texts(n));assert '我的项目' in texts(n);tap(node(n,text='取消'))
 print('PASS drawer project pinning and themed in-dialog device/project selection',flush=True)
 adb('shell','cmd','uimode','night','yes');n=home();tap(node(n,desc='新建对话'));n=wait(lambda n:'新建对话' in texts(n));shot(QA_PREFIX+'-new-dark.png');tap(node(n,text='取消'));adb('shell','cmd','uimode','night','no');adb('shell','settings','put','system','font_scale','1.5');n=home();tap(node(n,desc='新建对话'));n=wait(lambda n:'新建对话' in texts(n));shot(QA_PREFIX+'-new-large.png');tap(node(n,text='取消'));adb('shell','settings','put','system','font_scale','1.0');home()
 with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
 print('PASS dark/large font; no command submitted',flush=True)
if __name__=='__main__':main()
