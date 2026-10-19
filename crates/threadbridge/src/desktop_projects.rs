//! Desktop remote project metadata is independent of the executor's local projects.
use crate::store::Store;
use anyhow::{ensure, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Read, path::Path};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Project {
    pub host: String,
    pub path: String,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Assignment {
    pub host: String,
    pub native: String,
    pub path: String,
}
#[derive(Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Metadata {
    pub projects: Vec<Project>,
    pub assignments: Vec<Assignment>,
}

pub fn read_state(home: &Path) -> Option<Value> {
    let mut bytes = Vec::new();
    fs::File::open(home.join(".codex-global-state.json"))
        .ok()?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 4 * 1024 * 1024)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
}
fn valid(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.contains(['\0', '\r', '\n'])
}
pub fn from_state(state: &Value) -> Option<Metadata> {
    let projects = state["remote-projects"].as_array()?;
    let assignments = state["thread-project-assignments"].as_object()?;
    let memberships = state["thread-project-membership-host-ids"].as_object()?;
    if projects.len() > 1000 || assignments.len() > 5000 {
        return None;
    }
    let mut catalog = BTreeMap::new();
    for p in projects {
        let (id, host, path, name) = (
            p["id"].as_str()?,
            p["hostId"].as_str()?,
            p["remotePath"].as_str()?,
            p["label"].as_str()?,
        );
        if !valid(host, 256)
            || !host.starts_with("remote-ssh-")
            || !valid(path, 2048)
            || !valid(name, 512)
        {
            return None;
        }
        // Never collapse identical legacy project IDs belonging to different hosts.
        if catalog
            .insert(
                (host, id),
                Project {
                    host: host.into(),
                    path: path.into(),
                    name: name.into(),
                },
            )
            .is_some()
        {
            return None;
        }
    }
    let mut result = Metadata::default();
    for (native, a) in assignments {
        let (Some(host), Some(project)) = (a["hostId"].as_str(), a["projectId"].as_str()) else {
            continue;
        };
        if Uuid::parse_str(native).is_err()
            || a["projectKind"] != "remote"
            || memberships.get(native).and_then(Value::as_str) != Some(host)
        {
            continue;
        }
        if let Some(project) = catalog.get(&(host, project)) {
            result.assignments.push(Assignment {
                host: host.into(),
                native: native.clone(),
                path: project.path.clone(),
            });
        }
    }
    result.projects = catalog.into_values().collect();
    Some(result)
}

/// Resolve aliases only through the operator's existing fleet mapping.
pub fn resolve(mut metadata: Metadata, fleet: &Value, owner: &Value) -> Result<Metadata> {
    ensure!(
        metadata.projects.len() <= 1000 && metadata.assignments.len() <= 5000,
        "desktop_project_limit"
    );
    let target = |alias: &str| -> Option<String> {
        let name = owner["desktop_remote_hosts"][alias].as_str()?;
        fleet["hosts"][name]["host_id"].as_str().map(str::to_owned)
    };
    metadata.projects = metadata
        .projects
        .into_iter()
        .filter_map(|mut p| {
            p.host = target(&p.host)?;
            Some(p)
        })
        .collect();
    metadata.assignments = metadata
        .assignments
        .into_iter()
        .filter_map(|mut a| {
            a.host = target(&a.host)?;
            Some(a)
        })
        .collect();
    Ok(metadata)
}

