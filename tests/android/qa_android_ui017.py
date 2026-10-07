"""Process disclosure opens downward in the actual signed APK; synthetic Hub only."""
from qa_android_ui015 import *
import os
QA_VERSION=os.environ.get("THREADBRIDGE_QA_VERSION","0.1.17-test")
QA_PREFIX="ui-"+QA_VERSION.removesuffix("-test")
def bounds(n):return list(map(int,re.findall(r'\d+',n.attrib['bounds'])))
def main():
 assert ('versionName='+QA_VERSION) in adb('shell','dumpsys','package','dev.threadbridge')
 owner='fixture-windows';now=int(time.time())+300;cases=[]
 with closing(sqlite3.connect(DB)) as db,db:
  for tid, in db.execute("SELECT id FROM threads WHERE native LIKE 'ui017-%'").fetchall():
   for table in ('messages','thread_projects','thread_message_state'):db.execute(f'DELETE FROM {table} WHERE thread=?',(tid,))
   db.execute('DELETE FROM threads WHERE id=?',(tid,))
  for index,kind in enumerate(['short','long','earlier']):
   native='ui017-'+kind;tid=hashlib.sha256((owner+'\0default\0'+native).encode()).hexdigest();title='Downward '+kind
   db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,owner,native,title,'completed','fixture',now+index,1));db.execute('INSERT INTO thread_projects VALUES(?,?)',(tid,'/qa/project-a'));db.execute('INSERT INTO thread_message_state VALUES(?,1,?)',(tid,(now+index)*1000))
   body='过程开始 '+kind+'\n\n'+('\n\n'.join('过程说明第 '+str(i)+' 段' for i in range(36)) if kind=='long' else '第二段过程说明')
   messages=[('user-'+kind,'user','我的问题 '+kind),('native:process-'+kind,'assistant',body),('notify:final-'+kind,'assistant','最终回复 '+kind)]
   if kind=='earlier':messages += [('user-later','user','后续问题'),('notify:later','assistant','后续回复\n\n'+'\n\n'.join('后续正文 '+str(i) for i in range(16)))]
   for ordinal,(mid,role,text) in enumerate(messages):db.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(tid,mid,'turn',role,text,hashlib.sha256(text.encode()).hexdigest(),(now-120)*1000+ordinal*1000))
   cases.append((kind,title))
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
 for kind,title in cases:
  n=home();refresh();n=ui()
  if title not in texts(n):
   tap(node(n,desc='搜索对话标题'));n=ui();tap(next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText'));adb('shell','input','text',title.replace(' ','%s'));adb('shell','input','keyevent','4')
  n=wait(lambda n:any(x.attrib.get('text')==title and x.attrib.get('class')=='android.widget.TextView' for x in n.iter('node')));tap(next(x for x in n.iter('node') if x.attrib.get('text')==title and x.attrib.get('class')=='android.widget.TextView'));n=wait(lambda n:any(x.attrib.get('content-desc')=='展开过程' for x in n.iter('node')) if kind!='earlier' else '后续回复' in '\n'.join(texts(n)))
  if kind=='earlier':
   for _ in range(12):
    if any(x.attrib.get('content-desc')=='展开过程' and bounds(x)[1]>250 for x in n.iter('node')):break
    adb('shell','input','swipe',600,650,600,1300,350);n=ui()
  button=node(n,desc='展开过程');before=bounds(button)[1];tap(button)
  n=wait(lambda n:any(x.attrib.get('content-desc')=='收起过程' for x in n.iter('node')));after=bounds(node(n,desc='收起过程'))[1]
  assert abs(after-before)<=6,(kind,before,after)
  first=next(x for x in n.iter('node') if x.attrib.get('text','').startswith('过程开始 '+kind));assert bounds(first)[1]>bounds(node(n,desc='收起过程'))[3],(kind,first.attrib)
  if kind=='short':assert '最终回复 short' in texts(n)
  shot(QA_PREFIX+'-process-'+kind+'-expanded.png');tap(node(n,desc='收起过程'));n=wait(lambda n:any(x.attrib.get('content-desc')=='展开过程' for x in n.iter('node')));assert abs(bounds(node(n,desc='展开过程'))[1]-before)<=6,(kind,'collapse drift');assert not any(t.startswith('过程开始 '+kind) for t in texts(n));shot(QA_PREFIX+'-process-'+kind+'-collapsed.png')
  print('PASS '+kind+': fixed disclosure control, content below, collapse restores position',flush=True)
 with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
 home()
if __name__=='__main__':main()
