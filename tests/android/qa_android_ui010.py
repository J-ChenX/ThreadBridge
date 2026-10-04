"""Keyboard/composer and overlay regression on emulator-5554 and the synthetic 8798 Hub.

Requires the signed 0.1.10 APK already installed and paired to this fixture.
Never submits a message or connects to a real Codex conversation.
"""
from contextlib import closing
import hashlib
import json
import re
import runpy
import sqlite3
import time
import uuid
from pathlib import Path

q = runpy.run_path(str(Path(__file__).with_name('qa_android_ui018.py')))
ROOT = q['ROOT']
call, ui, wait, tap, find, texts, shot = (q[k] for k in ('call', 'ui', 'wait', 'tap', 'find', 'texts', 'shot'))
TITLE = '键盘与渐隐回归'


def edit(n):
    return next(x for x in n.iter('node') if x.attrib.get('class') == 'android.widget.EditText')


def bounds(node):
    return list(map(int, re.findall(r'\d+', node.attrib['bounds'])))


def height(n):
    b = bounds(edit(n))
    return b[3] - b[1]


def seed():
    host = 'ui-fixture-host'
    native = 'preview-ime-overlay-' + uuid.uuid4().hex
    tid = hashlib.sha256((host + '\0default\0' + native).encode()).hexdigest()
    visual_native = 'preview-ime-overlay-layout'
    visual_tid = hashlib.sha256((host + '\0default\0' + visual_native).encode()).hexdigest()
    now = int(time.time())
    body = '\n\n'.join(
        f'**阅读段落 {i}**\n\n正文会从浮动按钮下面经过，上下边缘逐渐淡出。'
        '这段内容用于验证滚动和键盘动画，消息始终可以完整阅读。'
        for i in range(1, 19)
    ) + '\n\n最后一段：输入框不会遮住最新回复。'
    with closing(sqlite3.connect(ROOT / 'local/ui-test-018/hub.sqlite', timeout=5)) as db, db:
        assert {r[0] for r in db.execute("SELECT id FROM devices WHERE role='agent'")} == {
            host, 'fixture-lerrem', 'fixture-nix', 'fixture-windows'
        }, 'Dedicated synthetic Hub required'
        count = db.execute('SELECT count(*) FROM commands').fetchone()[0]
        for current, source in [(tid, native), (visual_tid, visual_native)]:
            db.execute('INSERT OR REPLACE INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',
                       (current, host, source, TITLE, 'completed', 'overlay-turn', now, 1))
            for role, text, offset in [('user', '检查长正文、键盘和上下渐隐。', 0), ('assistant', body, 1)]:
                db.execute('INSERT OR REPLACE INTO messages VALUES(?,?,?,?,?,?,?)',
                           (current, 'overlay-' + role, 'overlay-turn', role, text,
                            hashlib.sha256(text.encode()).hexdigest(), now * 1000 + offset))
            db.execute("INSERT INTO events(kind,thread,created) VALUES('thread',?,?)", (current, now))
    return tid, visual_tid, count


def open_chat(tid):
    call('shell', 'am', 'force-stop', 'dev.threadbridge')
    call('shell', 'am', 'start', '-a', 'android.intent.action.VIEW', '-d',
         'threadbridge://thread/' + tid, 'dev.threadbridge')
    return wait(lambda n: any(x.attrib.get('class') == 'android.widget.EditText' for x in n.iter('node'))
                and '最后一段' in texts(n))


