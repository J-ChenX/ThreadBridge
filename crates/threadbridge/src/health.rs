//! Compatible, preallocated dual-slot health ledger. Never stores event bodies.
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const SLOT: usize = 65_536;
pub const LIMIT: usize = 128;
pub fn path(database: &Path) -> PathBuf {
    crate::collection::suffix(database, ".health")
}

fn slots(file: &mut File) -> Result<Option<(u64, Value)>> {
    let mut found = None;
    for slot in 0..2 {
        file.seek(SeekFrom::Start((slot * SLOT) as u64))?;
        let mut bytes = vec![0; SLOT];
        let mut read = 0;
        while read < SLOT {
            let n = file.read(&mut bytes[read..])?;
            if n == 0 {
                break;
            }
            read += n;
        }
        let trimmed = bytes[..read].split(|b| *b == 0).next().unwrap_or_default();
        let Ok(row) = serde_json::from_slice::<Value>(trimmed) else {
            continue;
        };
        let (Some(data), Some(checksum), Some(generation)) = (
            row["data"].as_str(),
            row["sha256"].as_str(),
            row["generation"].as_u64(),
        ) else {
            continue;
        };
        if format!("{:x}", Sha256::digest(data.as_bytes())) != checksum {
            continue;
        }
        let Ok(state) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if !state["failures"].is_object() {
            continue;
        }
        if found.as_ref().is_none_or(|(old, _)| generation > *old) {
            found = Some((generation, state));
        }
    }
    Ok(found)
}

pub fn read(database: &Path) -> Result<Value> {
    let mut file = crate::collection::open_file(&path(database), false, false)?;
    slots(&mut file)?
        .map(|(_, state)| state)
        .ok_or_else(|| anyhow::anyhow!("capture_health_corrupt"))
}

pub fn update(
    database: &Path,
    native: &str,
    turn: &str,
    reason: Option<&str>,
    attempt: Option<&str>,
) -> Result<()> {
    let health = path(database);
    let mut file = crate::collection::open_file(&health, true, true)?;
    fs2::FileExt::lock_exclusive(&file)?;
    let size = file.metadata()?.len();
    let (generation, mut state) = if size == 0 {
        fs2::FileExt::allocate(&file, (SLOT * 2) as u64)?;
        file.sync_all()?;
        crate::collection::sync_parent(&health)?;
        (
            0,
            json!({"failures": {}, "overflow": false, "updated_at": 0}),
        )
    } else {
        ensure!(size == (SLOT * 2) as u64, "capture_health_corrupt");
        slots(&mut file)?.ok_or_else(|| anyhow::anyhow!("capture_health_corrupt"))?
    };
    let key = format!("{native}:{turn}");
    let failures = state["failures"].as_object_mut().unwrap();
    match reason {
        None => {
            if failures.get(&key).is_some_and(|previous| {
                attempt.is_none() || previous["attempt"].as_str() == attempt
            }) {
                failures.remove(&key);
            }
        }
        Some(reason) if failures.contains_key(&key) || failures.len() < LIMIT => {
            failures.insert(key, json!({"thread_id": native, "turn_id": turn, "reason": reason, "recorded_at": crate::model::now(), "attempt": attempt}));
        }
        Some(_) => state["overflow"] = json!(true),
    }
    state["updated_at"] = json!(crate::model::now());
    let data = serde_json::to_string(&state)?;
    let next = generation
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("capture_health_full"))?;
    let mut row = serde_json::to_vec(
        &json!({"generation": next, "sha256": format!("{:x}", Sha256::digest(data.as_bytes())), "data": data}),
    )?;
    ensure!(row.len() <= SLOT, "capture_health_full");
    row.resize(SLOT, b' ');
    file.seek(SeekFrom::Start((next % 2) * SLOT as u64))?;
    file.write_all(&row)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_intent_fallback_and_body_absence() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("reply.sqlite");
        update(
            &db,
            "thread",
            "turn",
            Some("capture_not_confirmed"),
            Some("one"),
        )
        .unwrap();
        update(&db, "thread", "turn", None, Some("one")).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(path(&db))
            .unwrap();
        file.write_all(b"corrupt").unwrap();
        file.sync_all().unwrap();
        assert_eq!(
            read(&db).unwrap()["failures"]["thread:turn"]["reason"],
            "capture_not_confirmed"
        );
    }
    #[test]
    fn older_attempt_cannot_clear_newer_failure() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("reply.sqlite");
        update(&db, "t", "u", Some("first"), Some("one")).unwrap();
        update(&db, "t", "u", Some("second"), Some("two")).unwrap();
        update(&db, "t", "u", None, Some("one")).unwrap();
        assert_eq!(read(&db).unwrap()["failures"]["t:u"]["reason"], "second");
    }
    #[test]
    fn overflow_is_sticky() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("reply.sqlite");
        for n in 0..=LIMIT {
            update(&db, "t", &n.to_string(), Some("failure"), None).unwrap();
        }
        assert_eq!(
            read(&db).unwrap()["failures"].as_object().unwrap().len(),
            LIMIT
        );
        update(&db, "t", "0", None, None).unwrap();
        assert_eq!(read(&db).unwrap()["overflow"], true);
    }
    #[test]
    fn corrupt_size_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("reply.sqlite");
        std::fs::write(path(&db), "bad").unwrap();
        assert!(update(&db, "t", "u", None, None).is_err());
    }
}
