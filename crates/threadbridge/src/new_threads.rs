//! Durable first-message creation. Blank drafts stay on the phone until submitted.
use crate::{
    adapter::Adapter,
    model::*,
    native_remote::{self, RemoteConfig},
    store::Store,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSubmit {
    pub request_id: String,
    pub host_id: String,
    pub project: String,
    pub text: String,
    pub created_at: i64,
}
impl Store {
    pub fn submit_new(&self, device: &str, s: &CreateSubmit) -> Result<Value> {
        ensure!(
            uuid::Uuid::parse_str(&s.request_id).is_ok()
                && !s.text.trim().is_empty()
                && s.text.len() <= 32000
                && s.project.len() <= 2048
                && !s.project.contains(['\0', '\r', '\n'])
                && s.created_at >= now() - 120
                && s.created_at <= now() + 30,
            "invalid_request"
        );
        let digest = hash(&serde_json::to_string(s)?);
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        if let Some((id, old)) = tx
            .query_row(
                "SELECT id,digest FROM commands WHERE device=?1 AND request=?2",
                params![device, s.request_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            ensure!(old == digest, "idempotency_conflict");
            drop(tx);
            drop(c);
            return self.command(device, &id);
        }
        ensure!(
            tx.query_row("SELECT v='on' FROM settings WHERE k='writes'", [], |r| r
                .get::<_, bool>(
                0
            ))?,
            "restore_read_only"
        );
        ensure!(tx.query_row("SELECT count(*)>0 FROM devices d JOIN creation_hosts ch ON ch.host=d.id WHERE d.id=?1 AND d.role='agent' AND d.revoked=0 AND d.expires>?2 AND d.last_seen>?3 AND ch.last_seen>?3",params![s.host_id,now(),now()-15],|r|r.get::<_,bool>(0))?,"host_offline");
        ensure!(s.project.is_empty()||tx.query_row("SELECT EXISTS(SELECT 1 FROM thread_projects p JOIN threads t ON t.id=p.thread WHERE t.host=?1 AND p.project=?2) OR EXISTS(SELECT 1 FROM host_projects WHERE host=?1 AND project=?2)",params![s.host_id,s.project],|r|r.get::<_,bool>(0))?,"invalid_request");
        ensure!(tx.query_row("SELECT count(*) FROM commands WHERE status IN ('accepted','dispatching','unknown','upstream_queued')",[],|r|r.get::<_,i64>(0))?<128,"queue_full");
        let cmd = Command {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id: key(&s.host_id, &s.request_id),
            native_id: s.request_id.clone(),
            text: s.text.clone(),
            expected_revision: String::new(),
            kind: "create".into(),
            cursor: Some(s.project.clone()),
            expires_at: now() + 60,
        };
        tx.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) VALUES(?1,?2,?3,?4,?5,?6,?7,'accepted',?8,?9)",params![cmd.id,device,s.request_id,digest,s.host_id,cmd.thread_id,serde_json::to_string(&cmd)?,now(),cmd.expires_at])?;
        tx.execute(
            "INSERT INTO events(kind,thread,created) VALUES('command',?1,?2)",
            params![cmd.thread_id, now()],
        )?;
        tx.commit()?;
        drop(c);
        self.command(device, &cmd.id)
    }
    pub fn claim_creation(&self, host: &str) -> Result<Option<Command>> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        let payload:Option<String>=tx.query_row("SELECT payload FROM commands WHERE host=?1 AND status='accepted' AND expires>?2 AND json_extract(payload,'$.kind')='create' ORDER BY created,id LIMIT 1",params![host,now()],|r|r.get(0)).optional()?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let cmd: Command = serde_json::from_str(&payload)?;
        tx.execute(
            "UPDATE commands SET status='dispatching' WHERE id=?1",
            [&cmd.id],
        )?;
        tx.execute(
            "INSERT INTO capture_queue_send_times VALUES(?1,?2)",
            params![
                cmd.id,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis() as i64
            ],
        )?;
        // Commit before calling upstream; crash recovery never retries an uncertain creation.
        tx.execute(
            "INSERT INTO capture_queue_ledger VALUES(?1,'intent',NULL)",
            [&cmd.id],
        )?;
        tx.commit()?;
        Ok(Some(cmd))
    }
}

