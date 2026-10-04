#!/usr/bin/python3
"""Preallocated, dual-slot completion status; IDs/reasons only, no event body."""
import hashlib,json,os,time
from pathlib import Path
SLOT=65536
LIMIT=128

def read_at(fd,size,offset):
    if hasattr(os,'pread'):return os.pread(fd,size,offset)
    os.lseek(fd,offset,os.SEEK_SET);return os.read(fd,size)

def write_at(fd,data,offset):
    if hasattr(os,'pwrite'):return os.pwrite(fd,data,offset)
    os.lseek(fd,offset,os.SEEK_SET);return os.write(fd,data)

def lock_file(fd):
    if os.name=='nt':
        import msvcrt
        os.lseek(fd,0,os.SEEK_SET);msvcrt.locking(fd,msvcrt.LK_LOCK,1)
    else:
        import fcntl
        fcntl.flock(fd,fcntl.LOCK_EX)

def open_health(path,flags):
    if path.is_symlink():raise ValueError('capture_health_symlink')
    return os.open(path,flags|getattr(os,'O_NOFOLLOW',0)|getattr(os,'O_BINARY',0),0o600)

def read_slots(fd):
    found=[]
    for n in range(2):
        try:
            row=json.loads(read_at(fd,SLOT,n*SLOT).rstrip(b' \0'))
            data=row['data']
            if hashlib.sha256(data.encode()).hexdigest()!=row['sha256']:continue
            state=json.loads(data)
            if not isinstance(state.get('failures'),dict):continue
            found.append((int(row['generation']),state))
        except (ValueError,KeyError,TypeError):pass
    return max(found,key=lambda x:x[0]) if found else None

def health_path(database):return Path(str(database)+'.health')

def update_health(database,native,turn,reason,attempt=None):
    path=health_path(database);path.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
    fd=open_health(path,os.O_RDWR|os.O_CREAT)
    try:
        lock_file(fd)
        if os.fstat(fd).st_size==0:
            if hasattr(os,'posix_fallocate'):os.posix_fallocate(fd,0,SLOT*2)
            else:os.ftruncate(fd,SLOT*2)
            os.fsync(fd)
            if os.name!='nt':
                directory=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
                try:os.fsync(directory)
                finally:os.close(directory)
            current=(0,{'failures':{},'overflow':False,'updated_at':0})
        else:
            if os.fstat(fd).st_size!=SLOT*2:raise ValueError('capture_health_corrupt')
            current=read_slots(fd)
            if current is None:raise ValueError('capture_health_corrupt')
        generation,state=current;key=native+':'+turn
        if reason is None:
            previous=state['failures'].get(key)
            if previous is not None and (attempt is None or previous.get('attempt')==attempt):state['failures'].pop(key,None)
        elif key in state['failures'] or len(state['failures'])<LIMIT:
            state['failures'][key]={'thread_id':native,'turn_id':turn,'reason':reason,'recorded_at':int(time.time()),'attempt':attempt}
        else:state['overflow']=True
        state['updated_at']=int(time.time())
        data=json.dumps(state,separators=(',',':'),sort_keys=True)
        row=json.dumps({'generation':generation+1,'sha256':hashlib.sha256(data.encode()).hexdigest(),'data':data},separators=(',',':')).encode()
        if len(row)>SLOT:raise ValueError('capture_health_full')
        encoded=row.ljust(SLOT,b' ');offset=((generation+1)%2)*SLOT
        if write_at(fd,encoded,offset)!=SLOT:raise OSError('capture_health_short_write')
        os.fsync(fd)
    finally:os.close(fd)

def read_health(database):
    fd=open_health(health_path(database),os.O_RDONLY)
    try:
        result=read_slots(fd)
        if result is None:raise ValueError('capture_health_corrupt')
        return result[1]
    finally:os.close(fd)
