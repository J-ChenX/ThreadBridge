use crate::{model::*, store::Store};
fn setup() -> (tempfile::TempDir, Store, String, String, Snapshot) {
    let dir = tempfile::tempdir().unwrap();
    let db = Store::open(&dir.path().join("db.sqlite")).unwrap();
    let (host, _) = db.credential("agent", "test").unwrap();
    let (phone, _) = db.credential("phone", "test").unwrap();
    db.heartbeat(&host).unwrap();
    let s = Snapshot {
        thread: Thread {
            id: key(&host, "native"),
            native_id: "native".into(),
            host_id: host.clone(),
            title: "test".into(),
            status: "idle".into(),
            revision: "turn-1".into(),
            updated_at: now(),
            can_send: true,
            history_cursor: None,
            project: String::new(),
        },
        messages: vec![],
        initial: true,
        history: false,
    };
    db.snapshot(&host, &s).unwrap();
    (dir, db, host, phone, s)
}
#[test]
fn snapshot_waits_for_another_process_writer_without_upgrade_failure() {
    let (dir, db, host, _phone, mut snapshot) = setup();
    snapshot.messages = vec![ChatMessage {
        id: "concurrent-final".into(),
        turn_id: "turn-1".into(),
        role: "assistant".into(),
        text: "durable reply".into(),
        version: hash("durable reply"),
        ordinal: now() * 1000,
    }];
    let writer = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    writer
        .execute_batch(
            "BEGIN IMMEDIATE; UPDATE devices SET last_seen=last_seen+1 WHERE role='agent'",
        )
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        writer.execute_batch("COMMIT").unwrap();
    });
    let result = db.capture_snapshot(&host, &snapshot, "test");
    release.join().unwrap();
    result.unwrap();
    let saved: String =
        db.0.lock()
            .unwrap()
            .query_row(
                "SELECT body FROM messages WHERE id='concurrent-final'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(saved, "durable reply");
}

