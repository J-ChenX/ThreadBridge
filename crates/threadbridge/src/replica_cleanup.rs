//! Targeted cleanup of internal agent or archived replicas, never of native Codex.
use crate::{capture, collection, health, model, native_remote, store::Store};
use anyhow::{ensure, Result};
use clap::Args;
use serde_json::json;
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Args)]
pub struct CleanupArgs {
    #[arg(long)]
    pub db: PathBuf,
    #[arg(long)]
    pub capture_db: PathBuf,
    #[arg(long)]
    pub index: PathBuf,
    #[arg(long)]
    pub host: String,
    #[arg(long, requires = "backup_dir")]
    pub apply: bool,
    #[arg(long)]
    pub backup_dir: Option<PathBuf>,
}

pub enum Kind {
    Subagent,
    Archived,
}
pub fn run(args: CleanupArgs, kind: Kind) -> Result<()> {
    ensure!(
        args.db.is_file() && args.capture_db.is_file(),
        "existing_replica_required"
    );
    uuid::Uuid::parse_str(&args.host)?;
    let source = collection::readonly(&args.capture_db)?;
    let hub = collection::readonly(&args.db)?;
    ensure!(
        hub.query_row(
            "SELECT count(*)=1 FROM devices WHERE id=?1 AND role='agent'",
            [&args.host],
            |r| r.get::<_, bool>(0)
        )?,
        "existing_host_required"
    );
    let index = collection::readonly(&args.index)?;
    let mut candidates = source
        .prepare("SELECT DISTINCT thread_id FROM captured_replies")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    for row in hub
        .prepare("SELECT native FROM threads WHERE host=?1")?
        .query_map([&args.host], |r| r.get::<_, String>(0))?
    {
        candidates.insert(row?);
    }
    let status = health::read(&args.capture_db)?;
    for failure in status["failures"].as_object().unwrap().values() {
        if let Some(native) = failure["thread_id"].as_str() {
            candidates.insert(native.into());
        }
    }
    let mut natives = Vec::new();
    for native in candidates {
        let excluded = match kind {
            Kind::Subagent => collection::is_subagent(&index, &native)?,
            Kind::Archived => collection::is_archived(&index, &native)?,
        };
        if excluded {
            natives.push(native);
        }
    }
    // Check the same durable-exclusion prerequisite for planning and apply,
    // before creating a backup directory or modifying either replica.
    if !natives.is_empty() {
        ensure!(
            collection::read_policy(&args.capture_db)?.is_some(),
            "collection_policy_required: configure the capture collection before cleanup"
        );
    }
    let count_key = match kind {
        Kind::Subagent => "subagent_threads",
        Kind::Archived => "archived_threads",
    };
    if !args.apply {
        println!(
            "{}",
            json!({"apply":false,(count_key):natives.len(),"native_ids":natives})
        );
        return Ok(());
    }
    // Require a new private backup directory; a failed backup never permits deletion.
    let backup = args.backup_dir.as_ref().unwrap();
    std::fs::create_dir(backup)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(backup, std::fs::Permissions::from_mode(0o700))?;
    }
    native_remote::backup(&args.db, &backup.join("hub.sqlite"))?;
    native_remote::backup(&args.capture_db, &backup.join("capture.sqlite"))?;
    for suffix in [".collection.json", ".collection-required", ".health"] {
        let path = collection::suffix(&args.capture_db, suffix);
        if path.exists() {
            std::fs::copy(path, backup.join(format!("capture.sqlite{suffix}")))?;
        }
    }
    drop(source);
    drop(hub);
    // Source exclusion is durable before Hub deletion, so notify cannot recreate it.
    collection::purge(&args.capture_db, &natives)?;
    let store = Store::open(&args.db)?;
    store
        .0
        .lock()
        .unwrap()
        .pragma_update(None, "secure_delete", true)?;
    for native in &natives {
        store.delete_copy(&model::key(&args.host, native))?;
    }
    capture::save_health(&store, &args.host, health::read(&args.capture_db)?)?;
    println!(
        "{}",
        json!({"apply":true,(count_key):natives.len(),"backup_dir":backup,"native_codex_unchanged":true})
    );
    Ok(())
}
