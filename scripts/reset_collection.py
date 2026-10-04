"""Explicit replica reset. Full four-host baseline first; preserve native data and pairing.

Run without --apply to inspect. --apply implements the user-approved data scope.
A partial failure leaves the target stopped; never restarts an unfiltered source.
"""
from contextlib import closing
import argparse
import hashlib
import json
import os
import sqlite3
import subprocess
import time
import uuid
from pathlib import Path
import fleet
from collection_policy import baseline, read_policy, write_policy, erase_replica, CAPTURE_TABLES, HUB_TABLES

SOURCE = fleet.ROOT / 'local/notify-capture/replies.sqlite'
def clear_replica(path,generation):
    erase_replica(path,generation)


def remote_code(host, operation):
    folder=host['helper'].rsplit('\\' if host['platform']=='windows' else '/',1)[0]
    preamble="import sys,json,pathlib,sqlite3,time;sys.path.insert(0,"+repr(folder)+");from collection_policy import baseline,write_policy,read_policy,erase_replica;from remote_capture import atomic_json,initialize;root=pathlib.Path("+repr(folder)+");data=json.load(sys.stdin);db=root/'data/replies.sqlite';index=pathlib.Path("+repr(host['codex_home'])+")/'state_5.sqlite';"
    if operation=='baseline':
        return preamble+"print(json.dumps(baseline(index,data['generation'])))"
    if operation in ('policy','finalize'):
        # Refresh full baseline at the installation boundary, using the host clock.
        return preamble+"policy=baseline(index,data['generation']);policy['enabled']="+repr(operation=='finalize')+";write_policy(db,policy);print(json.dumps({'generation':read_policy(db)['generation'],'excluded':len(policy['excluded'])}))"
    if operation=='clear':
        return preamble+"assert read_policy(db)['generation']==data['generation'];initialize(db);paths=[db]+list((root/'data').glob('health-recovery-*/replies.sqlite'));assert all(p.resolve().is_relative_to((root/'data').resolve()) for p in paths);[erase_replica(p,data['generation']) for p in paths];(root/'data/poll.json').unlink(missing_ok=True);indexes=list((root/'data').glob('health-recovery-*/index.sqlite'));assert all(p.resolve().is_relative_to((root/'data').resolve()) and not p.is_symlink() for p in indexes);[(p.unlink(),pathlib.Path(str(p)+'-wal').unlink(missing_ok=True),pathlib.Path(str(p)+'-shm').unlink(missing_ok=True)) for p in indexes];initialize(db);print(json.dumps({'cleared':True,'generation':data['generation'],'replicas':len(paths)}))"

    raise ValueError('operation')


def invoke(host, operation, generation):
    raw=fleet.ssh(host,fleet.command(host,remote_code(host,operation)),json.dumps({'generation':generation}).encode(),30)
    return json.loads(raw)


def identity(path):
    with closing(sqlite3.connect(path)) as c, c:
        return hashlib.sha256(json.dumps(c.execute('SELECT id,token,role,name,expires,revoked FROM devices ORDER BY id').fetchall()).encode()).hexdigest()


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--config',default=str(fleet.DEFAULT));p.add_argument('--apply',action='store_true');args=p.parse_args()
    c=fleet.load(args.config);assert len(c['hosts'])==4
    generation=str(uuid.uuid4());plans={}
    for name,host in c['hosts'].items():
        plans[name]=baseline(Path.home()/'.codex/state_5.sqlite',generation) if host.get('local') else invoke(host,'baseline',generation)
        assert plans[name]['generation']==generation
        print(name+': baseline checked; existing threads='+str(len(plans[name]['excluded'])),flush=True)
    if not args.apply:
        print('Read-only plan; no content or pairing changed.');return
    subprocess.run(['systemctl','--user','stop','threadbridge.target'],check=True)
    preserved=identity(c['hub_db'])
    # Install every persistent filter before deleting any capture content.
    for name,host in c['hosts'].items():
        if host.get('local'):
            policy=baseline(Path.home()/'.codex/state_5.sqlite',generation);policy['enabled']=False;write_policy(SOURCE,policy)
            result={'generation':read_policy(SOURCE)['generation']}
        else:result=invoke(host,'policy',generation)
        assert result['generation']==generation
    for name,host in c['hosts'].items():
        if not host.get('local'):
            assert invoke(host,'clear',generation)['cleared']
    clear_replica(SOURCE,generation)
    # Clear local active replicas and conversation-bearing backups, preserving
    # identities/configuration and immutable deduplication metadata in Hub copies.
    paths={Path(c['hub_db']),fleet.ROOT/'local/phone-test/pre-011.sqlite'}
    paths.update((fleet.ROOT/'local/backups').rglob('*.sqlite'))
    for name,host in c['hosts'].items():
        if not host.get('local'):
            paths.update((fleet.ROOT/'local/fleet'/name).rglob('*.sqlite'))
            (fleet.ROOT/'local/fleet'/name/'status.json').unlink(missing_ok=True)
    for path in sorted(paths):
        if path.exists():
            assert path.resolve().is_relative_to((fleet.ROOT/'local').resolve()) and not path.is_symlink()
            if path.name=='index.sqlite' and path.parent.name.startswith('health-recovery-'):
                path.unlink();Path(str(path)+'-wal').unlink(missing_ok=True);Path(str(path)+'-shm').unlink(missing_ok=True)
            else:clear_replica(path,generation)
    assert identity(c['hub_db'])==preserved
    # Health is reinitialized without old quarantines; old threads cannot enter.
    from remote_capture import initialize
    initialize(SOURCE)
    # All conversation content is now cleared. Rebaseline at the final boundary
    # to exclude threads created during cleanup; then enable each source.
    for name,host in c['hosts'].items():
        if host.get('local'):
            policy=baseline(Path.home()/'.codex/state_5.sqlite',generation);policy['enabled']=True;write_policy(SOURCE,policy)
        else:assert invoke(host,'finalize',generation)['generation']==generation
    report={'generation':generation,'completed_at':int(time.time()),'scope':'ThreadBridge replicas only; original Codex data and device credentials unchanged','hosts':{n:{'existing_ids':len(plans[n]['excluded'])} for n in plans},'pairing_identity_unchanged':True,'threads':0,'messages':0,'deduplication_preserved':True,'physical_phone_cache':'cleared by 0.1.8 on next successful sync'}
    with closing(sqlite3.connect(c['hub_db'])) as db, db:
        assert db.execute('SELECT count(*) FROM threads').fetchone()[0]==0
        assert db.execute('SELECT count(*) FROM messages').fetchone()[0]==0
    fleet.atomic(fleet.ROOT/'artifacts/collection-reset-verification.json',(json.dumps(report,ensure_ascii=False,indent=2)+'\n').encode())
    subprocess.run(['systemctl','--user','start','threadbridge.target'],check=True)
    print('Four-host replica reset complete; pairing retained.',flush=True)

if __name__=='__main__':main()
