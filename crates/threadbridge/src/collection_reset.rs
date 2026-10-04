//! Explicit fleet replica reset. Planning reads sources; apply preserves pairing and ledgers.
use anyhow::{ensure, Context, Result};
use clap::Args;
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Args)]
pub struct ResetArgs {
    #[arg(long, default_value = "local/fleet/fleet.json")]
    pub config: PathBuf,
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    #[arg(long)]
    pub local_index: Option<PathBuf>,
    #[arg(long)]
    pub capture_db: Option<PathBuf>,
    #[arg(long)]
    pub apply: bool,
}
async fn control(action: &str) -> Result<()> {
    let mut command = tokio::process::Command::new("systemctl");
    command.args(["--user", action, "threadbridge.target"]);
    crate::native_remote::bounded_process(command, &[], Duration::from_secs(30), 4096, 4096)
        .await?;
    Ok(())
}
async fn invoke(host: &Value, operation: &str, generation: &str) -> Result<Value> {
    crate::native_fleet::rpc(
        host,
        &json!({"op":operation,"generation":generation}),
        Duration::from_secs(30),
    )
    .await
}
fn identity(path: &Path) -> Result<String> {
    let db = crate::collection::readonly(path)?;
    let mut query =
        db.prepare("SELECT id,token,role,name,expires,revoked FROM devices ORDER BY id")?;
    let rows = query
        .query_map([], |r| {
            Ok(json!([
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?
            ]))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&rows)?)))
}
fn collect_databases(directory: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    ensure!(!directory.is_symlink(), "replica_directory_symlink_refused");
    for row in std::fs::read_dir(directory)? {
        let path = row?.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            collect_databases(&path, paths)?;
        } else if path.extension().is_some_and(|x| x == "sqlite") {
            paths.insert(path);
        }
    }
    Ok(())
}
fn safe_replica(path: &Path, local: &Path) -> Result<()> {
    ensure!(
        !path.is_symlink() && path.canonicalize()?.starts_with(local.canonicalize()?),
        "replica_outside_local_scope"
    );
    Ok(())
}
fn remove_index(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let file = crate::collection::suffix(path, suffix);
        if file.exists() {
            ensure!(!file.is_symlink(), "replica_symlink_refused");
            std::fs::remove_file(file)?;
        }
    }
    Ok(())
}
pub async fn run(args: ResetArgs) -> Result<()> {
    let root = args.root.canonicalize()?;
    let config_path = if args.config.is_absolute() {
        args.config
    } else {
        root.join(args.config)
    };
    let config = crate::native_fleet::load(&config_path)?;
    let hosts = config["hosts"].as_object().context("hosts_invalid")?;
    ensure!(hosts.len() == 4, "reset_requires_four_host_manifest");
    ensure!(
        hosts.values().filter(|h| h["local"] == true).count() == 1,
        "one_local_host_required"
    );
    let source = args
        .capture_db
        .unwrap_or_else(|| root.join("local/notify-capture/replies.sqlite"));
    let index = args
        .local_index
        .or_else(|| {
            std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .map(|p| p.join("state_5.sqlite"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex/state_5.sqlite"))
        })
        .context("local_index_required")?;
    let generation = uuid::Uuid::new_v4().to_string();
    let mut plans = serde_json::Map::new();
    for (name, host) in hosts {
        let plan = if host["local"] == true {
            crate::collection::baseline(&index, &generation)?
        } else {
            invoke(host, "baseline", &generation).await?
        };
        ensure!(
            plan["generation"].as_str() == Some(&generation),
            "baseline_generation_mismatch"
        );
        println!(
            "{name}: baseline checked; existing threads={}",
            plan["excluded"]
                .as_array()
                .context("baseline_excluded_invalid")?
                .len()
        );
        plans.insert(name.clone(), plan);
    }
    if !args.apply {
        println!("Read-only plan; no content or pairing changed.");
        return Ok(());
    }
    let hub = PathBuf::from(config["hub_db"].as_str().context("hub_db_missing")?);
    let local = root.join("local");
    let mut paths = BTreeSet::from([hub.clone(), root.join("local/phone-test/pre-011.sqlite")]);
    collect_databases(&root.join("local/backups"), &mut paths)?;
    for (name, host) in hosts {
        if host["local"] != true {
            collect_databases(&root.join("local/fleet").join(name), &mut paths)?;
        }
    }
    for path in paths
        .iter()
        .filter(|p| p.exists())
        .chain(std::iter::once(&source))
    {
        safe_replica(path, &local)?;
    }
    let preserved = identity(&hub)?;
    control("stop").await?;
    // Any failure after stop intentionally leaves the fleet stopped.
    for host in hosts.values() {
        let result = if host["local"] == true {
            let mut p = crate::collection::baseline(&index, &generation)?;
            p["enabled"] = json!(false);
            crate::collection::write_policy(&source, &p)?;
            json!({"generation":generation})
        } else {
            invoke(host, "policy", &generation).await?
        };
        ensure!(
            result["generation"].as_str() == Some(&generation),
            "policy_generation_mismatch"
        );
    }
    for host in hosts.values().filter(|h| h["local"] != true) {
        ensure!(
            invoke(host, "clear", &generation).await?["cleared"] == true,
            "remote_clear_unconfirmed"
        );
    }
    crate::collection::erase_replica(&source, &generation)?;
    for path in &paths {
        if !path.exists() {
            continue;
        }
        safe_replica(path, &local)?;
        if path.file_name().is_some_and(|n| n == "index.sqlite")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n.to_string_lossy().starts_with("health-recovery-"))
        {
            remove_index(path)?;
        } else {
            crate::collection::erase_replica(path, &generation)?;
        }
    }
    for (name, host) in hosts {
        if host["local"] != true {
            let status = root.join("local/fleet").join(name).join("status.json");
            if status.exists() {
                std::fs::remove_file(status)?;
            }
        }
    }
    ensure!(identity(&hub)? == preserved, "pairing_identity_changed");
    crate::native_remote::initialize(&source)?;
    for host in hosts.values() {
        if host["local"] == true {
            let mut p = crate::collection::baseline(&index, &generation)?;
            p["enabled"] = json!(true);
            crate::collection::write_policy(&source, &p)?;
        } else {
            ensure!(
                invoke(host, "finalize", &generation).await?["generation"].as_str()
                    == Some(&generation),
                "finalize_generation_mismatch"
            );
        }
    }
    let db = Connection::open(&hub)?;
    for table in ["threads", "messages"] {
        ensure!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))?
                == 0,
            "replica_clear_unconfirmed"
        );
    }
    let counts = plans
        .iter()
        .map(|(name, p)| {
            (
                name.clone(),
                json!({"existing_ids":p["excluded"].as_array().map_or(0,Vec::len)}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let report = json!({"generation":generation,"completed_at":crate::model::now(),"scope":"ThreadBridge replicas only; original Codex data and device credentials unchanged","hosts":counts,"pairing_identity_unchanged":true,"threads":0,"messages":0,"deduplication_preserved":true,"physical_phone_cache":"cleared by collection generation on next successful sync"});
    crate::native_fleet::atomic(
        &root.join("artifacts/collection-reset-verification.json"),
        &serde_json::to_vec_pretty(&report)?,
    )?;
    control("start").await?;
    println!("Four-host replica reset complete; pairing retained.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replica_scope_refuses_escape_and_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let local = root.path().join("local");
        std::fs::create_dir(&local).unwrap();
        assert!(safe_replica(outside.path(), &local).is_err());
        let file = local.join("inside.sqlite");
        std::fs::write(&file, "").unwrap();
        assert!(safe_replica(&file, &local).is_ok());
        #[cfg(unix)]
        {
            let link = local.join("link.sqlite");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(safe_replica(&link, &local).is_err());
        }
    }
    #[test]
    fn index_cleanup_removes_only_explicit_sqlite_sidecars() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("index.sqlite");
        for s in ["", "-wal", "-shm"] {
            std::fs::write(crate::collection::suffix(&p, s), "private").unwrap();
        }
        std::fs::write(d.path().join("policy.json"), "keep").unwrap();
        remove_index(&p).unwrap();
        assert!(d.path().join("policy.json").exists());
        assert!(!p.exists());
    }
}
