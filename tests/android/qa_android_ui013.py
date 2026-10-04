"""Refresh, pagination, jump timing and composer QA on the dedicated synthetic Hub.

Requires signed 0.1.13 paired to emulator-5554 / local 8798 Hub. No sends.
"""
from contextlib import closing
import hashlib
import json
import os
import runpy
import signal
import sqlite3
import subprocess
import time
import uuid
from pathlib import Path

q = runpy.run_path(str(Path(__file__).with_name('qa_android_ui012.py')))
ROOT, call, ui, wait, tap, find, texts, shot, edit, bounds, seed, open_chat, composer, dimensions, compact, expanded = (
    q[k] for k in ('ROOT', 'call', 'ui', 'wait', 'tap', 'find', 'texts', 'shot', 'edit', 'bounds', 'seed', 'open_chat', 'composer', 'dimensions', 'compact', 'expanded'))
DB = ROOT / 'local/ui-test-018/hub.sqlite'
TARGET_VERSION = os.environ.get('THREADBRIDGE_QA_VERSION', '0.1.13-test')
HOME_TITLE = os.environ.get('THREADBRIDGE_QA_HOME_TITLE', '你的设备')
original_shot=shot
def shot(name):original_shot(name.replace('0.1.13',TARGET_VERSION.removesuffix('-test')))

def has_jump(n):
    return any(x.attrib.get('content-desc') == '回到最新消息' for x in n.iter('node'))

def drag_older():
    call('shell', 'input', 'swipe', '600', '500', '600', '2150', '420')

def refresh_chat():
    call('shell', 'input', 'swipe', '600', '1950', '600', '800', '650')

def fixture_db():
    db = sqlite3.connect(DB, timeout=5)
    assert {r[0] for r in db.execute("SELECT id FROM devices WHERE role='agent'")} == {'ui-fixture-host','fixture-lerrem','fixture-nix','fixture-windows'}
    return db

def history_fixture():
    native = 'pull-history-' + uuid.uuid4().hex
    tid = hashlib.sha256(('ui-fixture-host\0default\0'+native).encode()).hexdigest()
    now = int(time.time())
    with closing(fixture_db()) as db, db:
        db.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,'ui-fixture-host',native,'边缘历史测试','completed','pull-turn',now,1))
        for i in range(65):
            text = f'分页记录 {i:02d}：这是用于验证边缘加载的合成消息。' + (' 最后一段' if i==64 else '')
            db.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(tid,f'pull-{i:02d}','pull-turn','assistant',text,hashlib.sha256(text.encode()).hexdigest(),now*1000+i))
    return tid

