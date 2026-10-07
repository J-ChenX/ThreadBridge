//! Native notify capture with the existing durable SQLite schema and collection policy.
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const MAX_REPLY_BYTES: usize = 256 * 1024;
pub const MIN_FREE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STORED_BYTES: u64 = 16 * 1024 * 1024;
const MAX_REPLIES: i64 = 1000;

#[derive(Clone, Debug)]
pub struct CaptureOptions {
    pub database: PathBuf,
    pub thread: Option<String>,
    pub catalog: Option<PathBuf>,
    pub all_tasks: bool,
    pub title: Option<String>,
    pub title_index: Option<PathBuf>,
    pub user_turn_index: Option<PathBuf>,
    pub storage_budget: Option<u64>,
    pub min_free_bytes: u64,
    pub legacy_limits: bool,
    pub store_candidates: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            database: PathBuf::new(),
            thread: None,
            catalog: None,
            all_tasks: false,
            title: None,
            title_index: None,
            user_turn_index: None,
            storage_budget: None,
            min_free_bytes: MIN_FREE_BYTES,
            legacy_limits: false,
            store_candidates: false,
        }
    }
}

/// Read at most limit + 1 bytes so an oversized untrusted line cannot allocate unbounded memory.
pub(crate) fn read_bounded_line(stream: &mut impl BufRead, limit: usize) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    stream
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut line)?;
    Ok(line)
}

pub fn indexed_title(native: &str, index: &Path) -> Result<Option<String>> {
    Ok(read_indexed_titles(index, Some(native))?.remove(native))
}

pub fn indexed_titles(index: &Path) -> Result<BTreeMap<String, String>> {
    read_indexed_titles(index, None)
}

fn read_indexed_titles(index: &Path, selected: Option<&str>) -> Result<BTreeMap<String, String>> {
    let mut stream = BufReader::new(File::open(index)?);
    let mut found = BTreeMap::new();
    let mut used = 0;
    loop {
        let line = read_bounded_line(&mut stream, 8192)?;
        if line.is_empty() {
            break;
        }
        used += line.len();
        ensure!(
            line.len() <= 8192 && used <= 8 * 1024 * 1024,
            "title_index_limit"
        );
        let entry: Value = serde_json::from_slice(&line)?;
        let entry = entry.as_object().context("invalid_title_index_metadata")?;
        ensure!(
            entry
                .keys()
                .all(|key| ["id", "thread_name", "updated_at"].contains(&key.as_str())),
            "invalid_title_index_metadata"
        );
        if let Some(native) = entry.get("id").and_then(Value::as_str) {
            if selected.is_some_and(|id| id != native) {
                continue;
            }
            let title = entry
                .get("thread_name")
                .and_then(Value::as_str)
                .context("invalid_title")?;
            ensure!(
                !title.trim().is_empty() && title.len() <= 512,
                "invalid_title"
            );
            found.insert(native.to_owned(), title.trim().to_owned());
        }
    }
    Ok(found)
}

pub(crate) fn open_database(path: &Path) -> Result<Connection> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut directories = fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directories.mode(0o700);
    }
    directories.create(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(!metadata.file_type().is_symlink(), "symlink_database");
    }
    let mut file = OpenOptions::new();
    file.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file.mode(0o600);
    }
    drop(file.open(path)?);
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(3))?;
    db.execute_batch("PRAGMA synchronous=FULL")?;
    Ok(db)
}

