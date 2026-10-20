//! Explicit one-shot conversion of an acknowledged queued message into guidance.
//! The queue item is removed before steering; uncertain operations are never replayed.
use crate::{
    adapter::Adapter,
    model::*,
    native_remote::{self, RemoteConfig},
    store::Store,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

pub fn submit(db: &Store, device: &str, id: &str) -> Result<Value> {
    let mut c = db.0.lock().unwrap();
    let tx = c.transaction()?;
    let command: String = tx.query_row(
        "SELECT id FROM commands WHERE device=?1 AND (id=?2 OR request=?2)",
        params![device, id],
        |r| r.get(0),
    )?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT status FROM steering_intents WHERE command=?1",
            [&command],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_deref().is_some_and(|s| s != "upstream_queued") {
        drop(tx);
        drop(c);
        return db.command(device, &command);
    }
    if previous.is_some() {
        tx.execute("DELETE FROM steering_intents WHERE command=?1", [&command])?;
    }
    ensure!(
        tx.query_row("SELECT v='on' FROM settings WHERE k='writes'", [], |r| r
            .get::<_, bool>(
            0
        ))?,
        "restore_read_only"
    );
    let turn:Option<String>=tx.query_row("SELECT ts.turn_id FROM commands x JOIN capture_queue_ledger q ON q.id=x.id JOIN thread_tasks ts ON ts.thread=x.thread JOIN devices d ON d.id=x.host JOIN capture_targets ct ON ct.thread=x.thread WHERE x.id=?1 AND x.status='upstream_queued' AND q.status='upstream_queued' AND q.queue_id IS NOT NULL AND json_extract(x.payload,'$.kind') IN ('send','create') AND ts.status IN ('running','active','inProgress') AND ts.turn_id<>'' AND d.revoked=0 AND d.expires>?2 AND d.last_seen>?3 AND ct.queue_enabled=1 AND ct.last_seen>?3",params![command,now(),now()-15],|r|r.get(0)).optional()?;
    let turn = turn.context("steer_not_ready")?;
    let (thread, text): (String, String) = tx.query_row(
        "SELECT thread,json_extract(payload,'$.text') FROM commands WHERE id=?1",
        [&command],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    crate::questions::validate_reply(&tx, "messages", &thread, &text)?;
    tx.execute(
        "INSERT INTO steering_intents VALUES(?1,'accepted',?2,NULL,?3)",
        params![command, turn, now()],
    )?;
    tx.execute(
        "UPDATE commands SET error='steer_in_progress' WHERE id=?1",
        [&command],
    )?;
    tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?2 FROM commands WHERE id=?1",params![command,now()])?;
    tx.commit()?;
    drop(c);
    db.command(device, &command)
}

fn settle(
    db: &Store,
    host: &str,
    id: &str,
    status: &str,
    turn: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    let mut c = db.0.lock().unwrap();
    let tx = c.transaction()?;
    tx.execute(
        "UPDATE steering_intents SET status=?2,error=?3 WHERE command=?1",
        params![id, status, error],
    )?;
    // A captured final may have settled the original command while the RPC ran.
    if tx.execute("UPDATE commands SET status=?3,native_turn=coalesce(?4,native_turn),error=?5 WHERE id=?1 AND host=?2 AND status='upstream_queued'",params![id,host,status,turn,error])?>0 {
  tx.execute("UPDATE capture_queue_ledger SET status=?2 WHERE id=?1",params![id,status])?;
  tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?2 FROM commands WHERE id=?1",params![id,now()])?;
 }
    tx.commit()?;
    Ok(())
}
pub fn recover(db: &Store, host: &str) -> Result<()> {
    let ids = {
        let c = db.0.lock().unwrap();
        let mut q=c.prepare("SELECT s.command FROM steering_intents s JOIN commands x ON x.id=s.command WHERE x.host=?1 AND s.status='dispatching'")?;
        let ids = q
            .query_map([host], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };
    for id in ids {
        settle(
            db,
            host,
            &id,
            "unknown",
            None,
            Some("steer_result_unknown_no_retry"),
        )?;
    }
    Ok(())
}
pub fn expire_unattempted(db: &Store, host: &str) -> Result<()> {
    let ids = {
        let c = db.0.lock().unwrap();
        let mut q=c.prepare("SELECT s.command FROM steering_intents s JOIN commands x ON x.id=s.command WHERE x.host=?1 AND s.status='accepted' AND s.created<?2")?;
        let ids = q
            .query_map(params![host, now() - 60], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };
    for id in ids {
        settle(
            db,
            host,
            &id,
            "upstream_queued",
            None,
            Some("steer_unavailable"),
        )?;
    }
    Ok(())
}
pub async fn dispatch_pending(
    db: &Store,
    source: &Path,
    codex: &Path,
    host: &str,
    verified: &str,
) -> Result<()> {
    let item = {
        let mut c = db.0.lock().unwrap();
        let tx = c.transaction()?;
        let row:Option<(String,String,String,String,String)>=tx.query_row("SELECT x.id,t.native,q.queue_id,json_extract(x.payload,'$.text'),s.turn FROM steering_intents s JOIN commands x ON x.id=s.command JOIN threads t ON t.id=x.thread JOIN capture_queue_ledger q ON q.id=x.id WHERE x.host=?1 AND s.status='accepted' AND x.status='upstream_queued' ORDER BY s.created,s.command LIMIT 1",[host],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        if let Some((id, ..)) = &row {
            tx.execute(
                "UPDATE steering_intents SET status='dispatching' WHERE command=?1",
                [id],
            )?;
        }
        tx.commit()?;
        row
    };
    let Some((id, native, queue, text, turn)) = item else {
        return Ok(());
    };
    let request = json!({"op":"steer","request_id":id,"thread":native,"queue_id":queue,"text":text,"expected_turn":turn,"expected_version":verified});
    let result = async {
        if codex.file_name().is_some_and(|n| n == "codex-proxy") {
            let mut process = tokio::process::Command::new(codex);
            process.args(["threadbridge-steer", &request.to_string()]);
            let bytes =
                native_remote::bounded_process(process, &[], Duration::from_secs(55), 8192, 4096)
                    .await?;
            Ok::<_, anyhow::Error>(serde_json::from_slice::<Value>(&bytes)?)
        } else {
            let home = std::env::var_os("CODEX_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".codex"))
                })
                .context("codex_home_missing")?;
            execute(
                &RemoteConfig {
                    codex: codex.into(),
                    codex_home: home,
                    verified_version: Some(verified.into()),
                },
                source,
                &request,
            )
            .await
        }
    }
    .await;
    match result {
        Ok(r) => settle(
            db,
            host,
            &id,
            r["status"].as_str().unwrap_or("unknown"),
            r["native_turn_id"].as_str(),
            r["error"].as_str(),
        )?,
        Err(_) => settle(
            db,
            host,
            &id,
            "unknown",
            None,
            Some("steer_result_unknown_no_retry"),
        )?,
    }
    Ok(())
}

pub async fn execute(config: &RemoteConfig, database: &Path, request: &Value) -> Result<Value> {
    let id = request["request_id"].as_str().context("invalid_request")?;
    let native = request["thread"].as_str().context("invalid_request")?;
    let queue = request["queue_id"].as_str().context("invalid_request")?;
    let text = request["text"].as_str().context("invalid_request")?;
    let turn = request["expected_turn"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("invalid_request")?;
    for value in [id, native, queue] {
        uuid::Uuid::parse_str(value)?;
    }
    ensure!(
        !text.trim().is_empty() && text.len() <= 32000,
        "invalid_request"
    );
    let version = config
        .verified_version
        .as_deref()
        .context("queue_version_unverified")?;
    if request["expected_version"] != version
        || native_remote::version(config).await.unwrap_or_default() != version
    {
        return Ok(
            json!({"schema":1,"status":"upstream_queued","error":"steer_unavailable","native_turn_id":null}),
        );
    }
    let mut db = rusqlite::Connection::open(database)?;
    db.busy_timeout(Duration::from_secs(3))?;
    db.execute_batch("PRAGMA synchronous=FULL;CREATE TABLE IF NOT EXISTS phone_steers(request TEXT PRIMARY KEY,digest TEXT NOT NULL,status TEXT NOT NULL,turn TEXT,error TEXT)")?;
    let digest = hash(&request.to_string());
    if let Some((old, status, saved_turn, error)) = db
        .query_row(
            "SELECT digest,status,turn,error FROM phone_steers WHERE request=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
    {
        ensure!(old == digest, "idempotency_conflict");
        return Ok(
            json!({"schema":1,"status":if status=="intent"{"unknown"}else{&status},"native_turn_id":saved_turn,"error":error}),
        );
    }
    // No fallback executor: this action belongs to a loaded desktop task.
    let preparation = tokio::time::timeout(Duration::from_secs(20), async {
        let index = config.codex_home.join("state_5.sqlite");
        ensure!(
            crate::collection::allowed(
                database,
                index.is_file().then_some(index.as_path()),
                native
            )?,
            "steer_unavailable"
        );
        if text.trim_start().starts_with(crate::questions::REPLY_START) {
            crate::questions::validate_reply(&db, "captured_questions", native, text)?;
        }
        let mut adapter = Adapter::connect_with_home(
            &config.codex,
            None,
            Some(version),
            Some(&config.codex_home),
        )
        .await?;
        ensure!(adapter.allow_send, "queue_version_unverified");
        let read = adapter
            .rpc(
                "thread/read",
                json!({"threadId":native,"includeTurns":false}),
            )
            .await?;
        ensure!(
            read["thread"]["id"] == native && read["thread"]["status"]["type"] == "active",
            "steer_not_ready"
        );
        let turns = adapter
            .rpc(
                "thread/turns/list",
                json!({"threadId":native,"limit":1,"itemsView":"summary","sortDirection":"desc"}),
            )
            .await?;
        ensure!(
            turns["data"][0]["id"] == turn && turns["data"][0]["status"] == "inProgress",
            "steer_not_ready"
        );
        let mut cursor = Value::Null;
        let mut found = None;
        for _ in 0..10 {
            let page = adapter
                .rpc(
                    "thread/queue/list",
                    json!({"threadId":native,"limit":100,"cursor":cursor}),
                )
                .await?;
            let rows = page["data"].as_array().context("steer_queue_unavailable")?;
            if let Some(item) = rows.iter().find(|v| v["id"] == queue) {
                found = Some(item.clone());
                break;
            }
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        let item = found.context("steer_queue_missing")?;
        let input = item["input"].as_array().context("steer_queue_changed")?;
        ensure!(
            input.len() == 1 && input[0]["type"] == "text" && input[0]["text"] == text,
            "steer_queue_changed"
        );
        ensure!(
            item["clientUserMessageId"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "steer_queue_changed"
        );
        Ok::<_, anyhow::Error>((adapter, item))
    })
    .await
    .unwrap_or_else(|_| Err(anyhow::anyhow!("steer_unavailable")));
    let (mut adapter, item) = match preparation {
        Ok(v) => v,
        Err(e) => {
            let reason = e.to_string();
            let error = if matches!(
                reason.as_str(),
                "steer_not_ready"
                    | "steer_queue_missing"
                    | "steer_queue_changed"
                    | "question_no_longer_pending"
                    | "invalid_question_reply"
            ) {
                reason.as_str()
            } else {
                "steer_unavailable"
            };
            return Ok(
                json!({"schema":1,"status":"upstream_queued","error":error,"native_turn_id":null}),
            );
        }
    };
    db.execute(
        "INSERT INTO phone_steers VALUES(?1,?2,'intent',NULL,NULL)",
        params![id, digest],
    )?;
    // Remove the acknowledged queue item first, preventing a second execution if
    // steering acknowledgement is lost. The immutable phone ledger retains input.
    let removed = adapter
        .rpc(
            "thread/queue/delete",
            json!({"threadId":native,"queuedSubmissionId":queue}),
        )
        .await;
    let (status, result_turn, error) = if removed.as_ref().is_ok_and(|r| r["deleted"] == true) {
        match adapter.rpc("turn/steer",json!({"threadId":native,"expectedTurnId":turn,"clientUserMessageId":item["clientUserMessageId"],"input":item["input"]})).await {
   Ok(r) if r["turnId"]==turn =>("steer_accepted",Some(turn),None),
   _=>("unknown",None,Some("steer_result_unknown_no_retry")),
  }
    } else {
        ("unknown", None, Some("steer_result_unknown_no_retry"))
    };
    let tx = db.transaction()?;
    tx.execute(
        "UPDATE phone_steers SET status=?2,turn=?3,error=?4 WHERE request=?1",
        params![id, status, result_turn, error],
    )?;
    tx.commit()?;
    Ok(json!({"schema":1,"status":status,"native_turn_id":result_turn,"error":error}))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    const NATIVE: &str = "00000000-0000-4000-8000-000000000001";
    const TURN: &str = "00000000-0000-4000-8000-000000000002";
    const QUEUE: &str = "00000000-0000-4000-8000-000000000003";
    fn fixture(mode: Option<&str>) -> (tempfile::TempDir, RemoteConfig, std::path::PathBuf, Value) {
        let d = tempfile::tempdir().unwrap();
        let codex = d.path().join("codex");
        symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/mock_codex.py"),
            &codex,
        )
        .unwrap();
        let mut settings =
            json!({"mode":"steer","native_id":NATIVE,"turn_id":TURN,"queue_id":QUEUE});
        if let Some(mode) = mode {
            settings[mode] = json!(true);
        }
        std::fs::write(d.path().join("fixture.json"), settings.to_string()).unwrap();
        let config = RemoteConfig {
            codex,
            codex_home: d.path().into(),
            verified_version: Some("codex-cli fixture".into()),
        };
        let db = d.path().join("source.sqlite");
        native_remote::initialize(&db).unwrap();
        let request = json!({"op":"steer","request_id":uuid::Uuid::new_v4().to_string(),"thread":NATIVE,"queue_id":QUEUE,"expected_turn":TURN,"text":"explicit phone guidance","expected_version":"codex-cli fixture"});
        (d, config, db, request)
    }
    #[tokio::test]
    async fn removes_the_same_queue_item_before_one_idempotent_steer() {
        let (d, config, db, r) = fixture(None);
        let first = execute(&config, &db, &r).await.unwrap();
        assert_eq!(first["status"], "steer_accepted");
        assert_eq!(first["native_turn_id"], TURN);
        assert_eq!(execute(&config, &db, &r).await.unwrap(), first);
        assert_eq!(
            std::fs::read_to_string(d.path().join("calls")).unwrap(),
            "delete\nsteer\n"
        );
        let mut changed = r.clone();
        changed["text"] = json!("different");
        assert_eq!(
            execute(&config, &db, &changed)
                .await
                .unwrap_err()
                .to_string(),
            "idempotency_conflict"
        );
    }
    #[tokio::test]
    async fn failed_preflight_keeps_original_queue_and_can_be_retried() {
        for mode in [
            "no_socket",
            "idle",
            "wrong_turn",
            "queue_missing",
            "queue_changed",
        ] {
            let (d, config, db, r) = fixture(Some(mode));
            let result = execute(&config, &db, &r).await.unwrap();
            assert_eq!(result["status"], "upstream_queued");
            assert!(!d.path().join("calls").exists());
            assert_eq!(
                rusqlite::Connection::open(&db)
                    .unwrap()
                    .query_row("SELECT count(*) FROM phone_steers", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
    #[tokio::test]
    async fn revoked_or_unverifiable_collection_never_removes_the_queue_item() {
        for mode in ["disabled", "excluded", "missing"] {
            let (d, config, db, r) = fixture(None);
            if mode == "missing" {
                std::fs::write(crate::collection::suffix(&db, ".collection-required"), b"")
                    .unwrap();
            } else {
                crate::collection::write_policy(&db,&json!({"schema":1,"generation":uuid::Uuid::new_v4().to_string(),"cutoff_ms":1,"enabled":mode!="disabled","excluded":if mode=="excluded"{vec![NATIVE]}else{vec![]}})).unwrap();
            }
            assert_eq!(
                execute(&config, &db, &r).await.unwrap()["status"],
                "upstream_queued"
            );
            assert!(!d.path().join("calls").exists());
        }
    }
    #[tokio::test]
    async fn uncertain_delete_or_steer_never_runs_twice() {
        for mode in [
            "lose_delete",
            "delete_missing",
            "lose_steer",
            "reject_steer",
        ] {
            let (d, config, db, r) = fixture(Some(mode));
            let first = execute(&config, &db, &r).await.unwrap();
            assert_eq!(first["status"], "unknown");
            assert_eq!(execute(&config, &db, &r).await.unwrap(), first);
            assert_eq!(
                std::fs::read_to_string(d.path().join("calls")).unwrap(),
                if matches!(mode, "lose_steer" | "reject_steer") {
                    "delete\nsteer\n"
                } else {
                    "delete\n"
                }
            );
        }
    }
    #[test]
    fn submit_is_device_scoped_requires_known_queue_and_releases_only_with_evidence() {
        let d = tempfile::tempdir().unwrap();
        let db = Store::open(&d.path().join("hub.sqlite")).unwrap();
        let (host, _) = db.credential("agent", "fixture").unwrap();
        let (phone, _) = db.credential("phone", "fixture").unwrap();
        db.heartbeat(&host).unwrap();
        let thread = key(&host, NATIVE);
        let id = uuid::Uuid::new_v4().to_string();
        {
            let c = db.0.lock().unwrap();
            c.execute(
                "INSERT INTO threads VALUES(?1,?2,?3,'fixture','queue_ready','prior',?4,1,NULL)",
                params![thread, host, NATIVE, now()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO capture_targets VALUES(?1,?2,?3,1,?4)",
                params![thread, host, NATIVE, now()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO thread_tasks VALUES(?1,?2,'running')",
                params![thread, TURN],
            )
            .unwrap();
            c.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) VALUES(?1,?2,?1,'digest',?3,?4,?5,'upstream_queued',?6,?7)",params![id,phone,host,thread,json!({"kind":"send","text":"explicit phone guidance"}).to_string(),now(),now()+60]).unwrap();
            c.execute(
                "INSERT INTO capture_queue_ledger VALUES(?1,'upstream_queued',?2)",
                params![id, QUEUE],
            )
            .unwrap();
        }
        assert!(submit(&db, "foreign-phone", &id).is_err());
        let receipt = submit(&db, &phone, &id).unwrap();
        assert_eq!(receipt["error"], "steer_in_progress");
        assert_eq!(submit(&db, &phone, &id).unwrap(), receipt);
        db.0.lock()
            .unwrap()
            .execute("UPDATE steering_intents SET created=?1", [now() - 61])
            .unwrap();
        expire_unattempted(&db, &host).unwrap();
        assert_eq!(
            db.command(&phone, &id).unwrap()["error"],
            "steer_unavailable"
        );
        assert_eq!(
            submit(&db, &phone, &id).unwrap()["error"],
            "steer_in_progress"
        );
        settle(&db, &host, &id, "steer_accepted", Some(TURN), None).unwrap();
        assert_eq!(db.command(&phone, &id).unwrap()["status"], "steer_accepted");
        db.receipt(&host, &id, "upstream_queued", None, None)
            .unwrap();
        assert_eq!(db.command(&phone, &id).unwrap()["status"], "steer_accepted");
        db.receipt(&host, &id, "codex_accepted", Some(TURN), None)
            .unwrap();
        assert_eq!(db.command(&phone, &id).unwrap()["status"], "codex_accepted");
        let interrupted = uuid::Uuid::new_v4().to_string();
        {
            let c = db.0.lock().unwrap();
            c.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) SELECT ?1,device,?1,digest,host,thread,payload,'upstream_queued',created,expires FROM commands WHERE id=?2",params![interrupted,id]).unwrap();
            c.execute(
                "INSERT INTO capture_queue_ledger VALUES(?1,'upstream_queued',?2)",
                params![interrupted, QUEUE],
            )
            .unwrap();
            c.execute(
                "INSERT INTO steering_intents VALUES(?1,'dispatching',?2,NULL,?3)",
                params![interrupted, TURN, now()],
            )
            .unwrap();
        }
        recover(&db, &host).unwrap();
        recover(&db, &host).unwrap();
        assert_eq!(
            db.command(&phone, &interrupted).unwrap()["status"],
            "unknown"
        );
        assert_eq!(
            db.command(&phone, &interrupted).unwrap()["error"],
            "steer_result_unknown_no_retry"
        );
        assert_eq!(db.command(&phone, &id).unwrap()["status"], "codex_accepted");
    }
}
