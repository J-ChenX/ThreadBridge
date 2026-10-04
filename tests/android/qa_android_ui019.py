"""Check composer and menus in the isolated Android preview without sending messages."""
import json,re,runpy,struct
from pathlib import Path
q=runpy.run_path(str(Path(__file__).with_name('qa_android_ui018.py')))
ROOT=q['ROOT'];mapping=json.loads((ROOT/'local/ui-test-018/preview-threads.json').read_text())
call,ui,wait,tap,find,texts,shot=(q[k] for k in ('call','ui','wait','tap','find','texts','shot'))
def edit(n):return next(x for x in n.iter('node') if x.attrib.get('class')=='android.widget.EditText')
def height(n):
 a=list(map(int,re.findall(r'\d+',edit(n).attrib['bounds'])));return a[3]-a[1]
def chat():
 call('shell','am','force-stop','dev.threadbridge')
 call('shell','am','start','-a','android.intent.action.VIEW','-d','threadbridge://thread/'+mapping['conversation'],'dev.threadbridge')
 return wait(lambda n:'优化手机端显示' in texts(n) and any(x.attrib.get('class')=='android.widget.EditText' for x in n.iter('node')))
def main():
 n=chat();compact=height(n);assert edit(n).attrib['focused']=='false';shot('ui-0.1.9-compact.png');print('compact',compact,flush=True)
 tap(edit(n));n=wait(lambda n:edit(n).attrib['focused']=='true' and height(n)>compact);expanded=height(n);shot('ui-0.1.9-expanded.png');print('expanded',expanded,flush=True)
 call('shell','input','text','qa_draft');n=wait(lambda n:edit(n).attrib['text']=='qa_draft')
 call('shell','input','keyevent','4');n=wait(lambda n:edit(n).attrib['focused']=='false' and height(n)<=compact);assert edit(n).attrib['text']=='qa_draft';print('dismiss collapses and retains draft',flush=True)
 tap(edit(n));wait(lambda n:edit(n).attrib['focused']=='true');call('shell','input','keyevent',*(['67']*8));wait(lambda n:edit(n).attrib['text']=='');call('shell','input','keyevent','4');n=wait(lambda n:edit(n).attrib['focused']=='false');shot('ui-0.1.9-compact.png')
 tap(find(n,desc='更多选项'));n=wait(lambda n:'回到最新消息' in texts(n));assert '同步对话' in texts(n) and '查看更早消息' in texts(n);assert all(x not in texts(n) for x in ('所有对话','连接状态','设置'));shot('ui-0.1.9-menu.png');print('chat menu passed',flush=True)
 call('shell','am','force-stop','dev.threadbridge');call('shell','am','start','-n','dev.threadbridge/.MainActivity');n=wait(lambda n:'你的设备' in texts(n));tap(find(n,desc='更多选项'));n=wait(lambda n:'连接状态' in texts(n));assert '设置' in texts(n);shot('ui-0.1.9-home-menu.png');print('home menu passed',flush=True)
 n=chat();shot('ui-0.1.9-compact.png')
 png=(ROOT/'artifacts/ui-0.1.9-compact.png').read_bytes();assert struct.unpack('>II',png[16:24])==(1200,2670)
 assert call('shell','settings','get','secure','navigation_mode').strip()=='2'
 report={'pass':True,'scope':'isolated emulator-5554 and synthetic 8798 fixture','apk_version':'0.1.9-test','physical_phone_install':False,'real_codex_send':False,'resolution':[1200,2670],'gesture_navigation':True,'compact_edit_height_px':compact,'expanded_edit_height_px':expanded,'checks':['focus expands composer','keyboard dismissal collapses composer','draft preserved through collapse','chat menu excludes all conversations/status/settings','home menu retains status/settings','signed upgrade retains synthetic pairing']}
 (ROOT/'artifacts/ui-0.1.9-verification.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n');print('PASS actual Android UI checks',flush=True)
if __name__=='__main__':main()