/// Executes a single creation with inherited desktop permissions. No model turn is
/// dispatched before the new native ID is saved in the local replica ledger.
pub async fn create(config: &RemoteConfig, database: &Path, request: &Value) -> Result<Value> {
    let id = request["request_id"].as_str().context("invalid_request")?;
    uuid::Uuid::parse_str(id)?;
    let project = request["project"].as_str().context("invalid_request")?;
    let text = request["text"].as_str().context("invalid_request")?;
    ensure!(
        !text.trim().is_empty()
            && text.len() <= 32000
            && project.len() <= 2048
            && !project.contains(['\0', '\r', '\n']),
        "invalid_request"
    );
    let version = config
        .verified_version
        .as_deref()
        .context("queue_version_unverified")?;
    ensure!(
        request["expected_version"] == version && native_remote::version(config).await? == version,
        "queue_version_unverified"
    );
    let mut db = rusqlite::Connection::open(database)?;
    db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS phone_creations(request TEXT PRIMARY KEY,digest TEXT NOT NULL,native TEXT,status TEXT NOT NULL)")?;
    let digest = hash(&serde_json::to_string(request)?);
    if let Some((old, native, status)) = db
        .query_row(
            "SELECT digest,native,status FROM phone_creations WHERE request=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    {
        ensure!(digest == old, "idempotency_conflict");
        return Ok(
            json!({"native_id":native,"status":if status=="upstream_queued"{status}else{"unknown".into()}}),
        );
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS captured_project_catalog(project_path TEXT PRIMARY KEY)",
    )?;
    if !project.is_empty() {
        ensure!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM captured_projects WHERE project_path=?1) OR EXISTS(SELECT 1 FROM captured_project_catalog WHERE project_path=?1)",
                [project],
                |r| r.get::<_, bool>(0)
            )?,
            "invalid_project"
        );
        ensure!(
            Path::new(project).is_absolute() && Path::new(project).is_dir(),
            "project_unavailable"
        );
    }
    let health = crate::health::read(database)?;
    ensure!(health["overflow"] == false, "capture_unavailable");
    let mut adapter =
        Adapter::connect_with_home(&config.codex, None, Some(version), Some(&config.codex_home))
            .await?;
    db.execute(
        "INSERT INTO phone_creations VALUES(?1,?2,NULL,'intent')",
        params![id, digest],
    )?;
    let response = adapter
        .rpc(
            "thread/start",
            if project.is_empty() {
                json!({})
            } else {
                json!({"cwd":project})
            },
        )
        .await?;
    let native = response["thread"]["id"]
        .as_str()
        .context("creation_identity_missing")?;
    uuid::Uuid::parse_str(native)?;
    db.execute(
        "UPDATE phone_creations SET native=?2 WHERE request=?1",
        params![id, native],
    )?;
    let output = native_remote::codex_output(
        config,
        &[
            "queue".into(),
            "--thread".into(),
            native.into(),
            format!("--message={text}"),
        ],
        Duration::from_secs(10),
        4096,
    )
    .await;
    // Once native identity is durable, an uncertain queue must still project it to the phone.
    let queued = output
        .as_deref()
        .unwrap_or("")
        .trim()
        .strip_prefix("Queued message ")
        .and_then(|s| s.strip_suffix(&format!(" for thread {native}.")))
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .is_some();
    let status = if queued { "upstream_queued" } else { "unknown" };
    let tx = db.transaction()?;
    tx.execute(
        "UPDATE phone_creations SET status=?2 WHERE request=?1",
        params![id, status],
    )?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS captured_projects(thread_id TEXT PRIMARY KEY,project_path TEXT NOT NULL)")?;
    tx.execute(
        "INSERT OR REPLACE INTO captured_projects VALUES(?1,?2)",
        params![native, project],
    )?;
    tx.commit()?;
    Ok(json!({"native_id":native,"status":status}))
}

