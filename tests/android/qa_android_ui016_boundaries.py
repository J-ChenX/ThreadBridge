"""Cross project/device header transitions using only synthetic UI fixtures."""
from qa_android_ui015 import *
import os
QA_PREFIX='ui-'+os.environ.get('THREADBRIDGE_QA_VERSION','0.1.16-test').removesuffix('-test')
def main():
 now=int(time.time())+180
 with closing(sqlite3.connect(DB)) as db,db:
  # Keep project A and its transcript ahead of newly seeded project B even
  # when this check runs several minutes after the transcript fixture.
  for tid,native in db.execute("SELECT id,native FROM threads WHERE native LIKE 'ui016-%' AND native NOT LIKE 'ui016-boundary-%'").fetchall():
   updated=now+300+int(native.removeprefix('ui016-'))
   db.execute('UPDATE threads SET updated=? WHERE id=?',(updated,tid));db.execute('UPDATE thread_message_state SET activity_at=? WHERE thread=?',(updated*1000,tid))
  for tid, in db.execute("SELECT id FROM threads WHERE native LIKE 'ui016-boundary-%'").fetchall():
   for table in ('messages','thread_projects','thread_message_state'):db.execute(f'DELETE FROM {table} WHERE thread=?',(tid,))
   db.execute('DELETE FROM threads WHERE id=?',(tid,))
  for host,project,name,prefix,delta in [('fixture-windows','/qa/project-b','第二项目','B',0),('fixture-nix','/qa/project-c','另一设备项目','C',-60)]:
   db.execute('INSERT OR REPLACE INTO host_projects VALUES(?,?)',(host,project));db.execute('INSERT OR REPLACE INTO project_names VALUES(?,?,?)',(host,project,name))
   for i in range(14):
    native=f'ui016-boundary-{prefix}-{i:02}';tid=hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest()
    db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,host,native,f'Boundary {prefix} {i:02}','completed','fixture',now+delta+i,1));db.execute('INSERT INTO thread_projects VALUES(?,?)',(tid,project));db.execute('INSERT INTO thread_message_state VALUES(?,1,?)',(tid,(now+delta+i)*1000))
  commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
 def check(drawer=False):
  n=home();refresh();n=wait(lambda n:'UI16 Transcript' in texts(n))
  if drawer:
   tap(node(n,text='UI16 Transcript'));n=wait(lambda n:'明确的最终回复' in texts(n));tap(node(n,desc='打开对话列表'));n=wait(lambda n:'主页' in texts(n))
  seen=set();transitions=[]
  for _ in range(28):
   nodes=list(n.iter('node'));pinned=[x for x in nodes if x.attrib.get('content-desc','').startswith('固定项目 ')]
   devices=[x for x in nodes if x.attrib.get('content-desc','').startswith('折叠设备 ')]
   if pinned:
    assert len(pinned)==1,[x.attrib for x in pinned]
    label=pinned[0].attrib['content-desc'].removeprefix('固定项目 ')
    device_node=min(devices,key=lambda x:int(re.findall(r'\d+',x.attrib['bounds'])[1]));device=device_node.attrib['content-desc'].removeprefix('折叠设备 ')
    parents={child:parent for parent in n.iter() for child in parent};device_row=device_node
    while device_row.attrib.get('clickable')!='true':device_row=parents[device_row]
    project_top=int(re.findall(r'\d+',pinned[0].attrib['bounds'])[1]);device_bottom=int(re.findall(r'\d+',device_row.attrib['bounds'])[3])
    assert project_top>=device_bottom-3,('fixed project overlaps device',project_top,device_bottom)
    expected={'我的项目':'Windows','第二项目':'Windows','另一设备项目':'nix'}
    if label in expected:
     assert device==expected[label],(label,device);seen.add(label);transitions.append((device,label))
     if label!='我的项目':shot(QA_PREFIX+'-'+('drawer' if drawer else 'home')+'-boundary-'+('b' if label=='第二项目' else 'c')+'.png')
   if {'第二项目','另一设备项目'}<=seen:break
   x=350 if drawer else 600;adb('shell','input','swipe',x,1600,x,1200,350);n=ui()
  assert {'我的项目','第二项目','另一设备项目'}<=seen,(seen,transitions,texts(n))
  print('PASS '+('drawer' if drawer else 'home')+' project A→B and device Windows→nix fixed headers switch with their own project',flush=True)
 check();check(True)
 with closing(sqlite3.connect(DB)) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
 home()
if __name__=='__main__':main()
