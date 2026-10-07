use crate::model::*;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};

#[derive(Debug)]
pub enum SendError {
    NotDispatched(anyhow::Error),
    Unknown(anyhow::Error),
}
impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDispatched(e) | Self::Unknown(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for SendError {}

pub struct Adapter {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
    pub allow_send: bool,
    pub version: String,
}
impl Adapter {
    pub async fn connect(
        codex: &Path,
        socket: Option<&Path>,
        verified: Option<&str>,
    ) -> Result<Self> {
        Self::connect_with_home(codex, socket, verified, None).await
    }
    pub async fn connect_with_home(
        codex: &Path,
        socket: Option<&Path>,
        verified: Option<&str>,
        home: Option<&Path>,
    ) -> Result<Self> {
        let version = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(codex)
                .arg("--version")
                .output(),
        )
        .await??;
        anyhow::ensure!(version.status.success(), "version check failed");
        let version = String::from_utf8(version.stdout)?.trim().to_string();
        let allow_send = verified.is_some_and(|v| v == version);
        let mut cmd = tokio::process::Command::new(codex);
        cmd.args(["app-server", "proxy"]);
        if let Some(home) = home {
            cmd.env("CODEX_HOME", home);
        }
        if let Some(socket) = socket {
            cmd.arg("--sock").arg(socket);
        }
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut a = Self {
            child,
            input,
            output,
            next: 0,
            allow_send,
            version,
        };
        a.rpc("initialize",json!({"clientInfo":{"name":"threadbridge","version":"0.1.0"},"capabilities":{"experimentalApi":true}})).await?;
        a.input
            .write_all(b"{\"method\":\"initialized\",\"params\":{}}\n")
            .await?;
        Ok(a)
    }
    async fn rpc_inner(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        let mut body = serde_json::to_vec(&json!({"id":id,"method":method,"params":params}))?;
        body.push(b'\n');
        self.input.write_all(&body).await?;
        for _ in 0..128 {
            let mut line = Vec::new();
            let n = (&mut self.output)
                .take((FRAME_LIMIT + 1) as u64)
                .read_until(b'\n', &mut line)
                .await?;
            if n == 0 {
                bail!("desktop_disconnected")
            }
            if n > FRAME_LIMIT || line.last() != Some(&b'\n') {
                bail!("upstream_response_too_large")
            }
            let v: Value = serde_json::from_slice(&line)?;
            if v["id"] == id {
                if v.get("error").is_some() {
                    bail!("upstream_rejected")
                }
                return v.get("result").cloned().context("missing_rpc_result");
            }
        }
        bail!("upstream_event_flood")
    }
    pub async fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(12), self.rpc_inner(method, params))
            .await
            .context("upstream_timeout")?
    }
    pub async fn list(&mut self, cursor: Option<&str>) -> Result<Value> {
        self.rpc("thread/list",json!({"limit":20,"cursor":cursor,"sourceKinds":["appServer","cli","vscode"],"useStateDbOnly":true,"sortKey":"updated_at"})).await
    }
    pub async fn snapshot(
        &mut self,
        host: &str,
        native: &str,
        cursor: Option<&str>,
        initial: bool,
    ) -> Result<Snapshot> {
        let read = self
            .rpc(
                "thread/read",
                json!({"threadId":native,"includeTurns":false}),
            )
            .await?;
        let t = &read["thread"];
        anyhow::ensure!(t["id"].as_str() == Some(native), "identity_mismatch");
        let history=self.rpc("thread/turns/list",json!({"threadId":native,"limit":3,"cursor":cursor,"itemsView":"full","sortDirection":"desc"})).await?;
        let turns = history["data"].as_array().context("unsupported_history")?;
        let mut messages = Vec::new();
        let mut revision = String::new();
        let mut status = t["status"]["type"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        for (turn_index, turn) in turns.iter().enumerate() {
            let turn_id = turn["id"].as_str().context("missing_turn_id")?;
            if turn_index == 0 {
                revision = turn_id.into();
                if status == "idle" {
                    status = match turn["status"].as_str() {
                        Some("completed") => "completed",
                        Some("failed") => "failed",
                        Some("interrupted") => "interrupted",
                        _ => "idle",
                    }
                    .into()
                }
            }
            let timestamp = turn["startedAt"]
                .as_i64()
                .unwrap_or(t["updatedAt"].as_i64().unwrap_or(0));
            for (i, item) in turn["items"].as_array().into_iter().flatten().enumerate() {
                let (role, text) = match item["type"].as_str() {
                    Some("userMessage") => (
                        "user",
                        item["content"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|v| v["type"] == "text")
                            .filter_map(|v| v["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    Some("agentMessage")
                        if matches!(item["phase"].as_str(), Some("final_answer" | "final")) =>
                    {
                        ("assistant", item["text"].as_str().unwrap_or("").into())
                    }
                    _ => continue,
                };
                if text.len() > 256 * 1024 {
                    bail!("message_exceeds_supported_256kib")
                }
                let id = item["id"]
                    .as_str()
                    .context("missing_message_id")?
                    .to_string();
                messages.push(ChatMessage {
                    id,
                    turn_id: turn_id.into(),
                    role: role.into(),
                    version: hash(&text),
                    text,
                    ordinal: timestamp.saturating_mul(1000) + i as i64,
                });
            }
        }
        let can_send =
            self.allow_send && ["idle", "completed"].contains(&status.as_str()) && cursor.is_none();
        Ok(Snapshot {
            thread: Thread {
                id: key(host, native),
                native_id: native.into(),
                host_id: host.into(),
                title: t["name"]
                    .as_str()
                    .unwrap_or("Codex task")
                    .chars()
                    .take(200)
                    .collect(),
                status,
                revision,
                updated_at: t["updatedAt"].as_i64().unwrap_or(now()),
                can_send,
                history_cursor: history["nextCursor"].as_str().map(str::to_string),
                project: t["cwd"].as_str().unwrap_or("").to_owned(),
            },
            messages,
            initial,
            history: cursor.is_some(),
        })
    }
    async fn preflight(&mut self, cmd: &Command) -> Result<()> {
        anyhow::ensure!(self.allow_send, "write_capability_unverified");
        let mut cursor: Option<String> = None;
        let mut loaded = false;
        // A stored thread alone does not prove it belongs to this live executor.
        // Bound the search, and never resume a task merely to make it sendable.
        for _ in 0..10 {
            let page = self
                .rpc("thread/loaded/list", json!({"limit":100,"cursor":cursor}))
                .await?;
            let ids = page["data"]
                .as_array()
                .context("unsupported_loaded_threads")?;
            if ids
                .iter()
                .any(|id| id.as_str() == Some(cmd.native_id.as_str()))
            {
                loaded = true;
                break;
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        anyhow::ensure!(loaded, "target_not_loaded_in_existing_executor");
        let current = self
            .snapshot("preflight", &cmd.native_id, None, false)
            .await?;
        anyhow::ensure!(
            current.thread.can_send && current.thread.revision == cmd.expected_revision,
            "state_changed_before_dispatch"
        );
        anyhow::ensure!(cmd.expires_at > now(), "expired_before_dispatch");
        Ok(())
    }
    pub async fn send(&mut self, cmd: &Command) -> std::result::Result<String, SendError> {
        self.preflight(cmd)
            .await
            .map_err(SendError::NotDispatched)?;
        // Once turn/start is attempted its result may be unknown; never retry automatically.
        let result = self
            .rpc(
                "turn/start",
                json!({"threadId":cmd.native_id,"input":[{"type":"text","text":cmd.text}]}),
            )
            .await
            .map_err(SendError::Unknown)?;
        result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("missing_authoritative_turn_id")
            .map_err(SendError::Unknown)
    }
}
impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    async fn fixture(loaded: bool) -> (tempfile::TempDir, Adapter) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codex-fixture");
        std::fs::write(
            dir.path().join("fixture.json"),
            json!({"mode": "proxy", "loaded": loaded}).to_string(),
        )
        .unwrap();
        symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/mock_codex.py"),
            &path,
        )
        .unwrap();
        let adapter = Adapter::connect(&path, None, Some("codex-cli fixture"))
            .await
            .unwrap();
        (dir, adapter)
    }

    fn command() -> Command {
        Command {
            id: "command".into(),
            thread_id: "bridge".into(),
            native_id: "original".into(),
            text: "fixture".into(),
            expected_revision: "prior".into(),
            expires_at: now() + 60,
            kind: "send".into(),
            cursor: None,
        }
    }

    #[tokio::test]
    async fn stored_but_not_loaded_task_cannot_be_sent() {
        let (_dir, mut adapter) = fixture(false).await;
        let error = adapter.send(&command()).await.unwrap_err();
        assert!(error.to_string().contains("target_not_loaded"));
    }

    #[tokio::test]
    async fn loaded_task_keeps_authoritative_turn_id() {
        let (_dir, mut adapter) = fixture(true).await;
        assert_eq!(
            adapter.send(&command()).await.unwrap(),
            "existing-task-turn"
        );
        let mut expired = command();
        expired.expires_at = now() - 1;
        assert!(matches!(
            adapter.send(&expired).await,
            Err(SendError::NotDispatched(_))
        ));
        let mut stale = command();
        stale.expected_revision = "changed".into();
        assert!(matches!(
            adapter.send(&stale).await,
            Err(SendError::NotDispatched(_))
        ));
        adapter.allow_send = false;
        assert!(adapter
            .send(&command())
            .await
            .unwrap_err()
            .to_string()
            .contains("unverified"));
    }
}