def main():
    assert 'versionName=0.1.10-test' in call('shell', 'dumpsys', 'package', 'dev.threadbridge')
    call('shell', 'cmd', 'uimode', 'night', 'no')
    call('shell', 'settings', 'put', 'system', 'font_scale', '1.0')
    tid, visual_tid, commands = seed()
    n = open_chat(tid)
    compact = height(n)
    assert edit(n).attrib['focused'] == 'false'
    assert bounds(edit(n))[0] >= 144 and bounds(edit(n))[2] <= 960, 'Composer must retain the wider side margins'
    for desc in ['打开对话列表', '更多选项']:
        b = bounds(find(n, desc=desc))
        assert b[2] - b[0] == b[3] - b[1], (desc, b)
    shot('ui-0.1.10-latest.png')
    tap(edit(n))
    n = wait(lambda n: edit(n).attrib['focused'] == 'true')
    assert height(n) == compact, 'Empty focus must not force a taller composer'
    print('PASS empty focus stays compact', flush=True)
    shot('ui-0.1.10-keyboard.png')
    call('shell', 'input', 'text', 'first_line')
    call('shell', 'input', 'keyevent', '66')
    call('shell', 'input', 'text', 'second_line')
    call('shell', 'input', 'keyevent', '66')
    call('shell', 'input', 'text', 'third_line')
    # Semantics bounds include Android's 48dp minimum touch target even when
    # the field is visually shorter. Three lines exceed that touch target.
    n = wait(lambda n: 'third_line' in edit(n).attrib['text'] and height(n) > compact)
    draft = edit(n).attrib['text']
    assert '\n' in draft.replace('\\n', '\n')
    expanded = height(n)
    shot('ui-0.1.10-multiline.png')
    call('shell', 'input', 'keyevent', '4')
    n = wait(lambda n: edit(n).attrib['focused'] == 'false' and height(n) == compact)
    preview = draft.replace('\\n', ' ').replace('\n', ' ')
    assert edit(n).attrib['text'] == preview, 'Collapsed draft must display a single-line preview'
    n = open_chat(tid)
    assert edit(n).attrib['text'] == preview
    tap(edit(n))
    n = wait(lambda n: edit(n).attrib['focused'] == 'true' and height(n) == expanded)
    assert edit(n).attrib['text'] == draft, 'Original multiline draft must survive process restart'
    print('PASS multiline growth, dismissal and persistent draft', flush=True)
    tap(find(n, desc='更多选项'))
    n = wait(lambda n: TITLE in texts(n) and '回到最新消息' in texts(n))
    shot('ui-0.1.10-menu.png')
    call('shell', 'input', 'keyevent', '4')
    n = wait(lambda n: edit(n).attrib['focused'] == 'false' and height(n) == compact)
    print('PASS menu title and composer dismissal', flush=True)
    # Use a second synthetic conversation for visual checks, retaining the
    # first conversation's draft as part of the persistence regression.
    n = open_chat(visual_tid)
    call('shell', 'dumpsys', 'gfxinfo', 'dev.threadbridge', 'reset')
    for _ in range(3):
        tap(edit(n)); time.sleep(.7)
        call('shell', 'input', 'keyevent', '4'); time.sleep(.7)
    (ROOT / 'artifacts/ui-0.1.10-keyboard-gfx.txt').write_text(call('shell', 'dumpsys', 'gfxinfo', 'dev.threadbridge'))
    call('shell', 'input', 'swipe', '600', '600', '600', '1500', '450')
    n = wait(lambda n: any(x.attrib.get('content-desc') == '回到最新消息' for x in n.iter('node')))
    shot('ui-0.1.10-gradients.png')
    call('shell', 'cmd', 'uimode', 'night', 'yes'); time.sleep(1)
    shot('ui-0.1.10-dark.png')
    call('shell', 'settings', 'put', 'system', 'font_scale', '1.5')
    n = open_chat(visual_tid)
    assert bounds(edit(n))[3] < 2670
    assert bounds(find(n, desc='更多选项'))[2] <= 1200
    shot('ui-0.1.10-large-font.png')
    print('PASS light/dark gradients and large font layout', flush=True)
    call('shell', 'settings', 'put', 'system', 'font_scale', '1.0')
    call('shell', 'cmd', 'uimode', 'night', 'no')
    n = open_chat(visual_tid)
    tap(find(n, desc='打开对话列表'))
    wait(lambda n: '连接与设置' in texts(n))
    call('shell', 'input', 'keyevent', '4')
    wait(lambda n: '连接与设置' not in texts(n))
    call('shell', 'input', 'keyevent', '4')
    n = wait(lambda n: '你的设备' in texts(n))
    tap(find(n, desc='更多选项'))
    n = wait(lambda n: '连接状态' in texts(n) and '设置' in texts(n))
    shot('ui-0.1.10-home-menu.png')
    n = open_chat(visual_tid)
    with closing(sqlite3.connect(ROOT / 'local/ui-test-018/hub.sqlite')) as db:
        assert db.execute('SELECT count(*) FROM commands').fetchone()[0] == commands
    report = dict(passed=True, apk_version='0.1.10-test', physical_phone=False, real_codex_send=False,
                  compact_edit_height_px=compact, multiline_edit_height_px=expanded,
                  checks=['empty focus stays compact', 'multiline grows naturally', 'IME dismissal clears focus',
                          'draft persists through dismissal and process restart', 'menu dismisses keyboard',
                          'circular floating controls', 'title retained in menu', 'scroll jump within long reply',
                          'light/dark gradients captured', 'large font layout', 'drawer/home menu retained',
                          'no messages submitted'])
    (ROOT / 'artifacts/ui-0.1.10-verification.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
