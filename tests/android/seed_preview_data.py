"""Seed future-looking synthetic dialogs in the dedicated loopback UI fixture only."""
import hashlib,json,sqlite3,time,uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2];DB=ROOT/'local/ui-test-018/hub.sqlite'
def key(host,native):return hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest()
def seed():
 now=int(time.time());mapping={}
 with sqlite3.connect(DB) as db:
  actual={r[0] for r in db.execute("SELECT id FROM devices WHERE role='agent'")}
  assert actual=={'ui-fixture-host','fixture-lerrem','fixture-nix','fixture-windows'}
  for name,host,title in [('conversation','ui-fixture-host','优化手机端显示'),('lerrem','fixture-lerrem','整理今天的想法'),('nix','fixture-nix','检查服务运行'),('windows','fixture-windows','继续我的项目')]:
   native='preview-'+name;tid=key(host,native);mapping[name]=tid;turn='preview-turn-'+name
   db.execute('INSERT OR REPLACE INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,host,native,title,'completed',turn,now,1))
   question='让手机端更清爽一些。' if name=='conversation' else '在这里继续今天的对话。'
   answer='已调整为 **更清爽的对话界面**。\n\n- 左侧按四台设备分类\n- 长按对话可以删除副本\n- 操作统一放在右上角\n\n你可以直接截图，指出想调整的位置。' if name=='conversation' else '这是电脑预览中的演示对话。\n\n点击左上角，可以切换到其他设备。'
   for index,(role,text) in enumerate([('user',question),('assistant',answer)]):db.execute('INSERT OR REPLACE INTO messages VALUES(?,?,?,?,?,?,?)',(tid,'preview-'+role,turn,role,text,hashlib.sha256(text.encode()).hexdigest(),now*1000+index))
   db.execute("INSERT INTO events(kind,thread,created) VALUES('thread',?,?)",(tid,now))
 (ROOT/'local/ui-test-018/preview-threads.json').write_text(json.dumps(mapping));return mapping
if __name__=='__main__':seed();print('Synthetic preview dialogs ready; real Hub untouched.')
