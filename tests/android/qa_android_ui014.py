"""Home sticky groups, five-item preview and persistent local unread state.

Dedicated emulator-5554 and synthetic localhost Hub only; never submits commands.
"""
from contextlib import closing
import hashlib
import json
import runpy
import time
import uuid
from pathlib import Path

q=runpy.run_path(str(Path(__file__).with_name('qa_android_ui013.py')))
ROOT,call,ui,wait,tap,find,texts,shot,bounds=(q[k] for k in ('ROOT','call','ui','wait','tap','find','texts','shot','bounds'))
DB=q['DB'];fixture_db=q['fixture_db']

def seed_home():
    now=int(time.time())+100;rows=[]
    with closing(fixture_db()) as db,db:
        commands=db.execute('SELECT count(*) FROM commands').fetchone()[0]
        old=[r[0] for r in db.execute("SELECT id FROM threads WHERE native LIKE 'home014-%'")]
        for tid in old:
            db.execute("DELETE FROM messages WHERE thread=?",(tid,))
            db.execute("DELETE FROM threads WHERE id=?",(tid,))
        # The new tracker creates its first baseline when the upgraded app syncs.
        for i in range(12):
            native='home014-'+uuid.uuid4().hex
            tid=hashlib.sha256(('fixture-windows\0default\0'+native).encode()).hexdigest()
            title=f'主页样例{i+1:02d}'
            db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,'fixture-windows',native,title,'completed','home-r1',now+i,1))
            text='最后一段：这是主页已读测试的合成内容。'
            db.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(tid,'home-message','home-r1','assistant',text,hashlib.sha256(text.encode()).hexdigest(),(now+i)*1000))
            rows.append(dict(id=tid,title=title,updated=now+i))
    return rows,commands

def launch_home():
    call('shell','am','force-stop','dev.threadbridge')
    call('shell','am','start','-n','dev.threadbridge/.MainActivity')
    return wait(lambda n:'设备列表' in texts(n) and '主页样例' in texts(n))

def unread_titles(n):
    return [x.attrib.get('content-desc','') for x in n.iter('node') if x.attrib.get('content-desc','').startswith('未读对话 主页样例')]

def expanded_button(n):
    return next(x for x in n.iter('node') if x.attrib.get('text','').startswith('展开更多'))

def main():
    # Seed before the first new-version launch, preserving real pairing/cache.
    call('shell','am','force-stop','dev.threadbridge')
    # Reset only read markers in this dedicated synthetic emulator fixture.
    call('shell','su','0','rm','-f','/data/user/0/dev.threadbridge/shared_prefs/conversation_reads.xml','/data/user/0/dev.threadbridge/shared_prefs/conversation_reads.xml.bak')
    rows,commands=seed_home()
    call('install','-r',str(ROOT/'artifacts/ThreadBridge-0.1.14.apk'))
    assert 'versionName=0.1.14-test' in call('shell','dumpsys','package','dev.threadbridge')
    call('shell','cmd','uimode','night','no');call('shell','settings','put','system','font_scale','1.0')
    n=launch_home();time.sleep(1);n=ui()
    assert texts(n).count('设备列表')==1 and '续桥' not in texts(n)
    titles=[x.attrib['text'] for x in n.iter('node') if x.attrib.get('text','').startswith('主页样例')]
    assert titles==['主页样例12','主页样例11','主页样例10','主页样例09','主页样例08'],titles
    assert not unread_titles(n),'Initial historical snapshot is an already-read baseline'
    shot('ui-0.1.14-home-preview.png')
    print('PASS device list title and five recent preview',flush=True)
    tap(expanded_button(n));n=wait(lambda n:'主页样例01' in texts(n))
    shot('ui-0.1.14-home-expanded.png')
    call('shell','input','swipe','600','1900','600','850','450');n=ui()
    b=bounds(find(n,text='Windows'));assert b[1]<350,('Device name should stick below toolbar',b)
    shot('ui-0.1.14-home-sticky.png')
    print('PASS expanded rows and sticky device header',flush=True)
    # Relaunch resets the saved UI tree for a clear at-top refresh.
    n=launch_home()
    # Seven old rows change without a push event; all seven unread must appear,
    # including the two outside the recent-five preview.
    with closing(fixture_db()) as db,db:
        for r in rows[:7]:
            db.execute('UPDATE threads SET updated=?,revision=? WHERE id=?',(r['updated']+500,'home-r2',r['id']))
    call('shell','input','swipe','600','600','600','1800','650')
    n=wait(lambda n:len(unread_titles(n))==7)
    assert any(x.attrib.get('content-desc')=='未读设备 Windows' for x in n.iter('node'))
    shot('ui-0.1.14-home-unread.png')
    n=launch_home();assert len(unread_titles(n))==7,'Unread flags survive process restart'
    print('PASS all seven unread visible across restart',flush=True)
    tap(find(n,text='主页样例07'));wait(lambda n:any(x.attrib.get('class')=='android.widget.EditText' for x in n.iter('node')))
    time.sleep(.6);call('shell','input','keyevent','4');n=wait(lambda n:'设备列表' in texts(n))
    assert '未读对话 主页样例07' not in unread_titles(n)
    assert len(unread_titles(n))==6
    n=launch_home();assert len(unread_titles(n))==6,'Read flags survive process restart'
    for r in rows[:6]:
        call('shell','am','start','-a','android.intent.action.VIEW','-d','threadbridge://thread/'+r['id'],'dev.threadbridge')
        wait(lambda n:'最后一段' in texts(n));call('shell','input','keyevent','4');wait(lambda n:'设备列表' in texts(n))
    n=ui();assert not unread_titles(n)
    assert not any(x.attrib.get('content-desc')=='未读设备 Windows' for x in n.iter('node'))
    print('PASS 5 recent, 7 unread, read clearing, persistence and sticky header',flush=True)
    # A device previously below receives newer content and moves to the top.
    with closing(fixture_db()) as db,db:
        other=db.execute("SELECT id FROM threads WHERE host='fixture-nix' LIMIT 1").fetchone()[0]
        db.execute('UPDATE threads SET updated=?,revision=? WHERE id=?',(int(time.time())+1000,'nix-home-r2',other))
    call('shell','input','swipe','600','600','600','1800','650')
    n=wait(lambda n:any(x.attrib.get('content-desc')=='未读设备 nix' for x in n.iter('node')))
    assert bounds(find(n,text='nix'))[1]<bounds(find(n,text='Windows'))[1]
    tap(find(n,desc='更多选项'));n=wait(lambda n:'连接状态' in texts(n))
    assert '同步对话' not in texts(n)
    call('shell','input','keyevent','4');shot('ui-0.1.14-home-reordered.png')
    with closing(fixture_db()) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
    report=dict(passed=True,apk_version='0.1.14-test',physical_phone=False,real_codex_send=False,checks=['single device list toolbar title','five recent preview','second-level expansion','device header sticks','seven unread all shown','blue dots read clearing','read/unread survives restart','newest device moves first','home pull sync without push','no sync menu action','no commands submitted'])
    (ROOT/'artifacts/ui-0.1.14-home-verification.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(report,ensure_ascii=False,indent=2))

if __name__=='__main__':main()
