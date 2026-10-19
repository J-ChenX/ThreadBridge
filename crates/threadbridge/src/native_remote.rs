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
// Keep a complete maximum-size image record and its preceding completion in
// the polling window, so 16–20 MiB records do not lose the confirmed anchor.
pub const TAIL_BYTES: u64 = 32 * 1024 * 1024;
pub const TOTAL_SCAN: u64 = 128 * 1024 * 1024;
pub const DB_BYTES: u64 = 32 * 1024 * 1024;
pub const REQUEST_BYTES: usize = 256 * 1024;
const MAX_METADATA_LINE: usize = 4 * 1024 * 1024;
const MAX_LINE: usize = crate::native_input::MAX_LINE;
const VISIBLE_HISTORY_VERSION: u64 = 4;
pub const TABLES: [&str; 5] = [
    "captured_replies",
    "captured_user_messages",
    "captured_turn_order",
    "captured_request_ids",
    "captured_projects",
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
#[cfg(any(windows, test))]
fn relocated_windows_codex(configured: &Path) -> Result<PathBuf> {
    let bin = configured
        .parent()
        .and_then(Path::parent)
        .context("codex_install_layout")?;
    let named = |path: &Path, name: &str| {
        path.file_name()
            .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(name))
    };
    ensure!(
        named(configured, "codex.exe")
            && named(bin, "bin")
            && bin.parent().is_some_and(
                |p| named(p, "Codex") && p.parent().is_some_and(|p| named(p, "OpenAI"))
            ),
        "codex_install_layout"
    );
    let mut candidates = Vec::new();
    for (position, entry) in fs::read_dir(bin)?.enumerate() {
        ensure!(position < 32, "codex_install_catalog_limit");
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() == 16
            && name.bytes().all(|b| b.is_ascii_hexdigit())
            && entry.file_type()?.is_dir()
            && entry.path().join("codex.exe").is_file()
        {
            candidates.push(entry.path().join("codex.exe"));
        }
    }
    ensure!(candidates.len() == 1, "codex_install_ambiguous_or_missing");
    Ok(candidates.remove(0))
}
pub fn load_config(path: &Path) -> Result<RemoteConfig> {
    let config: RemoteConfig = serde_json::from_slice(&fs::read(path)?)?;
    #[cfg(windows)]
    let config = {
        let mut config = config;
        if !config.codex.exists() {
            config.codex = relocated_windows_codex(&config.codex)?;
        }
        config
    };
    Ok(config)
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn fingerprint(path: &Path) -> Result<Value> {
    let m = fs::metadata(crate::rollout::resolve(path)?)?;
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
 CREATE TABLE IF NOT EXISTS captured_project_catalog(project_path TEXT PRIMARY KEY);
 CREATE TABLE IF NOT EXISTS captured_project_names(project_path TEXT PRIMARY KEY,project_name TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS captured_projects(thread_id TEXT PRIMARY KEY,project_path TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS captured_replies(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,reply TEXT NOT NULL,utf8_bytes INTEGER NOT NULL,captured_at INTEGER NOT NULL,title TEXT NOT NULL DEFAULT '',PRIMARY KEY(thread_id,turn_id));
 CREATE TABLE IF NOT EXISTS captured_request_ids(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,request_id TEXT NOT NULL,PRIMARY KEY(thread_id,turn_id));
 CREATE TABLE IF NOT EXISTS captured_user_messages(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,input_digest TEXT NOT NULL,PRIMARY KEY(thread_id,message_id));
 CREATE TABLE IF NOT EXISTS captured_visible_messages(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(thread_id,message_id));
 CREATE TABLE IF NOT EXISTS captured_task_state(thread_id TEXT PRIMARY KEY,turn_id TEXT NOT NULL,status TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS captured_turn_order(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,completed_at INTEGER NOT NULL,PRIMARY KEY(thread_id,turn_id));")?;
    crate::questions::initialize(&c)?;
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
#[cfg(test)]
pub fn completed_events(
    path: &Path,
    native: &str,
    home: &Path,
    tail_bytes: u64,
) -> Result<(Vec<(i64, Value)>, u64)> {
    completed_source(path, native, home, tail_bytes, None)
}
pub(crate) fn completed_source(
    path: &Path,
    native: &str,
    home: &Path,
    tail_bytes: u64,
    source: Option<&mut crate::rollout::Source>,
) -> Result<(Vec<(i64, Value)>, u64)> {
    let resolved = crate::rollout::resolve(path)?.canonicalize()?;
    let allowed = ["sessions", "archived_sessions"].into_iter().any(|n| {
        home.join(n)
            .canonicalize()
            .is_ok_and(|r| resolved.starts_with(r))
    });
    ensure!(allowed, "rollout_path_outside_sessions");
    let mut owned;
    let file = match source {
        Some(source) => source,
        None => {
            owned = crate::rollout::Source::open(&resolved, TOTAL_SCAN)?;
            &mut owned
        }
    };
    let size = file.len();
    let cost = file.scan_cost(tail_bytes);
    let mut stream = BufReader::new(file);
    let mut line = Vec::new();
    stream
        .by_ref()
        .take((MAX_METADATA_LINE + 1) as u64)
        .read_until(b'\n', &mut line)?;
    ensure!(line.len() <= MAX_METADATA_LINE, "session_metadata_limit");
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
    Ok((events, cost))
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
        c.prepare("SELECT id,rollout_path FROM threads ORDER BY updated_at DESC LIMIT 200")?;
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    refresh_projects(database, &index)?;
    let titles = crate::thread_titles::read(&index)?;
    let mut total = 0u64;
    for (native, raw) in rows {
        if total >= total_scan {
            break;
        }
        if !collection::allowed(database, Some(&index), &native)? || quarantined.contains(&native) {
            continue;
        }
        let mut file_fingerprint = Value::Null;
        let attempt = (|| -> Result<()> {
            Uuid::parse_str(&native)?;
            let path = Path::new(&raw);
            file_fingerprint = fingerprint(path)?;
            let old = cache.get(&native).cloned().unwrap_or_else(|| json!({}));
            ensure!(
                cache.contains_key(&native) || cache.len() < 5000,
                "poll_cache_budget"
            );
            if old["file"] == file_fingerprint
                && ((!old["error_at"].as_f64().is_some_and(|t| t > 0.0)
                    && old["visible_history_version"] == VISIBLE_HISTORY_VERSION)
                    || now() - old["error_at"].as_f64().unwrap_or(0.0) < 300.0)
            {
                return Ok(());
            }
            let mut source = match crate::rollout::Source::open(path, total_scan - total) {
                Ok(source) => source,
                Err(error) if total > 0 && error.to_string() == "rollout_source_limit" => {
                    total = total_scan;
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            let size = source.len();
            let scan_cost = source.scan_cost(tail);
            if total + scan_cost > total_scan {
                return Ok(());
            }
            // Charge decompression even when parsing or identity checks fail.
            total += scan_cost;
            let (events, _) = completed_source(path, &native, &home, tail, Some(&mut source))?;
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
                    title: titles.get(&native).cloned(),
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
                latest = latest.max(*stamp);
                confirmed = event["turn-id"].as_str().map(str::to_owned);
                processed += 1;
            }
            // Keep the confirmed completion watermark even if history backfill
            // fails or must wait for the next request's remaining scan budget.
            cache.insert(
                native.clone(),
                json!({"file":Value::Null,"completed_at":latest,"turn_id":confirmed}),
            );
            if processed == candidates.len() {
                ensure!(
                    source.is_compressed() || size.min(tail).saturating_add(size) <= total_scan,
                    "poll_scan_budget"
                );
                if source.is_compressed() {
                    // Reuse the already bounded decoded history. Reading it again
                    // from disk would starve histories larger than half the budget.
                    source.seek(SeekFrom::Start(0))?;
                    crate::visible_history::backfill_source(
                        database,
                        &index,
                        &native,
                        size as usize,
                        Some(&mut source),
                    )?;
                    if fingerprint(path)? == file_fingerprint {
                        cache.insert(native.clone(),json!({"file":file_fingerprint,"completed_at":latest,"turn_id":confirmed,"visible_history_version":VISIBLE_HISTORY_VERSION}));
                    }
                } else if total.saturating_add(size) <= total_scan {
                    total += size;
                    crate::visible_history::backfill_bounded(
                        database,
                        &index,
                        &native,
                        size as usize,
                    )?;
                    // A live append during the scan must be revisited next poll.
                    if fingerprint(path)? == file_fingerprint {
                        cache.insert(native.clone(),json!({"file":file_fingerprint,"completed_at":latest,"turn_id":confirmed,"visible_history_version":VISIBLE_HISTORY_VERSION}));
                    }
                }
            }
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
    crate::thread_titles::persist(database, &index, &titles)?;
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
        let write = async move {
            stdin.write_all(input).await?;
            stdin.shutdown().await?;
            // ChildStdin must be dropped to close the pipe and deliver EOF.
            drop(stdin);
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
    command.args(args).env("CODEX_HOME", &config.codex_home);
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

/// Only explicit, consistent remote project membership identifies the desktop owner.
/// Missing/unreadable state is not evidence that previously known ownership vanished.
fn desktop_remote_threads(home: &Path) -> Option<Value> {
    let state = crate::desktop_projects::read_state(home)?;
    let assignments = state["thread-project-assignments"].as_object()?;
    if assignments.len() > 5000 {
        return None;
    }
    let memberships = state["thread-project-membership-host-ids"].as_object()?;
    let mut result = serde_json::Map::new();
    for (native, project) in assignments {
        if Uuid::parse_str(native).is_err() || project["projectKind"] != "remote" {
            continue;
        }
        let Some(host) = project["hostId"].as_str() else {
            continue;
        };
        if host.len() <= 256
            && host.starts_with("remote-ssh-")
            && memberships.get(native).and_then(Value::as_str) == Some(host)
        {
            result.insert(native.clone(), json!(host));
        }
    }
    Some(Value::Object(result))
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
    crate::message_images::initialize(&Connection::open(database)?)?;
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
    // Process-only changes must reach replicas without a new completed turn.
    // Hash canonical rows: deletions/reinsertions can retain count/max(rowid).
    let mut visible_digest = Sha256::new();
    let mut statement = c.prepare("SELECT thread_id,turn_id,message_id,text,created_at FROM captured_visible_messages ORDER BY thread_id,message_id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let record = (
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        );
        visible_digest.update(serde_json::to_vec(&record)?);
    }
    identity.push(json!(format!("{:x}", visible_digest.finalize())));
    let questions = c.prepare("SELECT thread_id,turn_id,message_id,text,created_at FROM captured_questions ORDER BY thread_id,message_id")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    identity.push(json!(questions));
    let projects: Vec<(String, String)> = c
        .prepare("SELECT thread_id,project_path FROM captured_projects ORDER BY thread_id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    identity.push(json!(projects));
    let mut images_digest = Sha256::new();
    let mut images = c.prepare("SELECT thread_id,message_id,source,image_id,mime,bytes FROM captured_assistant_images ORDER BY thread_id,message_id,source")?;
    let mut rows = images.query([])?;
    while let Some(row) = rows.next()? {
        for col in 0..5 {
            images_digest.update(serde_json::to_vec(&row.get::<_, String>(col)?)?);
        }
        let bytes: Vec<u8> = row.get(5)?;
        images_digest.update((bytes.len() as u64).to_le_bytes());
        images_digest.update(bytes);
    }
    identity.push(json!(format!("{:x}", images_digest.finalize())));
    let tasks = c
        .prepare("SELECT thread_id,turn_id,status FROM captured_task_state ORDER BY thread_id")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    identity.push(json!(tasks));
    let catalog = c
        .prepare("SELECT project_path FROM captured_project_catalog ORDER BY project_path")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    identity.push(json!(catalog));
    let names = c
        .prepare(
            "SELECT project_path,project_name FROM captured_project_names ORDER BY project_path",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    identity.push(json!(names));
    let titles = c
        .prepare("SELECT thread_id,turn_id,title FROM captured_replies ORDER BY thread_id,turn_id")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    identity.push(json!(titles));
    let names = c
        .prepare("SELECT thread_id,title FROM captured_thread_titles ORDER BY thread_id")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    identity.push(json!(names));
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
    if let Some(origins) = desktop_remote_threads(&config.codex_home) {
        result["desktop_remote_threads"] = origins;
    }
    if let Some(projects) = crate::desktop_projects::read_state(&config.codex_home)
        .and_then(|state| crate::desktop_projects::from_state(&state))
    {
        result["desktop_projects"] = serde_json::to_value(projects)?;
    }
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
/// Only workspace metadata is read; this does not import historical conversations.
pub fn refresh_projects(database: &Path, index: &Path) -> Result<()> {
    crate::project_metadata::refresh(database, index)
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
    let question_reply = text.trim_start().starts_with(crate::questions::REPLY_START);
    if question_reply {
        poll(config, database)?;
        crate::questions::validate_reply(&readonly(database)?, "captured_questions", native, text)?;
    }
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
    // A validated pending question is an authoritative target even before the
    // conversation's first completion. Ordinary messages keep the existing gate.
    if !question_reply {
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
    }
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
        Some("create") => crate::new_threads::create(config, database, request).await,
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
    let config = load_config(&args.config)?;
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
    fn windows_upgrade_resolution_is_exact_and_never_grants_version_verification() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("OpenAI/Codex/bin");
        let configured = bin.join("0123456789abcdef/codex.exe");
        let installed = bin.join("fedcba9876543210/codex.exe");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(&installed, b"fixture").unwrap();
        assert_eq!(relocated_windows_codex(&configured).unwrap(), installed);
        assert!(
            relocated_windows_codex(&temp.path().join("other/bin/0123456789abcdef/codex.exe"))
                .is_err()
        );
        fs::create_dir_all(configured.parent().unwrap()).unwrap();
        fs::write(&configured, b"fixture").unwrap();
        assert!(relocated_windows_codex(&configured).is_err());
    }
    #[test]
    fn compression_migration_keeps_watermark_and_recovers_both_sides() {
        let mut f = Fixture::new();
        let first = f.append("first human");
        f.capture();
        let second = f.append("second human");
        append_process(&f, &second, "process", "visible process");
        let compressed = crate::collection::suffix(&f.path, ".zst");
        let bytes = fs::read(&f.path).unwrap();
        fs::write(
            &compressed,
            zstd::stream::encode_all(bytes.as_slice(), 1).unwrap(),
        )
        .unwrap();
        fs::remove_file(&f.path).unwrap();
        health::update(
            &f.db,
            &f.native,
            "poll",
            Some("capture_not_confirmed"),
            None,
        )
        .unwrap();
        f.capture();
        f.capture();
        assert_eq!(f.count(), 2);
        assert!(health::read(&f.db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
        assert_eq!(
            crate::native_input::read_turn(&f.index, &f.native, &first)
                .unwrap()
                .0[0]
                .text,
            "first human"
        );
        let db = Connection::open(&f.db).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_user_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT text FROM captured_visible_messages WHERE message_id='process'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "visible process"
        );
        let cache: Value =
            serde_json::from_slice(&fs::read(f.db.parent().unwrap().join("poll.json")).unwrap())
                .unwrap();
        assert_eq!(cache[&f.native]["turn_id"], second);
        assert_eq!(
            fs::read(&compressed).unwrap(),
            zstd::stream::encode_all(bytes.as_slice(), 1).unwrap()
        );
        assert!(!f.path.exists());
    }
    #[test]
    fn compressed_history_obeys_decoded_budget_and_identity() {
        let mut f = Fixture::new();
        f.append("human");
        let bytes = fs::read(&f.path).unwrap();
        let compressed = crate::collection::suffix(&f.path, ".zst");
        fs::write(
            &compressed,
            zstd::stream::encode_all(bytes.as_slice(), 1).unwrap(),
        )
        .unwrap();
        fs::remove_file(&f.path).unwrap();
        poll_limits(&f.config, &f.db, 100, bytes.len() as u64 - 1).unwrap();
        assert_eq!(f.count(), 0);
        assert!(!health::read(&f.db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
        assert!(
            completed_events(&f.path, &Uuid::new_v4().to_string(), &f.home, TAIL_BYTES).is_err()
        );
        let outside = f._temp.path().join("outside.jsonl.zst");
        fs::copy(&compressed, &outside).unwrap();
        assert!(completed_events(&outside, &f.native, &f.home, TAIL_BYTES).is_err());
    }
    #[test]
    fn compressed_backfill_reuses_decoded_bytes_without_starving_next_history() {
        let mut f = Fixture::new();
        let turn = f.append("human");
        f.capture();
        append_process(&f, &turn, "process", "visible process");
        let bytes = fs::read(&f.path).unwrap();
        fs::write(
            crate::collection::suffix(&f.path, ".zst"),
            zstd::stream::encode_all(bytes.as_slice(), 1).unwrap(),
        )
        .unwrap();
        fs::remove_file(&f.path).unwrap();
        poll_limits(&f.config, &f.db, bytes.len() as u64, bytes.len() as u64 + 1).unwrap();
        let cache: Value =
            serde_json::from_slice(&fs::read(f.db.parent().unwrap().join("poll.json")).unwrap())
                .unwrap();
        assert_eq!(cache[&f.native]["file"], fingerprint(&f.path).unwrap());
        let db = Connection::open(&f.db).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_visible_messages", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        assert!(health::read(&f.db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
    }
    fn append_process(f: &Fixture, turn: &str, id: &str, text: &str) {
        let record = json!({"type":"response_item","timestamp":"2023-11-14T22:13:25Z","payload":{"type":"message","role":"assistant","id":id,"phase":"commentary","content":[{"type":"output_text","text":text}],"internal_chat_message_metadata_passthrough":{"turn_id":turn}}});
        let mut file = fs::OpenOptions::new().append(true).open(&f.path).unwrap();
        writeln!(file, "{record}").unwrap();
    }
    #[test]
    fn old_poll_cache_backfills_process_without_a_new_completion() {
        let mut f = Fixture::new();
        let turn = f.append("human");
        append_process(&f, &turn, "process", "visible process");
        f.capture();
        let db = Connection::open(&f.db).unwrap();
        db.execute("DELETE FROM captured_visible_messages", [])
            .unwrap();
        let cache_path = f.db.parent().unwrap().join("poll.json");
        let mut cache: Value = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        cache[&f.native]
            .as_object_mut()
            .unwrap()
            .remove("visible_history_version");
        atomic_json(&cache_path, &cache).unwrap();
        let before = fingerprint(&f.path).unwrap();
        f.capture();
        f.capture();
        assert_eq!(fingerprint(&f.path).unwrap(), before);
        assert_eq!(f.count(), 1);
        assert_eq!(
            db.query_row("SELECT text FROM captured_visible_messages", [], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
            "visible process"
        );
        let cache: Value = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        assert_eq!(cache[&f.native]["turn_id"], turn);
        assert_eq!(
            cache[&f.native]["visible_history_version"],
            VISIBLE_HISTORY_VERSION
        );
    }
    #[test]
    fn process_backfill_obeys_scan_budget_and_preserves_completion_watermark() {
        let mut f = Fixture::new();
        let turn = f.append("human");
        f.capture();
        append_process(&f, &turn, "process", "visible process");
        let size = fs::metadata(&f.path).unwrap().len();
        poll_limits(&f.config, &f.db, size, size + 100).unwrap();
        assert!(!health::read(&f.db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
        let cache: Value =
            serde_json::from_slice(&fs::read(f.db.parent().unwrap().join("poll.json")).unwrap())
                .unwrap();
        assert_eq!(cache[&f.native]["turn_id"], turn);
        assert!(
            Connection::open(&f.db)
                .unwrap()
                .query_row("SELECT count(*) FROM captured_visible_messages", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
                == 0
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn snapshot_tracks_live_process_and_same_count_content_changes() {
        let mut f = Fixture::new();
        f.append("human");
        f.config.codex = "/bin/echo".into();
        let first = snapshot(&f.config, &f.db, &json!({})).await.unwrap();
        append_process(&f, &Uuid::new_v4().to_string(), "live", "live process");
        let live = snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
            .await
            .unwrap();
        assert_eq!(live["changed"], true);
        assert_eq!(f.count(), 1);
        // Capture rows are immutable, but a replica reset can reuse rowids.
        let db = Connection::open(&f.db).unwrap();
        db.execute(
            "UPDATE captured_visible_messages SET text='replaced process'",
            [],
        )
        .unwrap();
        let replaced = snapshot(&f.config, &f.db, &json!({"revision":live["revision"]}))
            .await
            .unwrap();
        assert_eq!(replaced["changed"], true);
        assert_eq!(
            snapshot(&f.config, &f.db, &json!({"revision":replaced["revision"]}))
                .await
                .unwrap()["changed"],
            false
        );
    }
    #[test]
    fn multi_image_record_preserves_capture_and_later_turns() {
        let mut f = Fixture::new();
        f.append("first message");
        f.capture();
        let turn = Uuid::new_v4().to_string();
        let mut content = vec![json!({"type":"input_text","text":"six photos"})];
        let mut kinds = vec!["user.text"];
        for byte in 0..6 {
            let encoded = STANDARD.encode(vec![byte; 2304 * 1024]);
            content.push(json!({"type":"input_image","image_url":format!("data:image/jpeg;base64,{encoded}")}));
            kinds.push("user.image");
        }
        let user = json!({"type":"response_item","payload":{"type":"message","role":"user","id":"six-photos","content":content,"internal_chat_message_metadata_passthrough":{"turn_id":turn,"create_time":1700000004,"content_item_kinds":kinds}}});
        let line = user.to_string();
        assert!(line.len() > 16 * 1024 * 1024);
        let completed = json!({"type":"event_msg","timestamp":"2023-11-14T22:13:24Z","payload":{"type":"task_complete","turn_id":turn,"last_agent_message":"photo reply"}});
        {
            let mut file = fs::OpenOptions::new().append(true).open(&f.path).unwrap();
            writeln!(file, "{line}\n{completed}").unwrap();
        }
        // Recover from a failed poll using the already confirmed first turn.
        health::update(
            &f.db,
            &f.native,
            "poll",
            Some("capture_not_confirmed"),
            None,
        )
        .unwrap();
        f.append("after the photos");
        f.capture();
        f.capture();
        crate::visible_history::backfill(&f.db, &f.index, &f.native).unwrap();
        assert_eq!(f.count(), 3);
        let db = Connection::open(&f.db).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_user_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_images", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            6
        );
        assert!(health::read(&f.db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
    }
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
    fn poll_skips_subagents_before_reading_their_rollout_or_reporting_failure() {
        let f = Fixture::new();
        let c = Connection::open(&f.index).unwrap();
        c.execute_batch("ALTER TABLE threads ADD COLUMN source TEXT")
            .unwrap();
        c.execute(
            "UPDATE threads SET source=?1",
            [r#"{"subagent":{"thread_spawn":{}}}"#],
        )
        .unwrap();
        fs::remove_file(&f.path).unwrap();
        f.capture();
        assert_eq!(f.count(), 0);
        assert!(health::read(&f.db).unwrap()["failures"]
            .as_object()
            .unwrap()
            .is_empty());
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
    #[cfg(unix)]
    #[tokio::test]
    async fn desktop_project_changes_are_sent_even_without_body_changes() {
        let mut f = Fixture::new();
        f.append("human");
        f.config.codex = "/bin/echo".into();
        let first = snapshot(&f.config, &f.db, &json!({})).await.unwrap();
        let alias = "remote-ssh-discovered:server";
        let mut state = json!({
            "remote-projects":[{"id":"new-project","hostId":alias,"remotePath":"/code/screen_control","label":"控屏"}],
            "thread-project-assignments":{&f.native:{"projectKind":"remote","hostId":alias,"projectId":"new-project"}},
            "thread-project-membership-host-ids":{&f.native:alias}
        });
        let path = f.home.join(".codex-global-state.json");
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        let result = snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
            .await
            .unwrap();
        assert_eq!(result["changed"], false);
        assert!(result.get("database").is_none());
        assert_eq!(result["desktop_projects"]["projects"][0]["name"], "控屏");
        assert_eq!(
            result["desktop_projects"]["assignments"][0]["native"],
            f.native
        );
        state["remote-projects"][0]["label"] = json!("改名后");
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        let result = snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
            .await
            .unwrap();
        assert_eq!(result["changed"], false);
        assert_eq!(result["desktop_projects"]["projects"][0]["name"], "改名后");
        fs::write(path, b"{broken").unwrap();
        let result = snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
            .await
            .unwrap();
        assert!(result.get("desktop_projects").is_none());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn question_only_snapshot_and_state_changes_reach_remote_replicas() {
        let mut f = Fixture::new();
        f.config.codex = "/bin/echo".into();
        let turn = Uuid::new_v4().to_string();
        let question = json!({"type":"response_item","timestamp":"2026-10-19T01:00:00Z","payload":{"type":"function_call","name":"request_user_input_async","call_id":"call_remote_question","arguments":json!({"questions":[{"title":"选择网络"}]}).to_string(),"internal_chat_message_metadata_passthrough":{"turn_id":turn}}});
        use std::io::Write;
        writeln!(
            fs::OpenOptions::new().append(true).open(&f.path).unwrap(),
            "{question}"
        )
        .unwrap();
        let first = snapshot(&f.config, &f.db, &json!({})).await.unwrap();
        let db = Connection::open(&f.db).unwrap();
        let (count, body): (i64, String) = db
            .query_row("SELECT count(*),text FROM captured_questions", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(count, 1);
        let q: crate::questions::Question = serde_json::from_str(&body).unwrap();
        assert_eq!(q.status, "pending");
        assert_eq!(
            db.query_row(
                "SELECT title FROM captured_thread_titles WHERE thread_id=?1",
                [&f.native],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "测试对话"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_replies", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
                .await
                .unwrap()["changed"],
            false
        );
        // State alone changes the snapshot, without a new question/message row.
        db.execute("UPDATE captured_questions SET text=json_set(text,'$.status','answered','$.answer','公网')",[]).unwrap();
        let answered = snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
            .await
            .unwrap();
        assert_eq!(answered["changed"], true);
        assert_eq!(
            snapshot(&f.config, &f.db, &json!({"revision":answered["revision"]}))
                .await
                .unwrap()["changed"],
            false
        );
        // Replica purge includes questions while preserving the native record.
        crate::collection::write_policy(&f.db, &json!({"schema":1,"enabled":true,"generation":Uuid::new_v4().to_string(),"cutoff_ms":1,"excluded":[]})).unwrap();
        crate::collection::purge(&f.db, std::slice::from_ref(&f.native)).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_questions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(f.path.exists());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn rename_repairs_existing_turns_and_changes_snapshot_without_new_messages() {
        let mut f = Fixture::new();
        let index = Connection::open(&f.index).unwrap();
        index.execute_batch("ALTER TABLE threads ADD COLUMN name TEXT; UPDATE threads SET title='# Files mentioned by the user:',name='Actual name'").unwrap();
        f.append("human");
        f.capture();
        f.append("second turn");
        f.capture();
        f.config.codex = "/bin/echo".into();
        let first = snapshot(&f.config, &f.db, &json!({})).await.unwrap();
        let capture = Connection::open(&f.db).unwrap();
        assert_eq!(
            capture
                .query_row(
                    "SELECT count(*) FROM captured_replies WHERE title='Actual name'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        let messages: Vec<(String, String)> = capture
            .prepare("SELECT turn_id,reply FROM captured_replies ORDER BY rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let fingerprint_before = fingerprint(&f.path).unwrap();
        capture
            .execute("UPDATE captured_replies SET title='wrong old title'", [])
            .unwrap();
        index
            .execute("UPDATE threads SET name='Renamed conversation'", [])
            .unwrap();
        let renamed = snapshot(&f.config, &f.db, &json!({"revision":first["revision"]}))
            .await
            .unwrap();
        assert_eq!(renamed["changed"], true);
        assert_ne!(renamed["revision"], first["revision"]);
        assert_eq!(
            capture
                .query_row(
                    "SELECT count(*) FROM captured_replies WHERE title='Renamed conversation'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        let after: Vec<(String, String)> = capture
            .prepare("SELECT turn_id,reply FROM captured_replies ORDER BY rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(messages, after);
        assert_eq!(fingerprint_before, fingerprint(&f.path).unwrap());
        let repeated = snapshot(&f.config, &f.db, &json!({"revision":renamed["revision"]}))
            .await
            .unwrap();
        assert_eq!(repeated["changed"], false);
    }

    #[test]
    fn rename_refreshes_old_replicas_outside_the_recent_discovery_window() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let index = Connection::open(&f.index).unwrap();
        index.execute_batch("ALTER TABLE threads ADD COLUMN name TEXT;ALTER TABLE threads ADD COLUMN thread_source TEXT;UPDATE threads SET name='Older renamed thread'").unwrap();
        for _ in 0..201 {
            index.execute("INSERT INTO threads(id,rollout_path,title,updated_at,thread_source) VALUES(?1,'missing rollout','internal',2,'subagent')", [Uuid::new_v4().to_string()]).unwrap();
        }
        f.capture();
        assert_eq!(f.count(), 1);
        assert_eq!(
            Connection::open(&f.db)
                .unwrap()
                .query_row("SELECT title FROM captured_replies", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "Older renamed thread"
        );
    }
    #[test]
    fn poll_keeps_registered_root_instead_of_raw_cwd() {
        let mut f = Fixture::new();
        let c = Connection::open(&f.index).unwrap();
        c.execute_batch("ALTER TABLE threads ADD COLUMN cwd TEXT NOT NULL DEFAULT '/code/repo/sub';CREATE TABLE projects(id TEXT,name TEXT);CREATE TABLE project_roots(project_id TEXT,position INTEGER,path TEXT);INSERT INTO projects VALUES('p','真实名称');INSERT INTO project_roots VALUES('p',0,'/code/repo');").unwrap();
        f.append("done");
        f.capture();
        let db = Connection::open(&f.db).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT project_path FROM captured_projects WHERE thread_id=?1",
                [&f.native],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "/code/repo"
        );
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
    async fn subprocess_receives_eof_before_output_is_collected() {
        for input in [b"".as_slice(), "RPC 请求正文".as_bytes()] {
            let output =
                bounded_process(Command::new("cat"), input, Duration::from_secs(2), 512, 512)
                    .await
                    .unwrap();
            assert_eq!(output, input);
        }
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
    async fn pending_question_can_be_answered_before_first_completion() {
        let mut f = Fixture::new();
        f.config.verified_version = Some("fixture".into());
        let turn = Uuid::new_v4().to_string();
        let call = json!({"type":"response_item","timestamp":"2026-10-19T01:00:00Z","payload":{"type":"function_call","name":"request_user_input_async","call_id":"call_queued_question","arguments":json!({"questions":[{"title":"选择网络"}]}).to_string(),"internal_chat_message_metadata_passthrough":{"turn_id":turn}}});
        writeln!(
            fs::OpenOptions::new().append(true).open(&f.path).unwrap(),
            "{call}"
        )
        .unwrap();
        let id = json!(["request_user_input_async", "call_queued_question", 0]).to_string();
        let text = format!(
            "<send_user_message_question_reply>\n{}\n</send_user_message_question_reply>\n",
            json!([{"questionItemId":id,"question":"选择网络","answer":"公网"}])
        );
        let request = json!({"thread":f.native,"text":text,"expected_version":"fixture"});
        let mut calls = Vec::new();
        dispatch_with_runner(&f.config, &f.db, &request, |args, _, _| {
            calls.push(args.clone());
            std::future::ready(Ok(if args == ["--version"] {
                "fixture\n".into()
            } else {
                "queued fixture".into()
            }))
        })
        .await
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1],
            vec![
                "queue".to_owned(),
                "--thread".into(),
                f.native.clone(),
                format!("--message={text}")
            ]
        );
        let db = Connection::open(&f.db).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_replies", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM captured_questions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
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

#[cfg(test)]
mod desktop_origin_tests {
    use super::*;
    #[test]
    fn desktop_ownership_requires_consistent_explicit_remote_membership() {
        let dir = tempfile::tempdir().unwrap();
        assert!(desktop_remote_threads(dir.path()).is_none());
        let native = Uuid::new_v4().to_string();
        let local = Uuid::new_v4().to_string();
        let stale = Uuid::new_v4().to_string();
        let alias = "remote-ssh-discovered:server";
        let state = json!({"thread-project-assignments":{
            &native:{"projectKind":"remote","hostId":alias},
            &stale:{"projectKind":"remote","hostId":alias},
            &local:{"projectKind":"local","hostId":"local"}
        },"thread-project-membership-host-ids":{&native:alias,&stale:"local",&local:"local"}});
        fs::write(
            dir.path().join(".codex-global-state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        assert_eq!(
            desktop_remote_threads(dir.path()).unwrap(),
            json!({native:alias})
        );
        fs::write(dir.path().join(".codex-global-state.json"), b"{broken").unwrap();
        assert!(desktop_remote_threads(dir.path()).is_none());
    }
}
