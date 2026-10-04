//! Opt-in, bounded capture of human text from the explicitly completed native turn.
//! Notify input-messages may contain history or system context and are never input sources.
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs::File, io::BufReader, path::Path, time::Duration};

pub const MAX_LINE: usize = 4 * 1024 * 1024;
pub const MAX_SCAN: usize = 256 * 1024 * 1024;
pub const MAX_INPUT: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UserMessage {
    pub message_id: String,
    pub text: String,
    pub created_at: i64,
    pub input_digest: String,
    #[serde(default)]
    pub images: Vec<UserImage>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UserImage {
    pub id: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub fn parse_user(payload: &Value) -> Result<Option<UserMessage>> {
    if payload["type"] != "message" || payload["role"] != "user" {
        return Ok(None);
    }
    let meta = &payload["internal_chat_message_metadata_passthrough"];
    let Some(kinds) = meta["content_item_kinds"].as_array() else {
        return Ok(None);
    };
    if kinds.is_empty()
        || kinds
            .iter()
            .any(|k| ![Some("user.text"), Some("user.image")].contains(&k.as_str()))
    {
        return Ok(None);
    }
    let Some(content) = payload["content"].as_array() else {
        return Ok(None);
    };
    let has_images = kinds.iter().any(|k| k == "user.image");
    if content.is_empty()
        || content
            .iter()
            .any(|x| ![Some("input_text"), Some("input_image")].contains(&x["type"].as_str()))
    {
        return Ok(None);
    }
    if has_images {
        if kinds.len() != content.len()
            || kinds.iter().zip(content).any(|(kind, item)| {
                (kind == "user.text" && item["type"] != "input_text")
                    || (kind == "user.image" && item["type"] != "input_image")
            })
        {
            return Ok(None);
        }
    } else if content.iter().any(|x| x["type"] != "input_text") {
        return Ok(None);
    }
    let mut raw = String::new();
    let mut text = String::new();
    let mut images = Vec::new();
    for item in content {
        if item["type"] == "input_text" {
            let value = item["text"]
                .as_str()
                .context("user_turn_metadata_invalid")?;
            raw.push_str(value);
            let value = if has_images
                && value
                    .trim_start()
                    .starts_with("# Files mentioned by the user:")
            {
                value
                    .split_once("## My request:\n")
                    .map(|(_, request)| request)
                    .unwrap_or(value)
            } else {
                value
            };
            let trimmed = value.trim();
            if has_images
                && ((trimmed.starts_with("<image ") && trimmed.ends_with('>'))
                    || trimmed == "</image>")
            {
                continue;
            }
            text.push_str(value);
        } else {
            let url = item["image_url"]
                .as_str()
                .context("user_image_unavailable")?;
            let (header, encoded) = url.split_once(',').context("user_image_unavailable")?;
            let mime = match header {
                "data:image/jpeg;base64" => "image/jpeg",
                "data:image/png;base64" => "image/png",
                "data:image/webp;base64" => "image/webp",
                _ => bail!("user_image_format_unsupported"),
            };
            ensure!(encoded.len() <= 3 * 1024 * 1024, "user_image_limit");
            let bytes = STANDARD.decode(encoded).context("user_image_invalid")?;
            ensure!(
                !bytes.is_empty() && bytes.len() <= 2 * 1024 * 1024 && images.len() < 16,
                "user_image_limit"
            );
            let id = format!("{:x}", Sha256::digest(&bytes));
            if !images.iter().any(|image: &UserImage| image.id == id) {
                images.push(UserImage {
                    id: id.clone(),
                    mime: mime.into(),
                    bytes,
                });
            }
            text.push_str(&format!("\n![图片](threadbridge-image:{id})\n"));
        }
    }
    ensure!(
        !text.trim().is_empty() && text.len() <= MAX_INPUT && raw.len() <= MAX_INPUT,
        "user_turn_input_limit"
    );
    let message_id = payload["id"]
        .as_str()
        .context("user_turn_metadata_invalid")?;
    let created = meta["create_time"]
        .as_f64()
        .context("user_turn_metadata_invalid")?;
    ensure!(
        !message_id.is_empty()
            && message_id.chars().count() <= 128
            && created.is_finite()
            && (created * 1000.0).abs() <= i64::MAX as f64,
        "user_turn_metadata_invalid"
    );
    let input_digest = if has_images {
        format!("{:x}", Sha256::digest(serde_json::to_vec(content)?))
    } else {
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    };
    let text = if !has_images && marker(&text).is_some() {
        text["[ThreadBridge request:".len() + 36 + 2..].to_owned()
    } else {
        text
    };
    Ok(Some(UserMessage {
        message_id: message_id.into(),
        text,
        created_at: (created * 1000.0) as i64,
        input_digest,
        images,
    }))
}

/// Preserve the old marker syntax exactly; only the prefix is removed from visible text.
pub(crate) fn marker(text: &str) -> Option<&str> {
    let body = text.strip_prefix("[ThreadBridge request:")?;
    let candidate = body.get(..36)?;
    if !candidate
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
        || !body.get(36..)?.starts_with("]\n")
    {
        return None;
    }
    Some(candidate)
}

pub fn read_turn(index: &Path, native: &str, turn: &str) -> Result<(Vec<UserMessage>, i64)> {
    uuid::Uuid::parse_str(native).context("invalid_thread_identity")?;
    uuid::Uuid::parse_str(turn).context("invalid_turn_identity")?;
    let db = Connection::open_with_flags(index, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_secs(3))?;
    let rollout: Option<String> = db
        .query_row(
            "SELECT rollout_path FROM threads WHERE id=?1",
            [native],
            |r| r.get(0),
        )
        .optional()?;
    let rollout = rollout.context("user_turn_source_missing")?;
    drop(db);
    let mut stream = BufReader::new(File::open(rollout)?);
    let mut rows = Vec::new();
    let mut used = 0;
    let mut identity = false;
    loop {
        let line = crate::native_capture::read_bounded_line(&mut stream, MAX_LINE)?;
        if line.is_empty() {
            break;
        }
        used += line.len();
        ensure!(
            line.len() <= MAX_LINE && used <= MAX_SCAN,
            "user_turn_source_limit"
        );
        let record: Value = serde_json::from_slice(&line)?;
        ensure!(record.is_object(), "user_turn_source_invalid");
        let payload = &record["payload"];
        if record["type"] == "session_meta" {
            identity = payload["id"] == native;
            ensure!(identity, "user_turn_identity_mismatch");
        }
        if !identity {
            continue;
        }
        if record["type"] == "event_msg"
            && payload["type"] == "task_complete"
            && payload["turn_id"] == turn
        {
            let timestamp = record["timestamp"]
                .as_str()
                .context("user_turn_metadata_invalid")?;
            let completed = chrono::DateTime::parse_from_rfc3339(timestamp)
                .context("user_turn_metadata_invalid")?
                .timestamp_millis();
            return Ok((rows, completed));
        }
        if record["type"] != "response_item"
            || payload["type"] != "message"
            || payload["role"] != "user"
        {
            continue;
        }
        let meta = &payload["internal_chat_message_metadata_passthrough"];
        if meta["turn_id"] != turn {
            continue;
        }
        if let Some(message) = parse_user(payload)? {
            rows.push(message);
        }
        ensure!(rows.len() <= 100, "user_turn_input_limit");
    }
    bail!("user_turn_not_complete")
}

pub fn save_turn(
    database: &Path,
    native: &str,
    turn: &str,
    rows: &[UserMessage],
    completed_at: i64,
) -> Result<()> {
    save_messages(database, native, turn, rows, Some(completed_at))
}
pub fn save_messages(
    database: &Path,
    native: &str,
    turn: &str,
    rows: &[UserMessage],
    completed_at: Option<i64>,
) -> Result<()> {
    let mut db = crate::native_capture::open_database(database)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS captured_user_messages(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,input_digest TEXT NOT NULL,PRIMARY KEY(thread_id,message_id)); CREATE TABLE IF NOT EXISTS captured_turn_order(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,completed_at INTEGER NOT NULL,PRIMARY KEY(thread_id,turn_id));")?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS captured_images(thread_id TEXT NOT NULL,message_id TEXT NOT NULL,image_id TEXT NOT NULL,mime TEXT NOT NULL,bytes BLOB NOT NULL,PRIMARY KEY(thread_id,message_id,image_id))")?;
    if let Some(completed_at) = completed_at {
        let old: Option<i64> = tx
            .query_row(
                "SELECT completed_at FROM captured_turn_order WHERE thread_id=?1 AND turn_id=?2",
                params![native, turn],
                |r| r.get(0),
            )
            .optional()?;
        ensure!(
            old.is_none_or(|value| value == completed_at),
            "conflicting_turn_order"
        );
        tx.execute(
            "INSERT OR IGNORE INTO captured_turn_order VALUES(?1,?2,?3)",
            params![native, turn, completed_at],
        )?;
    }
    for row in rows {
        let old: Option<(String, String, i64, String)> = tx.query_row("SELECT turn_id,text,created_at,input_digest FROM captured_user_messages WHERE thread_id=?1 AND message_id=?2", params![native, row.message_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
        ensure!(
            old.is_none_or(|value| value
                == (
                    turn.into(),
                    row.text.clone(),
                    row.created_at,
                    row.input_digest.clone()
                )),
            "conflicting_user_input"
        );
        tx.execute(
            "INSERT OR IGNORE INTO captured_user_messages VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                native,
                turn,
                row.message_id,
                row.text,
                row.created_at,
                row.input_digest
            ],
        )?;
        for image in &row.images {
            let existing: Option<(String, Vec<u8>)> = tx.query_row("SELECT mime,bytes FROM captured_images WHERE thread_id=?1 AND message_id=?2 AND image_id=?3",params![native,row.message_id,image.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            ensure!(
                existing
                    .as_ref()
                    .is_none_or(|(mime, bytes)| mime == &image.mime && bytes == &image.bytes),
                "conflicting_user_image"
            );
            if existing.is_none() {
                let used: i64 = tx.query_row(
                    "SELECT coalesce(sum(length(bytes)),0) FROM captured_images",
                    [],
                    |r| r.get(0),
                )?;
                ensure!(
                    used + image.bytes.len() as i64 <= 64 * 1024 * 1024,
                    "image_storage_budget"
                );
                tx.execute(
                    "INSERT INTO captured_images VALUES(?1,?2,?3,?4,?5)",
                    params![native, row.message_id, image.id, image.mime, image.bytes],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn load_images(db: &Connection, native: &str, message: &str) -> Result<Vec<UserImage>> {
    if !db.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE type='table' AND name='captured_images'",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(vec![]);
    }
    let mut q=db.prepare("SELECT image_id,mime,bytes FROM captured_images WHERE thread_id=?1 AND message_id=?2 ORDER BY image_id")?;
    let rows = q
        .query_map(params![native, message], |r| {
            Ok(UserImage {
                id: r.get(0)?,
                mime: r.get(1)?,
                bytes: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn capture_users(database: &Path, index: &Path, event: &Value) -> Result<()> {
    let native = event["thread-id"]
        .as_str()
        .context("missing_reply_identity")?;
    let turn = event["turn-id"]
        .as_str()
        .context("missing_reply_identity")?;
    let (rows, completed_at) = read_turn(index, native, turn)?;
    save_turn(database, native, turn, &rows, completed_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;
    const N: &str = "00000000-0000-4000-8000-000000000031";
    const T: &str = "00000000-0000-4000-8000-000000000032";
    struct Fixture {
        _root: TempDir,
        index: std::path::PathBuf,
        rollout: std::path::PathBuf,
        database: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            let index = root.path().join("index.sqlite");
            let rollout = root.path().join("rollout.jsonl");
            let database = root.path().join("capture.sqlite");
            let db = Connection::open(&index).unwrap();
            db.execute_batch("CREATE TABLE threads(id TEXT,rollout_path TEXT)")
                .unwrap();
            db.execute(
                "INSERT INTO threads VALUES(?1,?2)",
                params![N, rollout.to_str().unwrap()],
            )
            .unwrap();
            Self {
                _root: root,
                index,
                rollout,
                database,
            }
        }
        fn source(&self, complete: bool, native: &str) -> Vec<Value> {
            let mut rows = vec![
                json!({"type":"session_meta","payload":{"id":native}}),
                user("old", "previous human", "user.text", N),
                user("env", "SYSTEM CONTEXT", "additional_content.environment", T),
                user(
                    "human",
                    &format!("[ThreadBridge request:{N}]\nphone text"),
                    "user.text",
                    T,
                ),
            ];
            if complete {
                rows.push(json!({"type":"event_msg","timestamp":"1970-01-01T00:01:40.900Z","payload":{"type":"task_complete","turn_id":T}}));
            }
            self.write(&rows);
            rows
        }
        fn write(&self, rows: &[Value]) {
            fs::write(
                &self.rollout,
                rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
            )
            .unwrap();
        }
        fn read(&self) -> Result<(Vec<UserMessage>, i64)> {
            read_turn(&self.index, N, T)
        }
    }
    fn user(id: &str, text: &str, kind: &str, turn: &str) -> Value {
        json!({"type":"response_item","payload":{"type":"message","id":id,"role":"user","content":[{"type":"input_text","text":text}],"internal_chat_message_metadata_passthrough":{"turn_id":turn,"create_time":100.6,"content_item_kinds":[kind]}}})
    }
    fn event() -> Value {
        json!({"type":"agent-turn-complete","thread-id":N,"turn-id":T,"last-assistant-message":"final"})
    }
    fn options(f: &Fixture) -> crate::native_capture::CaptureOptions {
        crate::native_capture::CaptureOptions {
            database: f.database.clone(),
            all_tasks: true,
            user_turn_index: Some(f.index.clone()),
            min_free_bytes: 0,
            ..Default::default()
        }
    }

    #[test]
    fn current_human_only_marker_compatibility_and_durable_replay() {
        let f = Fixture::new();
        f.source(true, N);
        let (rows, complete) = f.read().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (
                &rows[0].message_id[..],
                &rows[0].text[..],
                rows[0].created_at
            ),
            ("human", "phone text", 100600)
        );
        assert_eq!(complete, 100900);
        assert_eq!(
            rows[0].input_digest,
            format!(
                "{:x}",
                Sha256::digest(format!("[ThreadBridge request:{N}]\nphone text").as_bytes())
            )
        );
        save_turn(&f.database, N, T, &rows, complete).unwrap();
        save_turn(&f.database, N, T, &rows, complete).unwrap();
        assert_eq!(
            Connection::open(&f.database)
                .unwrap()
                .query_row("SELECT count(*) FROM captured_user_messages", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let bytes = fs::read(&f.database).unwrap();
        for private in [b"SYSTEM CONTEXT".as_slice(), b"previous human".as_slice()] {
            assert!(!bytes.windows(private.len()).any(|w| w == private));
        }
    }

    #[test]
    fn wrong_identity_and_unfinished_turn_are_refused() {
        let f = Fixture::new();
        f.source(true, T);
        assert_eq!(
            f.read().unwrap_err().to_string(),
            "user_turn_identity_mismatch"
        );
        f.source(false, N);
        assert_eq!(f.read().unwrap_err().to_string(), "user_turn_not_complete");
    }

    #[test]
    fn opt_in_catalog_capture_saves_both_sides_once() {
        let f = Fixture::new();
        f.source(true, N);
        let options = options(&f);
        assert_eq!(
            crate::native_capture::capture(&event().to_string(), &options).unwrap(),
            "captured"
        );
        assert_eq!(
            crate::native_capture::capture(&event().to_string(), &options).unwrap(),
            "duplicate"
        );
        let db = Connection::open(&f.database).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_replies", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT text FROM captured_user_messages", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "phone text"
        );
    }

    #[test]
    fn failed_input_source_keeps_final_reply_and_durable_failure() {
        let f = Fixture::new();
        f.source(false, N);
        assert_eq!(
            crate::native_capture::capture(&event().to_string(), &options(&f))
                .unwrap_err()
                .to_string(),
            "user_turn_not_complete"
        );
        assert_eq!(
            Connection::open(&f.database)
                .unwrap()
                .query_row("SELECT reply FROM captured_replies", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "final"
        );
        assert_eq!(
            crate::health::read(&f.database).unwrap()["failures"][format!("{N}:{T}")]["reason"],
            "user_input_capture_failed"
        );
    }

    #[test]
    fn conflicting_input_and_order_replays_roll_back_without_replacement() {
        let f = Fixture::new();
        f.source(true, N);
        let (rows, complete) = f.read().unwrap();
        save_turn(&f.database, N, T, &rows, complete).unwrap();
        let mut changed = rows.clone();
        changed[0].text = "changed".into();
        assert_eq!(
            save_turn(&f.database, N, T, &changed, complete)
                .unwrap_err()
                .to_string(),
            "conflicting_user_input"
        );
        assert_eq!(
            save_turn(&f.database, N, T, &rows, complete + 1)
                .unwrap_err()
                .to_string(),
            "conflicting_turn_order"
        );
        assert_eq!(
            Connection::open(&f.database)
                .unwrap()
                .query_row("SELECT text FROM captured_user_messages", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "phone text"
        );
        assert_eq!(
            save_turn(&f.database, N, N, &changed, complete)
                .unwrap_err()
                .to_string(),
            "conflicting_user_input"
        );
        assert_eq!(
            Connection::open(&f.database)
                .unwrap()
                .query_row("SELECT count(*) FROM captured_turn_order", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn malformed_metadata_and_oversize_current_input_are_refused() {
        let f = Fixture::new();
        let mut rows = f.source(true, N);
        rows[3]["payload"]["id"] = json!("x".repeat(129));
        f.write(&rows);
        assert_eq!(
            f.read().unwrap_err().to_string(),
            "user_turn_metadata_invalid"
        );
        rows[3]["payload"]["id"] = json!("human");
        rows[3]["payload"]["content"][0]["text"] = json!("x".repeat(MAX_INPUT + 1));
        f.write(&rows);
        assert_eq!(f.read().unwrap_err().to_string(), "user_turn_input_limit");
        rows[3]["payload"]["content"][0]["text"] = json!("text");
        rows[3]["payload"]["internal_chat_message_metadata_passthrough"]["create_time"] =
            json!("not a number");
        f.write(&rows);
        assert_eq!(
            f.read().unwrap_err().to_string(),
            "user_turn_metadata_invalid"
        );
    }

    #[test]
    fn bounded_source_line_and_input_count_fail_closed() {
        let f = Fixture::new();
        fs::write(&f.rollout, "x".repeat(MAX_LINE + 1)).unwrap();
        assert_eq!(f.read().unwrap_err().to_string(), "user_turn_source_limit");
        let mut rows = vec![json!({"type":"session_meta","payload":{"id":N}})];
        rows.extend((0..101).map(|i| user(&format!("human-{i}"), "text", "user.text", T)));
        f.write(&rows);
        assert_eq!(f.read().unwrap_err().to_string(), "user_turn_input_limit");
    }

    #[test]
    fn assistant_system_mixed_content_and_previous_turn_never_become_human_input() {
        let f = Fixture::new();
        let mut rows = f.source(true, N);
        let mut assistant = user("assistant", "PRIVATE ASSISTANT", "user.text", T);
        assistant["payload"]["role"] = json!("assistant");
        let mut mixed = user("mixed", "PRIVATE MIXED", "user.text", T);
        mixed["payload"]["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"input_image","image_url":"private"}));
        rows.insert(1, assistant);
        rows.insert(1, mixed);
        f.write(&rows);
        let (input, _) = f.read().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].text, "phone text");
    }
}