def main():
    assert 'versionName='+TARGET_VERSION in call('shell','dumpsys','package','dev.threadbridge')
    call('shell','cmd','uimode','night','no');call('shell','settings','put','system','font_scale','1.0')
    tid, visual_tid, commands = seed()
    n = open_chat(tid);n=wait(compact);base=bounds(composer(n))
    assert not any(x in texts(n) for x in ['草稿已保存在手机','正在恢复草稿'])
    tap(edit(n));wait(expanded)
    call('shell','input','text','draft_kept');call('shell','input','keyevent','4')
    wait(compact);n=open_chat(tid);assert 'draft_kept' in edit(n).attrib['text']
    tap(find(n,desc='更多选项'));n=wait(lambda n:'连接状态' in texts(n))
    assert not any(x in texts(n) for x in ['同步对话','查看更早消息','回到最新消息','从电脑加载历史'])
    call('shell','input','keyevent','4')
    print('PASS draft persistence and simplified chat menu',flush=True)
    n=open_chat(visual_tid);drag_older();n=wait(has_jump)
    shot('ui-0.1.13-gradients.png')
    time.sleep(3.2);assert not has_jump(ui()), 'Button must disappear after 3 seconds idle'
    drag_older();n=wait(has_jump)
    # Pause only the isolated fixture process to prove navigation is local.
    pids=subprocess.check_output(['pgrep','-f',r'^/home/echova/code/ThreadBridge/target/release/threadbridge hub --db /home/echova/code/ThreadBridge/local/ui-test-018/hub.sqlite --listen 127.0.0.1:8798$'],text=True).split()
    assert len(pids)==1
    pid=int(pids[0]);os.kill(pid,signal.SIGSTOP)
    try:
        start=time.monotonic();tap(find(n,desc='回到最新消息'))
        n=wait(lambda n:'最后一段' in texts(n) and not has_jump(n),seconds=5)
        elapsed=time.monotonic()-start
        assert elapsed<5, 'Jump must not await the 20 second network timeout'
    finally:os.kill(pid,signal.SIGCONT)
    print('PASS idle hiding and immediate jump with Hub paused',flush=True)
    # No events emitted: only a user refresh can retrieve this changed message.
    with closing(fixture_db()) as db, db:
        body=db.execute("SELECT body FROM messages WHERE thread=? AND id='overlay-assistant'",(visual_tid,)).fetchone()[0]
        body += '\n\n底部手势同步已更新'
        db.execute("UPDATE messages SET body=?,version=? WHERE thread=? AND id='overlay-assistant'",(body,hashlib.sha256(body.encode()).hexdigest(),visual_tid))
    refresh_chat();wait(lambda n:'底部手势同步已更新' in texts(n))
    print('PASS bottom edge refresh fetches messages without push event',flush=True)
    if not os.environ.get('THREADBRIDGE_QA_CHAT_ONLY'):
        call('shell','input','keyevent','4');n=wait(lambda n:HOME_TITLE in texts(n))
        with closing(fixture_db()) as db, db:
            db.execute('UPDATE threads SET title=? WHERE id=?',('主页下拉已同步',visual_tid))
        call('shell','input','swipe','600','650','600','1800','650')
        wait(lambda n:'主页下拉已同步' in texts(n))
        shot('ui-0.1.13-home-refresh.png')
        print('PASS home pull refresh retrieves changed title without push event',flush=True)
    history_tid=history_fixture();n=open_chat(history_tid)
    # Each page has 30 small messages. Pull beyond the top loads further pages.
    for _ in range(14):
        drag_older();time.sleep(.2)
        n=ui()
        if '分页记录 00' in texts(n):break
    assert '分页记录 00' in texts(n),'Top edge must load both older pages'
    shot('ui-0.1.13-history.png')
    print('PASS top edge history pagination reaches oldest message',flush=True)
    n=open_chat(visual_tid);n=wait(compact);base=bounds(composer(n));shot('ui-0.1.13-compact.png')
    os.kill(pid,signal.SIGSTOP)
    try:
        refresh_chat();n=wait(lambda n:'暂时无法完成' in texts(n),seconds=40)
        tap(find(n,text='知道了'));n=wait(compact)
        assert bounds(composer(n))==base,'Disconnect must not move the composer'
        assert '草稿已保存在手机' not in texts(n)
        shot('ui-0.1.13-offline.png')
    finally:os.kill(pid,signal.SIGCONT)
    print('PASS offline composer keeps identical geometry',flush=True)
    tap(edit(n));wait(expanded);shot('ui-0.1.13-expanded.png');call('shell','input','keyevent','4');wait(compact)
    call('shell','cmd','uimode','night','yes');time.sleep(1);shot('ui-0.1.13-dark.png')
    call('shell','cmd','uimode','night','no')
    with closing(fixture_db()) as db:assert db.execute('SELECT count(*) FROM commands').fetchone()[0]==commands
    report=dict(passed=True,apk_version=TARGET_VERSION,physical_phone=False,real_codex_send=False,jump_observed_with_ui_dump_seconds=round(elapsed,2),checks=['persistent draft and no footer hint','offline composer same geometry','simplified chat menu','3 second jump idle hiding','jump while network paused','bottom pull sync without events','top pull paginates 65 messages','52dp/100dp composer expansion','light/dark screenshots','no messages submitted'])
    (ROOT/('artifacts/ui-'+TARGET_VERSION.removesuffix('-test')+'-chat-verification.json')).write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    if not os.environ.get('THREADBRIDGE_QA_CHAT_ONLY'):report['checks'].append('home pull sync without events')
    print(json.dumps(report,ensure_ascii=False,indent=2))

if __name__=='__main__':main()