pub async fn dispatch(
    db: &Store,
    source: &Path,
    codex: &Path,
    host: &str,
    verified: &str,
    cmd: &Command,
) -> Result<()> {
    let request = json!({"op":"create","request_id":cmd.id,"project":cmd.cursor.as_deref().unwrap_or(""),"text":cmd.text,"expected_version":verified});
    let result = if codex.file_name().is_some_and(|n| n == "codex-proxy") {
        let mut process = tokio::process::Command::new(codex);
        process.args(["threadbridge-create", &serde_json::to_string(&request)?]);
        let output =
            native_remote::bounded_process(process, &[], Duration::from_secs(40), 4096, 4096)
                .await?;
        serde_json::from_slice::<Value>(&output)?
    } else {
        let home = std::env::var_os("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".codex"))
            })
            .context("codex_home_missing")?;
        create(
            &RemoteConfig {
                codex: codex.into(),
                codex_home: home,
                verified_version: Some(verified.into()),
            },
            source,
            &request,
        )
        .await?
    };
    let native = result["native_id"]
        .as_str()
        .context("creation_unconfirmed")?;
    uuid::Uuid::parse_str(native)?;
    let id = key(host, native);
    let thread = Thread {
        id: id.clone(),
        native_id: native.into(),
        host_id: host.into(),
        title: cmd.text.chars().take(80).collect(),
        status: "queue_ready".into(),
        revision: String::new(),
        updated_at: now(),
        can_send: true,
        history_cursor: None,
        project: cmd.cursor.clone().unwrap_or_default(),
    };
    {
        let c = db.0.lock().unwrap();
        c.execute("INSERT OR IGNORE INTO capture_targets(thread,host,native,queue_enabled,last_seen) VALUES(?1,?2,?3,1,?4)",params![id,host,native,now()])?;
    }
    db.capture_snapshot(
        host,
        &Snapshot {
            thread,
            messages: vec![],
            initial: true,
            history: false,
        },
        "",
    )?;
    {
        let c = db.0.lock().unwrap();
        c.execute(
            "INSERT OR REPLACE INTO creation_results VALUES(?1,?2)",
            params![cmd.id, id],
        )?;
        c.execute(
            "UPDATE commands SET thread=?2 WHERE id=?1",
            params![cmd.id, id],
        )?;
        c.execute(
            "UPDATE capture_queue_ledger SET status=?2 WHERE id=?1",
            params![cmd.id, result["status"].as_str().unwrap_or("unknown")],
        )?;
    }
    db.receipt(
        host,
        &cmd.id,
        result["status"].as_str().unwrap_or("unknown"),
        None,
        None,
    )?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    const NATIVE: &str = "00000000-0000-4000-8000-000000000001";
    fn fixture(
        reply: bool,
        lose_start: bool,
    ) -> (tempfile::TempDir, RemoteConfig, std::path::PathBuf, Value) {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex");
        symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/mock_codex.py"),
            &codex,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("fixture.json"),
            serde_json::to_vec(
                &json!({"mode":"create","native_id":NATIVE,"reply":reply,"lose_start":lose_start}),
            )
            .unwrap(),
        )
        .unwrap();
        let config = RemoteConfig {
            codex,
            codex_home: dir.path().into(),
            verified_version: Some("codex-cli fixture".into()),
        };
        let database = dir.path().join("capture.sqlite");
        native_remote::initialize(&database).unwrap();
        rusqlite::Connection::open(&database)
            .unwrap()
            .execute(
                "INSERT INTO captured_projects VALUES(?1,?2)",
                params![NATIVE, dir.path().to_str().unwrap()],
            )
            .unwrap();
        let request = json!({"op":"create","request_id":uuid::Uuid::new_v4().to_string(),"project":dir.path().to_str().unwrap(),"text":"phone starts here","expected_version":"codex-cli fixture"});
        (dir, config, database, request)
    }
    #[tokio::test]
    async fn creates_once_with_exact_project_and_human_input() {
        let (dir, config, database, request) = fixture(true, false);
        let result = create(&config, &database, &request).await.unwrap();
        assert_eq!(result["native_id"], NATIVE);
        assert_eq!(result["status"], "upstream_queued");
        assert_eq!(create(&config, &database, &request).await.unwrap(), result);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("calls")).unwrap(),
            "start\nqueue\n"
        );
        let mut changed = request.clone();
        changed["text"] = json!("different");
        assert_eq!(
            create(&config, &database, &changed)
                .await
                .unwrap_err()
                .to_string(),
            "idempotency_conflict"
        );
    }
    #[tokio::test]
    async fn lost_creation_or_queue_ack_is_never_retried() {
        for (reply, lose_start) in [(false, false), (true, true)] {
            let (dir, config, database, request) = fixture(reply, lose_start);
            let first = create(&config, &database, &request).await;
            if lose_start {
                assert!(first.is_err())
            } else {
                assert_eq!(first.unwrap()["status"], "unknown")
            }
            assert_eq!(
                create(&config, &database, &request).await.unwrap()["status"],
                "unknown"
            );
            assert_eq!(
                std::fs::read_to_string(dir.path().join("calls")).unwrap(),
                if lose_start {
                    "start\n"
                } else {
                    "start\nqueue\n"
                }
            );
        }
    }
    #[tokio::test]
    async fn queue_execution_error_preserves_native_identity_without_retry() {
        let (dir, config, database, request) = fixture(true, false);
        let mut settings: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("fixture.json")).unwrap())
                .unwrap();
        settings["queue_error"] = json!(true);
        std::fs::write(
            dir.path().join("fixture.json"),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        let result = create(&config, &database, &request).await.unwrap();
        assert_eq!(result["native_id"], NATIVE);
        assert_eq!(result["status"], "unknown");
        assert_eq!(create(&config, &database, &request).await.unwrap(), result);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("calls")).unwrap(),
            "start\nqueue\n"
        );
    }
    #[tokio::test]
    async fn unrecognized_project_or_version_cannot_create() {
        let (dir, config, database, mut request) = fixture(true, false);
        request["project"] = json!("/unknown");
        assert!(create(&config, &database, &request).await.is_err());
        assert!(!dir.path().join("calls").exists());
        request["expected_version"] = json!("wrong");
        assert_eq!(
            create(&config, &database, &request)
                .await
                .unwrap_err()
                .to_string(),
            "queue_version_unverified"
        );
    }
    #[test]
    fn hub_creation_is_idempotent_and_host_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Store::open(&dir.path().join("hub.sqlite")).unwrap();
        let (host, _) = db.credential("agent", "fixture").unwrap();
        let (phone, _) = db.credential("phone", "fixture").unwrap();
        db.heartbeat(&host).unwrap();
        db.0.lock()
            .unwrap()
            .execute(
                "INSERT INTO creation_hosts VALUES(?1,?2)",
                params![host, now()],
            )
            .unwrap();
        let s = CreateSubmit {
            request_id: uuid::Uuid::new_v4().to_string(),
            host_id: host.clone(),
            project: String::new(),
            text: "new human text".into(),
            created_at: now(),
        };
        let command = db.submit_new(&phone, &s).unwrap();
        assert_eq!(db.submit_new(&phone, &s).unwrap(), command);
        assert!(db.claim(&host).unwrap().is_none());
        let claimed = db.claim_creation(&host).unwrap().unwrap();
        assert_eq!(claimed.text, s.text);
        assert!(db.claim_creation(&host).unwrap().is_none());
        let mut bad = s.clone();
        bad.text = "changed".into();
        assert_eq!(
            db.submit_new(&phone, &bad).unwrap_err().to_string(),
            "idempotency_conflict"
        );
        bad.request_id = uuid::Uuid::new_v4().to_string();
        bad.host_id = "other".into();
        assert_eq!(
            db.submit_new(&phone, &bad).unwrap_err().to_string(),
            "host_offline"
        );
        assert!(db
            .command("another-phone", command["id"].as_str().unwrap())
            .is_err());
    }
    #[test]
    fn device_catalog_does_not_require_importing_old_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("state.sqlite");
        let capture = dir.path().join("capture.sqlite");
        let c = rusqlite::Connection::open(&index).unwrap();
        c.execute_batch("CREATE TABLE threads(id TEXT,cwd TEXT)")
            .unwrap();
        c.execute(
            "INSERT INTO threads VALUES('old-excluded',?1)",
            [dir.path().to_str().unwrap()],
        )
        .unwrap();
        native_remote::initialize(&capture).unwrap();
        native_remote::refresh_projects(&capture, &index).unwrap();
        let c = rusqlite::Connection::open(&capture).unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM captured_replies", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row(
                "SELECT project_path FROM captured_project_catalog",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            dir.path().to_str().unwrap()
        );
        let db = Store::open(&dir.path().join("hub.sqlite")).unwrap();
        let (host, _) = db.credential("agent", "fixture").unwrap();
        let (phone, _) = db.credential("phone", "fixture").unwrap();
        db.heartbeat(&host).unwrap();
        db.0.lock()
            .unwrap()
            .execute(
                "INSERT INTO creation_hosts VALUES(?1,?2)",
                params![host, now()],
            )
            .unwrap();
        db.0.lock()
            .unwrap()
            .execute(
                "INSERT INTO host_projects VALUES(?1,?2)",
                params![host, dir.path().to_str().unwrap()],
            )
            .unwrap();
        assert_eq!(
            db.hosts().unwrap()["hosts"][0]["projects"][0],
            dir.path().to_str().unwrap()
        );
        let submit = CreateSubmit {
            request_id: uuid::Uuid::new_v4().to_string(),
            host_id: host,
            project: dir.path().to_str().unwrap().into(),
            text: "first in project".into(),
            created_at: now(),
        };
        assert!(db.submit_new(&phone, &submit).is_ok());
    }
    #[test]
    fn search_matches_chinese_fragments_and_escapes_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        let db = Store::open(&dir.path().join("hub.sqlite")).unwrap();
        let (host, _) = db.credential("agent", "fixture").unwrap();
        for (native, title) in [
            (NATIVE, "优化手机对话界面"),
            ("00000000-0000-4000-8000-000000000002", "Project 100%_Done"),
            ("00000000-0000-4000-8000-000000000003", "Équipe discussion"),
        ] {
            db.snapshot(
                &host,
                &Snapshot {
                    thread: Thread {
                        id: key(&host, native),
                        native_id: native.into(),
                        host_id: host.clone(),
                        title: title.into(),
                        status: "idle".into(),
                        revision: String::new(),
                        updated_at: now(),
                        can_send: false,
                        history_cursor: None,
                        project: "/code/fixture".into(),
                    },
                    messages: vec![],
                    initial: true,
                    history: false,
                },
            )
            .unwrap();
        }
        assert_eq!(
            db.search_threads(0, Some("手机界面")).unwrap()["threads"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.search_threads(0, Some("ＰＲＯ done")).unwrap()["threads"][0]["title"],
            "Project 100%_Done"
        );
        assert_eq!(
            db.search_threads(0, Some("équipe")).unwrap()["threads"][0]["title"],
            "Équipe discussion"
        );
        let special = db.search_threads(0, Some("%_")).unwrap();
        assert_eq!(special["threads"][0]["title"], "Project 100%_Done");
        assert_eq!(special["threads"][0]["project"], "/code/fixture");
        assert_eq!(
            db.search_threads(0, Some("PROJECT done")).unwrap()["threads"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}
