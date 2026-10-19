"""Actual debug APK + dedicated emulator + temporary synthetic Hub; never calls Codex."""
from contextlib import closing
import hashlib
import json
import os
import re
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import uuid
import urllib.request
from qa_android_ui015 import adb, ui, texts, node, tap, wait, shot, ROOT
from qa_android_settings021 import enter
from qa_android_keyentry021 import lan_toggle

PACKAGE='dev.threadbridge.debug'

def main():
    assert 'package:'+PACKAGE not in adb('shell','pm','list','packages',PACKAGE),'Fresh isolated debug package required'
    with tempfile.TemporaryDirectory(prefix='threadbridge-question-ui-') as directory:
        db=Path(directory)/'hub.sqlite'
        with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
        proc=subprocess.Popen([ROOT/'target/debug/threadbridge','hub','--db',db,'--listen',f'127.0.0.1:{port}'],env=dict(os.environ,HOME=directory,CODEX_HOME=directory),stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        stop=threading.Event()
        host,native,turn=[str(uuid.uuid4()) for _ in range(3)]
        tid=hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest()
        key='synthetic'+uuid.uuid4().hex
        qid=json.dumps(['request_user_input_async','call_ui',0],separators=(',',':'))
        mid='question:'+hashlib.sha256(qid.encode()).hexdigest()
        question={'schema':1,'id':qid,'kind':'async','title':'路由器是否启用了双 WAN？','options':[{'label':'已启用','description':'按设备分配出口'},{'label':'未启用','description':'只有一个出口'}],'status':'pending','answer':None}
        def mutate_question():
            body=json.dumps(question,ensure_ascii=False)
            with closing(sqlite3.connect(db,timeout=3)) as c,c:
                c.execute('UPDATE messages SET body=?,version=? WHERE thread=? AND id=?',(body,hashlib.sha256(body.encode()).hexdigest(),tid,mid))
                c.execute('UPDATE thread_message_state SET revision=revision+1 WHERE thread=?',(tid,))
                c.execute("INSERT INTO events(kind,thread,created) VALUES('thread',?,?)",(tid,int(time.time())))
        def keep_online():
            while not stop.wait(.3):
                with closing(sqlite3.connect(db,timeout=3)) as c,c:
                    c.execute('UPDATE devices SET last_seen=? WHERE id=?',(int(time.time()),host))
        def open_thread():
            adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity')
            tree=wait(lambda n:'待回答的问题' in texts(n) or any(question['title'] in text for text in texts(n)))
            if '待回答的问题' in texts(tree):
                row=node(tree,text='待回答的问题');print('Opening question fixture at '+row.attrib['bounds'],flush=True);tap(row)
            time.sleep(.5)
            (ROOT/'artifacts/questions-navigation.xml').write_text(adb('shell','cat','/sdcard/ui015.xml'))
            return wait(lambda n:any(t.startswith('问题 ·') for t in texts(n)) or '提交回答' in texts(n) or '此问题需要在电脑端回答。' in texts(n) or any(o['label'] in texts(n) for o in question['options']))
        try:
            for _ in range(100):
                if db.exists():
                    with closing(sqlite3.connect(db)) as c:
                        if c.execute("SELECT count(*) FROM sqlite_master WHERE name='devices'").fetchone()[0]:break
                time.sleep(.05)
            opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
            for _ in range(100):
                try:
                    with opener.open(f'http://127.0.0.1:{port}/health',timeout=1) as response:assert response.status==200
                    break
                except OSError:time.sleep(.1)
            else:raise AssertionError('Synthetic Hub did not start')
            now=int(time.time());body=json.dumps(question,ensure_ascii=False)
            with closing(sqlite3.connect(db)) as c,c:
                c.execute("INSERT INTO devices(id,role,name,expires,last_seen) VALUES(?,'agent','问题测试电脑',?,?)",(host,now+3600,now))
                c.execute("INSERT INTO devices(id,token,role,name,expires) VALUES('phone',?,'phone','Question QA',?)",(hashlib.sha256(key.encode()).hexdigest(),now+3600))
                c.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,host,native,'待回答的问题','queue_ready','',now,1))
                c.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(tid,mid,turn,'question',body,hashlib.sha256(body.encode()).hexdigest(),now*1000))
                c.execute('INSERT INTO thread_message_state VALUES(?,1,?)',(tid,now*1000))
            threading.Thread(target=keep_online,daemon=True).start()
            adb('reverse',f'tcp:{port}',f'tcp:{port}')
            adb('install',ROOT/'android/app/build/outputs/apk/debug/app-debug.apk')
            adb('shell','am','start','-n',PACKAGE+'/dev.threadbridge.MainActivity')
            n=wait(lambda n:'连接你的电脑' in texts(n));tap(node(n,text='连接密钥'))
            wait(lambda n:'输入地址与连接密钥，让对话继续。' in texts(n))
            enter(0,f'http://127.0.0.1:{port}');enter(1,key)
            n=ui()
            if '局域网测试' not in texts(n):adb('shell','input','swipe',600,1700,600,700,350);n=ui()
            tap(lan_toggle(n));wait(lambda n:lan_toggle(n).attrib.get('checked')=='true');tap(node(ui(),text='连接'))
            wait(lambda n:'设备列表' in texts(n),seconds=45)
            n=open_thread()
            assert '问题 · 待回答' in texts(n) and '提交回答' in texts(n)
            assert not any('questionItemId' in t or '"schema"' in t for t in texts(n))
            shot('questions-light.png');print('PASS question card visible before any final',flush=True)
            # Option selection only edits the draft; no implicit answer submission.
            tap(node(n,text='未启用'))
            n=wait(lambda n:'已选：未启用' in texts(n))
            with closing(sqlite3.connect(db)) as c:assert c.execute('SELECT count(*) FROM commands').fetchone()[0]==0
            tap(node(n,text='提交回答'))
            end=time.monotonic()+15
            while True:
                with closing(sqlite3.connect(db)) as c:rows=c.execute('SELECT id,payload FROM commands').fetchall()
                if rows:break
                assert time.monotonic()<end,'answer submission missing';time.sleep(.2)
            assert len(rows)==1
            command,payload=rows[0];payload=json.loads(payload)
            reply=json.loads(payload['text'].split('>',1)[1].rsplit('<',1)[0])[0]
            assert reply=={'questionItemId':qid,'question':question['title'],'answer':'未启用'}
            with closing(sqlite3.connect(db)) as c,c:
                c.execute("UPDATE commands SET status='upstream_queued' WHERE id=?",(command,))
                c.execute("INSERT INTO events(kind,thread,created) VALUES('command',?,?)",(tid,int(time.time())))
            n=wait(lambda n:'已入桌面队列，尚未确认开始执行' in texts(n))
            assert '问题 · 待回答' in texts(n)
            buttons=[x for x in n.iter('node') if x.attrib.get('text')=='提交回答']
            parents={child:parent for parent in n.iter() for child in parent}
            target=buttons[0]
            while target.attrib.get('clickable')!='true' and target in parents:target=parents[target]
            assert target.attrib.get('enabled')=='false',target.attrib
            shot('questions-queued.png');print('PASS explicit selection and exact reply submission; queue acknowledgement leaves question pending',flush=True)
            question.update(status='answered',answer='未启用');mutate_question()
            with closing(sqlite3.connect(db)) as c,c:
                c.execute("UPDATE commands SET status='codex_accepted' WHERE id=?",(command,))
                c.execute("INSERT INTO events(kind,thread,created) VALUES('command',?,?)",(tid,int(time.time())))
            n=wait(lambda n:'问题 · 已回答' in texts(n))
            assert '回答：未启用' in texts(n) and '提交回答' not in texts(n)
            adb('shell','am','force-stop',PACKAGE);n=open_thread();wait(lambda n:'问题 · 已回答' in texts(n))
            # Read-only blocking questions remain visible, with no misleading queue reply action.
            question.update(kind='blocking',status='pending',answer=None);mutate_question()
            wait(lambda n:'此问题需要在电脑端回答。' in texts(n))
            question.update(kind='async');mutate_question();wait(lambda n:'提交回答' in texts(n))
            adb('shell','wm','size','750x1500');adb('shell','wm','density','320')
            adb('shell','cmd','uimode','night','yes');adb('shell','settings','put','system','font_scale','2.0')
            for name in ('window_animation_scale','transition_animation_scale','animator_duration_scale'):adb('shell','settings','put','global',name,'0')
            n=open_thread()
            for _ in range(6):
                if any(question['title'] in t for t in texts(n)):break
                adb('shell','input','swipe',375,350,375,1100,300);n=ui()
            shot('questions-small-dark-large-font.png')
            assert any(question['title'] in t for t in texts(n))
            adb('shell','settings','put','system','accelerometer_rotation','0');adb('shell','settings','put','system','user_rotation','1')
            time.sleep(1);n=open_thread()
            def submit_clear_of_composer(tree):
                labels=[x for x in tree.iter('node') if x.attrib.get('text')=='提交回答']
                composers=[x for x in tree.iter('node') if x.attrib.get('resource-id')=='conversation_composer']
                if not labels or not composers:return False
                parents={child:parent for parent in tree.iter() for child in parent}
                target=labels[0]
                while target.attrib.get('clickable')!='true' and target in parents:target=parents[target]
                bounds=list(map(int,re.findall(r'\d+',target.attrib['bounds'])))
                composer_bounds=list(map(int,re.findall(r'\d+',composers[0].attrib['bounds'])))
                return bounds[1]>=170 and bounds[3]+8<=composer_bounds[1]
            for _ in range(8):
                if submit_clear_of_composer(n):break
                adb('shell','input','swipe',1400,400,1400,150,450);n=ui()
            shot('questions-landscape.png')
            assert submit_clear_of_composer(n),'Landscape submit must remain reachable above the floating composer'
            with closing(sqlite3.connect(db)) as c:assert c.execute('SELECT count(*) FROM commands').fetchone()[0]==1
            (ROOT/'artifacts/questions-ui-verification.json').write_text(json.dumps({'pass':True,'scope':'dedicated emulator + isolated debug APK + temporary synthetic Hub','real_codex_send':False,'physical_phone':False,'checks':['question before first final','question body and options','selection does not submit','exact question ID and answer in command','queue ack does not resolve card','double submit disabled','source answer resolves card','relaunch cached question','blocking question visible','375dp dark 2x font disabled animations','landscape submit fully above floating composer','one explicit synthetic command']},ensure_ascii=False,indent=2)+'\n')
            print('PASS: actual Android question card, option, reply, queued vs answered, restart and adaptive layouts',flush=True)
        finally:
            stop.set()
            adb('shell','am','force-stop',PACKAGE)
            adb('reverse','--remove',f'tcp:{port}')
            if 'package:'+PACKAGE in adb('shell','pm','list','packages',PACKAGE):adb('uninstall',PACKAGE)
            adb('shell','settings','put','system','user_rotation','0');adb('shell','settings','put','system','accelerometer_rotation','1')
            adb('shell','settings','put','system','font_scale','1.0');adb('shell','cmd','uimode','night','no')
            adb('shell','wm','size','reset');adb('shell','wm','density','reset')
            for name in ('window_animation_scale','transition_animation_scale','animator_duration_scale'):adb('shell','settings','put','global',name,'1')
            proc.terminate();proc.wait(timeout=3)

if __name__=='__main__':main()