fn submit(s: &Snapshot) -> Submit {
    Submit {
        request_id: uuid::Uuid::new_v4().to_string(),
        thread_id: s.thread.id.clone(),
        text: "hello".into(),
        expected_revision: s.thread.revision.clone(),
        created_at: now(),
        kind: "send".into(),
        cursor: None,
    }
}
#[test]
fn rename_emits_one_refresh_without_marking_messages_unread_or_notifying() {
    let (_dir, db, host, _phone, mut s) = setup();
    s.thread.status = "completed".into();
    db.snapshot(&host, &s).unwrap();
    let before = db.events(0).unwrap()["cursor"].as_i64().unwrap();
    let message_state = db.threads(0).unwrap()["threads"][0].clone();
    s.initial = false;
    s.thread.title = "Actual name".into();
    db.snapshot(&host, &s).unwrap();
    let renamed = db.threads(0).unwrap()["threads"][0].clone();
    assert_eq!(renamed["title"], "Actual name");
    assert_eq!(renamed["revision"], message_state["revision"]);
    assert_eq!(
        renamed["message_revision"],
        message_state["message_revision"]
    );
    assert_eq!(
        renamed["message_activity_at"],
        message_state["message_activity_at"]
    );
    assert_eq!(db.events(0).unwrap()["cursor"], before + 1);
    db.snapshot(&host, &s).unwrap();
    assert_eq!(db.events(0).unwrap()["cursor"], before + 1);
    assert_eq!(
        db.0.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn visible_message_state_survives_replay_reopen_and_same_timestamp_updates() {
    let (dir, db, host, _phone, mut s) = setup();
    let completed_revision = s.thread.revision.clone();
    let activity = s.thread.updated_at * 1000 + 123;
    s.messages = vec![ChatMessage {
        id: "live-z".into(),
        turn_id: "running".into(),
        role: "assistant".into(),
        text: "working".into(),
        version: hash("working"),
        ordinal: activity,
    }];
    db.snapshot(&host, &s).unwrap();
    let first = db.threads(0).unwrap()["threads"][0].clone();
    assert_eq!(first["message_revision"], 1);
    assert_eq!(first["message_activity_at"], activity);
    db.snapshot(&host, &s).unwrap();
    assert_eq!(db.threads(0).unwrap()["threads"][0]["message_revision"], 1);
    s.thread.title = "renamed".into();
    s.messages.clear();
    db.snapshot(&host, &s).unwrap();
    assert_eq!(db.threads(0).unwrap()["threads"][0]["message_revision"], 1);
    // A second message shares the timestamp and sorts before the first ID.
    s.messages = vec![ChatMessage {
        id: "live-a".into(),
        turn_id: "running".into(),
        role: "user".into(),
        text: "more".into(),
        version: hash("more"),
        ordinal: activity,
    }];
    db.snapshot(&host, &s).unwrap();
    let reopened = Store::open(&dir.path().join("db.sqlite")).unwrap();
    let next = reopened.threads(0).unwrap()["threads"][0].clone();
    assert_eq!(next["revision"], completed_revision);
    assert_eq!(next["updated_at"], first["updated_at"]);
    assert_eq!(next["message_revision"], 2);
    s.messages[0].text = "changed".into();
    s.messages[0].version = hash("changed");
    reopened.snapshot(&host, &s).unwrap();
    assert_eq!(
        reopened.threads(0).unwrap()["threads"][0]["message_revision"],
        3
    );
    reopened
        .delete_source(&host, &s.thread.native_id, Some("live-a"))
        .unwrap();
    assert_eq!(
        reopened.threads(0).unwrap()["threads"][0]["message_revision"],
        4
    );
    reopened.delete_copy(&s.thread.id).unwrap();
    assert_eq!(
        reopened
            .0
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM thread_message_state", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn existing_hub_messages_seed_activity_without_invalidating_read_baselines() {
    let (dir, db, host, _phone, mut s) = setup();
    let activity = s.thread.updated_at * 1000 + 456;
    s.messages = vec![ChatMessage {
        id: "old".into(),
        turn_id: "turn-1".into(),
        role: "assistant".into(),
        text: "old reply".into(),
        version: hash("old reply"),
        ordinal: activity,
    }];
    db.snapshot(&host, &s).unwrap();
    db.0.lock()
        .unwrap()
        .execute("DROP TABLE thread_message_state", [])
        .unwrap();
    db.0.lock()
        .unwrap()
        .execute(
            "DELETE FROM settings WHERE k='message_state_initialized'",
            [],
        )
        .unwrap();
    let upgraded = Store::open(&dir.path().join("db.sqlite")).unwrap();
    let row = upgraded.threads(0).unwrap()["threads"][0].clone();
    assert_eq!(row["message_revision"], 0);
    assert_eq!(row["message_activity_at"], activity);
    upgraded.snapshot(&host, &s).unwrap();
    assert_eq!(
        upgraded.threads(0).unwrap()["threads"][0]["message_revision"],
        0
    );
}
#[test]
fn same_id_is_deduplicated_and_different_content_rejected() {
    let (_d, db, _h, p, s) = setup();
    let mut req = submit(&s);
    let first = db.submit(&p, &req).unwrap();
    assert_eq!(db.submit(&p, &req).unwrap()["id"], first["id"]);
    req.text = "different".into();
    assert!(db.submit(&p, &req).is_err());
}
#[test]
fn crash_does_not_redispatch_and_late_receipt_resolves_unknown() {
    let (_d, db, h, p, s) = setup();
    let result = db.submit(&p, &submit(&s)).unwrap();
    let cmd = db.claim(&h).unwrap().unwrap();
    db.recover().unwrap();
    assert!(db.claim(&h).unwrap().is_none());
    assert_eq!(
        db.command(&p, result["id"].as_str().unwrap()).unwrap()["status"],
        "unknown"
    );
    db.receipt(&h, &cmd.id, "codex_accepted", Some("turn-2"), None)
        .unwrap();
    assert_eq!(db.command(&p, &cmd.id).unwrap()["status"], "codex_accepted");
}
#[test]
fn wrong_host_cannot_ack_and_other_phone_cannot_read() {
    let (_d, db, h, p, s) = setup();
    let r = db.submit(&p, &submit(&s)).unwrap();
    let cmd = db.claim(&h).unwrap().unwrap();
    db.receipt("wrong", &cmd.id, "codex_accepted", Some("turn-2"), None)
        .unwrap();
    assert_eq!(db.command(&p, &cmd.id).unwrap()["status"], "dispatching");
    assert!(db.command("other", r["id"].as_str().unwrap()).is_err());
}
#[test]
fn revoked_credentials_fail_and_pairing_is_single_use() {
    let (_d, db, _h, p, _s) = setup();
    let code = db.pair_code().unwrap();
    assert!(db.pair(&code, "phone").is_ok());
    assert!(db.pair(&code, "phone").is_err());
    db.revoke(&p).unwrap();
    assert!(!db.valid_device(&p));
}
#[test]
fn offline_busy_and_stale_revision_are_rejected() {
    let (_d, db, h, p, s) = setup();
    let mut req = submit(&s);
    req.expected_revision = "stale".into();
    assert!(db.submit(&p, &req).is_err());
    db.disconnected(&h).unwrap();
    assert!(db.submit(&p, &submit(&s)).is_err());
}
#[test]
fn long_unicode_body_is_version_bound_and_not_silently_cut() {
    let (_d, db, h, _p, mut s) = setup();
    let text = "中文🙂".repeat(18000);
    s.messages.push(ChatMessage {
        id: "m".into(),
        turn_id: "t".into(),
        role: "assistant".into(),
        version: hash(&text),
        text: text.clone(),
        ordinal: 1,
    });
    db.snapshot(&h, &s).unwrap();
    let mut all = String::new();
    let mut offset = 0;
    loop {
        let v = db.chunk(&s.thread.id, "m", &hash(&text), offset).unwrap();
        all.push_str(v["text"].as_str().unwrap());
        match v["next_offset"].as_i64() {
            Some(n) => offset = n,
            None => break,
        }
    }
    assert_eq!(all, text);
    assert!(db.chunk(&s.thread.id, "m", "old-version", 0).is_err());
}
#[test]
fn backup_includes_committed_wal_and_refuses_overwrite() {
    let (d, db, _h, _p, _s) = setup();
    let dest = d.path().join("backup.sqlite");
    db.backup(&dest).unwrap();
    assert!(db.backup(&dest).is_err());
    let restored = Store::open(&dest).unwrap();
    assert_eq!(
        restored.threads(0).unwrap()["threads"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn explicit_source_deletion_cannot_be_resurrected_by_stale_snapshot() {
    let (_d, db, h, _p, mut s) = setup();
    s.messages = vec![ChatMessage {
        id: "m".into(),
        turn_id: "t".into(),
        role: "assistant".into(),
        text: "private".into(),
        version: "v1".into(),
        ordinal: 1,
    }];
    db.snapshot(&h, &s).unwrap();
    db.delete_source(&h, "native", Some("m")).unwrap();
    db.snapshot(&h, &s).unwrap();
    assert!(db.messages(&s.thread.id, i64::MAX).unwrap()["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    db.delete_source(&h, "native", None).unwrap();
    db.snapshot(&h, &s).unwrap();
    assert!(db.threads(0).unwrap()["threads"]
        .as_array()
        .unwrap()
        .is_empty());
}
#[test]
fn cancellation_is_only_before_dispatch() {
    let (_d, db, h, p, s) = setup();
    let r = db.submit(&p, &submit(&s)).unwrap();
    let id = r["id"].as_str().unwrap();
    assert_eq!(db.cancel(&p, id).unwrap()["status"], "cancelled");
    assert!(db.claim(&h).unwrap().is_none());
    let r = db.submit(&p, &submit(&s)).unwrap();
    db.claim(&h).unwrap().unwrap();
    assert!(db.cancel(&p, r["id"].as_str().unwrap()).is_err());
}

#[test]
fn expired_command_commits_even_when_claim_queue_is_empty() {
    let (_d, db, h, p, s) = setup();
    let r = db.submit(&p, &submit(&s)).unwrap();
    db.0.lock()
        .unwrap()
        .execute("UPDATE commands SET expires=0", [])
        .unwrap();
    assert!(db.claim(&h).unwrap().is_none());
    let status: String =
        db.0.lock()
            .unwrap()
            .query_row(
                "SELECT status FROM commands WHERE id=?1",
                [r["id"].as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(status, "expired");
}

#[test]
fn offline_expiry_is_visible_when_phone_checks_receipt() {
    let (_d, db, _h, p, s) = setup();
    let r = db.submit(&p, &submit(&s)).unwrap();
    db.0.lock()
        .unwrap()
        .execute("UPDATE commands SET expires=0", [])
        .unwrap();
    assert_eq!(
        db.command(&p, r["id"].as_str().unwrap()).unwrap()["status"],
        "expired"
    );
}

#[test]
fn each_command_transition_commits_a_sync_event() {
    let (_d, db, h, p, s) = setup();
    let cursor = db.events(0).unwrap()["cursor"].as_i64().unwrap();
    let r = db.submit(&p, &submit(&s)).unwrap();
    let cmd = db.claim(&h).unwrap().unwrap();
    db.disconnected(&h).unwrap();
    db.receipt(&h, &cmd.id, "codex_accepted", Some("turn-2"), None)
        .unwrap();
    db.receipt(&h, &cmd.id, "codex_accepted", Some("turn-2"), None)
        .unwrap();
    let events = db.events(cursor).unwrap();
    assert_eq!(events["events"].as_array().unwrap().len(), 4);
    assert!(events["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["kind"] == "command" && e["thread_id"] == s.thread.id));
    assert_eq!(
        db.command(&p, r["id"].as_str().unwrap()).unwrap()["status"],
        "codex_accepted"
    );
}

#[test]
fn message_pages_bound_bytes_and_do_not_skip_tied_ordinals() {
    let (_d, db, h, _p, mut s) = setup();
    // Each snapshot is under the transport budget; the persisted page is much larger.
    for batch in 0..9 {
        s.messages = (0..5)
            .map(|n| ChatMessage {
                id: format!("m-{:03}", batch * 5 + n),
                turn_id: "t".into(),
                role: "assistant".into(),
                text: "中🙂".repeat(16000),
                version: "v1".into(),
                ordinal: 7,
            })
            .collect();
        db.snapshot(&h, &s).unwrap();
    }
    let mut before = i64::MAX;
    let mut before_id = None;
    let mut ids = std::collections::HashSet::new();
    loop {
        let page = db
            .messages_page(&s.thread.id, before, before_id.as_deref())
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 270 * 1024);
        for m in page["messages"].as_array().unwrap() {
            assert!(ids.insert(m["id"].as_str().unwrap().to_string()));
        }
        let Some(next) = page["next_before"].as_i64() else {
            break;
        };
        before = next;
        before_id = Some(page["next_before_id"].as_str().unwrap().to_string());
    }
    assert_eq!(ids.len(), 45);
}

#[test]
fn revoked_or_expired_host_never_advertises_send_availability() {
    let (_d, db, h, _p, _s) = setup();
    db.0.lock()
        .unwrap()
        .execute("UPDATE devices SET expires=0 WHERE id=?1", [&h])
        .unwrap();
    assert_eq!(db.threads(0).unwrap()["threads"][0]["host_online"], false);
    assert_eq!(db.threads(0).unwrap()["threads"][0]["can_send"], false);
    assert_eq!(db.hosts().unwrap()["hosts"][0]["online"], false);
    db.0.lock()
        .unwrap()
        .execute(
            "UPDATE devices SET expires=?2,revoked=1 WHERE id=?1",
            rusqlite::params![h, now() + 3600],
        )
        .unwrap();
    assert_eq!(db.threads(0).unwrap()["threads"][0]["can_send"], false);
}

#[test]
fn history_import_does_not_notify_and_new_notification_has_ttl() {
    let (_d, db, h, _p, mut s) = setup();
    s.initial = false;
    s.history = true;
    s.thread.status = "completed".into();
    s.thread.revision = "old-completion".into();
    db.snapshot(&h, &s).unwrap();
    let count: i64 =
        db.0.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM outbox", [], |r| r.get(0))
            .unwrap();
    assert_eq!(count, 0);
    s.history = false;
    s.thread.revision = "new-completion".into();
    db.snapshot(&h, &s).unwrap();
    db.snapshot(&h, &s).unwrap();
    let (count, expiry): (i64, i64) =
        db.0.lock()
            .unwrap()
            .query_row("SELECT count(*),expires FROM outbox", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
    assert_eq!(count, 1);
    assert!(expiry >= now() + 86390);
}

#[test]
fn legacy_outbox_migration_does_not_replay_old_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE outbox(id TEXT PRIMARY KEY,thread TEXT NOT NULL,status TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,next_at INTEGER NOT NULL,sent INTEGER NOT NULL DEFAULT 0); INSERT INTO outbox(id,thread,status,next_at) VALUES('old','t','completed',0);").unwrap();
    drop(c);
    let db = Store::open(&path).unwrap();
    let expiry: i64 =
        db.0.lock()
            .unwrap()
            .query_row("SELECT expires FROM outbox WHERE id='old'", [], |r| {
                r.get(0)
            })
            .unwrap();
    assert_eq!(expiry, 0);
}

#[test]
fn new_revision_reopens_history_after_offline_gap_without_resetting_same_revision_progress() {
    let (_d, db, h, _p, mut s) = setup();
    // The user had exhausted all earlier source history.
    s.history = true;
    s.thread.history_cursor = None;
    db.snapshot(&h, &s).unwrap();
    // More than the recent-window size arrived while the Agent was offline.
    s.history = false;
    s.thread.revision = "after-ten-new-turns".into();
    s.thread.history_cursor = Some("middle-seven-turns".into());
    db.snapshot(&h, &s).unwrap();
    assert_eq!(
        db.threads(0).unwrap()["threads"][0]["history_cursor"],
        "middle-seven-turns"
    );
    // Loading the next history page advances progress for this source revision.
    s.history = true;
    s.thread.history_cursor = Some("older-page".into());
    db.snapshot(&h, &s).unwrap();
    s.history = false;
    s.thread.history_cursor = Some("middle-seven-turns".into());
    db.snapshot(&h, &s).unwrap();
    assert_eq!(
        db.threads(0).unwrap()["threads"][0]["history_cursor"],
        "older-page"
    );
    s.history = true;
    s.thread.history_cursor = None;
    db.snapshot(&h, &s).unwrap();
    s.history = false;
    s.thread.history_cursor = Some("middle-seven-turns".into());
    db.snapshot(&h, &s).unwrap();
    assert!(db.threads(0).unwrap()["threads"][0]["history_cursor"].is_null());
}

#[test]
fn copy_deletion_survives_reopen_and_cannot_resurrect_or_redispatch() {
    let (d, db, h, p, mut s) = setup();
    s.messages.push(ChatMessage {
        id: "private".into(),
        turn_id: "turn-1".into(),
        role: "user".into(),
        text: "private content".into(),
        version: "v".into(),
        ordinal: 1,
    });
    db.snapshot(&h, &s).unwrap();
    let req = submit(&s);
    let receipt = db.submit(&p, &req).unwrap();
    db.delete_copy(&s.thread.id).unwrap();
    assert!(db.claim(&h).unwrap().is_none());
    assert_eq!(db.submit(&p, &req).unwrap()["id"], receipt["id"]);
    assert_eq!(
        db.command(&p, receipt["id"].as_str().unwrap()).unwrap()["status"],
        "cancelled"
    );
    let reopened = Store::open(&d.path().join("db.sqlite")).unwrap();
    reopened.snapshot(&h, &s).unwrap();
    assert_eq!(
        reopened.threads(0).unwrap()["threads"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        reopened.messages(&s.thread.id, i64::MAX).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(reopened.valid_device(&p));
    assert_eq!(
        reopened.hosts().unwrap()["hosts"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        reopened.threads(0).unwrap()["collection_generation"],
        reopened.events(0).unwrap()["collection_generation"]
    );
}
#[test]
fn private_backup_copy_keeps_credentials_and_rejects_links() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("credential.json");
    let dest = dir.path().join("bundle");
    std::fs::write(&source, b"synthetic credential").unwrap();
    crate::private_directory(&dest).unwrap();
    crate::copy_private(&source, &dest.join("credential.json")).unwrap();
    assert_eq!(
        std::fs::read(dest.join("credential.json")).unwrap(),
        b"synthetic credential"
    );
    assert!(crate::copy_private(&source, &dest.join("credential.json")).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(dest.join("credential.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(source, &link).unwrap();
        assert!(crate::copy_private(&link, &dest.join("link")).is_err());
    }
}
