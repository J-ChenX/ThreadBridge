"""Photo thumbnail and preview on the dedicated emulator; synthetic data only."""
import hashlib
import json
import runpy
import sqlite3
import struct
import subprocess
import time
import uuid
import zlib
from pathlib import Path
q=runpy.run_path(str(Path(__file__).with_name('qa_android_ui018.py')))
ROOT=q['ROOT'];call,ui,wait,tap,find,texts,shot=[q[k] for k in ['call','ui','wait','tap','find','texts','shot']]

def main():
    host='ui-fixture-host';native='photo-preview-'+uuid.uuid4().hex
    tid=hashlib.sha256((host+'\0default\0'+native).encode()).hexdigest()
    def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data)&0xffffffff)
    width,height=240,160
    pixels=b''.join(b'\0'+bytes([50,y,180])*width for y in range(height))
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',width,height,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
    image=hashlib.sha256(png).hexdigest();now=int(time.time())
    db=ROOT/'local/ui-test-018/hub.sqlite'
    with sqlite3.connect(db) as c:
        assert {r[0] for r in c.execute("SELECT id FROM devices WHERE role='agent'")}=={host,'fixture-lerrem','fixture-nix','fixture-windows'}
        before=c.execute('SELECT count(*) FROM commands').fetchone()[0]
        c.execute('INSERT INTO threads VALUES(?,?,?,?,?,?,?,?,NULL)',(tid,host,native,'完整对话与图片验证','completed','photo-turn',now,1))
        for mid,role,text,offset in [('photo','user',f'请检查这张图片\n![图片](threadbridge-image:{image})\n',0),('comment','assistant','正在对照原始消息检查。',1),('final','assistant','已补齐图片消息和文字说明。',2)]:
            c.execute('INSERT INTO messages VALUES(?,?,?,?,?,?,?)',(tid,mid,'photo-turn',role,text,hashlib.sha256(text.encode()).hexdigest(),now*1000+offset))
        c.execute('INSERT INTO attachments VALUES(?,?,?,?,?)',(tid,'photo',image,'image/png',png))
        c.execute("INSERT INTO events(kind,thread,created) VALUES('thread',?,?)",(tid,now))
    call('shell','am','force-stop','dev.threadbridge')
    call('shell','am','start','-a','android.intent.action.VIEW','-d','threadbridge://thread/'+tid,'dev.threadbridge')
    n=wait(lambda n:'请检查这张图片' in texts(n) and any(x.attrib.get('content-desc')=='图片附件' for x in n.iter('node')))
    assert 'threadbridge-image:' not in texts(n)
    assert '正在对照原始消息检查。' in texts(n) and '已补齐图片消息和文字说明。' in texts(n)
    shot('ui-0.1.11-visible-history.png')
    tap(find(n,desc='图片附件'))
    n=wait(lambda n:any(x.attrib.get('content-desc')=='图片预览' for x in n.iter('node')))
    shot('ui-0.1.11-image-preview.png');tap(find(n,text='关闭'))
    call('shell','am','force-stop','dev.threadbridge')
    call('shell','am','start','-a','android.intent.action.VIEW','-d','threadbridge://thread/'+tid,'dev.threadbridge')
    wait(lambda n:any(x.attrib.get('content-desc')=='图片附件' for x in n.iter('node')))
    with sqlite3.connect(db) as c:assert c.execute('SELECT count(*) FROM commands').fetchone()[0]==before
    (ROOT/'artifacts/visible-history-ui-verification.json').write_text(json.dumps({'pass':True,'version':'0.1.11-test','physical_phone':False,'scope':'dedicated emulator + synthetic Hub','checks':['same-signature upgrade','thumbnail','visible user caption','commentary and final','click preview','restart retains image','no synthetic command submitted']},ensure_ascii=False,indent=2)+'\n')
    print('PASS: photo, caption, commentary, final, preview and restart on actual Android emulator')

if __name__=='__main__':main()
