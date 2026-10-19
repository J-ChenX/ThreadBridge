"""Real APK UI on a dedicated emulator with synthetic HTTP data; sends no commands."""
import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import threading
import time
import urllib.parse
import uuid
import xml.etree.ElementTree as ET
import zlib

ROOT=Path(__file__).resolve().parents[2]
ADB=ROOT/'local/android-sdk/platform-tools/adb'
OUT=ROOT/'artifacts/image-state-ui'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial',default='emulator-5554')
    parser.add_argument('--package',default='dev.threadbridge.debug')
    parser.add_argument('--apk',type=Path,default=ROOT/'android/app/build/outputs/apk/debug/app-debug.apk')
    parser.add_argument('--reset-debug-fixture',action='store_true',help='Reset only the synthetic debug app on this dedicated emulator; never used for upgrade QA')
    args=parser.parse_args()
    assert args.serial.startswith('emulator-')
    OUT.mkdir(parents=True,exist_ok=True)
    def adb(*cmd):return subprocess.check_output([str(ADB),'-s',args.serial,*map(str,cmd)],text=True,timeout=35)
    assert adb('shell','getprop','ro.kernel.qemu').strip()=='1'
    def ui():
        adb('shell','uiautomator','dump','/sdcard/image-state-qa.xml')
        return ET.fromstring(adb('shell','cat','/sdcard/image-state-qa.xml'))
    def texts(tree):return [n.get('text','') for n in tree.iter('node')]
    def descs(tree):return [n.get('content-desc','') for n in tree.iter('node')]
    def node(tree,text=None,desc=None):return next(n for n in tree.iter('node') if (text is None or n.get('text')==text) and (desc is None or n.get('content-desc')==desc))
    def tap(n):
        a,b,c,d=map(int,re.findall(r'\d+',n.get('bounds')));adb('shell','input','tap',(a+c)//2,(b+d)//2)
    def wait(fn,seconds=35):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            tree=ui()
            if fn(tree):return tree
            time.sleep(.2)
        raise AssertionError('UI timeout: '+str(texts(tree))+' '+str(descs(tree)))
    def open_image(tree):
        # Large text in landscape can place the older photo above the viewport.
        # Scroll it clear of the status/header region before tapping its bounds.
        for _ in range(6):
            candidates=[n for n in tree.iter('node') if n.get('content-desc')=='查看图片附件']
            if candidates:
                bounds=list(map(int,re.findall(r'\d+',candidates[0].get('bounds'))))
                if (bounds[1]+bounds[3])//2>=140 and bounds[3]-bounds[1]>=120:
                    tap(candidates[0]);return
            screen=list(map(int,re.findall(r'\d+',next(tree.iter('node')).get('bounds'))))
            width,height=screen[2],screen[3]
            adb('shell','input','swipe',width//2,int(height*.45),width//2,int(height*.72),350)
            tree=ui()
        raise AssertionError('Could not reveal image attachment for tapping')
    def shot(name):
        (OUT/(name+'.xml')).write_bytes(ET.tostring(ui()))
        with (OUT/(name+'.png')).open('wb') as f:subprocess.run([str(ADB),'-s',args.serial,'exec-out','screencap','-p'],stdout=f,check=True,timeout=20)
    def png_chunk(kind,body):return struct.pack('>I',len(body))+kind+body+struct.pack('>I',zlib.crc32(kind+body)&0xffffffff)
    width,height=900,480
    raw=b''.join(b'\0'+b''.join(bytes((40+x*180//width,80+y*100//height,180)) for x in range(width)) for y in range(height))
    photo=b'\x89PNG\r\n\x1a\n'+png_chunk(b'IHDR',struct.pack('>IIBBBBB',width,height,8,2,0,0,0))+png_chunk(b'IDAT',zlib.compress(raw))+png_chunk(b'IEND',b'')
    digest=hashlib.sha256(photo).hexdigest()
    state={'online':True,'task':'running','image_requests':0,'image_enabled':True,'pause_sync':False,'sync_waiting':False,'commands':0,'projection':False}
    collection='image-state-'+uuid.uuid4().hex
    tid='a'*64;mid='input:photo';now=int(time.time())
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_POST(self):
            state['commands']+=1;self.send_error(500)
        def do_GET(self):
            path=urllib.parse.urlsplit(self.path).path
            if self.headers.get('Authorization')!='Bearer synthetic-image-key':self.send_error(401);return
            body={'collection_generation':collection}
            if path=='/v1/events':
                if state['pause_sync']:
                    state['sync_waiting']=True
                    deadline=time.monotonic()+8
                    while state['pause_sync'] and time.monotonic()<deadline:time.sleep(.05)
                body.update(events=[],cursor=1,snapshot_required=False)
            elif path=='/v1/threads':
                body.update(threads=[dict(id=tid,title='Image cache QA',host_id='image-host',status='queue_ready',revision='old-final',updated_at=now,can_send=False,host_online=state['online'],history_cursor=None,message_revision=1,message_activity_at=now*1000,project='/qa/images',project_known=True,task_status=state['task'],task_turn_id='live-turn')],next_offset=None,cursor=1)
            elif path=='/v1/hosts':body['hosts']=[dict(id='image-host',name='Image QA',online=state['online'],projects=['/qa/images'])]
            elif path=='/v1/capture-health':body.update(hosts=[dict(host_id='image-host',checked_at=int(time.time()),status=dict(failures={},overflow=False,**({'projection_error':'visible_history_projection_failed'} if state['projection'] else {})))],server_time=int(time.time()))
            elif path.endswith('/messages'):
                text='![图片](threadbridge-image:'+digest+')\nCached original photo'
                body.update(messages=[dict(id=mid,turn_id='image-turn',role='user',text=text,version='v1',ordinal=now*1000,characters=len(text),timestamp_ms=now*1000),dict(id='reply',turn_id='image-turn',role='assistant',text='Conversation background for blurred preview.',version='v1',ordinal=now*1000+1,characters=43,timestamp_ms=now*1000+1)],next_before=None)
            elif '/images/' in path:
                state['image_requests']+=1
                if not state['image_enabled']:self.send_error(503);return
                body.update(base64=base64.b64encode(photo).decode(),mime='image/png')
            else:self.send_error(404);return
            raw=json.dumps(body).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)));self.end_headers()
            try:self.wfile.write(raw)
            except BrokenPipeError:pass
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    port=server.server_port
    try:
        adb('reverse',f'tcp:{port}',f'tcp:{port}')
        adb('install','-r',args.apk)
        if args.reset_debug_fixture:
            assert args.package=='dev.threadbridge.debug','Only isolated debug fixture data may be reset'
            adb('shell','pm','clear',args.package)
        adb('shell','am','force-stop',args.package)
        adb('shell','am','start','-n',args.package+'/dev.threadbridge.MainActivity')
        tree=wait(lambda n:'连接你的电脑' in texts(n))
        tap(node(tree,text='连接密钥'))
        tree=wait(lambda n:len([x for x in n.iter('node') if x.get('class')=='android.widget.EditText'])>=2)
        fields=[x for x in tree.iter('node') if x.get('class')=='android.widget.EditText']
        for index,value in enumerate([f'http://127.0.0.1:{port}','synthetic-image-key']):
            field=[x for x in ui().iter('node') if x.get('class')=='android.widget.EditText'][index]
            tap(field);current=field.get('text','');adb('shell','input','keyevent','123')
            adb('shell','input','keyevent',*(['67']*(len(current)+8)+['112']*(len(current)+8)))
            adb('shell','input','text',value);adb('shell','input','keyevent','4')
        tree=ui()
        if '局域网测试' not in texts(tree):adb('shell','input','swipe',500,1600,500,700,300);tree=ui()
        switches=[x for x in tree.iter('node') if x.get('checkable')=='true']
        tap(switches[-1]);tap(node(ui(),text='连接'))
        tree=wait(lambda n:'Image cache QA' in texts(n),45)
        assert '对话正在进行' in descs(tree)
        shot('home-running');tap(node(tree,text='Image cache QA'))
        tree=wait(lambda n:'查看图片附件' in descs(n) or 'Cached original photo' in texts(n))
        assert '对话正在进行' in descs(tree)
        shot('chat-running')
        open_image(tree)
        tree=wait(lambda n:'下载图片' in descs(n))
        assert not {'图片','复位','100%','双指缩放 · 双击放大'}.intersection(texts(tree))
        assert '关闭图片预览' not in descs(tree)
        shot('viewer-blur');print('PASS live list/header and immersive blurred viewer',flush=True)
        adb('shell','mkdir','-p','/sdcard/Pictures')
        prior_downloads=set(adb('shell','find','/sdcard/Pictures/ThreadBridge','-type','f').splitlines()) if 'ThreadBridge' in adb('shell','ls','/sdcard/Pictures') else set()
        tap(node(tree,desc='下载图片'));time.sleep(1)
        downloads=adb('shell','find','/sdcard/Pictures/ThreadBridge','-type','f').splitlines()
        created=set(downloads)-prior_downloads;assert len(created)==1,'Expected one newly downloaded file'
        saved=subprocess.check_output([str(ADB),'-s',args.serial,'exec-out','cat',created.pop()],timeout=10)
        assert saved==photo,'Downloaded image must preserve exact original bytes'
        print('PASS download exact original',flush=True)
        adb('shell','input','tap',100,300)
        tree=wait(lambda n:'查看图片附件' in descs(n) or 'Cached original photo' in texts(n))
        assert '下载图片' not in descs(tree),'Blank-space tap did not dismiss'
        assert state['image_requests']==1,state
        # Existing memory image remains immediately usable even while sync is blocked.
        adb('shell','input','keyevent','4');tree=wait(lambda n:'Image cache QA' in texts(n))
        state['pause_sync']=True;state['sync_waiting']=False
        tap(node(tree,text='Image cache QA'))
        deadline=time.monotonic()+5
        while not state['sync_waiting'] and time.monotonic()<deadline:time.sleep(.05)
        assert state['sync_waiting']
        tree=ui();assert '查看图片附件' in descs(tree),'Memory image queued behind network sync'
        state['pause_sync']=False
        assert state['image_requests']==1
        # Process restart must read the verified disk original with image API disabled.
        state['image_enabled']=False
        adb('shell','am','force-stop',args.package);adb('shell','am','start','-n',args.package+'/dev.threadbridge.MainActivity')
        tree=wait(lambda n:'Image cache QA' in texts(n));tap(node(tree,text='Image cache QA'))
        tree=wait(lambda n:'查看图片附件' in descs(n) or 'Cached original photo' in texts(n));open_image(tree)
        tree=wait(lambda n:'下载图片' in descs(n));tap(node(tree,desc='下载图片'));time.sleep(.7)
        assert state['image_requests']==1,'Restart fetched a cached original again'
        print('PASS blank dismissal, memory under blocked sync and disk after process restart with image API disabled',flush=True)
        adb('shell','input','keyevent','4');wait(lambda n:'查看图片附件' in descs(n) or 'Cached original photo' in texts(n))
        adb('shell','input','keyevent','4');wait(lambda n:'Image cache QA' in texts(n))
        state['task']='completed'
        tree=wait(lambda n:'Image cache QA' in texts(n) and '对话正在进行' not in descs(n))
        state['online']=False
        tree=wait(lambda n:'设备离线' in descs(n));shot('home-offline')
        state['projection']=True
        tree=wait(lambda n:'同步状态尚未确认' in texts(n));tap(node(tree,text='查看'))
        tree=wait(lambda n:any('Image QA：部分对话历史读取失败' in t for t in texts(n)));shot('specific-health-error')
        adb('shell','input','keyevent','4');state['projection']=False
        wait(lambda n:'同步状态尚未确认' not in texts(n));print('PASS completion/offline icons, specific health error and automatic recovery',flush=True)
        # Small dark screen, large text, reduced motion, landscape and wide screen.
        adb('shell','cmd','uimode','night','yes');adb('shell','settings','put','system','font_scale','2.0')
        for key in ('animator_duration_scale','transition_animation_scale','window_animation_scale'):adb('shell','settings','put','global',key,'0')
        adb('shell','wm','size','750x1334');adb('shell','wm','density','320')
        tree=wait(lambda n:'Image cache QA' in texts(n));tap(node(tree,text='Image cache QA'));tree=wait(lambda n:'查看图片附件' in descs(n) or 'Cached original photo' in texts(n));open_image(tree);wait(lambda n:'下载图片' in descs(n));shot('small-dark-large-font')
        adb('shell','input','keyevent','4');adb('shell','settings','put','system','accelerometer_rotation','0');adb('shell','settings','put','system','user_rotation','1')
        tree=wait(lambda n:'查看图片附件' in descs(n) or 'Image cache QA' in texts(n))
        if 'Image cache QA' in texts(tree):tap(node(tree,text='Image cache QA'));tree=wait(lambda n:'查看图片附件' in descs(n) or 'Cached original photo' in texts(n))
        open_image(tree);wait(lambda n:'下载图片' in descs(n));shot('landscape')
        adb('shell','input','keyevent','4');adb('shell','settings','put','system','user_rotation','0');adb('shell','wm','size','1600x2560');adb('shell','wm','density','240')
        tree=wait(lambda n:'查看图片附件' in descs(n) or 'Image cache QA' in texts(n))
        if 'Image cache QA' in texts(tree):tap(node(tree,text='Image cache QA'));tree=wait(lambda n:'查看图片附件' in descs(n) or 'Cached original photo' in texts(n))
        open_image(tree);wait(lambda n:'下载图片' in descs(n));shot('tablet')
        assert state['commands']==0
        report=dict(apk=str(args.apk.resolve()),apk_sha256=hashlib.sha256(args.apk.read_bytes()).hexdigest(),package=args.package,emulator=args.serial,original_sha256=digest,image_http_requests=state['image_requests'],download_exact_original=True,memory_during_blocked_sync=True,disk_after_process_restart=True,blank_tap_dismisses=True,live_status_and_offline_indicator=True,health_detail_and_recovery=True,small_dark_large_text_reduced_motion=True,landscape=True,tablet=True,upstream_commands=0,physical_phone_tested=False)
        (OUT/'report.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report,indent=2))
    finally:
        state['pause_sync']=False
        adb('shell','wm','size','reset');adb('shell','wm','density','reset');adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no');adb('shell','settings','put','system','user_rotation','0')
        for key in ('animator_duration_scale','transition_animation_scale','window_animation_scale'):adb('shell','settings','put','global',key,'1')
        server.shutdown();server.server_close()


if __name__=='__main__':main()
