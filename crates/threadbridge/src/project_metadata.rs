//! Read project identity without importing any conversation body.
use anyhow::{ensure, Result};
use rusqlite::{params, Connection};
use std::{collections::BTreeMap, path::Path};

fn valid(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.contains(['\0', '\r', '\n'])
}
fn path_key(value: &str) -> String {
    let path = value.replace('\\', "/").trim_end_matches('/').to_owned();
    if path.as_bytes().get(1) == Some(&b':') {
        path.to_lowercase()
    } else {
        path
    }
}
fn within(cwd: &str, root: &str) -> bool {
    let (cwd, root) = (path_key(cwd), path_key(root));
    cwd == root
        || cwd
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn git_worktree_repository(path: &str) -> Option<String> {
    let key = path.replace('\\', "/");
    let key = key.strip_prefix("//?/").unwrap_or(&key);
    key.split_once("/.git/worktrees/")
        .map(|(root, _)| root.to_owned())
}
fn worktree_root(cwd: &str) -> Option<String> {
    for ancestor in Path::new(cwd).ancestors().take(32) {
        let file = ancestor.join(".git");
        let Ok(size) = std::fs::metadata(&file) else {
            continue;
        };
        if !size.is_file() || size.len() > 4096 {
            return None;
        }
        let text = std::fs::read_to_string(file).ok()?;
        let git = text.trim().strip_prefix("gitdir: ")?;
        let path = ancestor.join(git).canonicalize().ok()?;
        return git_worktree_repository(path.to_str()?);
    }
    None
}
#[derive(Default)]
pub struct Metadata {
    pub threads: BTreeMap<String, String>,
    pub names: BTreeMap<String, String>,
}
pub fn read(index: &Path) -> Result<Metadata> {
    let c = crate::collection::readonly(index)?;
    let columns = c
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|n| n == "cwd") {
        return Ok(Metadata::default());
    }
    let registered: bool = c.query_row("SELECT count(*)=2 FROM sqlite_master WHERE type='table' AND name IN ('projects','project_roots')", [], |r| r.get(0))?;
    let mut roots = Vec::<(String, String, String)>::new();
    if registered {
        let mut q=c.prepare("SELECT p.id,p.name,r.path FROM projects p JOIN project_roots r ON r.project_id=p.id ORDER BY r.position LIMIT 1001")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let row = row?;
            ensure!(roots.len() < 1000, "project_catalog_limit");
            if valid(&row.1, 512) && valid(&row.2, 2048) && !row.2.is_empty() {
                roots.push(row);
            }
        }
    }
    let mut result = Metadata::default();
    for (_, name, path) in &roots {
        result.names.insert(path.clone(), name.clone());
    }
    let sql = if columns.iter().any(|n| n == "project_id") {
        "SELECT id,cwd,coalesce(project_id,'') FROM threads LIMIT 100001"
    } else {
        "SELECT id,cwd,'' FROM threads LIMIT 100001"
    };
    let mut q = c.prepare(sql)?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (native, cwd, project_id) = row?;
        ensure!(result.threads.len() < 100000, "thread_catalog_limit");
        if !valid(&cwd, 2048) {
            continue;
        }
        let project = if !registered {
            if !cwd.is_empty() {
                result.names.entry(cwd.clone()).or_default();
            }
            cwd
        } else {
            let explicit = roots
                .iter()
                .find(|(id, _, _)| !project_id.is_empty() && *id == project_id);
            let direct = roots
                .iter()
                .filter(|(_, _, root)| within(&cwd, root))
                .max_by_key(|(_, _, root)| root.len());
            let worktree = if explicit.is_none() && direct.is_none() {
                worktree_root(&cwd)
            } else {
                None
            };
            explicit
                .or(direct)
                .or_else(|| {
                    roots.iter().find(|(_, _, root)| {
                        worktree
                            .as_ref()
                            .is_some_and(|p| path_key(p) == path_key(root))
                    })
                })
                .map(|(_, _, root)| root.clone())
                .unwrap_or_default()
        };
        result.threads.insert(native, project);
    }
    Ok(result)
}
pub fn refresh(database: &Path, index: &Path) -> Result<()> {
    let source = crate::collection::readonly(index)?;
    let has_cwd = source
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|c| c == "cwd");
    if !has_cwd {
        return Ok(());
    }
    let data = read(index)?;
    let mut db = Connection::open(database)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = db.transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS captured_projects(thread_id TEXT PRIMARY KEY,project_path TEXT NOT NULL);CREATE TABLE IF NOT EXISTS captured_project_catalog(project_path TEXT PRIMARY KEY);CREATE TABLE IF NOT EXISTS captured_project_names(project_path TEXT PRIMARY KEY,project_name TEXT NOT NULL);DELETE FROM captured_projects;DELETE FROM captured_project_catalog;DELETE FROM captured_project_names;")?;
    for (native, path) in data.threads {
        tx.execute(
            "INSERT OR REPLACE INTO captured_projects VALUES(?1,?2)",
            params![native, path],
        )?;
    }
    for (path, name) in data.names {
        tx.execute("INSERT INTO captured_project_catalog VALUES(?1)", [&path])?;
        tx.execute(
            "INSERT INTO captured_project_names VALUES(?1,?2)",
            params![path, name],
        )?;
    }
    tx.commit()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registered_projects_do_not_classify_unrelated_working_directories() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("index.sqlite");
        let c = Connection::open(&index).unwrap();
        c.execute_batch("CREATE TABLE threads(id TEXT,cwd TEXT,project_id TEXT);CREATE TABLE projects(id TEXT,name TEXT);CREATE TABLE project_roots(project_id TEXT,position INTEGER,path TEXT);INSERT INTO projects VALUES('p','自定义名称');INSERT INTO project_roots VALUES('p',0,'/code/repo');INSERT INTO threads VALUES('a','/code/repo/sub',''),('b','/code/repo-other',''),('c','/random','p');").unwrap();
        let m = read(&index).unwrap();
        assert_eq!(m.threads["a"], "/code/repo");
        assert_eq!(m.threads["b"], "");
        assert_eq!(m.threads["c"], "/code/repo");
        assert_eq!(m.names["/code/repo"], "自定义名称");
    }
    #[test]
    fn worktree_subdirectory_and_windows_path_are_recognized() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let git = repo.join(".git/worktrees/slot");
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::create_dir_all(tree.join("sub/deep")).unwrap();
        std::fs::write(tree.join(".git"), format!("gitdir: {}", git.display())).unwrap();
        assert_eq!(
            worktree_root(tree.join("sub/deep").to_str().unwrap()),
            Some(repo.to_str().unwrap().into())
        );
        assert_eq!(
            git_worktree_repository("\\\\?\\C:\\code\\repo\\.git\\worktrees\\slot"),
            Some("C:/code/repo".into())
        );
    }
    #[test]
    fn refreshing_removes_deleted_project_mappings() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("index");
        let db = dir.path().join("capture");
        let c = Connection::open(&index).unwrap();
        c.execute_batch(
            "CREATE TABLE threads(id TEXT,cwd TEXT);INSERT INTO threads VALUES('old','/old');",
        )
        .unwrap();
        refresh(&db, &index).unwrap();
        c.execute("DELETE FROM threads", []).unwrap();
        refresh(&db, &index).unwrap();
        let c = Connection::open(db).unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM captured_projects", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM captured_project_catalog", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
    }
}
