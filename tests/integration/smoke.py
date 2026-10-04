#!/usr/bin/env python3
"""Real loopback HTTP + WebSocket + SQLite test; never connects to Codex."""
import json, pathlib, socket, subprocess, tempfile, time, uuid, urllib.request, urllib.error, sys, base64, os, struct, sqlite3, hashlib
binary = pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/threadbridge').resolve()
opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
def request(base,path,token=None,data=None):
    headers={'Content-Type':'application/json'}
    if token:headers['Authorization']='Bearer '+token
    req=urllib.request.Request(base+path,headers=headers,data=json.dumps(data).encode() if data is not None else None)
    try:
        with opener.open(req,timeout=5) as r:return r.status,json.load(r)
    except urllib.error.HTTPError as e:return e.code,json.load(e)
class EventSocket:
    """Minimal read-only RFC6455 client for this loopback test; standard library only."""
    def __init__(self, port, token, after=0):
        self.sock=socket.create_connection(('127.0.0.1',port),timeout=5)
        self.reader=self.sock.makefile('rb')
        key=base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f'GET /v1/events/ws?after={after} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n').encode())
        assert b'101' in self.reader.readline()
        headers={}
        while (line:=self.reader.readline())!=b'\r\n':
            assert line
            k,v=line.decode().split(':',1);headers[k.lower()]=v.strip()
        assert headers['sec-websocket-accept']==base64.b64encode(hashlib.sha1((key+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
    def read(self):
        head=self.reader.read(2);assert len(head)==2
        assert head[0]==0x81 and not head[1]&0x80
        length=head[1]&0x7f
        if length==126:length=struct.unpack('!H',self.reader.read(2))[0]
        elif length==127:length=struct.unpack('!Q',self.reader.read(8))[0]
        assert length<=1024*1024
        return json.loads(self.reader.read(length))
    def close(self):
        self.reader.close();self.sock.close()
def eventually(fn,timeout=20):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        try:
            result=fn()
            if result:return result
        except (OSError,KeyError,IndexError):pass
        time.sleep(.2)
    raise AssertionError('Timed out awaiting condition')
with tempfile.TemporaryDirectory(prefix='threadbridge-smoke-') as tmp:
    root=pathlib.Path(tmp);db=root/'hub.sqlite';cfg=root/'agent.json'
    with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    base=f'http://127.0.0.1:{port}'
    subprocess.run([binary,'register-agent','--db',db,'--name','Smoke demo','--output',cfg],check=True,stdout=subprocess.DEVNULL)
    config=json.loads(cfg.read_text());config['hub']=base;cfg.write_text(json.dumps(config))
    log=(root/'server.log').open('w')
    hub=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],stdout=log,stderr=log)
    agent=None
    try:
        eventually(lambda:request(base,'/health')[0]==200)
        assert request(base,'/v1/threads')[0]==401
        pairing=json.loads(subprocess.check_output([binary,'pair','--db',db,'--url',base]))
        code,result=request(base,'/v1/pair',data={'code':pairing['code'],'name':'Smoke phone'})
        assert code==200;token=result['token'];phone=result['device_id']
        assert request(base,'/v1/pair',data={'code':pairing['code'],'name':'Replay'})[0]==409
        agent=subprocess.Popen([binary,'agent','--config',cfg,'--db',root/'agent.sqlite','--demo'],stdout=log,stderr=log)
        thread=eventually(lambda:request(base,'/v1/threads',token)[1]['threads'][0])
        baseline=request(base,'/v1/events',token)[1]['cursor']
        events_ws=EventSocket(port,token,baseline)
        payload={'request_id':str(uuid.uuid4()),'thread_id':thread['id'],'text':'中文 round trip ✓','expected_revision':thread['revision'],'created_at':int(time.time()),'kind':'send','cursor':None}
        status,accepted=request(base,'/v1/commands',token,payload);assert status==200,(status,accepted)
        assert request(base,'/v1/commands',token,payload)[1]['id']==accepted['id']
        changed=dict(payload,text='changed');assert request(base,'/v1/commands',token,changed)[0]==409
        receipt=eventually(lambda:(r if (r:=request(base,'/v1/commands/'+payload['request_id'],token)[1])['status']=='codex_accepted' else None))
        assert receipt['native_turn_id'].startswith('demo-')
        observed=[]
        def receipt_event():
            event=events_ws.read();observed.extend(event.get('events',[]))
            return len([e for e in observed if e['kind']=='command'])>=3
        eventually(receipt_event)
        cursor=max(e['seq'] for e in observed)
        events_ws.close()
        events_ws=EventSocket(port,token,cursor)
        resumed=events_ws.read();assert resumed['cursor']>=cursor
        events_ws.close()
        # Old authoritative receipt beyond the most recent 128 rows must reconcile.
        agent.terminate();agent.wait(timeout=5)
        with sqlite3.connect(db) as c:
            c.execute("UPDATE commands SET status='unknown',native_turn=NULL WHERE id=?",(accepted['id'],))
        with sqlite3.connect(root/'agent.sqlite') as c:
            c.executemany("INSERT INTO agent_ledger(id,status) VALUES(?,'rejected')",[(str(uuid.uuid4()),) for _ in range(140)])
        agent=subprocess.Popen([binary,'agent','--config',cfg,'--db',root/'agent.sqlite','--demo'],stdout=log,stderr=log)
        eventually(lambda:request(base,'/v1/commands/'+payload['request_id'],token)[1]['status']=='codex_accepted')
        # A full Hub restart preserves the ledger and never redispatches accepted work.
        agent.terminate();agent.wait(timeout=5)
        hub.terminate();hub.wait(timeout=5)
        hub=subprocess.Popen([binary,'hub','--db',db,'--listen',f'127.0.0.1:{port}'],stdout=log,stderr=log)
        eventually(lambda:request(base,'/health')[0]==200)
        agent=subprocess.Popen([binary,'agent','--config',cfg,'--db',root/'agent.sqlite','--demo'],stdout=log,stderr=log)
        eventually(lambda:request(base,'/v1/threads',token)[1]['threads'][0]['host_online'])
        assert request(base,'/v1/commands/'+payload['request_id'],token)[1]['native_turn_id']==receipt['native_turn_id']
        with sqlite3.connect(root/'agent.sqlite') as c:
            assert c.execute("SELECT count(*) FROM agent_ledger WHERE id=?",(accepted['id'],)).fetchone()[0]==1

        messages=eventually(lambda: any('中文 round trip' in m['text'] for m in request(base,f"/v1/threads/{thread['id']}/messages",token)[1]['messages']))
        assert request(base,'/v1/events?after=0',token)[1]['events']
        subprocess.run([binary,'backup','--db',db,'--output',root/'backup.sqlite'],check=True,stdout=subprocess.DEVNULL)
        subprocess.run([binary,'restore','--backup',root/'backup.sqlite','--destination',root/'restored.sqlite'],check=True,stdout=subprocess.DEVNULL)
        import sqlite3
        with sqlite3.connect(root/'restored.sqlite') as c:
            assert c.execute("SELECT v FROM settings WHERE k='writes'").fetchone()[0]=='off'
            assert c.execute('SELECT count(*) FROM devices WHERE revoked=0').fetchone()[0]==0
        subprocess.run([binary,'revoke','--db',db,'--device',phone],check=True)
        assert request(base,'/v1/threads',token)[0]==401
        print(json.dumps({'result':'pass','transport':'real loopback HTTP/WebSocket','adapter':'isolated demo, no Codex calls','checks':['authentication','one-time pairing','task sync','durable send','idempotency','same-key conflict','receipt','reply sync','events','phone WebSocket receipts','WebSocket cursor resume','old receipt beyond 128 entries','restart without resend','consistent backup','read-only restore','revocation']},ensure_ascii=False,indent=2))
    finally:
        for p in [agent,hub]:
            if p is not None:
                p.terminate()
                try:p.wait(timeout=5)
                except subprocess.TimeoutExpired:p.kill();p.wait()
        log.close()
