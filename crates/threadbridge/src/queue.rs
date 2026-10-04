//! One-shot allowlisted queue delivery. A queued item is not an accepted turn.
use crate::{model::*, store::Store};
use anyhow::Result;
use rusqlite::OptionalExtension;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command as Process};

pub async fn dispatch(
    db: &Store,
    codex: &Path,
    host: &str,
    native: &str,
    cmd: &Command,
) -> Result<()> {
    anyhow::ensure!(
        cmd.native_id == native && cmd.thread_id == key(host, native),
        "queue_scope_mismatch"
    );
    let prior: Option<String> =
        db.0.lock()
            .unwrap()
            .query_row(
                "SELECT status FROM capture_queue_ledger WHERE id=?1",
                [&cmd.id],
                |r| r.get(0),
            )
            .optional()?;
    if let Some(status) = prior {
        if status == "codex_accepted" {
            return Ok(());
        }
        let status = if status == "intent" {
            "unknown"
        } else {
            &status
        };
        db.receipt(host, &cmd.id, status, None, None)?;
        return Ok(());
    }
    if cmd.expires_at <= now() || cmd.kind != "send" {
        db.receipt(
            host,
            &cmd.id,
            "rejected",
            None,
            Some("expired_or_unsupported"),
        )?;
        return Ok(());
    }
    {
        let c = db.0.lock().unwrap();
        let current: (String, bool) = c.query_row(
            "SELECT revision,can_send FROM threads WHERE id=?1",
            [&cmd.thread_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        anyhow::ensure!(
            current.0 == cmd.expected_revision && current.1,
            "queue_state_changed"
        );
        c.execute(
            "INSERT INTO capture_queue_ledger(id,status) VALUES(?1,'intent')",
            [&cmd.id],
        )?;
    }
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    db.0.lock().unwrap().execute(
        "INSERT INTO capture_queue_send_times(id,started_at) VALUES(?1,?2)",
        rusqlite::params![cmd.id, started_at],
    )?;
    // Keep correlation in the ledger; desktop input must be the human text.
    let text = &cmd.text;
    let mut child = match Process::new(codex)
        .args(["queue", "--thread", native, "--message", text])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            db.0.lock().unwrap().execute(
                "UPDATE capture_queue_ledger SET status='rejected' WHERE id=?1",
                [&cmd.id],
            )?;
            db.receipt(
                host,
                &cmd.id,
                "rejected",
                None,
                Some("queue_process_not_started"),
            )?;
            return Ok(());
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .take(4097)
            .read_to_end(&mut bytes)
            .await?;
        anyhow::ensure!(bytes.len() <= 4096, "queue_output_limit");
        anyhow::ensure!(child.wait().await?.success(), "queue_failed");
        let output = String::from_utf8(bytes)?;
        let prefix = "Queued message ";
        let suffix = format!(" for thread {native}.");
        let id = output
            .trim()
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(&suffix))
            .ok_or_else(|| anyhow::anyhow!("queue_ack_unknown"))?;
        uuid::Uuid::parse_str(id)?;
        Ok::<_, anyhow::Error>(id.to_owned())
    })
    .await;
    let (status, queued) = match result {
        Ok(Ok(id)) => ("upstream_queued", Some(id)),
        _ => {
            let _ = child.start_kill();
            ("unknown", None)
        }
    };
    db.0.lock().unwrap().execute(
        "UPDATE capture_queue_ledger SET status=?2,queue_id=?3 WHERE id=?1",
        rusqlite::params![cmd.id, status, queued],
    )?;
    db.receipt(
        host,
        &cmd.id,
        status,
        None,
        if status == "unknown" {
            Some("queue_result_unknown_no_retry")
        } else {
            None
        },
    )?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    const NATIVE: &str = "00000000-0000-4000-8000-000000000001";
    fn fixture(reply: bool) -> (tempfile::TempDir, Store, std::path::PathBuf, Command) {
        let dir = tempfile::tempdir().unwrap();
        let db = Store::open(&dir.path().join("hub.sqlite")).unwrap();
        let id = "00000000-0000-4000-8000-000000000003";
        let thread = key("host", NATIVE);
        {
            let c = db.0.lock().unwrap();
            c.execute("INSERT INTO devices(id,role,name,expires,last_seen) VALUES('host','agent','fixture',?1,?2)",rusqlite::params![now()+60,now()]).unwrap();
            c.execute("INSERT INTO threads VALUES(?1,'host',?2,'capture','queue_ready','prior',?3,1,NULL)",rusqlite::params![thread,NATIVE,now()]).unwrap();
            c.execute(
                "INSERT INTO capture_targets VALUES(?1,'host',?2,1,?3)",
                rusqlite::params![thread, NATIVE, now()],
            )
            .unwrap();
            c.execute("INSERT INTO commands VALUES(?1,'phone',?1,'digest','host',?2,'{}','dispatching',?3,?4,NULL,NULL)",rusqlite::params![id,thread,now(),now()+60]).unwrap();
        }
        let bin = dir.path().join("mock-codex");
        std::fs::write(
            dir.path().join("fixture.json"),
            serde_json::json!({"mode": "queue", "native_id": NATIVE, "reply": reply}).to_string(),
        )
        .unwrap();
        symlink(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/mock_codex.py"),
            &bin,
        )
        .unwrap();
        let cmd = Command {
            id: id.into(),
            thread_id: thread,
            native_id: NATIVE.into(),
            text: "explicit phone action".into(),
            expected_revision: "prior".into(),
            kind: "send".into(),
            cursor: None,
            expires_at: now() + 60,
        };
        (dir, db, bin, cmd)
    }
    #[tokio::test]
    async fn queued_and_unknown_receipts_never_resubmit() {
        for valid in [true, false] {
            let (dir, db, bin, cmd) = fixture(valid);
            dispatch(&db, &bin, "host", NATIVE, &cmd).await.unwrap();
            dispatch(&db, &bin, "host", NATIVE, &cmd).await.unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("calls")).unwrap(),
                "call\n"
            );
            let receipt = db.command("phone", &cmd.id).unwrap();
            assert_eq!(
                receipt["status"],
                if valid { "upstream_queued" } else { "unknown" }
            );
            assert!(receipt["native_turn_id"].is_null());
            let mut wrong = cmd.clone();
            wrong.native_id = "other".into();
            assert!(dispatch(&db, &bin, "host", NATIVE, &wrong).await.is_err());
        }
    }
    #[tokio::test]
    async fn interrupted_intent_reopens_unknown_without_launching_process() {
        let (dir, db, bin, cmd) = fixture(true);
        db.0.lock()
            .unwrap()
            .execute(
                "INSERT INTO capture_queue_ledger(id,status) VALUES(?1,'intent')",
                [&cmd.id],
            )
            .unwrap();
        dispatch(&db, &bin, "host", NATIVE, &cmd).await.unwrap();
        assert!(!dir.path().join("calls").exists());
        assert_eq!(db.command("phone", &cmd.id).unwrap()["status"], "unknown");
    }
}
