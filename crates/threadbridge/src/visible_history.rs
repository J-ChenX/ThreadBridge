//! Import only user-authored content and visible assistant text from native history.
//! Tool calls, tool results, reasoning and file-change cards never enter this view.
use crate::{collection, health, native_capture, native_input};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs::File, io::BufReader, path::Path};

pub fn backfill(database: &Path, index: &Path, native: &str) -> Result<usize> {
    uuid::Uuid::parse_str(native)?;
    let _guard = collection::lock(database)?;
    if !collection::allowed(database, Some(index), native)? {
        return Ok(0);
    }
    let source = collection::readonly(index)?;
    let path: String = source.query_row(
        "SELECT rollout_path FROM threads WHERE id=?1",
        [native],
        |r| r.get(0),
    )?;
    let registered: bool = source.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE name='projects'",
        [],
        |r| r.get(0),
    )?;
    let mut stream = BufReader::new(File::open(path)?);
    let mut identity = false;
    let mut used = 0;
    let mut users = BTreeMap::<String, Vec<native_input::UserMessage>>::new();
    let mut comments = Vec::<(String, String, String, i64)>::new();
    let mut finals = BTreeMap::<String, String>::new();
    let mut completed = BTreeMap::<String, i64>::new();
    let mut confirmed = BTreeMap::<String, (String, i64)>::new();
    let mut empty_finals = BTreeMap::<String, bool>::new();
    let mut empty_completions = BTreeMap::<String, bool>::new();
    loop {
        let line = native_capture::read_bounded_line(&mut stream, native_input::MAX_LINE)?;
        if line.is_empty() {
            break;
        }
        used += line.len();
        ensure!(
            line.len() <= native_input::MAX_LINE && used <= native_input::MAX_SCAN,
            "visible_history_limit"
        );
        let record: Value = match serde_json::from_slice(&line) {
            Ok(record) => record,
            Err(_) if !line.ends_with(b"\n") => break, // A live writer has not committed this line.
            Err(error) => return Err(error.into()),
        };
        let p = &record["payload"];
        if record["type"] == "session_meta" {
            ensure!(p["id"] == native, "visible_history_identity_mismatch");
            identity = true;
            if !registered {
                if let Some(project) = p["cwd"].as_str().filter(|p| {
                    (p.starts_with('/')
                        || (p.len() > 2
                            && p.as_bytes()[1] == b':'
                            && matches!(p.as_bytes()[2], b'\\' | b'/')))
                        && p.len() <= 2048
                        && !p.contains(['\0', '\n', '\r'])
                }) {
                    let db = native_capture::open_database(database)?;
                    db.execute_batch("CREATE TABLE IF NOT EXISTS captured_project_catalog(project_path TEXT PRIMARY KEY);CREATE TABLE IF NOT EXISTS captured_projects(thread_id TEXT PRIMARY KEY,project_path TEXT NOT NULL)")?;
                    db.execute("INSERT INTO captured_projects VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET project_path=excluded.project_path",params![native,project])?;
                }
            }
        }
        if !identity {
            continue;
        }
        if record["type"] == "response_item" && p["type"] == "message" {
            let meta = &p["internal_chat_message_metadata_passthrough"];
            let Some(turn) = meta["turn_id"].as_str() else {
                continue;
            };
            uuid::Uuid::parse_str(turn)?;
            if let Some(user) = native_input::parse_user(p)? {
                users.entry(turn.into()).or_default().push(user);
            } else if p["role"] == "assistant"
                && [Some("commentary"), Some("final_answer")].contains(&p["phase"].as_str())
            {
                let content = p["content"]
                    .as_array()
                    .context("visible_history_content_missing")?;
                if p["phase"] == "final_answer" {
                    let empty = !content.is_empty()
                        && content.iter().all(|item| {
                            item["type"] == "output_text"
                                && item["text"].as_str().is_some_and(str::is_empty)
                        });
                    empty_finals
                        .entry(turn.into())
                        .and_modify(|previous| *previous &= empty)
                        .or_insert(empty);
                }
                if content.iter().any(|item| item["type"] != "output_text") {
                    continue;
                }
                let text = content
                    .iter()
                    .map(|item| {
                        item["text"]
                            .as_str()
                            .context("visible_history_text_missing")
                    })
                    .collect::<Result<Vec<_>>>()?
                    .concat();
                if text.is_empty() {
                    continue;
                }
                ensure!(
                    text.len() <= native_input::MAX_INPUT,
                    "visible_history_message_limit"
                );
                let id = p["id"]
                    .as_str()
                    .context("visible_history_message_identity_missing")?;
                ensure!(
                    !id.is_empty() && id.len() <= 128,
                    "visible_history_message_identity_invalid"
                );
                let stamp = chrono::DateTime::parse_from_rfc3339(
                    record["timestamp"]
                        .as_str()
                        .context("visible_history_timestamp_missing")?,
                )?
                .timestamp_millis();
                if p["phase"] == "commentary" {
                    comments.push((turn.into(), id.into(), text, stamp));
                } else {
                    finals.insert(turn.into(), text);
                }
            }
        }
        if record["type"] == "event_msg" && p["type"] == "task_complete" {
            let turn = p["turn_id"]
                .as_str()
                .context("visible_history_turn_missing")?;
            uuid::Uuid::parse_str(turn)?;
            let stamp = chrono::DateTime::parse_from_rfc3339(
                record["timestamp"]
                    .as_str()
                    .context("visible_history_timestamp_missing")?,
            )?
            .timestamp_millis();
            completed.insert(turn.into(), stamp);
            let empty = p["error"].is_null()
                && p.get("last_agent_message").is_some_and(|reply| {
                    reply.is_null() || reply.as_str().is_some_and(str::is_empty)
                });
            empty_completions
                .entry(turn.into())
                .and_modify(|previous| *previous &= empty)
                .or_insert(empty);
            if p["error"].is_null() {
                if let Some(text) = p["last_agent_message"].as_str() {
                    if finals
                        .get(turn)
                        .is_some_and(|final_text| final_text == text)
                    {
                        confirmed.insert(turn.into(), (text.into(), stamp));
                    }
                }
            }
        }
        ensure!(
            comments.len() <= 5000 && users.values().map(Vec::len).sum::<usize>() <= 5000,
            "visible_history_count_limit"
        );
    }
    ensure!(identity, "visible_history_identity_missing");
    let mut count = 0;
    for (turn, messages) in &users {
        native_input::save_messages(
            database,
            native,
            turn,
            messages,
            completed.get(turn).copied(),
        )?;
        count += messages.len();
        if completed.contains_key(turn) && health::path(database).exists() {
            let state = health::read(database)?;
            if state["failures"][format!("{native}:{turn}")]["reason"]
                == "user_input_capture_failed"
            {
                health::update(database, native, turn, None, None)?;
            }
        }
    }
    let mut db = native_capture::open_database(database)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS captured_visible_messages(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(thread_id,message_id))")?;
    let tx = db.transaction()?;
    for (turn, id, text, stamp) in &comments {
        let old: Option<(String,String,i64)> = tx.query_row("SELECT turn_id,text,created_at FROM captured_visible_messages WHERE thread_id=?1 AND message_id=?2",params![native,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        ensure!(
            old.as_ref()
                .is_none_or(|old| old == &(turn.clone(), text.clone(), *stamp)),
            "conflicting_visible_message"
        );
        if old.is_none() {
            let used: i64 = tx.query_row(
                "SELECT coalesce(sum(length(CAST(text AS BLOB))),0) FROM captured_visible_messages",
                [],
                |r| r.get(0),
            )?;
            ensure!(
                used + text.len() as i64 <= 32 * 1024 * 1024,
                "visible_history_storage_limit"
            );
            tx.execute(
                "INSERT INTO captured_visible_messages VALUES(?1,?2,?3,?4,?5)",
                params![native, turn, id, text, stamp],
            )?;
        }
    }
    tx.commit()?;
    count += comments.len();
    let title: Option<String> = db
        .query_row(
            "SELECT title FROM captured_replies WHERE thread_id=?1 ORDER BY rowid DESC LIMIT 1",
            [native],
            |r| r.get(0),
        )
        .optional()?;
    for (turn, (text, stamp)) in confirmed {
        let exists: bool = db.query_row(
            "SELECT count(*)>0 FROM captured_replies WHERE thread_id=?1 AND turn_id=?2",
            params![native, turn],
            |r| r.get(0),
        )?;
        if !exists {
            native_capture::capture_completion(&json!({"type":"agent-turn-complete","thread-id":native,"turn-id":turn,"last-assistant-message":text}).to_string(),native,database,title.as_deref(),None,None,native_capture::MIN_FREE_BYTES,false)?;
            db.execute(
                "UPDATE captured_replies SET captured_at=?3 WHERE thread_id=?1 AND turn_id=?2",
                params![native, turn, stamp / 1000],
            )?;
        }
    }
    // Reconcile only old empty-reply misclassifications proven by both native
    // records. Never clear a real save failure or infer success from absence.
    if health::path(database).exists() {
        let state = health::read(database)?;
        for (turn, empty) in empty_finals {
            if !empty || empty_completions.get(&turn) != Some(&true) {
                continue;
            }
            let failure = &state["failures"][format!("{native}:{turn}")];
            if failure["reason"] != "missing_reply_identity" {
                continue;
            }
            let saved: bool = db.query_row(
                "SELECT count(*)>0 FROM captured_replies WHERE thread_id=?1 AND turn_id=?2",
                params![native, turn],
                |r| r.get(0),
            )?;
            if !saved {
                health::update(database, native, &turn, None, failure["attempt"].as_str())?;
            }
        }
    }
    Ok(count)
}
