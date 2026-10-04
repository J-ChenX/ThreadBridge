//! Offline recovery preserves backups and quarantines any source it cannot prove.
//! The fleet poller must be stopped before prepare; finalization rechecks all proofs.
use crate::{
    health, native_input,
    native_remote::{self, RemoteConfig, DB_BYTES, TABLES, TAIL_BYTES, TOTAL_SCAN},
};
use anyhow::{ensure, Context, Result};
use clap::{Args, Subcommand};
use rusqlite::{types::ValueRef, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use uuid::Uuid;
#[derive(Args)]
pub struct RecoveryArgs {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub database: PathBuf,
    #[command(subcommand)]
    pub action: RecoveryAction,
}
#[derive(Subcommand)]
pub enum RecoveryAction {
    Prepare {
        #[arg(long)]
        identifier: Option<String>,
    },
    Batch {
        directory: PathBuf,
    },
    Finalize {
        directory: PathBuf,
    },
}
fn value_rows(db: &Connection, query: &str) -> Result<Vec<Value>> {
    let mut s = db.prepare(query)?;
    let width = s.column_count();
    let rows = s
        .query_map([], |r| {
            let mut values = Vec::new();
            for n in 0..width {
                let value = match r.get_ref(n)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => json!(v),
                    ValueRef::Real(v) => json!(v),
                    ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                    ValueRef::Blob(v) => json!(v),
                };
                values.push(value);
            }
            Ok(json!(values))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
pub fn binding(database: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    let db = native_remote::readonly(database)?;
    db.execute_batch("BEGIN")?;
    let mut used = 0u64;
    for table in TABLES {
        digest.update(table.as_bytes());
        for row in value_rows(&db, &format!("SELECT rowid,* FROM {table} ORDER BY rowid"))? {
            let encoded = serde_json::to_vec(&row)?;
            used += encoded.len() as u64;
            ensure!(used <= 2 * DB_BYTES, "recovery_binding_budget");
            digest.update(&encoded);
            digest.update(b"\n");
        }
    }
    for path in [
        health::path(database),
        database
            .parent()
            .context("database_parent")?
            .join("poll.json"),
    ] {
        digest.update(
            path.file_name()
                .context("binding_filename")?
                .to_string_lossy()
                .as_bytes(),
        );
        if path.exists() {
            let mut input = fs::File::open(path)?;
            let mut buf = [0u8; 1024 * 1024];
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                digest.update(&buf[..n]);
            }
        } else {
            digest.update(b"missing");
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}
pub fn index_signature(index: &Path, targets: &[String]) -> Result<String> {
    let db = native_remote::readonly(index)?;
    db.execute_batch("BEGIN")?;
    let recent = value_rows(
        &db,
        "SELECT id,rollout_path FROM threads ORDER BY updated_at DESC,id DESC LIMIT 200",
    )?;
    let mut paths = Vec::new();
    for native in targets {
        let row = db
            .query_row(
                "SELECT rollout_path FROM threads WHERE id=?",
                [native],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        paths.push(json!([native, row.map(|r| vec![r])]));
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!([recent, paths]))?)
    ))
}
fn cache_file(database: &Path) -> Result<PathBuf> {
    Ok(database
        .parent()
        .context("database_parent")?
        .join("poll.json"))
}
fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn target_list(state: &Value) -> Result<Vec<String>> {
    state["targets"]
        .as_array()
        .context("recovery_targets_invalid")?
        .iter()
        .map(|v| Ok(v.as_str().context("recovery_target_invalid")?.to_owned()))
        .collect()
}
pub fn prepare(config: &RemoteConfig, database: &Path, identifier: &str) -> Result<PathBuf> {
    Uuid::parse_str(identifier)?;
    let index = config.codex_home.join("state_5.sqlite");
    ensure!(
        fs::metadata(database)?.len() <= DB_BYTES && fs::metadata(&index)?.len() <= DB_BYTES,
        "recovery_backup_budget"
    );
    let parent = database.parent().context("database_parent")?;
    ensure!(
        fs2::available_space(parent)? >= 64 * 1024 * 1024 + 2 * DB_BYTES,
        "recovery_free_space_reserve"
    );
    let directory = parent.join(format!("health-recovery-{identifier}"));
    fs::create_dir(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    let cache = read(&cache_file(database)?)?;
    let cache = cache.as_object().context("recovery_cache_invalid")?;
    ensure!(cache.len() < 5000, "recovery_cache_coverage_unconfirmed");
    let status = health::read(database)?;
    let failures = status["failures"]
        .as_object()
        .context("recovery_health_invalid")?;
    let prior: BTreeSet<String> = failures
        .values()
        .filter_map(|v| v["thread_id"].as_str().map(str::to_owned))
        .collect();
    let mut universe: BTreeSet<String> =
        cache.keys().cloned().chain(prior.iter().cloned()).collect();
    native_remote::backup(database, &directory.join("replies.sqlite"))?;
    let db = native_remote::readonly(database)?;
    for table in TABLES {
        let mut s = db.prepare(&format!("SELECT DISTINCT thread_id FROM {table}"))?;
        for row in s.query_map([], |r| r.get::<_, String>(0))? {
            universe.insert(row?);
        }
    }
    native_remote::backup(&index, &directory.join("index.sqlite"))?;
    let db = native_remote::readonly(&directory.join("index.sqlite"))?;
    let mut s = db.prepare("SELECT id FROM threads ORDER BY updated_at DESC,id DESC LIMIT 200")?;
    for row in s.query_map([], |r| r.get::<_, String>(0))? {
        universe.insert(row?);
    }
    ensure!(universe.len() <= 5000, "recovery_target_limit");
    for native in &universe {
        Uuid::parse_str(native)?;
    }
    fs::copy(health::path(database), directory.join("original.health"))?;
    fs::copy(cache_file(database)?, directory.join("poll.json"))?;
    let targets = universe.into_iter().collect::<Vec<_>>();
    let state = json!({"binding":binding(database)?,"targets":targets,"results":{},"proofs":{},"codex_home":config.codex_home,"index_signature":index_signature(&directory.join("index.sqlite"),&targets)?,"prior_failures":prior});
    native_remote::atomic_json(&directory.join("recovery.json"), &state)?;
    Ok(directory)
}
pub fn batch(config: &RemoteConfig, database: &Path, directory: &Path) -> Result<Value> {
    batch_limits(config, database, directory, TOTAL_SCAN)
}
pub fn batch_limits(
    config: &RemoteConfig,
    database: &Path,
    directory: &Path,
    total_scan: u64,
) -> Result<Value> {
    let mut state = read(&directory.join("recovery.json"))?;
    ensure!(
        state["binding"] == binding(database)?,
        "recovery_capture_changed"
    );
    let cache = read(&directory.join("poll.json"))?;
    let index = directory.join("index.sqlite");
    let source = native_remote::readonly(&index)?;
    let db = native_remote::readonly(&directory.join("replies.sqlite"))?;
    let targets = target_list(&state)?;
    let mut budget = total_scan;
    for native in &targets {
        if state["results"].get(native).is_some() {
            continue;
        }
        let mut deferred = false;
        let mut proof = None;
        let result = (|| -> Result<()> {
            ensure!(
                !state["prior_failures"]
                    .as_array()
                    .context("prior_failures_invalid")?
                    .iter()
                    .any(|v| v == native),
                "prior_failure_retained"
            );
            let old = &cache[native];
            ensure!(
                old["error_at"].as_f64().unwrap_or(0.0) == 0.0 && old["turn_id"].is_string(),
                "watermark_unconfirmed"
            );
            let raw = source
                .query_row(
                    "SELECT rollout_path FROM threads WHERE id=?",
                    [native],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .context("source_unavailable")?;
            let path = PathBuf::from(raw);
            let before = native_remote::fingerprint(&path)?;
            ensure!(old["file"] == before, "pending_source_change");
            let mut s = db.prepare(
                "SELECT turn_id,reply FROM captured_replies WHERE thread_id=? ORDER BY rowid",
            )?;
            let rows = s
                .query_map([native], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ensure!(
                !rows.is_empty() && old["turn_id"] == rows.last().unwrap().0,
                "capture_watermark_mismatch"
            );
            let turns: BTreeSet<&str> = rows.iter().map(|(t, _)| t.as_str()).collect();
            for table in &TABLES[1..] {
                let mut s = db.prepare(&format!(
                    "SELECT DISTINCT turn_id FROM {table} WHERE thread_id=?"
                ))?;
                let recorded = s
                    .query_map([native], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<BTreeSet<_>, _>>()?;
                ensure!(
                    recorded.iter().all(|t| turns.contains(t.as_str()))
                        && (*table == "captured_request_ids" || recorded.len() == turns.len()),
                    "partial_turn_records_unconfirmed"
                );
            }
            let size = fs::metadata(&path)?.len();
            let cost = size
                .min(TAIL_BYTES)
                .saturating_add(size.saturating_mul(rows.len() as u64));
            ensure!(cost <= total_scan, "source_revalidation_budget");
            if cost > budget {
                deferred = true;
                return Ok(());
            }
            budget -= cost;
            let (events, _) =
                native_remote::completed_events(&path, native, &config.codex_home, TAIL_BYTES)?;
            ensure!(
                !events.is_empty() && events.last().unwrap().1["turn-id"] == old["turn_id"],
                "completion_chain_unconfirmed"
            );
            let first = events
                .iter()
                .position(|(_, e)| e["turn-id"] == rows[0].0)
                .context("stored_completion_chain_gap")?;
            ensure!(
                events[first..]
                    .iter()
                    .map(|(_, e)| e["turn-id"].as_str().unwrap_or(""))
                    .eq(rows.iter().map(|(t, _)| t.as_str())),
                "stored_completion_chain_gap"
            );
            let completions: BTreeMap<&str, (i64, &str)> = events
                .iter()
                .filter_map(|(t, e)| {
                    Some((
                        e["turn-id"].as_str()?,
                        (*t, e["last-assistant-message"].as_str()?),
                    ))
                })
                .collect();
            for (turn, reply) in &rows {
                let (mut users, stamp) = native_input::read_turn(&index, native, turn)?;
                let mut s=db.prepare("SELECT message_id,text,created_at,input_digest FROM captured_user_messages WHERE thread_id=? AND turn_id=?")?;
                let mut stored = s
                    .query_map([native.as_str(), turn.as_str()], |r| {
                        Ok(native_input::UserMessage {
                            message_id: r.get(0)?,
                            text: r.get(1)?,
                            created_at: r.get(2)?,
                            input_digest: r.get(3)?,
                        })
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let order=db.query_row("SELECT completed_at FROM captured_turn_order WHERE thread_id=? AND turn_id=?",[native.as_str(),turn.as_str()],|r|r.get::<_,i64>(0)).optional()?;
                let sort = |v: &native_input::UserMessage| {
                    (
                        v.message_id.clone(),
                        v.text.clone(),
                        v.created_at,
                        v.input_digest.clone(),
                    )
                };
                users.sort_by_key(sort);
                stored.sort_by_key(sort);
                ensure!(
                    !users.is_empty()
                        && users == stored
                        && order == Some(stamp)
                        && completions.get(turn.as_str()) == Some(&(stamp, reply.as_str())),
                    "stored_turn_unconfirmed"
                );
            }
            ensure!(
                native_remote::fingerprint(&path)? == before,
                "source_changed_during_revalidation"
            );
            proof = Some(json!({"path":path,"resolved":path.canonicalize()?,"fingerprint":before}));
            Ok(())
        })();
        if deferred {
            break;
        }
        let reason = match result {
            Ok(()) => Value::Null,
            Err(error) => json!(error.to_string()),
        };
        state["results"]
            .as_object_mut()
            .context("recovery_results_invalid")?
            .insert(native.clone(), reason);
        if let Some(p) = proof {
            state["proofs"]
                .as_object_mut()
                .context("recovery_proofs_invalid")?
                .insert(native.clone(), p);
        }
    }
    native_remote::atomic_json(&directory.join("recovery.json"), &state)?;
    Ok(json!({"covered":state["results"].as_object().unwrap().len(),"total":targets.len()}))
}
pub fn finalize(database: &Path, directory: &Path) -> Result<Value> {
    let state = read(&directory.join("recovery.json"))?;
    let targets = target_list(&state)?;
    let results = state["results"]
        .as_object()
        .context("recovery_results_invalid")?;
    let actual: BTreeSet<_> = results.keys().cloned().collect();
    ensure!(
        actual == targets.iter().cloned().collect(),
        "recovery_incomplete"
    );
    let blocked = results
        .iter()
        .filter(|(_, reason)| !reason.is_null())
        .map(|(native, _)| native)
        .collect::<Vec<_>>();
    ensure!(blocked.len() <= health::LIMIT, "recovery_failure_capacity");
    let rebuilt = directory.join("rebuilt.sqlite");
    ensure!(
        !health::path(&rebuilt).exists(),
        "recovery_already_finalized"
    );
    health::update(&rebuilt, "initial", "initial", None, None)?;
    for native in &blocked {
        health::update(&rebuilt, native, "recovery", Some("recovery_pending"), None)?;
    }
    let new = health::read(&rebuilt)?;
    ensure!(
        new["overflow"] == false
            && new["failures"]
                .as_object()
                .context("health_failures_invalid")?
                .len()
                == blocked.len(),
        "recovery_status_unconfirmed"
    );
    ensure!(
        state["binding"] == binding(database)?,
        "recovery_capture_changed"
    );
    let home = Path::new(
        state["codex_home"]
            .as_str()
            .context("recovery_home_invalid")?,
    );
    ensure!(
        state["index_signature"] == index_signature(&home.join("state_5.sqlite"), &targets)?,
        "recovery_index_changed"
    );
    for proof in state["proofs"]
        .as_object()
        .context("recovery_proofs_invalid")?
        .values()
    {
        let path = Path::new(proof["path"].as_str().context("proof_path_invalid")?);
        ensure!(
            json!(path.canonicalize()?) == proof["resolved"]
                && native_remote::fingerprint(path)? == proof["fingerprint"],
            "recovery_source_changed"
        );
    }
    crate::native_fleet::replace(&health::path(&rebuilt), &health::path(database))?;
    #[cfg(unix)]
    fs::File::open(database.parent().context("database_parent")?)?.sync_all()?;
    Ok(
        json!({"covered":targets.len(),"blocked":blocked.len(),"healthy":targets.len()-blocked.len(),"overflow":false}),
    )
}
pub async fn run(args: RecoveryArgs) -> Result<()> {
    let config: RemoteConfig = serde_json::from_slice(&fs::read(args.config)?)?;
    let result = match args.action {
        RecoveryAction::Prepare { identifier } => {
            json!({"directory":prepare(&config,&args.database,&identifier.unwrap_or_else(||Uuid::new_v4().to_string()))?})
        }
        RecoveryAction::Batch { directory } => batch(&config, &args.database, &directory)?,
        RecoveryAction::Finalize { directory } => finalize(&args.database, &directory)?,
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use native_remote::fixtures::Fixture;
    fn prepared(f: &Fixture) -> PathBuf {
        prepare(&f.config, &f.db, &Uuid::new_v4().to_string()).unwrap()
    }
    #[test]
    fn overflow_recovery_retains_failures_and_proves_healthy_source() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let bad = Uuid::new_v4().to_string();
        for n in 0..129 {
            health::update(
                &f.db,
                &bad,
                &n.to_string(),
                Some("capture_not_confirmed"),
                None,
            )
            .unwrap();
        }
        assert_eq!(health::read(&f.db).unwrap()["overflow"], true);
        let dir = prepared(&f);
        let original = fs::read(dir.join("original.health")).unwrap();
        assert_eq!(batch(&f.config, &f.db, &dir).unwrap()["covered"], 2);
        let result = finalize(&f.db, &dir).unwrap();
        assert_eq!(result["healthy"], 1);
        assert_eq!(result["blocked"], 1);
        let state = health::read(&f.db).unwrap();
        assert_eq!(state["overflow"], false);
        assert!(state["failures"].get(format!("{bad}:recovery")).is_some());
        assert_eq!(fs::read(dir.join("original.health")).unwrap(), original);
        health::update(&f.db, &bad, "poll", None, None).unwrap();
        assert!(health::read(&f.db).unwrap()["failures"]
            .get(format!("{bad}:recovery"))
            .is_some());
    }
    #[test]
    fn finalization_refuses_incomplete_and_changed_capture() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let dir = prepared(&f);
        assert!(finalize(&f.db, &dir)
            .unwrap_err()
            .to_string()
            .contains("incomplete"));
        batch(&f.config, &f.db, &dir).unwrap();
        health::update(&f.db, &f.native, "new", Some("capture_not_confirmed"), None).unwrap();
        assert!(finalize(&f.db, &dir)
            .unwrap_err()
            .to_string()
            .contains("capture_changed"));
    }
    #[test]
    fn pending_completion_is_quarantined() {
        let mut f = Fixture::new();
        f.append("first");
        f.capture();
        let dir = prepared(&f);
        f.append("新轮次尚未捕获");
        batch(&f.config, &f.db, &dir).unwrap();
        let result = finalize(&f.db, &dir).unwrap();
        assert_eq!(result["healthy"], 0);
        assert!(health::read(&f.db).unwrap()["failures"]
            .get(format!("{}:recovery", f.native))
            .is_some());
    }
    #[test]
    fn impossible_revalidation_budget_stays_quarantined() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let dir = prepared(&f);
        batch_limits(&f.config, &f.db, &dir, 1).unwrap();
        assert_eq!(finalize(&f.db, &dir).unwrap()["healthy"], 0);
    }
    #[test]
    fn finalization_rechecks_sources_and_index() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        let dir = prepared(&f);
        batch(&f.config, &f.db, &dir).unwrap();
        f.append("源核验后新增轮次");
        assert!(finalize(&f.db, &dir)
            .unwrap_err()
            .to_string()
            .contains("source_changed"));
        let other = prepared(&f);
        batch(&f.config, &f.db, &other).unwrap();
        Connection::open(&f.index)
            .unwrap()
            .execute(
                "INSERT INTO threads VALUES(?,?,?,99)",
                rusqlite::params![Uuid::new_v4().to_string(), f.path.to_string_lossy(), "new"],
            )
            .unwrap();
        assert!(finalize(&f.db, &other)
            .unwrap_err()
            .to_string()
            .contains("index_changed"));
    }
    #[test]
    fn partial_turn_records_stay_quarantined() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        Connection::open(&f.db)
            .unwrap()
            .execute(
                "INSERT INTO captured_turn_order VALUES(?,?,?)",
                rusqlite::params![f.native, Uuid::new_v4().to_string(), 1],
            )
            .unwrap();
        let dir = prepared(&f);
        batch(&f.config, &f.db, &dir).unwrap();
        assert_eq!(finalize(&f.db, &dir).unwrap()["healthy"], 0);
    }
    #[test]
    fn missing_human_stays_quarantined_and_poll_cannot_clear_recovery() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        Connection::open(&f.db)
            .unwrap()
            .execute("DELETE FROM captured_user_messages", [])
            .unwrap();
        let dir = prepared(&f);
        batch(&f.config, &f.db, &dir).unwrap();
        assert_eq!(finalize(&f.db, &dir).unwrap()["healthy"], 0);
        let before = health::read(&f.db).unwrap();
        f.capture();
        assert_eq!(health::read(&f.db).unwrap(), before);
    }
    #[test]
    fn bounded_batches_defer_coverage_without_marking_source_healthy() {
        let mut f = Fixture::new();
        f.append("human");
        f.capture();
        f.native = Uuid::new_v4().to_string();
        f.path = f.home.join("sessions/second.jsonl");
        fs::write(
            &f.path,
            format!(
                "{}\n",
                json!({"type":"session_meta","payload":{"id":f.native}})
            ),
        )
        .unwrap();
        Connection::open(&f.index)
            .unwrap()
            .execute(
                "INSERT INTO threads VALUES(?,?,?,1)",
                rusqlite::params![f.native, f.path.to_string_lossy(), "second"],
            )
            .unwrap();
        f.count = 1;
        f.append("human");
        f.capture();
        let dir = prepared(&f);
        let cost = 2 * fs::metadata(&f.path).unwrap().len() + 50;
        assert_eq!(
            batch_limits(&f.config, &f.db, &dir, cost).unwrap()["covered"],
            1
        );
        assert!(finalize(&f.db, &dir)
            .unwrap_err()
            .to_string()
            .contains("incomplete"));
        assert_eq!(
            batch_limits(&f.config, &f.db, &dir, cost).unwrap()["covered"],
            2
        );
        assert_eq!(finalize(&f.db, &dir).unwrap()["healthy"], 2);
    }
}
