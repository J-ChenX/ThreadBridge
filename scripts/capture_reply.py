#!/usr/bin/env python3
"""Capture allowlisted Codex notify replies locally; no network or session reads."""
import argparse
import json
import os
import re
from pathlib import Path
import sqlite3
import sys
import time
import uuid

MAX_REPLY_BYTES = 256 * 1024
MAX_STORED_BYTES = 16 * 1024 * 1024
MAX_REPLIES = 1000


def indexed_title(native,index):
    """Only ID/name/time metadata; never open session or transcript files."""
    found=None;used=0
    with Path(index).open('rb') as stream:
        while True:
            line=stream.readline(8193)
            if not line:break
            used+=len(line)
            if len(line)>8192 or used>8*1024*1024:raise ValueError('title_index_limit')
            entry=json.loads(line)
            if not isinstance(entry,dict) or set(entry)-{'id','thread_name','updated_at'}:raise ValueError('invalid_title_index_metadata')
            if entry.get('id')==native:
                value=entry.get('thread_name')
                if not isinstance(value,str) or not value.strip() or len(value.encode('utf-8'))>512:raise ValueError('invalid_title')
                found=value.strip()
    return found

def capture(payload, allowed_thread, database, title=None, title_index=None):
    uuid.UUID(allowed_thread)
    event = json.loads(payload)
    if not isinstance(event, dict):
        raise ValueError('invalid_event')
    # Filter before opening storage; other task content is never persisted.
    if event.get('thread-id') != allowed_thread or event.get('type') != 'agent-turn-complete':
        return 'ignored'
    turn = event.get('turn-id')
    reply = event.get('last-assistant-message')
    if not isinstance(turn, str) or not isinstance(reply, str) or not reply:
        raise ValueError('missing_reply_identity')
    uuid.UUID(turn)
    size = len(reply.encode('utf-8'))
    if size > MAX_REPLY_BYTES:
        raise ValueError('reply_too_large')
    if title is None and title_index is not None:
        try:title = indexed_title(allowed_thread,title_index)
        except (OSError,ValueError,UnicodeError):title = None
    if title is None:
        title = "会话 · " + allowed_thread[:8]
    if not isinstance(title, str) or not title.strip() or len(title.encode("utf-8")) > 512:
        raise ValueError("invalid_title")
    title = title.strip()
    path = Path(database)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink():
        raise ValueError('symlink_database')
    os.umask(0o077)
    with sqlite3.connect(path, timeout=3) as db:
        db.execute('PRAGMA synchronous=FULL')
        db.execute('CREATE TABLE IF NOT EXISTS captured_replies (thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, reply TEXT NOT NULL, utf8_bytes INTEGER NOT NULL, captured_at INTEGER NOT NULL DEFAULT (unixepoch()), PRIMARY KEY(thread_id,turn_id))')
        db.execute('CREATE TABLE IF NOT EXISTS captured_request_ids(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,request_id TEXT NOT NULL,PRIMARY KEY(thread_id,turn_id))')
        db.execute('BEGIN IMMEDIATE')
        columns = {row[1] for row in db.execute('PRAGMA table_info(captured_replies)')}
        if 'title' not in columns:
            db.execute("ALTER TABLE captured_replies ADD COLUMN title TEXT NOT NULL DEFAULT ''")
        old = db.execute('SELECT reply FROM captured_replies WHERE thread_id=? AND turn_id=?', (allowed_thread, turn)).fetchone()
        if old:
            if old[0] != reply:
                raise ValueError('conflicting_reply')
            return 'duplicate'
        count, used = db.execute('SELECT count(*),coalesce(sum(utf8_bytes),0) FROM captured_replies').fetchone()
        if count >= MAX_REPLIES or used + size > MAX_STORED_BYTES:
            raise ValueError('capture_capacity_exceeded')
        db.execute('INSERT INTO captured_replies(thread_id,turn_id,reply,utf8_bytes,captured_at,title) VALUES (?,?,?,?,?,?)', (allowed_thread, turn, reply, size, int(time.time()), title))
        # Keep only a bridge-generated UUID marker, never raw input messages.
        candidates=set()
        inputs=event.get('input-messages', [])
        if isinstance(inputs,list):
            for text in inputs:
                if isinstance(text,str):
                    match=re.match(r'\A\[ThreadBridge request:([0-9a-f-]{36})\]\n',text)
                    if match:
                        try: uuid.UUID(match.group(1));candidates.add(match.group(1))
                        except ValueError: pass
        if len(candidates)==1:
            db.execute('INSERT INTO captured_request_ids VALUES (?,?,?)',(allowed_thread,turn,candidates.pop()))
    return 'captured'


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--thread', required=True)
    p.add_argument('--database', required=True)
    p.add_argument('--title', help='Approved local thread title; never used for routing')
    p.add_argument('--title-index', help='Optional local ID/title-only index, no transcript access')
    p.add_argument('payload')
    args = p.parse_args()
    try:
        result = capture(args.payload, args.thread, args.database, args.title, args.title_index)
    except (ValueError, OSError, sqlite3.Error):
        print('reply capture failed; no event content logged', file=sys.stderr)
        return 2
    print(result)
    return 0


if __name__ == '__main__':
    sys.exit(main())
