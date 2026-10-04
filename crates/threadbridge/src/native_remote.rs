//! Bounded native SSH RPC. No shell fragments or arbitrary operation names are accepted.
use crate::{collection, health, native_capture};
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::DateTime;
use clap::Args;
use flate2::{write::ZlibEncoder, Compression};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use uuid::Uuid;
pub const TAIL_BYTES: u64 = 16 * 1024 * 1024;
pub const TOTAL_SCAN: u64 = 128 * 1024 * 1024;
pub const DB_BYTES: u64 = 32 * 1024 * 1024;
pub const REQUEST_BYTES: usize = 256 * 1024;
const MAX_LINE: usize = 4 * 1024 * 1024;
pub const TABLES: [&str; 4] = [
    "captured_replies",
    "captured_user_messages",
    "captured_turn_order",
    "captured_request_ids",
];
#[derive(Args)]
pub struct RemoteArgs {
    #[arg(long)]
    pub config: PathBuf,
}
#[derive(Clone, Debug, Deserialize)]
pub struct RemoteConfig {
    pub codex: PathBuf,
    pub codex_home: PathBuf,
    pub verified_version: Option<String>,
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn fingerprint(path: &Path) -> Result<Value> {
    let m = fs::metadata(path)?;
    let t = m.modified()?.duration_since(UNIX_EPOCH)?;
    Ok(json!([m.len(), t.as_nanos() as u64]))
}
pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    crate::native_fleet::atomic(path, &serde_json::to_vec(value)?)
}
pub fn readonly(path: &Path) -> Result<Connection> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(Duration::from_secs(3))?;
    Ok(c)
}
pub fn initialize(database: &Path) -> Result<()> {
    if let Some(p) = database.parent() {
        fs::create_dir_all(p)?;
    }
    let c = Connection::open(database)?;
    c.busy_timeout(Duration::from_secs(3))?;
    c.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;PRAGMA max_page_count=8192;
 CREATE TABLE IF NOT EXISTS captured_replies(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,reply TEXT NOT NULL,utf8_bytes INTEGER NOT NULL,captured_at INTEGER NOT NULL,title TEXT NOT NULL DEFAULT '',PRIMARY KEY(thread_id,turn_id));
 CREATE TABLE IF NOT EXISTS captured_request_ids(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,request_id TEXT NOT NULL,PRIMARY KEY(thread_id,turn_id));
 CREATE TABLE IF NOT EXISTS captured_user_messages(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,input_digest TEXT NOT NULL,PRIMARY KEY(thread_id,message_id));
 CREATE TABLE IF NOT EXISTS captured_turn_order(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,completed_at INTEGER NOT NULL,PRIMARY KEY(thread_id,turn_id));")?;
    if !health::path(database).exists() {
        health::update(database, "initial", "initial", None, None)?;
    }
    Ok(())
}
pub fn backup(source: &Path, destination: &Path) -> Result<()> {
    let c = readonly(source)?;
    c.backup(rusqlite::DatabaseName::Main, destination, None)?;
    Ok(())
}
pub fn completed_events(
    path: &Path,
    native: &str,
    home: &Path,
    tail_bytes: u64,
) -> Result<(Vec<(i64, Value)>, u64)> {
    let resolved = path.canonicalize()?;
    let allowed = ["sessions", "archived_sessions"].into_iter().any(|n| {
        home.join(n)
            .canonicalize()
            .is_ok_and(|r| resolved.starts_with(r))
    });
    ensure!(allowed, "rollout_path_outside_sessions");
    let file = fs::File::open(&resolved)?;
    let size = file.metadata()?.len();
    let mut stream = BufReader::new(file);
    let mut line = Vec::new();
    stream
        .by_ref()
        .take((MAX_LINE + 1) as u64)
        .read_until(b'\n', &mut line)?;
    ensure!(line.len() <= MAX_LINE, "session_metadata_limit");
    let metadata: Value = serde_json::from_slice(&line)?;
    ensure!(
        metadata["type"] == "session_meta" && metadata["payload"]["id"] == native,
        "session_identity_mismatch"
    );
    if size > tail_bytes {
        stream.seek(SeekFrom::Start(size - tail_bytes))?;
        line.clear();
        stream
            .by_ref()
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut line)?;
    } else {
        stream.seek(SeekFrom::Start(0))?;
    }
    let mut used = 0;
    let mut events = Vec::new();
    loop {
        line.clear();
        let n = stream
            .by_ref()
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        used += n as u64;
        ensure!(
            n <= MAX_LINE && used <= tail_bytes + MAX_LINE as u64,
            "rollout_scan_limit"
        );
        let record: Value = serde_json::from_slice(&line)?;
        let p = &record["payload"];
        if record["type"] == "event_msg" && p["type"] == "task_complete" {
            let turn = p["turn_id"].as_str().context("completion_turn_missing")?;
            Uuid::parse_str(turn)?;
            let Some(reply) = p["last_agent_message"].as_str().filter(|r| !r.is_empty()) else {
                continue;
            };
            let stamp = DateTime::parse_from_rfc3339(
                record["timestamp"]
                    .as_str()
                    .context("completion_timestamp_missing")?,
            )?
            .timestamp_millis();
            events.push((stamp,json!({"type":"agent-turn-complete","thread-id":native,"turn-id":turn,"last-assistant-message":reply})));
        }
    }
    Ok((events, size.min(tail_bytes)))
}
pub fn poll(config: &RemoteConfig, database: &Path) -> Result<()> {
    poll_limits(config, database, TAIL_BYTES, TOTAL_SCAN)
}
pub fn poll_limits(
    config: &RemoteConfig,
    database: &Path,
    tail: u64,
    total_scan: u64,
) -> Result<()> {
    let home = config.codex_home.canonicalize()?;
    let index = home.join("state_5.sqlite");
    let cache_path = database
        .parent()
        .context("database_parent")?
        .join("poll.json");
    let mut cache = if cache_path.exists() {
        serde_json::from_slice::<Value>(&fs::read(&cache_path)?)?
            .as_object()
            .cloned()
            .context("poll_cache_invalid")?
    } else {
        Map::new()
    };
    let status = health::read(database)?;
    let quarantined: std::collections::HashSet<String> = status["failures"]
        .as_object()
        .context("health_failures_invalid")?
        .values()
        .filter(|v| v["turn_id"] == "recovery")
        .filter_map(|v| v["thread_id"].as_str().map(str::to_owned))
        .collect();
    let c = readonly(&index)?;
    let mut statement =
        c.prepare("SELECT id,rollout_path,title FROM threads ORDER BY updated_at DESC LIMIT 200")?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut total = 0u64;
    for (native, raw, title) in rows {
        if !collection::allowed(database, Some(&index), &native)? || quarantined.contains(&native) {
            continue;
        }
        let mut file_fingerprint = Value::Null;
        let attempt = (|| -> Result<()> {
            Uuid::parse_str(&native)?;
            let path = Path::new(&raw);
            let size = fs::metadata(path)?.len();
            file_fingerprint = fingerprint(path)?;
            let old = cache.get(&native).cloned().unwrap_or_else(|| json!({}));
            ensure!(
                cache.contains_key(&native) || cache.len() < 5000,
                "poll_cache_budget"
            );
            if old["file"] == file_fingerprint
                && (!old["error_at"].as_f64().is_some_and(|t| t > 0.0)
                    || now() - old["error_at"].as_f64().unwrap_or(0.0) < 300.0)
            {
                return Ok(());
            }
            if total + size.min(tail) > total_scan {
                return Ok(());
            }
            let (events, scanned) = completed_events(path, &native, &home, tail)?;
            total += scanned;
            let mut confirmed = old["turn_id"].as_str().map(str::to_owned);
            if old["completed_at"].as_i64().unwrap_or(0) > 0 && confirmed.is_none() {
                let db = Connection::open(database)?;
                confirmed=db.query_row("SELECT turn_id FROM captured_turn_order WHERE thread_id=? AND completed_at=?",params![native,old["completed_at"].as_i64()],|r|r.get(0)).optional()?;
                ensure!(confirmed.is_some(), "poll_watermark_unconfirmed");
            }
            let candidates = if let Some(ref turn) = confirmed {
                let anchor = events
                    .iter()
                    .rposition(|(_, e)| e["turn-id"] == turn.as_str())
                    .context("completed_turn_gap")?;
                &events[anchor + 1..]
            } else {
                &events[events.len().saturating_sub(1)..]
            };
            let mut latest = old["completed_at"].as_i64().unwrap_or(0);
            let mut processed = 0;
            for (stamp, event) in candidates {
                ensure!(
                    size.min(tail).saturating_add(size) <= total_scan,
                    "poll_scan_budget"
                );
                if total + size > total_scan {
                    break;
                }
                total += size;
                let opts = native_capture::CaptureOptions {
                    database: database.to_owned(),
                    thread: None,
                    catalog: None,
                    all_tasks: true,
                    title: None,
                    title_index: None,
                    user_turn_index: Some(index.clone()),
                    storage_budget: Some(DB_BYTES),
                    min_free_bytes: 64 * 1024 * 1024,
                    legacy_limits: false,
                    store_candidates: false,
                };
                let result = native_capture::capture(&serde_json::to_string(event)?, &opts)?;
                ensure!(
                    result == "captured" || result == "duplicate",
                    "capture_unconfirmed"
                );
                let display = title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| t.chars().take(150).collect::<String>())
                    .unwrap_or_else(|| format!("会话 · {}", &native[..8]));
                Connection::open(database)?.execute(
                    "UPDATE captured_replies SET title=? WHERE thread_id=? AND turn_id=?",
                    params![display, native, event["turn-id"].as_str()],
                )?;
                latest = latest.max(*stamp);
                confirmed = event["turn-id"].as_str().map(str::to_owned);
                processed += 1;
            }
            cache.insert(native.clone(),json!({"file":if processed==candidates.len(){file_fingerprint.clone()}else{Value::Null},"completed_at":latest,"turn_id":confirmed}));
            health::update(database, &native, "poll", None, None)?;
            Ok(())
        })();
        if attempt.is_err() {
            health::update(
                database,
                &native,
                "poll",
                Some("capture_not_confirmed"),
                None,
            )?;
            if cache.contains_key(&native) || cache.len() < 5000 {
                let mut saved = cache
                    .get(&native)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                saved.insert("file".into(), file_fingerprint);
                saved.insert("error_at".into(), json!(now()));
                cache.insert(native, json!(saved));
            }
        }
    }
    atomic_json(&cache_path, &json!(cache))
}
/// Pipe reads are bounded while a subprocess runs; timeout covers input and output.
pub async fn bounded_process(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    limit: usize,
    error_limit: usize,
) -> Result<Vec<u8>> {
    use std::process::Stdio;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("rpc_spawn_failed")?;
    let mut stdin = child.stdin.take().context("rpc_stdin")?;
    let stdout = child.stdout.take().context("rpc_stdout")?;
    let stderr = child.stderr.take().context("rpc_stderr")?;
    let operation = async {
        let write = async {
            stdin.write_all(input).await?;
            stdin.shutdown().await?;
            Ok::<(), anyhow::Error>(())
        };
        let read = async {
            let mut out = Vec::new();
            stdout
                .take((limit + 1) as u64)
                .read_to_end(&mut out)
                .await?;
            ensure!(out.len() <= limit, "rpc_stream_limit");
            Ok::<_, anyhow::Error>(out)
        };
        let errors = async {
            let mut err = Vec::new();
            stderr
                .take((error_limit + 1) as u64)
                .read_to_end(&mut err)
                .await?;
            ensure!(err.len() <= error_limit, "rpc_stream_limit");
            Ok::<_, anyhow::Error>(())
        };
        let (_, out, _) = tokio::try_join!(write, read, errors)?;
        let status = child.wait().await?;
        ensure!(status.success(), "rpc_result_unconfirmed");
        Ok::<_, anyhow::Error>(out)
    };
    match tokio::time::timeout(timeout, operation).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => {
            let _ = child.kill().await;
            Err(e)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(anyhow!("rpc_timeout"))
        }
    }
}
pub async fn codex_output(
    config: &RemoteConfig,
    args: &[String],
    timeout: Duration,
    limit: usize,
) -> Result<String> {
    let mut command = Command::new(&config.codex);
    command.args(args);
    Ok(String::from_utf8(
        bounded_process(command, &[], timeout, limit, 65536).await?,
    )?)
}
pub async fn version(config: &RemoteConfig) -> Result<String> {
    Ok(
        codex_output(config, &["--version".into()], Duration::from_secs(4), 512)
            .await?
            .trim()
            .to_owned(),
    )
}
pub async fn snapshot(config: &RemoteConfig, database: &Path, request: &Value) -> Result<Value> {
    if let Some(excluded) = request.get("excluded") {
        let ids = excluded.as_array().context("excluded_limit")?;
        ensure!(ids.len() <= 5000, "excluded_limit");
        let ids = ids
            .iter()
            .map(|v| {
                let s = v.as_str().context("excluded_invalid")?;
                Uuid::parse_str(s)?;
                Ok(s.to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        if !ids.is_empty() {
            collection::purge(database, &ids)?;
        }
    }
    initialize(database)?;
    poll(config, database)?;
    let c = Connection::open(database)?;
    let mut identity = Vec::new();
    for table in &TABLES[..3] {
        let counts = c.query_row(
            &format!("SELECT count(*),coalesce(max(rowid),0) FROM {table}"),
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?;
        identity.push(json!([counts.0, counts.1]));
    }
    let failures = health::read(database)?;
    identity.push(json!([failures["failures"], failures["overflow"]]));
    let revision = format!("{:x}", Sha256::digest(serde_json::to_vec(&identity)?));
    let blocked: std::collections::BTreeSet<_> = failures["failures"]
        .as_object()
        .context("health_failures_invalid")?
        .values()
        .filter_map(|v| v["thread_id"].as_str())
        .collect();
    let changed = request["revision"] != revision;
    let mut result = json!({"schema":1,"version":version(config).await?,"revision":revision,"capture_ok":blocked.is_empty()&&failures["overflow"]==false,"blocked_threads":blocked,"overflow":failures["overflow"],"changed":changed});
    if changed {
        let tmp = database
            .parent()
            .unwrap()
            .join(format!("snapshot-{}.sqlite", Uuid::new_v4()));
        let backup_result = (|| -> Result<String> {
            backup(database, &tmp)?;
            ensure!(fs::metadata(&tmp)?.len() <= DB_BYTES, "snapshot_budget");
            let mut z = ZlibEncoder::new(Vec::new(), Compression::new(3));
            z.write_all(&fs::read(&tmp)?)?;
            Ok(STANDARD.encode(z.finish()?))
        })();
        let _ = fs::remove_file(&tmp);
        result["database"] = json!(backup_result?);
        result["health"] = json!(STANDARD.encode(fs::read(health::path(database))?));
    }
    Ok(result)
}
pub async fn dispatch(config: &RemoteConfig, database: &Path, request: &Value) -> Result<Value> {
    dispatch_with_runner(config, database, request, |args, timeout, limit| {
        let config = config.clone();
        async move { codex_output(&config, &args, timeout, limit).await }
    })
    .await
}
async fn dispatch_with_runner<F, Fut>(
    config: &RemoteConfig,
    database: &Path,
    request: &Value,
    mut run: F,
) -> Result<Value>
where
    F: FnMut(Vec<String>, Duration, usize) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let native = request["thread"].as_str().context("queue_thread_invalid")?;
    Uuid::parse_str(native)?;
    let text = request["text"].as_str().context("queue_input_invalid")?;
    ensure!(
        !text.trim().is_empty() && text.len() <= 32000,
        "queue_input_invalid"
    );
    let expected = config
        .verified_version
        .as_deref()
        .filter(|s| !s.is_empty())
        .context("queue_version_unverified")?;
    ensure!(
        request["expected_version"] == expected,
        "queue_version_unverified"
    );
    ensure!(
        run(vec!["--version".into()], Duration::from_secs(4), 512)
            .await?
            .trim()
            == expected,
        "queue_version_unverified"
    );
    let h = health::read(database)?;
    ensure!(
        h["overflow"] == false
            && !h["failures"]
                .as_object()
                .context("health_failures_invalid")?
                .values()
                .any(|v| v["thread_id"] == native),
        "target_capture_unconfirmed"
    );
    let db = readonly(database)?;
    ensure!(
        db.query_row(
            "SELECT 1 FROM captured_replies WHERE thread_id=? LIMIT 1",
            [native],
            |r| r.get::<_, i64>(0)
        )
        .optional()?
        .is_some(),
        "queue_target_not_captured"
    );
    // One invocation only. A lost acknowledgement is never retried here.
    let output = run(
        vec![
            "queue".into(),
            "--thread".into(),
            native.into(),
            format!("--message={text}"),
        ],
        Duration::from_secs(10),
        4096,
    )
    .await?;
    Ok(json!({"schema":1,"stdout":output}))
}
pub fn reset(config: &RemoteConfig, database: &Path, op: &str, generation: &str) -> Result<Value> {
    Uuid::parse_str(generation)?;
    let index = config.codex_home.join("state_5.sqlite");
    match op {
        "baseline" => {
            let mut p = collection::baseline(&index, generation)?;
            p["schema"] = json!(1);
            Ok(p)
        }
        "policy" | "finalize" => {
            let mut p = collection::baseline(&index, generation)?;
            p["enabled"] = json!(op == "finalize");
            collection::write_policy(database, &p)?;
            let stored =
                collection::read_policy(database)?.context("collection_policy_required")?;
            Ok(
                json!({"schema":1,"generation":stored["generation"],"excluded":p["excluded"].as_array().context("policy_excluded")?.len()}),
            )
        }
        "clear" => {
            let p = collection::read_policy(database)?.context("collection_policy_required")?;
            ensure!(
                p["generation"] == generation,
                "collection_generation_mismatch"
            );
            initialize(database)?;
            let root = database
                .parent()
                .context("database_parent")?
                .canonicalize()?;
            let mut replicas = vec![database.to_owned()];
            let mut indexes = Vec::new();
            for entry in fs::read_dir(&root)? {
                let e = entry?;
                if e.file_name()
                    .to_string_lossy()
                    .starts_with("health-recovery-")
                    && e.file_type()?.is_dir()
                {
                    let dir = e.path();
                    let r = dir.join("replies.sqlite");
                    if r.exists() {
                        replicas.push(r)
                    }
                    let i = dir.join("index.sqlite");
                    if i.exists() {
                        indexes.push(i)
                    }
                }
            }
            for path in &replicas {
                ensure!(
                    !fs::symlink_metadata(path)?.file_type().is_symlink()
                        && path.canonicalize()?.starts_with(&root),
                    "replica_path_refused"
                );
                collection::erase_replica(path, generation)?;
            }
            let poll = root.join("poll.json");
            if poll.exists() {
                fs::remove_file(poll)?;
            }
            for path in indexes {
                ensure!(
                    !fs::symlink_metadata(&path)?.file_type().is_symlink()
                        && path.canonicalize()?.starts_with(&root),
                    "index_path_refused"
                );
                for suffix in ["", "-wal", "-shm"] {
                    let p = PathBuf::from(format!("{}{suffix}", path.display()));
                    if p.exists() {
                        fs::remove_file(p)?;
                    }
                }
            }
            initialize(database)?;
            Ok(json!({"schema":1,"cleared":true,"generation":generation,"replicas":replicas.len()}))
        }
        _ => bail!("rpc_operation_invalid"),
    }
}
pub async fn handle(config: &RemoteConfig, database: &Path, request: &Value) -> Result<Value> {
    match request["op"].as_str() {
        Some("snapshot") => snapshot(config, database, request).await,
        Some("version") => Ok(json!({"schema":1,"stdout":format!("{}\n",version(config).await?)})),
        Some("queue") => dispatch(config, database, request).await,
        Some(op @ ("baseline" | "policy" | "finalize" | "clear")) => reset(
            config,
            database,
            op,
            request["generation"]
                .as_str()
                .context("generation_missing")?,
        ),
        _ => bail!("rpc_operation_invalid"),
    }
}
pub async fn run(args: RemoteArgs) -> Result<()> {
    let raw = bounded_stdin()?;
    let request: Value = serde_json::from_slice(&raw)?;
    let config: RemoteConfig = serde_json::from_slice(&fs::read(&args.config)?)?;
    let database = args
        .config
        .parent()
        .context("config_parent")?
        .join("data/replies.sqlite");
    let result = handle(&config, &database, &request).await?;
    std::io::stdout().write_all(&serde_json::to_vec(&result)?)?;
    Ok(())
}
pub fn bounded_stdin() -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    std::io::stdin()
        .take((REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    ensure!(raw.len() <= REQUEST_BYTES, "request_limit");
    Ok(raw)
}
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    pub struct Fixture {
        pub _temp: tempfile::TempDir,
        pub home: PathBuf,
        pub path: PathBuf,
        pub index: PathBuf,
        pub db: PathBuf,
        pub native: String,
        pub config: RemoteConfig,
        pub count: i64,
    }
    impl Fixture {
        pub fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join(".codex");
            let path = home.join("sessions/session.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let index = home.join("state_5.sqlite");
            let db = temp.path().join("data/replies.sqlite");
            let native = Uuid::new_v4().to_string();
            let c = Connection::open(&index).unwrap();
            c.execute(
                "CREATE TABLE threads(id TEXT,rollout_path TEXT,title TEXT,updated_at INTEGER)",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO threads VALUES(?,?,?,?)",
                params![native, path.to_string_lossy(), "测试对话", 1],
            )
            .unwrap();
            fs::write(
                &path,
                format!(
                    "{}\n",
                    json!({"type":"session_meta","payload":{"id":native}})
                ),
            )
            .unwrap();
            initialize(&db).unwrap();
            let config = RemoteConfig {
                codex: "unused".into(),
                codex_home: home.clone(),
                verified_version: None,
            };
            Self {
                _temp: temp,
                home,
                path,
                index,
                db,
                native,
                config,
                count: 1,
            }
        }
        pub fn append(&mut self, text: &str) -> String {
            let turn = Uuid::new_v4().to_string();
            let stamp = 1700000000 + self.count;
            self.count += 3;
            let user = json!({"type":"response_item","payload":{"type":"message","role":"user","id":Uuid::new_v4().to_string(),"content":[{"type":"input_text","text":text}],"internal_chat_message_metadata_passthrough":{"turn_id":turn,"content_item_kinds":["user.text"],"create_time":stamp}}});
            let commentary = json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"过程不应同步"}]}});
            let completed = json!({"type":"event_msg","timestamp":chrono::DateTime::from_timestamp(stamp,0).unwrap().to_rfc3339(),"payload":{"type":"task_complete","turn_id":turn,"last_agent_message":"**最终回复**"}});
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(&self.path)
                .unwrap();
            for record in [user, commentary, completed] {
                writeln!(f, "{record}").unwrap();
            }
            turn
        }
        pub fn capture(&self) {
            poll(&self.config, &self.db).unwrap()
        }
        pub fn count(&self) -> i64 {
            Connection::open(&self.db)
                .unwrap()
                .query_row("SELECT count(*) FROM captured_replies", [], |r| r.get(0))
                .unwrap()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fixtures::Fixture;
    #[test]
    fn latest_seed_new_turns_preserve_human_order_and_ids() {
        let mut f = Fixture::new();
        f.append("旧用户");
        let latest = f.append("最新用户");
        f.capture();
        assert_eq!(f.count(), 1);
        assert_eq!(
            Connection::open(&f.db)
                .unwrap()
                .query_row("SELECT turn_id FROM captured_replies", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            latest
        );
        let raw = "a; $(shell) `quoted`\n中文";
        let third = f.append(raw);
        f.capture();
        f.capture();
        assert_eq!(f.count(), 2);
        let db = Connection::open(&f.db).unwrap();
        let stored: String = db
            .query_row(
                "SELECT text FROM captured_user_messages WHERE turn_id=?",
                [third],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, raw);
        let body: String = db
            .query_row("SELECT reply FROM captured_replies LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!body.contains("过程"));
    }
    #[test]
    fn session_root_and_identity_checked() {
        let mut f = Fixture::new();
        f.append("human");
        assert!(
            completed_events(&f.path, &Uuid::new_v4().to_string(), &f.home, TAIL_BYTES).is_err()
        );
        let outside = f._temp.path().join("other.jsonl");
        fs::copy(&f.path, &outside).unwrap();
        assert!(completed_events(&outside, &f.native, &f.home, TAIL_BYTES).is_err());
    }
    #[tokio::test]
    async fn readonly_and_version_gate_do_not_spawn_queue() {
        let f = Fixture::new();
        assert!(dispatch(
            &f.config,
            &f.db,
            &json!({"thread":f.native,"text":"hello","expected_version":null})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("version_unverified"));
        let mut config = f.config.clone();
        config.verified_version = Some("one".into());
        assert!(dispatch(
            &config,
            &f.db,
            &json!({"thread":f.native,"text":"hello","expected_version":"two"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("version_unverified"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn verified_queue_checks_capture_and_per_thread_health() {
        let mut f = Fixture::new();
        f.config.codex = "/bin/echo".into();
        f.config.verified_version = Some("unverified-version".into());
        assert!(dispatch(
            &f.config,
            &f.db,
            &json!({"thread":f.native,"text":"hello","expected_version":"unverified-version"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("version_unverified"));
        let verified = version(&f.config).await.unwrap();
        f.config.verified_version = Some(verified.clone());
        assert!(dispatch(
            &f.config,
            &f.db,
            &json!({"thread":f.native,"text":"hello","expected_version":verified})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("not_captured"));
        f.append("hello");
        f.capture();
        let bad = Uuid::new_v4().to_string();
        health::update(&f.db, &bad, "turn", Some("capture_not_confirmed"), None).unwrap();
        assert!(dispatch(
            &f.config,
            &f.db,
            &json!({"thread":bad,"text":"hello","expected_version":verified})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("target_capture_unconfirmed"));
        let accepted = dispatch(
            &f.config,
            &f.db,
            &json!({"thread":f.native,"text":"--config=evil","expected_version":verified}),
        )
        .await
        .unwrap();
        assert!(accepted["stdout"]
            .as_str()
            .unwrap()
            .contains("--message=--config=evil"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn snapshot_changes_only_on_content() {
        let mut f = Fixture::new();
        f.append("human");
        f.config.codex = "/bin/echo".into();
        let first = snapshot(&f.config, &f.db, &json!({})).await.unwrap();
        let second = snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
            .await
            .unwrap();
        assert_eq!(first["changed"], true);
        assert_eq!(second["changed"], false);
        assert!(second.get("database").is_none());
        let raw = STANDARD
            .decode(first["database"].as_str().unwrap())
            .unwrap();
        let mut db = Vec::new();
        flate2::read::ZlibDecoder::new(raw.as_slice())
            .read_to_end(&mut db)
            .unwrap();
        assert!(db.starts_with(b"SQLite format 3"));
    }
    #[test]
    fn tail_gap_does_not_advance_watermark() {
        let mut f = Fixture::new();
        let first = f.append("first");
        f.capture();
        f.append("不能丢的中间轮次");
        let mut file = fs::OpenOptions::new().append(true).open(&f.path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"tool","payload":{"log":"x".repeat(10000)}})
        )
        .unwrap();
        f.append("latest");
        poll_limits(&f.config, &f.db, 4096, TOTAL_SCAN).unwrap();
        let cache: Value =
            serde_json::from_slice(&fs::read(f.db.parent().unwrap().join("poll.json")).unwrap())
                .unwrap();
        assert_eq!(cache[&f.native]["turn_id"], first);
        assert_eq!(f.count(), 1);
        assert!(!health::read(&f.db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn watermark_survives_discovery_window_absence() {
        let mut f = Fixture::new();
        let first = f.append("first");
        f.capture();
        let c = Connection::open(&f.index).unwrap();
        c.execute("DELETE FROM threads", []).unwrap();
        f.capture();
        let cache: Value =
            serde_json::from_slice(&fs::read(f.db.parent().unwrap().join("poll.json")).unwrap())
                .unwrap();
        assert_eq!(cache[&f.native]["turn_id"], first);
        f.append("second");
        f.append("third");
        c.execute(
            "INSERT INTO threads VALUES(?,?,?,1)",
            params![f.native, f.path.to_string_lossy(), "test"],
        )
        .unwrap();
        f.capture();
        assert_eq!(f.count(), 3);
    }
    #[test]
    fn equal_reversed_timestamps_keep_every_turn() {
        let mut f = Fixture::new();
        f.append("first");
        f.capture();
        let second = f.append("second");
        let third = f.append("third");
        let raw = fs::read_to_string(&f.path).unwrap();
        let mut lines = String::new();
        for line in raw.lines() {
            let mut row: Value = serde_json::from_str(line).unwrap();
            if (row["payload"]["turn_id"] == second || row["payload"]["turn_id"] == third)
                && row["type"] == "event_msg"
            {
                row["timestamp"] = json!("2023-11-14T22:13:20Z");
            }
            lines.push_str(&row.to_string());
            lines.push('\n');
        }
        fs::write(&f.path, lines).unwrap();
        f.capture();
        f.capture();
        assert_eq!(f.count(), 3);
    }
    #[test]
    fn impossible_scan_budget_is_visible_and_cools_down() {
        let mut f = Fixture::new();
        f.append("human");
        let size = fs::metadata(&f.path).unwrap().len();
        poll_limits(&f.config, &f.db, size, size + 100).unwrap();
        let first = health::read(&f.db).unwrap();
        assert!(!first["failures"].as_object().unwrap().is_empty());
        poll_limits(&f.config, &f.db, size, size + 100).unwrap();
        assert_eq!(health::read(&f.db).unwrap(), first);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_output_is_bounded_during_execution() {
        let cmd = Command::new("yes");
        let start = std::time::Instant::now();
        assert!(
            bounded_process(cmd, &[], Duration::from_secs(2), 512, 65536)
                .await
                .unwrap_err()
                .to_string()
                .contains("stream_limit")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn stalled_input_obeys_deadline() {
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let start = std::time::Instant::now();
        assert!(bounded_process(
            cmd,
            &vec![b'x'; 1000000],
            Duration::from_millis(150),
            512,
            65536
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("timeout"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn reset_operations_preserve_original_index_and_require_generation() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let generation = Uuid::new_v4().to_string();
        let baseline = reset(&f.config, &f.db, "baseline", &generation).unwrap();
        assert_eq!(baseline["generation"], generation);
        assert_eq!(baseline["excluded"][0], f.native);
        reset(&f.config, &f.db, "policy", &generation).unwrap();
        assert_eq!(
            collection::read_policy(&f.db).unwrap().unwrap()["enabled"],
            false
        );
        assert!(reset(&f.config, &f.db, "clear", &Uuid::new_v4().to_string()).is_err());
        assert_eq!(
            reset(&f.config, &f.db, "clear", &generation).unwrap()["cleared"],
            true
        );
        assert_eq!(f.count(), 0);
        assert_eq!(
            Connection::open(&f.index)
                .unwrap()
                .query_row("SELECT count(*) FROM threads", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        reset(&f.config, &f.db, "finalize", &generation).unwrap();
        assert_eq!(
            collection::read_policy(&f.db).unwrap().unwrap()["enabled"],
            true
        );
    }
    #[tokio::test]
    async fn unknown_rpc_operations_are_rejected_without_process_execution() {
        let f = Fixture::new();
        assert!(
            handle(&f.config, &f.db, &json!({"op":"execute","code":"evil"}))
                .await
                .unwrap_err()
                .to_string()
                .contains("rpc_operation_invalid")
        );
    }
    #[tokio::test]
    async fn lost_queue_acknowledgement_is_never_retried() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        f.config.verified_version = Some("fixture".into());
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = attempts.clone();
        let request =
            json!({"thread":f.native,"text":"--config=evil","expected_version":"fixture"});
        let result =
            dispatch_with_runner(&f.config, &f.db, &request, move |args, timeout, limit| {
                let response = if args == ["--version"] {
                    assert_eq!(limit, 512);
                    Ok("fixture\n".into())
                } else {
                    assert_eq!(timeout, Duration::from_secs(10));
                    assert_eq!(limit, 4096);
                    assert_eq!(args.last().unwrap(), "--message=--config=evil");
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow!("rpc_timeout_after_queue_intent"))
                };
                std::future::ready(response)
            })
            .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