fn record(payload: &str, native: &str, options: &CaptureOptions) -> Result<String> {
    uuid::Uuid::parse_str(native).context("invalid_thread_identity")?;
    let event: Value = serde_json::from_str(payload)?;
    ensure!(event.is_object(), "invalid_event");
    if event["thread-id"] != native || event["type"] != "agent-turn-complete" {
        return Ok("ignored".into());
    }
    let turn = event["turn-id"]
        .as_str()
        .context("missing_reply_identity")?;
    let reply = event["last-assistant-message"]
        .as_str()
        .filter(|text| !text.is_empty())
        .context("missing_reply_identity")?;
    uuid::Uuid::parse_str(turn).context("invalid_turn_identity")?;
    let size = reply.len() as u64;
    ensure!(size <= MAX_REPLY_BYTES as u64, "reply_too_large");
    let title = options
        .title
        .clone()
        .or_else(|| {
            options
                .user_turn_index
                .as_deref()
                .and_then(|index| crate::thread_titles::read(index).ok()?.remove(native))
        })
        .or_else(|| {
            options
                .title_index
                .as_deref()
                .and_then(|index| indexed_title(native, index).ok().flatten())
        })
        .unwrap_or_else(|| format!("会话 · {}", &native[..8]));
    ensure!(
        !title.trim().is_empty() && title.len() <= 512,
        "invalid_title"
    );
    let mut db = open_database(&options.database)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS captured_replies (thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, reply TEXT NOT NULL, utf8_bytes INTEGER NOT NULL, captured_at INTEGER NOT NULL DEFAULT (unixepoch()), PRIMARY KEY(thread_id,turn_id)); CREATE TABLE IF NOT EXISTS captured_request_ids(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,request_id TEXT NOT NULL,PRIMARY KEY(thread_id,turn_id));")?;
    if options.store_candidates {
        db.execute_batch("CREATE TABLE IF NOT EXISTS captured_request_candidates(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,request_id TEXT NOT NULL,PRIMARY KEY(thread_id,turn_id,request_id))")?;
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS captured_project_catalog(project_path TEXT PRIMARY KEY);CREATE TABLE IF NOT EXISTS captured_projects(thread_id TEXT PRIMARY KEY,project_path TEXT NOT NULL)")?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let columns = {
        let mut query = tx.prepare("PRAGMA table_info(captured_replies)")?;
        let columns = query
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        columns
    };
    if !columns.iter().any(|column| column == "title") {
        tx.execute_batch("ALTER TABLE captured_replies ADD COLUMN title TEXT NOT NULL DEFAULT ''")?;
    }
    let registered = options
        .title_index
        .as_ref()
        .or(options.user_turn_index.as_ref())
        .and_then(|p| crate::collection::readonly(p).ok())
        .and_then(|c| {
            c.query_row(
                "SELECT count(*)>0 FROM sqlite_master WHERE name='projects'",
                [],
                |r| r.get::<_, bool>(0),
            )
            .ok()
        })
        .unwrap_or(false);
    if !registered {
        if let Some(project) = event["cwd"].as_str().filter(|p| {
            (p.starts_with('/')
                || (p.len() > 2
                    && p.as_bytes()[1] == b':'
                    && matches!(p.as_bytes()[2], b'\\' | b'/')))
                && p.len() <= 2048
                && !p.contains(['\0', '\n', '\r'])
        }) {
            tx.execute("INSERT INTO captured_projects VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET project_path=excluded.project_path",params![native,project])?;
        }
        if let Some(project) = event["cwd"]
            .as_str()
            .filter(|p| p.starts_with('/') || (p.len() > 2 && p.as_bytes()[1] == b':'))
        {
            if project.len() <= 2048 && !project.contains(['\0', '\r', '\n']) {
                tx.execute(
                    "INSERT OR IGNORE INTO captured_project_catalog VALUES(?1)",
                    [project],
                )?;
            }
        }
    }
    let old: Option<String> = tx
        .query_row(
            "SELECT reply FROM captured_replies WHERE thread_id=?1 AND turn_id=?2",
            params![native, turn],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        ensure!(old == reply, "conflicting_reply");
        tx.commit()?;
        return Ok("duplicate".into());
    }
    if options.legacy_limits || options.storage_budget.is_some() {
        let (count, used): (i64, u64) = tx.query_row(
            "SELECT count(*),coalesce(sum(utf8_bytes),0) FROM captured_replies",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let total = used.checked_add(size).context("storage_budget_exceeded")?;
        if options.legacy_limits {
            ensure!(
                count < MAX_REPLIES && total <= MAX_STORED_BYTES,
                "capture_capacity_exceeded"
            );
        }
        if let Some(budget) = options.storage_budget {
            ensure!(total <= budget, "storage_budget_exceeded");
        }
    }
    if !options.legacy_limits {
        let parent = options
            .database
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let reserve = options.min_free_bytes.saturating_add(size * 3);
        ensure!(fs2::available_space(parent)? >= reserve, "disk_space_low");
    }
    let captured = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    tx.execute("INSERT INTO captured_replies(thread_id,turn_id,reply,utf8_bytes,captured_at,title) VALUES(?1,?2,?3,?4,?5,?6)", params![native, turn, reply, size, captured, title.trim()])?;
    let mut candidates = BTreeSet::new();
    if let Some(inputs) = event["input-messages"].as_array() {
        for input in inputs {
            if let Some(candidate) = input.as_str().and_then(crate::native_input::marker) {
                if uuid::Uuid::parse_str(candidate).is_ok() {
                    candidates.insert(candidate);
                }
            }
        }
    }
    if options.store_candidates && candidates.len() <= 16 {
        for candidate in &candidates {
            tx.execute(
                "INSERT INTO captured_request_candidates VALUES(?1,?2,?3)",
                params![native, turn, candidate],
            )?;
        }
    }
    if candidates.len() == 1 {
        tx.execute(
            "INSERT INTO captured_request_ids VALUES(?1,?2,?3)",
            params![native, turn, candidates.first().unwrap()],
        )?;
    }
    tx.commit()?;
    Ok("captured".into())
}

/// A valid completion can carry no final text (for example an internal follow-up).
/// Missing or malformed fields still require a capture failure, not this exemption.
pub(crate) fn empty_completion(event: &Value) -> bool {
    event["type"] == "agent-turn-complete"
        && event["turn-id"]
            .as_str()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        && event
            .get("last-assistant-message")
            .is_some_and(|reply| reply.is_null() || reply.as_str().is_some_and(str::is_empty))
}

fn durable_capture(payload: &str, native: &str, options: &CaptureOptions) -> Result<String> {
    uuid::Uuid::parse_str(native).context("invalid_thread_identity")?;
    let event: Value = serde_json::from_str(payload)?;
    ensure!(event.is_object(), "invalid_event");
    if event["thread-id"] != native || event["type"] != "agent-turn-complete" {
        return Ok("ignored".into());
    }
    if empty_completion(&event) {
        return Ok("ignored_empty_reply".into());
    }
    let turn = event["turn-id"]
        .as_str()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .unwrap_or("unknown");
    let attempt = uuid::Uuid::new_v4().to_string();
    crate::health::update(
        &options.database,
        native,
        turn,
        Some("capture_not_confirmed"),
        Some(&attempt),
    )?;
    match record(payload, native, options) {
        Ok(result) => {
            crate::health::update(&options.database, native, turn, None, Some(&attempt))?;
            Ok(result)
        }
        Err(error) => {
            let message = error.to_string();
            let reason = if [
                "missing_reply_identity",
                "reply_too_large",
                "conflicting_reply",
                "invalid_title",
                "storage_budget_exceeded",
                "disk_space_low",
                "symlink_database",
            ]
            .contains(&message.as_str())
            {
                message.as_str()
            } else {
                "storage_write_failed"
            };
            // If the update itself fails, the durable unconfirmed intent remains unresolved.
            let _ = crate::health::update(
                &options.database,
                native,
                turn,
                Some(reason),
                Some(&attempt),
            );
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn capture_completion(
    payload: &str,
    native: &str,
    database: &Path,
    title: Option<&str>,
    title_index: Option<&Path>,
    storage_budget: Option<u64>,
    min_free_bytes: u64,
    store_candidates: bool,
) -> Result<String> {
    durable_capture(
        payload,
        native,
        &CaptureOptions {
            database: database.into(),
            title: title.map(str::to_owned),
            title_index: title_index.map(Path::to_path_buf),
            storage_budget,
            min_free_bytes,
            store_candidates,
            ..Default::default()
        },
    )
}

pub fn capture(payload: &str, options: &CaptureOptions) -> Result<String> {
    ensure!(
        options.storage_budget.is_none_or(|budget| budget > 0),
        "invalid_storage_budget"
    );
    let mut catalog = BTreeMap::<String, String>::new();
    if let Some(path) = &options.catalog {
        let mut raw = Vec::new();
        File::open(path)?
            .take(512 * 1024 + 1)
            .read_to_end(&mut raw)?;
        ensure!(raw.len() <= 512 * 1024, "catalog_too_large");
        catalog = serde_json::from_slice(&raw).context("invalid_catalog")?;
        ensure!(
            !catalog.is_empty() && catalog.len() <= 1000,
            "invalid_catalog"
        );
        for (native, title) in &catalog {
            uuid::Uuid::parse_str(native).context("invalid_catalog")?;
            ensure!(
                !title.trim().is_empty() && title.len() <= 512,
                "invalid_catalog_title"
            );
        }
    }
    ensure!(
        options.thread.is_some() || options.catalog.is_some() || options.all_tasks,
        "capture_scope_required"
    );
    if let Some(native) = &options.thread {
        uuid::Uuid::parse_str(native).context("invalid_thread_identity")?;
    }
    let event: Value = serde_json::from_str(payload)?;
    ensure!(event.is_object(), "invalid_event");
    let Some(native) = event["thread-id"].as_str() else {
        return Ok("ignored".into());
    };
    if options
        .thread
        .as_deref()
        .is_some_and(|allowed| allowed != native)
        || (options.thread.is_none() && !options.all_tasks && !catalog.contains_key(native))
    {
        return Ok("ignored".into());
    }
    let _lock = crate::collection::lock(&options.database)?;
    // The title index is JSONL, not a native SQLite identity/source index.
    let source = options.user_turn_index.as_deref();
    if !crate::collection::allowed(&options.database, source, native)? {
        return Ok("ignored".into());
    }
    // These turns may contain only tool results and have no human input either.
    // Do not turn an explicitly empty completion into a missing-input warning.
    if empty_completion(&event) {
        return Ok("ignored_empty_reply".into());
    }
    let user_error = if let Some(index) = options
        .user_turn_index
        .as_deref()
        .filter(|_| event["type"] == "agent-turn-complete")
    {
        crate::native_input::capture_users(&options.database, index, &event).err()
    } else {
        None
    };
    let mut actual = options.clone();
    if let Some(title) = catalog.get(native) {
        actual.title = Some(title.clone());
    }
    let result = if actual.legacy_limits {
        record(payload, native, &actual)?
    } else {
        durable_capture(payload, native, &actual)?
    };
    if let Some(error) = user_error {
        let turn = event["turn-id"]
            .as_str()
            .context("missing_reply_identity")?;
        crate::health::update(
            &options.database,
            native,
            turn,
            Some("user_input_capture_failed"),
            None,
        )?;
        return Err(error);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;
    const N: &str = "00000000-0000-4000-8000-000000000031";
    const T: &str = "00000000-0000-4000-8000-000000000032";
    const OTHER: &str = "00000000-0000-4000-8000-000000000033";

    fn event() -> Value {
        json!({"type":"agent-turn-complete","thread-id":N,"turn-id":T,"last-assistant-message":"中文回复\n".repeat(200),"input-messages":["PRIVATE INPUT"],"cwd":"PRIVATE PATH"})
    }
    fn options(root: &TempDir) -> CaptureOptions {
        CaptureOptions {
            database: root.path().join("capture.sqlite"),
            thread: Some(N.into()),
            min_free_bytes: 0,
            ..Default::default()
        }
    }
    fn saved(options: &CaptureOptions) -> Connection {
        Connection::open(&options.database).unwrap()
    }
    fn failure(options: &CaptureOptions) -> String {
        crate::health::read(&options.database).unwrap()["failures"][format!("{N}:{T}")]["reason"]
            .as_str()
            .unwrap()
            .into()
    }
    fn execute(event: &Value, options: &CaptureOptions) -> Result<String> {
        capture(&event.to_string(), options)
    }

    #[test]
    fn scope_and_event_filters_precede_storage() {
        let root = TempDir::new().unwrap();
        let options = options(&root);
        let mut event = event();
        event["thread-id"] = json!(OTHER);
        assert_eq!(execute(&event, &options).unwrap(), "ignored");
        event["thread-id"] = json!(N);
        event["type"] = json!("unrelated");
        assert_eq!(execute(&event, &options).unwrap(), "ignored");
        assert!(!options.database.exists());
    }

    #[test]
    fn explicit_empty_completion_has_no_reply_or_failure_even_with_input_index() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        let index = root.path().join("index.sqlite");
        let db = Connection::open(&index).unwrap();
        db.execute_batch(
            "CREATE TABLE threads(id TEXT,rollout_path TEXT,thread_source TEXT,archived INTEGER)",
        )
        .unwrap();
        db.execute(
            "INSERT INTO threads VALUES(?1,'unavailable-rollout.jsonl','user',0)",
            [N],
        )
        .unwrap();
        options.user_turn_index = Some(index);
        for empty in [Value::Null, json!("")] {
            let mut event = event();
            event["last-assistant-message"] = empty;
            assert_eq!(execute(&event, &options).unwrap(), "ignored_empty_reply");
            assert!(!options.database.exists());
            assert!(!crate::health::path(&options.database).exists());
        }
        let mut event = event();
        event
            .as_object_mut()
            .unwrap()
            .remove("last-assistant-message");
        assert!(execute(&event, &options).is_err());
        assert_eq!(failure(&options), "missing_reply_identity");
        event["last-assistant-message"] = Value::Null;
        event["turn-id"] = json!("invalid");
        assert!(execute(&event, &options).is_err());
    }

    #[test]
    fn exact_reply_title_identity_timestamp_and_idempotence_survive_reopen() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        options.title = Some(" 原任务标题 ".into());
        let event = event();
        assert_eq!(execute(&event, &options).unwrap(), "captured");
        assert_eq!(execute(&event, &options).unwrap(), "duplicate");
        let row: (String, String, String, i64, String) = saved(&options)
            .query_row(
                "SELECT title,thread_id,turn_id,captured_at,reply FROM captured_replies",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((&row.0[..], &row.1[..], &row.2[..]), ("原任务标题", N, T));
        assert!(row.3 > 0);
        assert_eq!(row.4, event["last-assistant-message"].as_str().unwrap());
        let bytes = fs::read(&options.database).unwrap();
        assert!(!bytes.windows(7).any(|w| w == b"PRIVATE"));
        assert_eq!(
            saved(&options)
                .query_row("SELECT count(*) FROM captured_replies", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn conflicting_duplicate_never_replaces_saved_reply() {
        let root = TempDir::new().unwrap();
        let options = options(&root);
        let mut event = event();
        execute(&event, &options).unwrap();
        event["last-assistant-message"] = json!("changed");
        assert_eq!(
            execute(&event, &options).unwrap_err().to_string(),
            "conflicting_reply"
        );
        assert_eq!(failure(&options), "conflicting_reply");
        assert_ne!(
            saved(&options)
                .query_row("SELECT reply FROM captured_replies", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "changed"
        );
    }

    #[test]
    fn oversize_and_missing_identity_do_not_create_reply_storage() {
        let root = TempDir::new().unwrap();
        let options = options(&root);
        let mut event = event();
        event["last-assistant-message"] = json!("x".repeat(MAX_REPLY_BYTES + 1));
        assert_eq!(
            execute(&event, &options).unwrap_err().to_string(),
            "reply_too_large"
        );
        assert!(!options.database.exists());
        assert_eq!(failure(&options), "reply_too_large");
        event.as_object_mut().unwrap().remove("turn-id");
        assert_eq!(
            execute(&event, &options).unwrap_err().to_string(),
            "missing_reply_identity"
        );
        assert!(!options.database.exists());
    }

    #[test]
    fn title_migration_keeps_legacy_rows_and_does_not_replay_them() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        options.title = Some("registered".into());
        let db = saved(&options);
        db.execute_batch("CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER,PRIMARY KEY(thread_id,turn_id))").unwrap();
        db.execute(
            "INSERT INTO captured_replies VALUES(?1,?2,'legacy',6,1)",
            params![N, OTHER],
        )
        .unwrap();
        drop(db);
        execute(&event(), &options).unwrap();
        let db = saved(&options);
        assert_eq!(
            db.query_row(
                "SELECT reply FROM captured_replies WHERE turn_id=?1",
                [OTHER],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "legacy"
        );
        assert_eq!(
            db.query_row(
                "SELECT title FROM captured_replies WHERE turn_id=?1",
                [T],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "registered"
        );
    }

    #[test]
    fn marker_stores_only_valid_uuid_and_ambiguity_never_acknowledges() {
        let root = TempDir::new().unwrap();
        let options = options(&root);
        let mut event = event();
        event["input-messages"] = json!([
            format!("[ThreadBridge request:{N}]\nPRIVATE INPUT"),
            format!("[ThreadBridge request:{N}]\nOTHER INPUT"),
            "[ThreadBridge request:------------------------------------]\ninvalid"
        ]);
        execute(&event, &options).unwrap();
        assert_eq!(
            saved(&options)
                .query_row("SELECT request_id FROM captured_request_ids", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            N
        );
        event["turn-id"] = json!(OTHER);
        event["input-messages"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!("[ThreadBridge request:{T}]\nOTHER INPUT")));
        execute(&event, &options).unwrap();
        assert_eq!(
            saved(&options)
                .query_row("SELECT count(*) FROM captured_request_ids", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(!fs::read(&options.database)
            .unwrap()
            .windows(7)
            .any(|w| w == b"PRIVATE"));
    }

    #[test]
    fn candidate_compatibility_is_bounded_and_does_not_infer_single_request() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        options.store_candidates = true;
        let mut event = event();
        let inputs: Vec<_> = (0..17)
            .map(|_| {
                json!(format!(
                    "[ThreadBridge request:{}]\ntext",
                    uuid::Uuid::new_v4()
                ))
            })
            .collect();
        event["input-messages"] = json!(&inputs[..16]);
        execute(&event, &options).unwrap();
        assert_eq!(
            saved(&options)
                .query_row(
                    "SELECT count(*) FROM captured_request_candidates",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            16
        );
        event["turn-id"] = json!(OTHER);
        event["input-messages"] = json!(inputs);
        execute(&event, &options).unwrap();
        assert_eq!(
            saved(&options)
                .query_row(
                    "SELECT count(*) FROM captured_request_candidates",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            16
        );
        assert_eq!(
            saved(&options)
                .query_row("SELECT count(*) FROM captured_request_ids", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn durable_capture_has_no_legacy_test_capacity_limit() {
        let root = TempDir::new().unwrap();
        let options = options(&root);
        let mut db = saved(&options);
        db.execute_batch("CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER,PRIMARY KEY(thread_id,turn_id))").unwrap();
        let tx = db.transaction().unwrap();
        let reply = "x".repeat(17000);
        for _ in 0..1001 {
            tx.execute(
                "INSERT INTO captured_replies VALUES(?1,?2,?3,17000,1)",
                params![N, uuid::Uuid::new_v4().to_string(), reply],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(db);
        assert_eq!(execute(&event(), &options).unwrap(), "captured");
        assert_eq!(
            saved(&options)
                .query_row("SELECT count(*) FROM captured_replies", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1002
        );
        let mut legacy = options;
        legacy.legacy_limits = true;
        let mut event = event();
        event["turn-id"] = json!(OTHER);
        assert_eq!(
            execute(&event, &legacy).unwrap_err().to_string(),
            "capture_capacity_exceeded"
        );
    }

    #[test]
    fn legacy_byte_capacity_is_bounded_and_existing_duplicate_still_succeeds() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        options.legacy_limits = true;
        let db = saved(&options);
        db.execute_batch("CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER,PRIMARY KEY(thread_id,turn_id))").unwrap();
        db.execute(
            "INSERT INTO captured_replies VALUES(?1,?2,'legacy',?3,1)",
            params![N, OTHER, MAX_STORED_BYTES],
        )
        .unwrap();
        drop(db);
        assert_eq!(
            execute(&event(), &options).unwrap_err().to_string(),
            "capture_capacity_exceeded"
        );
        let mut event = event();
        event["turn-id"] = json!(OTHER);
        event["last-assistant-message"] = json!("legacy");
        assert_eq!(execute(&event, &options).unwrap(), "duplicate");
        assert!(!crate::health::path(&options.database).exists());
    }

    #[test]
    fn budget_failure_is_durable_and_exact_retry_resolves_failure() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        options.storage_budget = Some(1);
        assert_eq!(
            execute(&event(), &options).unwrap_err().to_string(),
            "storage_budget_exceeded"
        );
        assert_eq!(failure(&options), "storage_budget_exceeded");
        assert_eq!(
            saved(&options)
                .query_row("SELECT count(*) FROM captured_replies", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        options.storage_budget = None;
        assert_eq!(execute(&event(), &options).unwrap(), "captured");
        assert_eq!(execute(&event(), &options).unwrap(), "duplicate");
        assert_eq!(
            crate::health::read(&options.database).unwrap()["failures"],
            json!({})
        );
    }

    #[test]
    fn disk_guard_keeps_existing_reply_and_records_missing_next_reply() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        let mut event = event();
        execute(&event, &options).unwrap();
        options.min_free_bytes = u64::MAX;
        event["turn-id"] = json!(OTHER);
        assert_eq!(
            execute(&event, &options).unwrap_err().to_string(),
            "disk_space_low"
        );
        assert_eq!(
            saved(&options)
                .query_row("SELECT count(*) FROM captured_replies", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            crate::health::read(&options.database).unwrap()["failures"][format!("{N}:{OTHER}")]
                ["reason"],
            "disk_space_low"
        );
    }

    #[test]
    fn storage_errors_record_generic_failure_without_error_or_event_content() {
        let root = TempDir::new().unwrap();
        let options = options(&root);
        fs::create_dir(&options.database).unwrap();
        assert!(execute(&event(), &options).is_err());
        assert_eq!(failure(&options), "storage_write_failed");
        let path = PathBuf::from(format!("{}.health", options.database.display()));
        let health = fs::read(path).unwrap();
        assert!(!health.windows(7).any(|w| w == b"PRIVATE"));
    }

    #[test]
    fn durable_unconfirmed_intent_is_cleared_only_after_success() {
        let root = TempDir::new().unwrap();
        let options = options(&root);
        crate::health::update(&options.database, N, T, Some("capture_not_confirmed"), None)
            .unwrap();
        assert_eq!(failure(&options), "capture_not_confirmed");
        execute(&event(), &options).unwrap();
        assert_eq!(
            crate::health::read(&options.database).unwrap()["failures"],
            json!({})
        );
    }

    #[test]
    fn title_index_is_metadata_only_and_failure_falls_back_without_transcript_reads() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        let index = root.path().join("index.jsonl");
        options.title_index = Some(index.clone());
        fs::write(
            &index,
            format!(
                "{}\n",
                json!({"id":N,"thread_name":"indexed title","updated_at":"2026-10-03"})
            ),
        )
        .unwrap();
        execute(&event(), &options).unwrap();
        assert_eq!(
            saved(&options)
                .query_row("SELECT title FROM captured_replies", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "indexed title"
        );
        fs::write(
            &index,
            format!(
                "{}\n",
                json!({"id":N,"thread_name":"untrusted","transcript":"PRIVATE INPUT"})
            ),
        )
        .unwrap();
        assert!(indexed_title(N, &index)
            .unwrap_err()
            .to_string()
            .contains("metadata"));
        let mut event = event();
        event["turn-id"] = json!(OTHER);
        execute(&event, &options).unwrap();
        assert_eq!(
            saved(&options)
                .query_row(
                    "SELECT title FROM captured_replies WHERE turn_id=?1",
                    [OTHER],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            format!("会话 · {}", &N[..8])
        );
        fs::write(&index, "x".repeat(8193)).unwrap();
        assert_eq!(
            indexed_title(N, &index).unwrap_err().to_string(),
            "title_index_limit"
        );
    }

    #[test]
    fn equal_catalog_titles_keep_native_ids_and_unapproved_tasks_are_ignored() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        let catalog = root.path().join("catalog.json");
        options.thread = None;
        options.catalog = Some(catalog.clone());
        fs::write(catalog, json!({N:"同名任务",T:"同名任务"}).to_string()).unwrap();
        for native in [N, T] {
            let mut event = event();
            event["thread-id"] = json!(native);
            event["turn-id"] = json!(native);
            event["last-assistant-message"] = json!(native);
            assert_eq!(execute(&event, &options).unwrap(), "captured");
            assert_eq!(execute(&event, &options).unwrap(), "duplicate");
        }
        let mut event = event();
        event["thread-id"] = json!(OTHER);
        assert_eq!(execute(&event, &options).unwrap(), "ignored");
        assert_eq!(saved(&options).query_row("SELECT count(*) FROM captured_replies WHERE title='同名任务' AND reply=thread_id",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn missing_scope_and_invalid_catalog_or_budget_are_rejected() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root);
        options.thread = None;
        assert_eq!(
            execute(&event(), &options).unwrap_err().to_string(),
            "capture_scope_required"
        );
        options.all_tasks = true;
        options.storage_budget = Some(0);
        assert_eq!(
            execute(&event(), &options).unwrap_err().to_string(),
            "invalid_storage_budget"
        );
        options.storage_budget = None;
        let catalog = root.path().join("catalog.json");
        fs::write(&catalog, "{}").unwrap();
        options.catalog = Some(catalog);
        assert_eq!(
            execute(&event(), &options).unwrap_err().to_string(),
            "invalid_catalog"
        );
        assert!(!options.database.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_database_is_rejected_and_new_database_is_private() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = TempDir::new().unwrap();
        let options = options(&root);
        let target = root.path().join("target.sqlite");
        fs::write(&target, []).unwrap();
        symlink(&target, &options.database).unwrap();
        assert_eq!(
            execute(&event(), &options).unwrap_err().to_string(),
            "symlink_database"
        );
        assert_eq!(fs::metadata(target).unwrap().len(), 0);
        fs::remove_file(&options.database).unwrap();
        execute(&event(), &options).unwrap();
        assert_eq!(
            fs::metadata(&options.database)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
