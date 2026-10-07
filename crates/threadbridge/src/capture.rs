//! Bounded local import of allowlisted notify replies into the existing Hub projection.
use crate::{model::*, store::Store};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub fn import(db: &Store, source: &Path, host: &str, native: &str) -> Result<usize> {
    import_title(db, source, host, native, None)
}

fn import_title(
    db: &Store,
    source: &Path,
    host: &str,
    native: &str,
    registered_title: Option<&str>,
) -> Result<usize> {
    uuid::Uuid::parse_str(native).context("invalid_capture_thread")?;
    let valid: bool = db.0.lock().unwrap().query_row(
        "SELECT count(*)>0 FROM devices WHERE id=?1 AND role='agent' AND revoked=0 AND expires>?2",
        rusqlite::params![host, now()],
        |r| r.get(0),
    )?;
    anyhow::ensure!(valid, "capture_requires_existing_authorized_host");
    if db.0.lock().unwrap().query_row(
        "SELECT count(*)>0 FROM tombstones WHERE thread=?1 AND message=''",
        [key(host, native)],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(0);
    }
    db.0.lock().unwrap().execute(
        "INSERT OR IGNORE INTO capture_targets(thread,host,native) VALUES(?1,?2,?3)",
        rusqlite::params![key(host, native), host, native],
    )?;
    let enabled: bool = db.0.lock().unwrap().query_row(
        "SELECT queue_enabled=1 AND last_seen>?2 FROM capture_targets WHERE thread=?1",
        rusqlite::params![key(host, native), now() - 15],
        |r| r.get(0),
    )?;
    let resume_owner: bool = db.0.lock().unwrap().query_row(
        "SELECT count(*)>0 FROM resume_owners WHERE thread=?1 AND expires>?2 AND last_seen>?3",
        rusqlite::params![key(host, native), now(), now() - 15],
        |r| r.get(0),
    )?;
    let ready_status = if resume_owner {
        "resume_ready"
    } else {
        "queue_ready"
    };
    let source = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(std::time::Duration::from_secs(3))?;
    // One consistent capture snapshot; never discover tasks or read session files.
    source.execute_batch("BEGIN")?;
    let count: i64 = source.query_row(
        "SELECT count(*) FROM captured_replies WHERE thread_id=?1",
        [native],
        |r| r.get(0),
    )?;
    if count == 0 {
        return Ok(0);
    }
    let last_row: i64 = db.0.lock().unwrap().query_row(
        "SELECT coalesce((SELECT last_row FROM capture_import_positions WHERE thread=?1),0)",
        [key(host, native)],
        |r| r.get(0),
    )?;
    let has_order: bool = source.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name='captured_turn_order'",
        [],
        |r| r.get(0),
    )?;
    let latest_sql = if has_order {
        "SELECT r.turn_id,coalesce(o.completed_at/1000,r.captured_at) FROM captured_replies r LEFT JOIN captured_turn_order o ON o.thread_id=r.thread_id AND o.turn_id=r.turn_id WHERE r.thread_id=?1 ORDER BY coalesce(o.completed_at,r.captured_at*1000) DESC,r.rowid DESC LIMIT 1"
    } else {
        "SELECT turn_id,captured_at FROM captured_replies WHERE thread_id=?1 ORDER BY rowid DESC LIMIT 1"
    };
    let (latest_turn, updated): (String, i64) =
        source.query_row(latest_sql, [native], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let has_title: bool = {
        let mut q = source.prepare("PRAGMA table_info(captured_replies)")?;
        let names = q
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        names.iter().any(|n| n == "title")
    };
    let fallback = registered_title
        .map(str::to_owned)
        .unwrap_or_else(|| format!("会话 · {}", &native[..8]));
    let title = if has_title {
        let value: String = source.query_row(
            "SELECT title FROM captured_replies WHERE thread_id=?1 ORDER BY rowid DESC LIMIT 1",
            [native],
            |r| r.get(0),
        )?;
        if value.trim().is_empty() {
            fallback.clone()
        } else {
            value
        }
    } else {
        fallback.clone()
    };
    anyhow::ensure!(title.len() <= 512, "invalid_capture_title");
    let project = if source.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name='captured_projects'",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        use rusqlite::OptionalExtension;
        source
            .query_row(
                "SELECT project_path FROM captured_projects WHERE thread_id=?1",
                [native],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default()
    } else {
        String::new()
    };
    let projection = Thread {
        id: key(host, native),
        native_id: native.into(),
        host_id: host.into(),
        title: title.clone(),
        status: if enabled {
            ready_status
        } else {
            "capture_only"
        }
        .into(),
        revision: latest_turn.clone(),
        updated_at: updated,
        can_send: enabled,
        history_cursor: None,
        project,
    };
    let has_users: bool = source.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name='captured_user_messages'",
        [],
        |r| r.get(0),
    )?;
    // Update readiness/title even with no new body; persisted cursor skips only bodies.
    db.capture_snapshot(
        host,
        &Snapshot {
            thread: projection.clone(),
            messages: vec![],
            initial: true,
            history: false,
        },
        &title,
    )?;
    let user_position: i64 = db.0.lock().unwrap().query_row(
        "SELECT coalesce((SELECT last_row FROM capture_user_import_positions WHERE thread=?1),0)",
        [&projection.id],
        |r| r.get(0),
    )?;
    let sql = if has_title {
        "SELECT turn_id,reply,utf8_bytes,captured_at,title,rowid FROM captured_replies WHERE thread_id=?1 AND rowid>?2 ORDER BY rowid"
    } else {
        "SELECT turn_id,reply,utf8_bytes,captured_at,'',rowid FROM captured_replies WHERE thread_id=?1 AND rowid>?2 ORDER BY rowid"
    };
    let sql = if has_users {
        sql.replace("rowid>?2", "(rowid>?2 OR turn_id IN (SELECT turn_id FROM captured_user_messages WHERE thread_id=?1 AND rowid>?3))")
    } else {
        sql.to_owned()
    };
    let mut query = source.prepare(&sql)?;
    let mut rows = if has_users {
        query.query(rusqlite::params![native, last_row, user_position])?
    } else {
        query.query(rusqlite::params![native, last_row])?
    };
    let mut imported = 0;
    while let Some(row) = rows.next()? {
        let turn: String = row.get(0)?;
        uuid::Uuid::parse_str(&turn).context("invalid_capture_turn")?;
        let text: String = row.get(1)?;
        let size: i64 = row.get(2)?;
        let captured: i64 = row.get(3)?;
        anyhow::ensure!(
            size == text.len() as i64 && size <= 256 * 1024 && !text.is_empty(),
            "invalid_capture_reply"
        );
        let record_title: String = row.get(4)?;
        let record_title = if record_title.trim().is_empty() {
            fallback.clone()
        } else {
            record_title
        };
        anyhow::ensure!(record_title.len() <= 512, "invalid_capture_title");
        let ordinal = if has_users {
            use rusqlite::OptionalExtension;
            source.query_row("SELECT completed_at FROM captured_turn_order WHERE thread_id=?1 AND turn_id=?2",rusqlite::params![native,turn],|r|r.get::<_,i64>(0)).optional()?.unwrap_or(captured.saturating_mul(1000))
        } else {
            captured.saturating_mul(1000)
        };
        db.capture_snapshot(
            host,
            &Snapshot {
                thread: projection.clone(),
                messages: vec![ChatMessage {
                    id: format!("notify:{turn}"),
                    turn_id: turn.clone(),
                    role: "assistant".into(),
                    version: hash(&text),
                    text,
                    ordinal,
                }],
                initial: true,
                history: false,
            },
            &record_title,
        )?;
        // Keep all UUID-only candidates; disambiguate only with durable active
        // command, original host/thread, expected prior completion and time.
        let mut requests = std::collections::BTreeSet::<String>::new();
        for table in ["captured_request_ids", "captured_request_candidates"] {
            let exists: bool = source.query_row(
                "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )?;
            if exists {
                let sql =
                    format!("SELECT request_id FROM {table} WHERE thread_id=?1 AND turn_id=?2");
                let mut q = source.prepare(&sql)?;
                for id in q.query_map(rusqlite::params![native, turn], |r| r.get::<_, String>(0))? {
                    requests.insert(id?);
                }
            }
        }
        use rusqlite::OptionalExtension;
        let row_id: i64 = row.get(5)?;
        let prior_sql = if has_order {
            "SELECT r.turn_id FROM captured_replies r LEFT JOIN captured_turn_order o ON o.thread_id=r.thread_id AND o.turn_id=r.turn_id WHERE r.thread_id=?1 AND (coalesce(o.completed_at,r.captured_at*1000),r.rowid)<(SELECT coalesce(c.completed_at,v.captured_at*1000),v.rowid FROM captured_replies v LEFT JOIN captured_turn_order c ON c.thread_id=v.thread_id AND c.turn_id=v.turn_id WHERE v.rowid=?2) ORDER BY coalesce(o.completed_at,r.captured_at*1000) DESC,r.rowid DESC LIMIT 1"
        } else {
            "SELECT turn_id FROM captured_replies WHERE thread_id=?1 AND rowid<?2 ORDER BY rowid DESC LIMIT 1"
        };
        let prior: Option<String> = source
            .query_row(prior_sql, rusqlite::params![native, row_id], |r| r.get(0))
            .optional()?;
        let mut eligible = Vec::new();
        if requests.len() <= 16 {
            let c = db.0.lock().unwrap();
            for request in requests {
                let evidence:Option<(String,i64)>=c.query_row("SELECT json_extract(x.payload,'$.expected_revision'),x.created FROM commands x JOIN capture_queue_ledger q ON q.id=x.id WHERE x.id=?1 AND x.host=?2 AND x.thread=?3 AND x.status IN ('dispatching','unknown','upstream_queued') AND q.status IN ('intent','unknown','upstream_queued')",rusqlite::params![request,host,key(host,native)],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                if let Some((expected, created)) = evidence {
                    if prior.as_deref() == Some(&expected) && created <= captured {
                        eligible.push(request);
                    }
                }
            }
        }
        if has_users {
            // Only current-turn human input, not notify's cumulative input list.
            // Require one active command, the preceding revision, exact input
            // digest and a source input time after the durable send intent.
            let c = db.0.lock().unwrap();
            let mut q = c.prepare("SELECT x.id,json_extract(x.payload,'$.expected_revision'),json_extract(x.payload,'$.text'),s.started_at,coalesce((SELECT cr.thread=x.thread FROM creation_results cr WHERE cr.command=x.id),0) AND json_extract(x.payload,'$.kind')='create' FROM commands x JOIN capture_queue_ledger q ON q.id=x.id LEFT JOIN capture_queue_send_times s ON s.id=x.id WHERE x.host=?1 AND x.thread=?2 AND x.status IN ('dispatching','unknown','upstream_queued') AND q.status IN ('intent','unknown','upstream_queued')")?;
            let active = q
                .query_map(rusqlite::params![host, key(host, native)], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, bool>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if let [(request, Some(expected), Some(input), Some(created), created_native)] =
                active.as_slice()
            {
                let matches: bool = source.query_row("SELECT count(*)=1 FROM captured_user_messages WHERE thread_id=?1 AND turn_id=?2 AND input_digest=?3 AND created_at>=?4 AND created_at<=?5",rusqlite::params![native,turn,hash(input),created,captured.saturating_mul(1000).saturating_add(999)],|r|r.get(0))?;
                if (prior.as_deref() == Some(expected)
                    || (*created_native && expected.is_empty() && prior.is_none()))
                    && matches
                {
                    eligible.push(request.clone());
                }
            }
        }
        eligible.sort();
        eligible.dedup();
        if eligible.len() == 1 {
            let request = &eligible[0];
            db.receipt(host, request, "codex_accepted", Some(&turn), None)?;
            db.0.lock().unwrap().execute(
                "UPDATE capture_queue_ledger SET status='codex_accepted' WHERE id=?1",
                [request],
            )?;
        }
        let row_id: i64 = row.get(5)?;
        db.0.lock().unwrap().execute("INSERT INTO capture_import_positions(thread,last_row) VALUES(?1,?2) ON CONFLICT(thread) DO UPDATE SET last_row=max(last_row,excluded.last_row)",rusqlite::params![key(host,native),row_id])?;
        imported += 1;
    }
    if has_users {
        let position: i64 = db.0.lock().unwrap().query_row("SELECT coalesce((SELECT last_row FROM capture_user_import_positions WHERE thread=?1),0)",[&projection.id],|r|r.get(0))?;
        let mut orders = source.prepare("SELECT r.turn_id,r.reply,o.completed_at FROM captured_replies r JOIN captured_turn_order o ON o.thread_id=r.thread_id AND o.turn_id=r.turn_id WHERE r.thread_id=?1 AND r.turn_id IN (SELECT turn_id FROM captured_user_messages WHERE thread_id=?1 AND rowid>?2)")?;
        for row in orders.query_map(rusqlite::params![native, user_position], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })? {
            let (turn, text, ordinal) = row?;
            db.capture_snapshot(
                host,
                &Snapshot {
                    thread: projection.clone(),
                    messages: vec![ChatMessage {
                        id: format!("notify:{turn}"),
                        turn_id: turn,
                        role: "assistant".into(),
                        version: hash(&text),
                        text,
                        ordinal,
                    }],
                    initial: true,
                    history: false,
                },
                &title,
            )?;
        }
        let mut q = source.prepare("SELECT rowid,turn_id,message_id,text,created_at FROM captured_user_messages WHERE thread_id=?1 AND rowid>?2 ORDER BY rowid")?;
        for row in q.query_map(rusqlite::params![native, position], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })? {
            let (row_id, turn, id, text, created) = row?;
            uuid::Uuid::parse_str(&turn)?;
            anyhow::ensure!(
                !text.is_empty() && text.len() <= 256 * 1024 && id.len() <= 128,
                "invalid_capture_user_input"
            );
            db.capture_snapshot(
                host,
                &Snapshot {
                    thread: projection.clone(),
                    messages: vec![ChatMessage {
                        id: format!("input:{id}"),
                        turn_id: turn,
                        role: "user".into(),
                        version: hash(&text),
                        text,
                        ordinal: created,
                    }],
                    initial: true,
                    history: false,
                },
                &title,
            )?;
            db.0.lock().unwrap().execute("INSERT INTO capture_user_import_positions(thread,last_row) VALUES(?1,?2) ON CONFLICT(thread) DO UPDATE SET last_row=excluded.last_row",rusqlite::params![projection.id,row_id])?;
        }
    }
    if source.query_row("SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name='captured_visible_messages'",[],|r|r.get::<_,bool>(0))? {
        let position: i64 = db.0.lock().unwrap().query_row("SELECT coalesce((SELECT last_row FROM capture_visible_import_positions WHERE thread=?1),0)",[&projection.id],|r|r.get(0))?;
        let mut q = source.prepare("SELECT rowid,turn_id,message_id,text,created_at FROM captured_visible_messages WHERE thread_id=?1 AND rowid>?2 ORDER BY rowid")?;
        for row in q.query_map(rusqlite::params![native,position],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?)))? {
            let (row,turn,id,text,ordinal)=row?;
            uuid::Uuid::parse_str(&turn)?;
            anyhow::ensure!(!text.is_empty() && text.len()<=256*1024 && id.len()<=128,"invalid_visible_message");
            db.capture_snapshot(host,&Snapshot {thread:projection.clone(),messages:vec![ChatMessage{id:format!("native:{id}"),turn_id:turn,role:"assistant".into(),version:hash(&text),text,ordinal}],initial:true,history:false},&title)?;
            db.0.lock().unwrap().execute("INSERT INTO capture_visible_import_positions VALUES(?1,?2) ON CONFLICT(thread) DO UPDATE SET last_row=excluded.last_row",rusqlite::params![projection.id,row])?;
        }
    }
    if source.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name='captured_images'",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        let mut q = source
            .prepare("SELECT message_id,image_id,mime FROM captured_images WHERE thread_id=?1")?;
        for row in q.query_map([native], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (message, image, mime) = row?;
            let hub_message = format!("input:{message}");
            let c = db.0.lock().unwrap();
            if c.query_row(
                "SELECT count(*)>0 FROM attachments WHERE thread=?1 AND message=?2 AND image=?3",
                rusqlite::params![projection.id, hub_message, image],
                |r| r.get::<_, bool>(0),
            )? {
                continue;
            }
            let bytes:Vec<u8>=source.query_row("SELECT bytes FROM captured_images WHERE thread_id=?1 AND message_id=?2 AND image_id=?3",rusqlite::params![native,message,image],|r|r.get(0))?;
            let used: i64 = c.query_row(
                "SELECT coalesce(sum(length(bytes)),0) FROM attachments",
                [],
                |r| r.get(0),
            )?;
            anyhow::ensure!(
                used + bytes.len() as i64 <= 64 * 1024 * 1024,
                "image_storage_budget"
            );
            anyhow::ensure!(
                image == hash_bytes(&bytes)
                    && bytes.len() <= 2 * 1024 * 1024
                    && ["image/png", "image/jpeg", "image/webp"].contains(&mime.as_str()),
                "invalid_image"
            );
            c.execute("INSERT OR IGNORE INTO attachments SELECT ?1,?2,?3,?4,?5 WHERE EXISTS(SELECT 1 FROM messages WHERE thread=?1 AND id=?2) AND NOT EXISTS(SELECT 1 FROM tombstones WHERE thread=?1 AND (message='' OR message=?2))",rusqlite::params![projection.id,format!("input:{message}"),image,mime,bytes])?;
        }
    }
    Ok(imported)
}
fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Independent read projection: no Codex process, CLI, model, or send permission.
pub fn import_projects(db: &Store, source: &Path, host: &str) -> Result<()> {
    let c = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut paths = std::collections::BTreeSet::new();
    for table in ["captured_project_catalog"] {
        if !c.query_row(
            "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get::<_, bool>(0),
        )? {
            continue;
        }
        for path in c
            .prepare(&format!(
                "SELECT DISTINCT project_path FROM {table} LIMIT 1000"
            ))?
            .query_map([], |r| r.get::<_, String>(0))?
        {
            let path = path?;
            if !path.is_empty() && path.len() <= 2048 && !path.contains(['\0', '\r', '\n']) {
                paths.insert(path);
            }
        }
    }
    let mut hub = db.0.lock().unwrap();
    let tx = hub.transaction()?;
    tx.execute("DELETE FROM host_projects WHERE host=?1", [host])?;
    for path in paths.into_iter().take(1000) {
        tx.execute(
            "INSERT INTO host_projects VALUES(?1,?2)",
            rusqlite::params![host, path],
        )?;
    }
    let exists = |table: &str| -> Result<bool> {
        Ok(c.query_row(
            "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )?)
    };
    if exists("captured_project_names")? {
        tx.execute("DELETE FROM project_names WHERE host=?1", [host])?;
        for row in c
            .prepare("SELECT project_path,project_name FROM captured_project_names LIMIT 1000")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (path, name) = row?;
            if path.len() <= 2048 && name.len() <= 512 {
                tx.execute(
                    "INSERT INTO project_names VALUES(?1,?2,?3)",
                    rusqlite::params![host, path, name],
                )?;
            }
        }
    }
    if exists("captured_projects")? {
        tx.execute(
            "DELETE FROM thread_projects WHERE thread IN (SELECT id FROM threads WHERE host=?1)",
            [host],
        )?;
        for row in c
            .prepare("SELECT thread_id,project_path FROM captured_projects LIMIT 100000")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (native, path) = row?;
            let id = key(host, &native);
            if path.len() <= 2048 {
                tx.execute("INSERT INTO thread_projects SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM threads WHERE id=?1 AND host=?3) ON CONFLICT(thread) DO UPDATE SET project=excluded.project",rusqlite::params![id,path,host])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

pub async fn sync_catalog(
    db: Store,
    source: std::path::PathBuf,
    host: String,
    catalog: Option<std::path::PathBuf>,
    all_captured: bool,
    native_index: Option<std::path::PathBuf>,
) -> Result<()> {
    use std::io::Read;
    anyhow::ensure!(
        catalog.is_some() != all_captured,
        "explicit_capture_scope_required"
    );
    let mut titles = std::collections::BTreeMap::<String, String>::new();
    if let Some(catalog) = catalog {
        let mut bytes = Vec::new();
        std::fs::File::open(catalog)?
            .take(512 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 512 * 1024, "catalog_limit");
        titles = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(!titles.is_empty() && titles.len() <= 1000, "catalog_limit");
        for (native, title) in &titles {
            uuid::Uuid::parse_str(native)?;
            anyhow::ensure!(
                !title.trim().is_empty() && title.len() <= 512,
                "invalid_catalog_title"
            );
        }
    }
    let valid: bool = db.0.lock().unwrap().query_row(
        "SELECT count(*)>0 FROM devices WHERE id=?1 AND role='agent' AND revoked=0 AND expires>?2",
        rusqlite::params![host, now()],
        |r| r.get(0),
    )?;
    anyhow::ensure!(valid, "capture_requires_existing_authorized_host");
    let mut fingerprints = std::collections::BTreeMap::new();
    loop {
        if let Some(index) = &native_index {
            let _ = crate::native_remote::refresh_projects(&source, index);
        }
        let _ = import_projects(&db, &source, &host);
        let selected = if all_captured {
            let read = || -> Result<std::collections::BTreeMap<String, String>> {
                let c = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                let mut q = c.prepare("SELECT DISTINCT thread_id FROM captured_replies")?;
                let natives = q
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for id in &natives {
                    uuid::Uuid::parse_str(id)?;
                }
                Ok(natives.into_iter().map(|n| (n, String::new())).collect())
            };
            match read() {
                Ok(v) => v,
                Err(_) => {
                    save_health(
                        &db,
                        &host,
                        serde_json::json!({"failures":[],"overflow":false,"projection_error":"capture_source_unavailable"}),
                    )?;
                    eprintln!("Completion projection unavailable; retrying without sending");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            }
        } else {
            titles.clone()
        };
        let mut health=read_health(&source).unwrap_or_else(|_|serde_json::json!({"failures":{},"overflow":false,"projection_error":"capture_status_unavailable"}));
        if !all_captured {
            if let Some(failures) = health
                .get_mut("failures")
                .and_then(serde_json::Value::as_object_mut)
            {
                failures.retain(|_, v| {
                    v["thread_id"]
                        .as_str()
                        .is_some_and(|id| selected.contains_key(id))
                });
            }
        }
        for (native, title) in &selected {
            if let Some(index) = &native_index {
                let refresh = (|| -> Result<()> {
                    let c = crate::collection::readonly(index)?;
                    let path: String = c.query_row(
                        "SELECT rollout_path FROM threads WHERE id=?1",
                        [native],
                        |r| r.get(0),
                    )?;
                    let fingerprint = crate::native_remote::fingerprint(Path::new(&path))?;
                    if fingerprints.get(native) != Some(&fingerprint) {
                        crate::visible_history::backfill(&source, index, native)?;
                        fingerprints.insert(native.clone(), fingerprint);
                    }
                    Ok(())
                })();
                if let Err(error) = refresh {
                    health["projection_error"] =
                        serde_json::json!("visible_history_projection_failed");
                    eprintln!("Visible history unavailable for {}: {error}", &native[..8]);
                }
            }
            let hint = if title.is_empty() {
                None
            } else {
                Some(title.as_str())
            };
            if let Err(error) = import_title(&db, &source, &host, native, hint) {
                health["projection_error"] = serde_json::json!("completion_projection_failed");
                eprintln!(
                    "Completion projection unavailable for {}: {error}; retrying without sending",
                    &native[..8]
                );
            }
        }
        save_health(&db, &host, health)?;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

pub(crate) fn save_health(db: &Store, host: &str, status: serde_json::Value) -> Result<()> {
    db.0.lock().unwrap().execute("INSERT INTO capture_health(host,status,checked_at) VALUES(?1,?2,?3) ON CONFLICT(host) DO UPDATE SET status=excluded.status,checked_at=excluded.checked_at",rusqlite::params![host,status.to_string(),now()])?;
    Ok(())
}

fn read_health(source: &Path) -> Result<serde_json::Value> {
    use std::io::Read;
    let path = std::path::PathBuf::from(format!("{}.health", source.display()));
    anyhow::ensure!(
        !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
        "invalid_capture_status"
    );
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(131073)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() == 131072, "invalid_capture_status");
    let mut valid = Vec::new();
    for slot in bytes.as_chunks::<65536>().0 {
        if let Ok(row) = serde_json::from_slice::<serde_json::Value>(slot) {
            if let (Some(data), Some(digest), Some(generation)) = (
                row["data"].as_str(),
                row["sha256"].as_str(),
                row["generation"].as_u64(),
            ) {
                if hash(data) == digest {
                    if let Ok(state) = serde_json::from_str::<serde_json::Value>(data) {
                        if state["failures"].is_object() {
                            valid.push((generation, state));
                        }
                    }
                }
            }
        }
    }
    valid.sort_by_key(|x| x.0);
    valid
        .pop()
        .map(|x| x.1)
        .ok_or_else(|| anyhow::anyhow!("invalid_capture_status"))
}

/// Discover only conversations already present in the configured capture database.
fn bridge_threads(source: &Path, native: Option<&str>) -> Result<Vec<String>> {
    if let Some(native) = native {
        uuid::Uuid::parse_str(native)?;
        return Ok(vec![native.to_owned()]);
    }
    let c = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_secs(3))?;
    let mut q = c.prepare("SELECT DISTINCT thread_id FROM captured_replies ORDER BY thread_id")?;
    let ids = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in &ids {
        uuid::Uuid::parse_str(id)?;
    }
    Ok(ids)
}

pub async fn run(
    db: Store,
    source: std::path::PathBuf,
    host: String,
    native: Option<String>,
    codex: std::path::PathBuf,
    enabled: bool,
    verified: Option<String>,
) -> Result<()> {
    if let Some(native) = &native {
        uuid::Uuid::parse_str(native)?;
    }
    if enabled {
        let version = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::process::Command::new(&codex)
                .arg("--version")
                .output(),
        )
        .await??;
        anyhow::ensure!(
            version.status.success()
                && verified.as_deref() == Some(String::from_utf8(version.stdout)?.trim()),
            "queue_version_unverified"
        );
    }
    {
        let pending: Vec<String> = {
            let c = db.0.lock().unwrap();
            let mut q=c.prepare("SELECT l.id FROM capture_queue_ledger l JOIN commands c ON c.id=l.id WHERE l.status='intent' AND c.host=?1 AND (?2 IS NULL OR c.thread=?2)")?;
            let selected = native.as_ref().map(|n| key(&host, n));
            let values = q
                .query_map(rusqlite::params![host, selected], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            values
        };
        for id in pending {
            db.receipt(
                &host,
                &id,
                "unknown",
                None,
                Some("interrupted_queue_no_retry"),
            )?;
            db.0.lock().unwrap().execute(
                "UPDATE capture_queue_ledger SET status='unknown' WHERE id=?1",
                [id],
            )?;
        }
    }
    loop {
        db.heartbeat(&host)?;
        if enabled && native.is_none() {
            db.0.lock().unwrap().execute("INSERT INTO creation_hosts VALUES(?1,?2) ON CONFLICT(host) DO UPDATE SET last_seen=excluded.last_seen",rusqlite::params![host,now()])?;
            if let Some(cmd) = db.claim_creation(&host)? {
                if crate::new_threads::dispatch(
                    &db,
                    &source,
                    &codex,
                    &host,
                    verified.as_deref().unwrap_or(""),
                    &cmd,
                )
                .await
                .is_err()
                {
                    db.0.lock().unwrap().execute(
                        "UPDATE capture_queue_ledger SET status='unknown' WHERE id=?1",
                        [&cmd.id],
                    )?;
                    db.receipt(
                        &host,
                        &cmd.id,
                        "unknown",
                        None,
                        Some("creation_unconfirmed_no_retry"),
                    )?;
                }
            }
        }
        let _ = import_projects(&db, &source, &host);
        match bridge_threads(&source, native.as_deref()) {
            Err(_) => eprintln!("Capture discovery unavailable; retrying without sending"),
            Ok(threads) => {
                // Refresh every target before potentially slow queue commands so that
                // unrelated conversations do not lose readiness during a submission.
                for id in &threads {
                    db.0.lock().unwrap().execute(
                        "INSERT INTO capture_targets(thread,host,native,queue_enabled,last_seen) SELECT ?1,?2,?3,?4,?5 WHERE NOT EXISTS(SELECT 1 FROM tombstones WHERE thread=?1 AND message='') ON CONFLICT(thread) DO UPDATE SET queue_enabled=excluded.queue_enabled,last_seen=excluded.last_seen",
                        rusqlite::params![key(&host,id),host,id,enabled,now()],
                    )?;
                }
                for id in threads {
                    if import(&db, &source, &host, &id).is_err() {
                        eprintln!("Capture sync unavailable; retrying without sending");
                    } else if enabled {
                        if let Some(cmd) = db.claim_capture(&host, &key(&host, &id))? {
                            if crate::queue::dispatch(&db, &codex, &host, &id, &cmd)
                                .await
                                .is_err()
                            {
                                db.receipt(
                                    &host,
                                    &cmd.id,
                                    "unknown",
                                    None,
                                    Some("queue_dispatch_unconfirmed"),
                                )?;
                            }
                        }
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const THREAD: &str = "00000000-0000-4000-8000-000000000001";
    const TURN: &str = "00000000-0000-4000-8000-000000000002";
    #[test]
    fn captured_reply_uses_phone_projection_and_replay_survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("capture.sqlite");
        let source = Connection::open(&capture).unwrap();
        source.execute_batch("CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER)").unwrap();
        source
            .execute(
                "INSERT INTO captured_replies VALUES(?1,?2,'TB-NOTIFY-OK',12,1)",
                [THREAD, TURN],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO captured_replies VALUES(?1,?2,'OTHER-TASK',10,2)",
                [TURN, THREAD],
            )
            .unwrap();
        let path = tmp.path().join("hub.sqlite");
        let hub = Store::open(&path).unwrap();
        // Synthetic identity metadata only: no credential is generated.
        hub.0
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO devices(id,role,name,expires) VALUES('host','agent','fixture',?1)",
                [now() + 60],
            )
            .unwrap();
        assert_eq!(import(&hub, &capture, "host", THREAD).unwrap(), 1);
        let cursor = hub.events(0).unwrap()["cursor"].clone();
        let threads = hub.threads(0).unwrap();
        assert_eq!(threads["threads"][0]["can_send"], false);
        let messages = hub.messages(&key("host", THREAD), i64::MAX).unwrap();
        assert_eq!(messages["messages"][0]["text"], "TB-NOTIFY-OK");
        assert_eq!(messages["messages"][0]["turn_id"], TURN);
        assert_eq!(messages["messages"].as_array().unwrap().len(), 1);
        let demo = Snapshot {
            thread: Thread {
                id: key("host", THREAD),
                native_id: THREAD.into(),
                host_id: "host".into(),
                title: "demo overwrite".into(),
                status: "idle".into(),
                revision: "demo".into(),
                updated_at: now(),
                can_send: true,
                history_cursor: None,
                project: String::new(),
            },
            messages: vec![],
            initial: false,
            history: false,
        };
        assert!(hub.snapshot("host", &demo).is_err());
        assert_eq!(hub.threads(0).unwrap()["threads"][0]["revision"], TURN);
        hub.0.lock().unwrap().execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) VALUES('mock','phone','mock','fixture','host',?1,'{}','accepted',?2,?3)",rusqlite::params![key("host",THREAD),now(),now()+60]).unwrap();
        assert!(hub.claim("host").unwrap().is_none());
        hub.0
            .lock()
            .unwrap()
            .execute("DELETE FROM commands WHERE id='mock'", [])
            .unwrap();
        drop(hub);
        let hub = Store::open(&path).unwrap();
        assert_eq!(import(&hub, &capture, "host", THREAD).unwrap(), 0);
        assert_eq!(hub.events(0).unwrap()["cursor"], cursor);
        assert_eq!(
            hub.0
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        hub.0
            .lock()
            .unwrap()
            .execute("UPDATE devices SET revoked=1", [])
            .unwrap();
        assert!(import(&hub, &capture, "host", THREAD).is_err());
    }
}
