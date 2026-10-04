"""Cold keyboard and queued draft restoration with the synthetic Hub paused."""
import hashlib
import json
import os
import runpy
import signal
import subprocess
import time
from pathlib import Path

q=runpy.run_path(str(Path(__file__).with_name('qa_android_ui014.py')))
c=q['q']
ROOT,call,wait,tap,edit,expanded,compact,shot=(c[k] for k in ('ROOT','call','wait','tap','edit','expanded','compact','shot'))
visual=hashlib.sha256(b'ui-fixture-host\0default\0preview-ime-overlay-layout').hexdigest()

pid=int(subprocess.check_output(['pgrep','-f',r'^/home/echova/code/ThreadBridge/target/release/threadbridge hub --db /home/echova/code/ThreadBridge/local/ui-test-018/hub.sqlite --listen 127.0.0.1:8798$'],text=True).strip())
call('install','-r',str(ROOT/'artifacts/ThreadBridge-0.1.14.apk'))
os.kill(pid,signal.SIGSTOP)
try:
 start=time.monotonic();n=c['open_chat'](visual)
 assert edit(n).attrib['enabled']=='true', 'A cold local draft must not wait behind network sync'
 assert edit(n).attrib['text']=='', 'Untouched visual fixture required'
 tap(edit(n));n=wait(expanded,seconds=6)
 elapsed=time.monotonic()-start
 assert elapsed<8
 call('shell','input','text','offline_draft');call('shell','input','keyevent','4');wait(compact)
 call('shell','input','keyevent','4');wait(lambda n:'设备列表' in c['texts'](n))
 call('shell','am','start','-a','android.intent.action.VIEW','-d','threadbridge://thread/'+visual,'dev.threadbridge')
 n=wait(lambda n:any(x.attrib.get('class')=='android.widget.EditText' for x in n.iter('node')))
 assert edit(n).attrib['text']=='offline_draft', 'Queued draft must restore without waiting for its write'
finally:os.kill(pid,signal.SIGCONT)
time.sleep(1.5);n=c['open_chat'](visual)
assert edit(n).attrib['text']=='offline_draft', 'Queued draft must persist after reconnect'
tap(edit(n));n=wait(expanded)
call('shell','input','keyevent','123')
call('shell','input','keyevent',*(['67']*len('offline_draft')))
wait(lambda n:edit(n).attrib['text']=='')
call('shell','input','keyevent','4');wait(compact)
time.sleep(.5)
report=dict(passed=True,version='0.1.14-test',physical_phone=False,real_codex_send=False,cold_keyboard_with_ui_dump_seconds=round(elapsed,2),checks=['cold draft/keyboard does not await network','queued draft restores locally','queued draft persists after reconnect','no commands submitted'])
(ROOT/'artifacts/ui-0.1.14-draft-verification.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
print(json.dumps(report,ensure_ascii=False,indent=2))
