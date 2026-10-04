#!/usr/bin/python3
"""Capture completed replies for an explicit catalog or all-task collection scope.

Collection policy bounds discovery; optional current-turn user reads are explicit.
No network access or title-based routing; deployment chooses the authorized scope.
"""
import argparse,json,sys,uuid
from pathlib import Path
from capture_completion import capture

def _capture_catalog(payload,catalog,database,title_index=None,all_tasks=False,storage_budget=None,min_free_bytes=64*1024*1024,user_turn_index=None):
    rows={}
    if catalog is not None:
        with Path(catalog).open("rb") as stream:raw=stream.read(512*1024+1)
        if len(raw)>512*1024:raise ValueError('catalog_too_large')
        rows=json.loads(raw)
        if not isinstance(rows,dict) or not 1<=len(rows)<=1000:raise ValueError('invalid_catalog')
        for native,title in rows.items():
            uuid.UUID(native)
            if not isinstance(title,str) or not title.strip() or len(title.encode('utf-8'))>512:raise ValueError('invalid_catalog_title')
    elif not all_tasks:raise ValueError('capture_scope_required')
    if storage_budget is not None and storage_budget<=0:raise ValueError('invalid_storage_budget')
    if min_free_bytes<0:raise ValueError('invalid_free_space_reserve')
    event=json.loads(payload)
    if not isinstance(event,dict):raise ValueError('invalid_event')
    native=event.get('thread-id')
    if not isinstance(native,str) or (not all_tasks and native not in rows):return 'ignored'
    from collection_policy import allowed
    if not allowed(database,user_turn_index or title_index,native):return 'ignored'
    user_error=None
    if user_turn_index is not None and event.get('type')=='agent-turn-complete':
        from capture_user_turn import capture_users
        try:capture_users(database,user_turn_index,event)
        except Exception as error:user_error=error
    result=capture(payload,native,database,rows.get(native),title_index,storage_budget,min_free_bytes)
    if user_error is not None:
        from capture_health import update_health
        update_health(database,native,event['turn-id'],'user_input_capture_failed')
        raise user_error
    return result

def capture_catalog(payload,catalog,database,*args,**kwargs):
    from collection_policy import collection_lock
    with collection_lock(database):
        return _capture_catalog(payload,catalog,database,*args,**kwargs)

def main():
    p=argparse.ArgumentParser(description=__doc__);scope=p.add_mutually_exclusive_group(required=True);scope.add_argument('--catalog');scope.add_argument('--all-tasks',action='store_true');p.add_argument('--title-index');p.add_argument('--database',required=True);p.add_argument('--storage-budget-bytes',type=int);p.add_argument('--min-free-bytes',type=int,default=64*1024*1024);p.add_argument('--user-turn-index',help='Opt-in: read only human text of the completed thread/turn from this local metadata index');p.add_argument('payload');a=p.parse_args()
    try:print(capture_catalog(a.payload,a.catalog,a.database,a.title_index,a.all_tasks,a.storage_budget_bytes,a.min_free_bytes,a.user_turn_index));return 0
    except Exception:print('completion capture failed; no event content logged',file=sys.stderr);return 2
if __name__=='__main__':sys.exit(main())
