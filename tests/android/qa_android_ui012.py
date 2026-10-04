"""Composer animation and inset scrims on emulator-5554 and synthetic Hub 8798.

Install and pair the signed 0.1.12 APK first. Never submits a message.
"""
import json
import runpy
import sqlite3
import time
from pathlib import Path

shared = runpy.run_path(str(Path(__file__).with_name('qa_android_ui010.py')))
ROOT, call, ui, wait, tap, find, texts, shot, edit, bounds, seed, open_chat = (
    shared[k] for k in ('ROOT', 'call', 'ui', 'wait', 'tap', 'find', 'texts', 'shot', 'edit', 'bounds', 'seed', 'open_chat')
)


def composer(n):
    return next(x for x in n.iter('node') if x.attrib.get('resource-id', '').endswith('conversation_composer'))


def dimensions(n):
    b = bounds(composer(n))
    return b[2] - b[0], b[3] - b[1]


def compact(n):
    return edit(n).attrib['focused'] == 'false' and dimensions(n) == (1008, 156)


def expanded(n):
    w, h = dimensions(n)
    return edit(n).attrib['focused'] == 'true' and w == 1128 and h >= 300


def main():
    assert 'versionName=0.1.12-test' in call('shell', 'dumpsys', 'package', 'dev.threadbridge')
    call('shell', 'cmd', 'uimode', 'night', 'no')
    call('shell', 'settings', 'put', 'system', 'font_scale', '1.0')
    tid, visual_tid, commands = seed()
    open_chat(tid)
    n = wait(compact)
    closed_bounds = bounds(composer(n))
    assert 2670 - closed_bounds[3] >= 96, 'At least 32dp clearance below the collapsed composer'
    shot('ui-0.1.12-compact.png')
    tap(edit(n))
    n = wait(expanded)
    assert dimensions(n) == (1128, 300), 'Empty focus must expand in width and height'
    shot('ui-0.1.12-expanded.png')
    print('PASS compact 52dp/32dp margins; focused 100dp/12dp margins', flush=True)
    for index, line in enumerate(['first_line', 'second_line', 'third_line', 'fourth_line', 'fifth_line']):
        if index: call('shell', 'input', 'keyevent', '66')
        call('shell', 'input', 'text', line)
    n = wait(lambda n: 'fifth_line' in edit(n).attrib['text'] and dimensions(n)[1] > 300)
    draft = edit(n).attrib['text']
    shot('ui-0.1.12-multiline.png')
    call('shell', 'input', 'keyevent', '4')
    n = wait(compact)
    assert edit(n).attrib['text'] == draft.replace('\\n', ' ').replace('\n', ' ')
    shot('ui-0.1.12-dismissed.png')
    open_chat(tid)
    n = wait(compact)
    tap(edit(n))
    n = wait(expanded)
    assert edit(n).attrib['text'] == draft
    tap(find(n, desc='更多选项'))
    wait(lambda n: '回到最新消息' in texts(n))
    call('shell', 'input', 'keyevent', '4')
    n = wait(compact)
    print('PASS multiline growth, dismissal, menu collapse and persistent draft', flush=True)
    open_chat(visual_tid)
    n = wait(compact)
    call('shell', 'dumpsys', 'gfxinfo', 'dev.threadbridge', 'reset')
    for _ in range(3):
        tap(edit(n)); time.sleep(.7)
        call('shell', 'input', 'keyevent', '4'); time.sleep(.7)
    (ROOT / 'artifacts/ui-0.1.12-keyboard-gfx.txt').write_text(call('shell', 'dumpsys', 'gfxinfo', 'dev.threadbridge'))
    call('shell', 'input', 'swipe', '600', '600', '600', '1500', '450')
    n = wait(lambda n: any(x.attrib.get('content-desc') == '回到最新消息' for x in n.iter('node')))
    shot('ui-0.1.12-gradients.png')
    call('shell', 'cmd', 'uimode', 'night', 'yes'); time.sleep(1)
    shot('ui-0.1.12-dark.png')
    call('shell', 'settings', 'put', 'system', 'font_scale', '1.5')
    n = open_chat(visual_tid)
    assert bounds(composer(n))[2] <= 1200 and bounds(composer(n))[3] < 2670
    tap(edit(n)); n = wait(expanded)
    shot('ui-0.1.12-large-font.png')
    call('shell', 'input', 'keyevent', '4')
    call('shell', 'settings', 'put', 'system', 'font_scale', '1.0')
    call('shell', 'cmd', 'uimode', 'night', 'no')
    open_chat(visual_tid); n = wait(compact)
    with sqlite3.connect(ROOT / 'local/ui-test-018/hub.sqlite') as db:
        assert db.execute('SELECT count(*) FROM commands').fetchone()[0] == commands
    report = dict(passed=True, apk_version='0.1.12-test', physical_phone=False,
                  real_codex_send=False, compact_surface_px=[1008, 156], expanded_surface_px=[1128, 300],
                  compact_bottom_clearance_px=2670-closed_bounds[3],
                  checks=['focus expands width and height', 'dismissal collapses both', '32dp bottom clearance',
                          'multiline growth and persistent draft', 'menu collapse', 'long reply jump',
                          'light/dark scrim screenshots', 'large font expansion', 'no messages submitted'])
    (ROOT / 'artifacts/ui-0.1.12-verification.json').write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n')
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
