"""Persistent, fail-closed collection boundary; never modifies Codex data."""
import json
import os
import sqlite3
import time
import uuid
from contextlib import contextmanager
from pathlib import Path


@contextmanager
def collection_lock(database):
    path=Path(str(database)+'.collection.lock')
    path.parent.mkdir(parents=True,exist_ok=True)
    descriptor=os.open(path,os.O_CREAT|os.O_RDWR,0o600)
    stream=os.fdopen(descriptor,'r+b')
    try:
        if os.fstat(descriptor).st_size==0:stream.write(b'0');stream.flush()
        deadline=time.monotonic()+60
        while True:
            try:
                if os.name=='nt':
                    import msvcrt
                    stream.seek(0);msvcrt.locking(descriptor,msvcrt.LK_NBLCK,1)
                else:
                    import fcntl
                    fcntl.flock(descriptor,fcntl.LOCK_EX|fcntl.LOCK_NB)
                break
            except OSError:
                if time.monotonic()>deadline:raise TimeoutError('collection_lock_timeout')
                time.sleep(.05)
        try:yield
        finally:
            if os.name=='nt':
                stream.seek(0);msvcrt.locking(descriptor,msvcrt.LK_UNLCK,1)
            else:fcntl.flock(descriptor,fcntl.LOCK_UN)
    finally:stream.close()


def policy_path(database):
    return Path(str(database) + '.collection.json')


def write_policy(database, policy):
    with collection_lock(database):
        _write_policy(database,policy)


def _write_policy(database, policy):
    path = policy_path(database)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix('.next')
    with temporary.open('w', encoding='utf-8') as stream:
        json.dump(policy, stream)
        stream.flush()
        os.fsync(stream.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    Path(str(database) + '.collection-required').touch(mode=0o600)


def read_policy(database):
    path = policy_path(database)
    if not path.exists():
        if Path(str(database) + '.collection-required').exists():
            raise ValueError('collection_policy_missing')
        return None  # Compatibility for isolated legacy fixtures, before reset.
    policy = json.loads(path.read_text(encoding='utf-8'))
    if policy.get('schema') != 1 or type(policy.get('cutoff_ms')) is not int or policy['cutoff_ms']<=0 or type(policy.get('enabled',True)) is not bool:
        raise ValueError('collection_policy_invalid')
    uuid.UUID(policy['generation'])
    if not isinstance(policy.get('excluded'), list) or len(policy['excluded'])>50000:
        raise ValueError('collection_policy_invalid')
    for native in policy['excluded']:uuid.UUID(native)
    return policy


def baseline(index, generation):
    with sqlite3.connect(Path(index).resolve().as_uri() + '?mode=ro', uri=True) as connection:
        # Consistent full index, including archived threads; no LIMIT or body reads.
        connection.execute('BEGIN')
        ids = [row[0] for row in connection.execute('SELECT id FROM threads')]
        cutoff = int(time.time() * 1000)
    for native in ids:
        uuid.UUID(native)
    return {'schema': 1, 'generation': generation, 'cutoff_ms': cutoff, 'excluded': ids}


def allowed(database, index, native):
    policy = read_policy(database)
    if policy is None:
        return True
    if not policy.get('enabled',True):
        return False
    if native in policy['excluded']:
        return False
    if not index:
        raise ValueError('collection_index_required')
    with sqlite3.connect(Path(index).resolve().as_uri() + '?mode=ro', uri=True, timeout=3) as connection:
        columns = {r[1] for r in connection.execute('PRAGMA table_info(threads)')}
        creation = 'COALESCE(created_at_ms,created_at*1000)' if 'created_at_ms' in columns else 'created_at*1000'
        row = connection.execute('SELECT ' + creation + ' FROM threads WHERE id=?', (native,)).fetchone()
    return bool(row and isinstance(row[0], int) and row[0] > policy['cutoff_ms'])


def purge(database, natives):
    with collection_lock(database):
        _purge(database,natives)


def _purge(database, natives):
    """Drop only captured replicas; persist exclusions before deleting content."""
    natives = set(natives)
    if not natives:
        return
    policy = read_policy(database)
    if policy is None:
        raise ValueError('collection_policy_required')
    added = natives - set(policy['excluded'])
    if added:
        policy['excluded'] = sorted(set(policy['excluded']) | natives)
        _write_policy(database, policy)
    with sqlite3.connect(database, timeout=3) as connection:
        tables = {r[0] for r in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        for table in ('captured_replies', 'captured_user_messages', 'captured_turn_order', 'captured_request_ids'):
            if table in tables:
                connection.executemany('DELETE FROM ' + table + ' WHERE thread_id=?', [(n,) for n in natives])
    if Path(str(database)+'.health').exists():
        from capture_health import read_health, update_health
        for failure in read_health(database)['failures'].values():
            if failure['thread_id'] in natives:
                update_health(database,failure['thread_id'],failure['turn_id'],None)



CAPTURE_TABLES=('captured_replies','captured_request_ids','captured_user_messages','captured_turn_order')
HUB_TABLES=('threads','messages','events','outbox','history_positions','completion_metadata','capture_targets','capture_health','capture_import_positions','capture_user_import_positions','resume_owners')

def erase_replica(path,generation):
    path=Path(path)
    if path.is_symlink():raise ValueError('replica_symlink_refused')
    with collection_lock(path),sqlite3.connect(path,timeout=5) as connection:
        connection.execute('PRAGMA secure_delete=ON')
        tables={r[0] for r in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if not ('captured_replies' in tables or {'devices','commands','messages'}<=tables):
            raise ValueError('not_threadbridge_replica')
        if 'tombstones' in tables and 'threads' in tables:
            connection.execute("INSERT OR IGNORE INTO tombstones SELECT id,'' FROM threads")
        for table in (*CAPTURE_TABLES,*HUB_TABLES):
            if table in tables:connection.execute('DELETE FROM '+table)
        if 'commands' in tables:
            connection.execute("UPDATE commands SET payload='{}',status=CASE WHEN status='accepted' THEN 'cancelled' WHEN status IN ('dispatching','upstream_queued') THEN 'unknown' ELSE status END,error='collection_reset'")
        if 'settings' in tables:
            connection.execute("INSERT OR REPLACE INTO settings VALUES('collection_generation',?)",(generation,))
        connection.commit();connection.execute('PRAGMA wal_checkpoint(TRUNCATE)');connection.execute('VACUUM')
    for suffix in ('.health','.health.next'):
        Path(str(path)+suffix).unlink(missing_ok=True)
