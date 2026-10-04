//! Persistent collection boundary shared by notify, poll, deletion and reset.
use anyhow::{ensure, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const CAPTURE_TABLES: &[&str] = &[
    "captured_replies",
    "captured_request_ids",
    "captured_request_candidates",
    "captured_user_messages",
    "captured_turn_order",
    "captured_visible_messages",
    "captured_images",
];
pub const HUB_TABLES: &[&str] = &[
    "threads",
    "messages",
    "events",
    "outbox",
    "history_positions",
    "completion_metadata",
    "capture_targets",
    "capture_health",
    "capture_import_positions",
    "capture_user_import_positions",
    "capture_visible_import_positions",
    "attachments",
    "thread_message_state",
    "resume_owners",
];
pub fn suffix(path: &Path, extra: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(extra);
    s.into()
}
pub fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}
pub fn open_file(path: &Path, writable: bool, create: bool) -> Result<File> {
    if create {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
    }
    ensure!(!path.is_symlink(), "symlink_file_refused");
    let mut options = OpenOptions::new();
    options.read(true).write(writable).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path)?;
    ensure!(file.metadata()?.is_file(), "regular_file_required");
    Ok(file)
}
pub struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
pub fn lock(database: &Path) -> Result<Lock> {
    let mut file = open_file(&suffix(database, ".collection.lock"), true, true)?;
    if file.metadata()?.len() == 0 {
        file.write_all(b"0")?;
        file.sync_all()?;
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(Lock(file)),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                ensure!(Instant::now() < deadline, "collection_lock_timeout");
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
}
pub fn policy_path(database: &Path) -> PathBuf {
    suffix(database, ".collection.json")
}
fn validate_policy(policy: &Value) -> Result<()> {
    ensure!(
        policy["schema"].as_i64() == Some(1) && policy["cutoff_ms"].as_i64().is_some_and(|n| n > 0),
        "collection_policy_invalid"
    );
    ensure!(
        policy.get("enabled").is_none_or(Value::is_boolean),
        "collection_policy_invalid"
    );
    uuid::Uuid::parse_str(
        policy["generation"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("collection_policy_invalid"))?,
    )?;
    let excluded = policy["excluded"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("collection_policy_invalid"))?;
    ensure!(excluded.len() <= 50_000, "collection_policy_invalid");
    for native in excluded {
        uuid::Uuid::parse_str(
            native
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("collection_policy_invalid"))?,
        )?;
    }
    Ok(())
}
pub fn read_policy(database: &Path) -> Result<Option<Value>> {
    let path = policy_path(database);
    if !path.exists() {
        ensure!(
            !suffix(database, ".collection-required").exists(),
            "collection_policy_missing"
        );
        return Ok(None);
    }
    let file = open_file(&path, false, false)?;
    ensure!(
        file.metadata()?.len() <= 4 * 1024 * 1024,
        "collection_policy_invalid"
    );
    let policy: Value = serde_json::from_reader(file)?;
    validate_policy(&policy)?;
    Ok(Some(policy))
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("next");
    let mut file = open_file(&temporary, true, true)?;
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    crate::native_fleet::replace(&temporary, path)?;
    sync_parent(path)?;
    Ok(())
}
fn write_unlocked(database: &Path, policy: &Value) -> Result<()> {
    validate_policy(policy)?;
    atomic(&policy_path(database), &serde_json::to_vec(policy)?)?;
    open_file(&suffix(database, ".collection-required"), true, true)?.sync_all()?;
    sync_parent(database)
}
pub fn write_policy(database: &Path, policy: &Value) -> Result<()> {
    let _guard = lock(database)?;
    write_unlocked(database, policy)
}
pub fn readonly(path: &Path) -> Result<Connection> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(Duration::from_secs(3))?;
    Ok(c)
}
pub fn baseline(index: &Path, generation: &str) -> Result<Value> {
    uuid::Uuid::parse_str(generation)?;
    let mut db = readonly(index)?;
    let tx = db.transaction()?;
    let mut ids = Vec::new();
    {
        let mut query = tx.prepare("SELECT id FROM threads")?;
        for row in query.query_map([], |r| r.get::<_, String>(0))? {
            let native = row?;
            uuid::Uuid::parse_str(&native)?;
            ids.push(native);
            ensure!(ids.len() <= 50_000, "collection_policy_invalid");
        }
    }
    let policy = json!({"schema":1,"generation":generation,"cutoff_ms":chrono::Utc::now().timestamp_millis(),"excluded":ids});
    tx.commit()?;
    Ok(policy)
}
/// The native index includes internal agent threads that are not user conversations.
/// Support both recent thread_source metadata and the older serialized source field.
pub fn is_subagent(index: &Connection, native: &str) -> Result<bool> {
    let columns = index
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let kind = if columns.contains("thread_source") {
        "thread_source"
    } else {
        "NULL"
    };
    let source = if columns.contains("source") {
        "source"
    } else {
        "NULL"
    };
    let row: Option<(Option<String>, Option<String>)> = index
        .query_row(
            &format!("SELECT {kind},{source} FROM threads WHERE id=?1"),
            [native],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((kind, source)) = row else {
        return Ok(false);
    };
    Ok(kind.as_deref() == Some("subagent")
        || source.is_some_and(|source| {
            source == "subagent"
                || serde_json::from_str::<Value>(&source).is_ok_and(|value| {
                    value.get("subagent").is_some() || value.as_str() == Some("subagent")
                })
        }))
}
pub fn is_archived(index: &Connection, native: &str) -> Result<bool> {
    let columns = index
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    if !columns.contains("archived") {
        return Ok(false);
    }
    Ok(index
        .query_row(
            "SELECT archived=1 FROM threads WHERE id=?1",
            [native],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}
pub fn allowed(database: &Path, index: Option<&Path>, native: &str) -> Result<bool> {
    let policy = read_policy(database)?;
    // Filter even when collection has no cutoff yet. Explicit native metadata is
    // authoritative; agent_created_thread remains an ordinary named conversation.
    let source = index.map(readonly).transpose()?;
    if let Some(source) = &source {
        if is_subagent(source, native)? || is_archived(source, native)? {
            return Ok(false);
        }
    }
    let Some(policy) = policy else {
        return Ok(true);
    };
    if policy["enabled"].as_bool() == Some(false)
        || policy["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str() == Some(native))
    {
        return Ok(false);
    }
    let db = source.ok_or_else(|| anyhow::anyhow!("collection_index_required"))?;
    let mut query = db.prepare("PRAGMA table_info(threads)")?;
    let ms = query
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|s| s == "created_at_ms");
    let creation = if ms {
        "COALESCE(created_at_ms,created_at*1000)"
    } else {
        "created_at*1000"
    };
    let stamp: Option<rusqlite::types::Value> = db
        .query_row(
            &format!("SELECT {creation} FROM threads WHERE id=?1"),
            [native],
            |r| r.get(0),
        )
        .optional()?;
    Ok(
        matches!(stamp,Some(rusqlite::types::Value::Integer(n)) if n > policy["cutoff_ms"].as_i64().unwrap()),
    )
}
pub fn purge(database: &Path, natives: &[String]) -> Result<()> {
    if natives.is_empty() {
        return Ok(());
    }
    let _guard = lock(database)?;
    let mut policy =
        read_policy(database)?.ok_or_else(|| anyhow::anyhow!("collection_policy_required"))?;
    let mut ids = policy["excluded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    for native in natives {
        uuid::Uuid::parse_str(native)?;
        ids.insert(native.clone());
    }
    policy["excluded"] = json!(ids);
    write_unlocked(database, &policy)?;
    let mut db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.busy_timeout(Duration::from_secs(3))?;
    db.pragma_update(None, "secure_delete", true)?;
    let tx = db.transaction()?;
    let tables = tables(&tx)?;
    for table in CAPTURE_TABLES {
        if tables.contains(*table) {
            for native in natives {
                tx.execute(&format!("DELETE FROM {table} WHERE thread_id=?1"), [native])?;
            }
        }
    }
    tx.commit()?;
    if crate::health::path(database).exists() {
        let health = crate::health::read(database)?;
        for failure in health["failures"].as_object().unwrap().values() {
            let native = failure["thread_id"].as_str().unwrap_or_default();
            if natives.iter().any(|n| n == native) {
                crate::health::update(
                    database,
                    native,
                    failure["turn_id"].as_str().unwrap_or_default(),
                    None,
                    None,
                )?;
            }
        }
    }
    Ok(())
}
fn tables(db: &Connection) -> Result<BTreeSet<String>> {
    Ok(db
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn erase_replica(path: &Path, generation: &str) -> Result<()> {
    uuid::Uuid::parse_str(generation)?;
    ensure!(!path.is_symlink(), "replica_symlink_refused");
    let _guard = lock(path)?;
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "secure_delete", true)?;
    let existing = tables(&db)?;
    ensure!(
        existing.contains("captured_replies")
            || ["devices", "commands", "messages"]
                .iter()
                .all(|n| existing.contains(*n)),
        "not_threadbridge_replica"
    );
    let tx = db.transaction()?;
    if existing.contains("tombstones") && existing.contains("threads") {
        tx.execute(
            "INSERT OR IGNORE INTO tombstones SELECT id,'' FROM threads",
            [],
        )?;
    }
    for table in CAPTURE_TABLES.iter().chain(HUB_TABLES) {
        if existing.contains(*table) {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
    }
    if existing.contains("commands") {
        tx.execute("UPDATE commands SET payload='{}',status=CASE WHEN status='accepted' THEN 'cancelled' WHEN status IN ('dispatching','upstream_queued') THEN 'unknown' ELSE status END,error='collection_reset'",[])?;
    }
    if existing.contains("settings") {
        tx.execute(
            "INSERT OR REPLACE INTO settings VALUES('collection_generation',?1)",
            [generation],
        )?;
    }
    tx.commit()?;
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    drop(db);
    for suffix in [".health", ".health.next"] {
        let p = self::suffix(path, suffix);
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String, Value) {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("index.sqlite");
        let db = dir.path().join("replies.sqlite");
        let old = uuid::Uuid::new_v4().to_string();
        let c = Connection::open(&index).unwrap();
        c.execute_batch("CREATE TABLE threads(id TEXT,created_at INTEGER,created_at_ms INTEGER)")
            .unwrap();
        c.execute("INSERT INTO threads VALUES(?1,1,1000)", [&old])
            .unwrap();
        let p = baseline(&index, &uuid::Uuid::new_v4().to_string()).unwrap();
        write_policy(&db, &p).unwrap();
        (dir, index, db, old, p)
    }
    #[test]
    fn old_and_late_restored_threads_stay_excluded() {
        let (_d, index, db, old, _) = fixture();
        assert!(!allowed(&db, Some(&index), &old).unwrap());
        let new = uuid::Uuid::new_v4().to_string();
        Connection::open(&index)
            .unwrap()
            .execute("INSERT INTO threads VALUES(?1,2,2000)", [&new])
            .unwrap();
        assert!(!allowed(&db, Some(&index), &new).unwrap());
    }
    #[test]
    fn fresh_and_same_second_creation() {
        let (_d, index, db, _, p) = fixture();
        let new = uuid::Uuid::new_v4().to_string();
        assert!(!allowed(&db, Some(&index), &new).unwrap());
        Connection::open(&index)
            .unwrap()
            .execute(
                "INSERT INTO threads VALUES(?1,?2,?3)",
                rusqlite::params![
                    new,
                    p["cutoff_ms"].as_i64().unwrap() / 1000,
                    p["cutoff_ms"].as_i64().unwrap() + 1
                ],
            )
            .unwrap();
        assert!(allowed(&db, Some(&index), &new).unwrap());
    }
    #[test]
    fn subagents_are_excluded_with_or_without_a_collection_cutoff() {
        let (_d, index, db, _, p) = fixture();
        let c = Connection::open(&index).unwrap();
        c.execute_batch("ALTER TABLE threads ADD COLUMN thread_source TEXT; ALTER TABLE threads ADD COLUMN source TEXT").unwrap();
        let cutoff = p["cutoff_ms"].as_i64().unwrap() + 1;
        for (kind, source, expected) in [
            ("subagent", "{}", false),
            (
                "user",
                r#"{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}"#,
                false,
            ),
            ("user", "subagent", false),
            ("user", "cli", true),
            ("agent_created_thread", "appServer", true),
        ] {
            let native = uuid::Uuid::new_v4().to_string();
            c.execute(
                "INSERT INTO threads VALUES(?1,1,?2,?3,?4)",
                rusqlite::params![native, cutoff, kind, source],
            )
            .unwrap();
            assert_eq!(allowed(&db, Some(&index), &native).unwrap(), expected);
            let no_policy = db.with_file_name("unmanaged.sqlite");
            assert_eq!(
                allowed(&no_policy, Some(&index), &native).unwrap(),
                expected
            );
        }
    }
    #[test]
    fn archive_filter_uses_identity_metadata_and_preserves_active_same_title_threads() {
        let (_d, index, db, _, p) = fixture();
        let c = Connection::open(&index).unwrap();
        c.execute_batch("ALTER TABLE threads ADD COLUMN archived INTEGER NOT NULL DEFAULT 0; ALTER TABLE threads ADD COLUMN title TEXT").unwrap();
        for archived in [0, 0, 1] {
            let native = uuid::Uuid::new_v4().to_string();
            c.execute(
                "INSERT INTO threads VALUES(?1,1,?2,?3,'same name')",
                rusqlite::params![native, p["cutoff_ms"].as_i64().unwrap() + 1, archived],
            )
            .unwrap();
            assert_eq!(allowed(&db, Some(&index), &native).unwrap(), archived == 0);
        }
    }
    #[test]
    fn missing_corrupt_or_disabled_policy_fails_closed() {
        let (_d, index, db, old, mut p) = fixture();
        std::fs::remove_file(policy_path(&db)).unwrap();
        assert!(allowed(&db, Some(&index), &old).is_err());
        std::fs::write(policy_path(&db), "{}").unwrap();
        assert!(allowed(&db, Some(&index), &old).is_err());
        p["enabled"] = json!(false);
        write_policy(&db, &p).unwrap();
        assert!(!allowed(&db, Some(&index), &uuid::Uuid::new_v4().to_string()).unwrap());
    }
    #[test]
    fn purge_removes_replica_and_health_but_preserves_source() {
        let (_d, index, db, old, _) = fixture();
        let c = Connection::open(&db).unwrap();
        c.execute_batch("CREATE TABLE captured_replies(thread_id TEXT,reply TEXT)")
            .unwrap();
        c.execute("INSERT INTO captured_replies VALUES(?1,'private')", [&old])
            .unwrap();
        crate::health::update(&db, &old, "turn", Some("capture_not_confirmed"), None).unwrap();
        purge(&db, std::slice::from_ref(&old)).unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM captured_replies", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            Connection::open(index)
                .unwrap()
                .query_row("SELECT count(*) FROM threads", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(crate::health::read(&db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn reset_preserves_identity_and_unknown_dedup_erases_bodies() {
        let (d, _, db, _, p) = fixture();
        let c = Connection::open(&db).unwrap();
        c.execute_batch("CREATE TABLE devices(id TEXT,token TEXT);INSERT INTO devices VALUES('phone','credential');CREATE TABLE commands(id TEXT,request TEXT,digest TEXT,payload TEXT,status TEXT,error TEXT);INSERT INTO commands VALUES('command','immutable-request','digest','secret','unknown',NULL);CREATE TABLE messages(thread TEXT,body TEXT);INSERT INTO messages VALUES('thread','private');CREATE TABLE settings(k TEXT PRIMARY KEY,v TEXT);").unwrap();
        drop(c);
        erase_replica(&db, p["generation"].as_str().unwrap()).unwrap();
        let c = Connection::open(&db).unwrap();
        assert_eq!(
            c.query_row("SELECT token FROM devices", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "credential"
        );
        assert_eq!(
            c.query_row("SELECT payload,status FROM commands", [], |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?
            )))
            .unwrap(),
            ("{}".into(), "unknown".into())
        );
        drop(c);
        let bytes = std::fs::read(d.path().join("replies.sqlite")).unwrap();
        assert!(!bytes.windows(7).any(|b| b == b"private"));
        assert!(!bytes.windows(6).any(|b| b == b"secret"));
    }
    #[test]
    fn cross_process_lock_fixture() {
        if let Some(path) = std::env::var_os("THREADBRIDGE_TEST_COLLECTION_LOCK") {
            let _guard = lock(Path::new(&path)).unwrap();
            std::fs::write(suffix(Path::new(&path), ".acquired"), b"yes").unwrap();
        }
    }
    #[test]
    fn cross_process_lock_serializes_delete() {
        let (_d, _, db, _, _) = fixture();
        let guard = lock(&db).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "collection::tests::cross_process_lock_fixture"])
            .env("THREADBRIDGE_TEST_COLLECTION_LOCK", &db)
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!suffix(&db, ".acquired").exists());
        drop(guard);
        assert!(child.wait().unwrap().success());
        assert!(suffix(&db, ".acquired").exists());
    }
}
