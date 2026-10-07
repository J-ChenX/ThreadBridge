//! Read conversation names from native metadata, independently of message activity.
use anyhow::{ensure, Result};
use rusqlite::{params, Connection};
use std::{collections::BTreeMap, path::Path, time::Duration};

fn display_name(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.contains('\0') {
        return None;
    }
    // Keep the existing capture title byte budget without cutting a UTF-8 character.
    // Fleet adds a device prefix of up to 36 bytes to the same 512-byte field.
    let mut end = value.len().min(476);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Some(value[..end].chars().take(150).collect())
}

pub fn read(index: &Path) -> Result<BTreeMap<String, String>> {
    let c = crate::collection::readonly(index)?;
    let columns = c
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let field = |name: &str| {
        if columns.iter().any(|column| column == name) {
            format!("substr({name},1,512)")
        } else {
            "NULL".to_owned()
        }
    };
    let indexed = index
        .parent()
        .map(|home| crate::native_capture::indexed_titles(&home.join("session_index.jsonl")))
        .transpose()
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut result = BTreeMap::new();
    let sql = format!(
        "SELECT id,{},{} FROM threads LIMIT 100001",
        field("name"),
        field("title")
    );
    let mut q = c.prepare(&sql)?;
    for (position, row) in q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .enumerate()
    {
        ensure!(position < 100000, "thread_title_catalog_limit");
        let (native, name, legacy) = row?;
        let title = name
            .as_deref()
            .and_then(display_name)
            .or_else(|| indexed.get(&native).and_then(|name| display_name(name)))
            .or_else(|| {
                // Older schemas only have title. Attachment scaffolding is not a name.
                legacy.as_deref().and_then(|title| {
                    if title
                        .trim_start()
                        .starts_with("# Files mentioned by the user:")
                    {
                        None
                    } else {
                        display_name(title)
                    }
                })
            })
            .unwrap_or_else(|| format!("会话 · {}", native.chars().take(8).collect::<String>()));
        result.insert(native, title);
    }
    Ok(result)
}

/// Repair only existing replicas, including threads outside the recent poll window.
pub fn refresh(database: &Path, index: &Path) -> Result<BTreeMap<String, String>> {
    let titles = read(index)?;
    let _guard = crate::collection::lock(database)?;
    let mut c = Connection::open(database)?;
    c.busy_timeout(Duration::from_secs(3))?;
    let columns = c
        .prepare("PRAGMA table_info(captured_replies)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == "title") {
        return Ok(titles);
    }
    let natives = c
        .prepare("SELECT DISTINCT thread_id FROM captured_replies LIMIT 100001")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(natives.len() <= 100000, "thread_title_catalog_limit");
    let mut selected = Vec::new();
    for native in natives {
        if let Some(title) = titles.get(&native) {
            if crate::collection::allowed(database, Some(index), &native)? {
                selected.push((native, title));
            }
        }
    }
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (native, title) in selected {
        tx.execute(
            "UPDATE captured_replies SET title=?1 WHERE thread_id=?2 AND title<>?1",
            params![title, native],
        )?;
    }
    tx.commit()?;
    Ok(titles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_take_priority_with_legacy_index_and_schema_fallbacks() {
        let temp = tempfile::tempdir().unwrap();
        let index = temp.path().join("state_5.sqlite");
        let c = Connection::open(&index).unwrap();
        c.execute_batch("CREATE TABLE threads(id TEXT,title TEXT,name TEXT);INSERT INTO threads VALUES('a','first input','Actual name'),('b','first input',NULL),('c','Legacy title','  '),('d','# Files mentioned by the user:\nprivate attachment',NULL)").unwrap();
        let jsonl = temp.path().join("session_index.jsonl");
        std::fs::write(
            &jsonl,
            format!(
                "{}\n{}\n{}\n",
                json!({"id":"a","thread_name":"Stale index"}),
                json!({"id":"b","thread_name":"Old index"}),
                json!({"id":"b","thread_name":"Current index"})
            ),
        )
        .unwrap();
        let names = read(&index).unwrap();
        assert_eq!(names["a"], "Actual name");
        assert_eq!(names["b"], "Current index");
        assert_eq!(names["c"], "Legacy title");
        assert_eq!(names["d"], "会话 · d");
        c.execute_batch("ALTER TABLE threads DROP COLUMN name")
            .unwrap();
        assert_eq!(read(&index).unwrap()["a"], "Stale index");
        std::fs::write(jsonl, "invalid metadata").unwrap();
        assert_eq!(read(&index).unwrap()["c"], "Legacy title");
    }

    #[test]
    fn long_names_keep_utf8_and_leave_room_for_fleet_prefixes() {
        let name = display_name(&"🦀".repeat(200)).unwrap();
        assert_eq!(name.len(), 476);
        assert!(format!("{} · {name}", "h".repeat(32)).len() <= 512);
        assert!(display_name(" \n ").is_none());
        assert!(display_name("invalid\0name").is_none());
    }
}