pub fn replace(store: &Store, owner: &str, mut data: Metadata) -> Result<()> {
    Uuid::parse_str(owner)?;
    ensure!(
        data.projects.len() <= 1000 && data.assignments.len() <= 5000,
        "desktop_project_limit"
    );
    for p in &data.projects {
        Uuid::parse_str(&p.host)?;
        ensure!(
            valid(&p.path, 2048) && valid(&p.name, 512),
            "invalid_desktop_project"
        );
    }
    for a in &data.assignments {
        Uuid::parse_str(&a.host)?;
        Uuid::parse_str(&a.native)?;
        ensure!(
            data.projects
                .iter()
                .filter(|p| p.host == a.host && p.path == a.path)
                .count()
                == 1,
            "invalid_desktop_assignment"
        );
    }
    data.projects.sort();
    data.assignments.sort();
    ensure!(
        data.projects
            .windows(2)
            .all(|p| (&p[0].host, &p[0].path) != (&p[1].host, &p[1].path)),
        "duplicate_desktop_project"
    );
    ensure!(
        data.assignments
            .windows(2)
            .all(|a| (&a[0].host, &a[0].native) != (&a[1].host, &a[1].native)),
        "duplicate_desktop_assignment"
    );
    let mut c = store.0.lock().unwrap();
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let previous = Metadata {
        projects: tx.prepare("SELECT host,path,name FROM desktop_project_catalog WHERE owner=?1 ORDER BY host,path,name")?
            .query_map([owner], |r| Ok(Project {host:r.get(0)?,path:r.get(1)?,name:r.get(2)?}))?
            .collect::<rusqlite::Result<_>>()?,
        assignments: tx.prepare("SELECT host,native,path FROM desktop_project_assignments WHERE owner=?1 ORDER BY host,native,path")?
            .query_map([owner], |r| Ok(Assignment {host:r.get(0)?,native:r.get(1)?,path:r.get(2)?}))?
            .collect::<rusqlite::Result<_>>()?,
    };
    if previous != data {
        tx.execute(
            "DELETE FROM desktop_project_assignments WHERE owner=?1",
            [owner],
        )?;
        tx.execute(
            "DELETE FROM desktop_project_catalog WHERE owner=?1",
            [owner],
        )?;
        for p in data.projects {
            tx.execute(
                "INSERT INTO desktop_project_catalog VALUES(?1,?2,?3,?4)",
                params![owner, p.host, p.path, p.name],
            )?;
        }
        for a in data.assignments {
            tx.execute(
                "INSERT INTO desktop_project_assignments VALUES(?1,?2,?3,?4)",
                params![owner, a.host, a.native, a.path],
            )?;
        }
        tx.execute(
            "INSERT INTO events(kind,thread,created) VALUES('metadata','',?1)",
            [crate::model::now()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remote_project_assignment_reaches_phone_and_survives_local_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("hub")).unwrap();
        let owner = Uuid::new_v4().to_string();
        let host = Uuid::new_v4().to_string();
        let native = Uuid::new_v4().to_string();
        let alias = "remote-ssh-discovered:server";
        let id = crate::model::key(&host, &native);
        let mut state = json!({
            "remote-projects":[{"id":"p","hostId":alias,"remotePath":"/code/screen_control","label":"控屏"}],
            "thread-project-assignments":{&native:{"projectKind":"remote","hostId":alias,"projectId":"p"}},
            "thread-project-membership-host-ids":{&native:alias}
        });
        let fleet = json!({"hosts":{"server":{"host_id":host}}});
        let desktop = json!({"desktop_remote_hosts":{alias:"server"}});
        {
            let c = store.0.lock().unwrap();
            c.execute(
                "INSERT INTO threads VALUES(?1,?2,?3,'原始标题','completed','same',42,0,NULL)",
                params![id, host, native],
            )
            .unwrap();
            c.execute("INSERT INTO thread_projects VALUES(?1,'')", [&id])
                .unwrap();
            c.execute("INSERT INTO thread_message_state VALUES(?1,7,123)", [&id])
                .unwrap();
            c.execute(
                "INSERT INTO messages VALUES(?1,'m','turn','assistant','原始正文','v',123)",
                [&id],
            )
            .unwrap();
        }
        let apply = |state: &Value| {
            let wire = serde_json::to_value(from_state(state).unwrap()).unwrap();
            replace(
                &store,
                &owner,
                resolve(serde_json::from_value(wire).unwrap(), &fleet, &desktop).unwrap(),
            )
            .unwrap();
        };
        let before = store.threads(0).unwrap()["threads"][0].clone();
        apply(&state);
        let row = store.threads(0).unwrap()["threads"][0].clone();
        assert_eq!(row["project"], "/code/screen_control");
        assert_eq!(row["project_name"], "控屏");
        assert_eq!(row["project_known"], true);
        for field in [
            "id",
            "native_id",
            "host_id",
            "revision",
            "updated_at",
            "message_revision",
            "message_activity_at",
        ] {
            assert_eq!(row[field], before[field], "{field}");
        }
        let cursor = store.threads(0).unwrap()["cursor"].clone();
        apply(&state);
        assert_eq!(store.threads(0).unwrap()["cursor"], cursor);
        // Executor refreshes may continue to classify this cwd as projectless.
        store
            .0
            .lock()
            .unwrap()
            .execute("UPDATE thread_projects SET project=''", [])
            .unwrap();
        assert_eq!(
            store.threads(0).unwrap()["threads"][0]["project_name"],
            "控屏"
        );
        state["remote-projects"][0]["label"] = json!("新名称");
        apply(&state);
        assert_eq!(
            store.threads(0).unwrap()["threads"][0]["project_name"],
            "新名称"
        );
        // Unknown SSH aliases cannot inject metadata for an arbitrary execution host.
        assert!(resolve(from_state(&state).unwrap(), &fleet, &json!({}))
            .unwrap()
            .projects
            .is_empty());
        // A second desktop claim is ambiguous: fall back to native classification.
        let other = Uuid::new_v4().to_string();
        replace(
            &store,
            &other,
            resolve(from_state(&state).unwrap(), &fleet, &desktop).unwrap(),
        )
        .unwrap();
        assert_eq!(store.threads(0).unwrap()["threads"][0]["project"], "");
        replace(&store, &other, Metadata::default()).unwrap();
        assert_eq!(
            store.threads(0).unwrap()["threads"][0]["project_name"],
            "新名称"
        );
        // Missing/corrupt desktop state is not an instruction to clear prior metadata.
        assert!(read_state(temp.path()).is_none());
        fs::write(temp.path().join(".codex-global-state.json"), "{broken").unwrap();
        assert!(read_state(temp.path()).is_none());
        assert_eq!(
            store.threads(0).unwrap()["threads"][0]["project_name"],
            "新名称"
        );
        // Explicit deletion or reassignment removes the old overlay.
        state["remote-projects"] = json!([]);
        apply(&state);
        assert_eq!(store.threads(0).unwrap()["threads"][0]["project"], "");
        let c = store.0.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT body FROM messages", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "原始正文"
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn stale_memberships_and_foreign_projects_are_not_assigned() {
        let native = Uuid::new_v4().to_string();
        let alias = "remote-ssh-discovered:one";
        let mut state = json!({
            "remote-projects":[{"id":"p","hostId":alias,"remotePath":"/repo","label":"名称"}],
            "thread-project-assignments":{&native:{"projectKind":"remote","hostId":alias,"projectId":"p"}},
            "thread-project-membership-host-ids":{&native:"local"}
        });
        assert!(from_state(&state).unwrap().assignments.is_empty());
        state["thread-project-membership-host-ids"][&native] = json!(alias);
        state["thread-project-assignments"][&native]["hostId"] = json!("remote-ssh-discovered:two");
        assert!(from_state(&state).unwrap().assignments.is_empty());
        state["remote-projects"][0]["remotePath"] = json!("/bad\npath");
        assert!(from_state(&state).is_none());
    }
}
