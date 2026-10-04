//! Opt-in original-ID resume candidate. A durable intent always precedes generation.
//! Unknown receipts are never replayed; only an authoritative captured turn can settle them.
use anyhow::{bail, ensure, Result};
use clap::Args;
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{timeout, Instant},
};
const FRAME: usize = 1024 * 1024;

#[derive(Args, Debug)]
#[command(group(clap::ArgGroup::new("mode").required(true).multiple(false).args(["command","worker","preflight"])))]
pub struct ResumeArgs {
    #[arg(long)]
    pub db: PathBuf,
    #[arg(long)]
    pub capture_db: PathBuf,
    #[arg(long)]
    pub codex: PathBuf,
    #[arg(long)]
    pub grant: PathBuf,
    #[arg(long)]
    pub command: Option<String>,
    #[arg(long)]
    pub worker: bool,
    #[arg(long)]
    pub preflight: bool,
    #[arg(long)]
    pub execute: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub native_id: String,
    pub host_id: String,
    pub cwd: PathBuf,
    pub verified_cli_version: String,
    pub expires_at: i64,
    pub tool_compatibility_confirmed: bool,
    pub handoff_confirmed: bool,
    pub approval_policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn key(host: &str, native: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{host}\0default\0{native}").as_bytes())
    )
}
pub fn grant_hash(grant: &Grant) -> Result<String> {
    // serde_json's default map is ordered, matching the prior canonical JSON hash.
    let encoded = serde_json::to_string(&serde_json::to_value(grant)?)?;
    let mut ascii = String::new();
    for ch in encoded.chars() {
        if ch as u32 >= 0x7f {
            for code in ch.encode_utf16(&mut [0; 2]) {
                ascii.push_str(&format!("\\u{code:04x}"));
            }
        } else {
            ascii.push(ch);
        }
    }
    Ok(format!("{:x}", Sha256::digest(ascii.as_bytes())))
}
pub fn validate_grant(grant: &Grant) -> Result<()> {
    uuid::Uuid::parse_str(&grant.native_id)?;
    uuid::Uuid::parse_str(&grant.host_id)?;
    ensure!(
        grant.cwd.is_absolute() && grant.cwd.is_dir(),
        "invalid_working_directory"
    );
    ensure!(
        matches!(
            grant.sandbox_mode.as_deref().unwrap_or("read-only"),
            "read-only" | "workspace-write"
        ),
        "unsupported_sandbox_mode"
    );
    ensure!(
        grant.approval_policy == "on-request",
        "unsupported_approval_policy"
    );
    ensure!(grant.expires_at > now(), "grant_expired");
    ensure!(
        grant.tool_compatibility_confirmed,
        "tool_compatibility_not_confirmed"
    );
    ensure!(grant.handoff_confirmed, "execution_handoff_not_confirmed");
    Ok(())
}
fn connect(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(3))?;
    c.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS resume_dispatch_ledger(id TEXT PRIMARY KEY,host TEXT NOT NULL,native TEXT NOT NULL,status TEXT NOT NULL,turn TEXT,error TEXT,created INTEGER NOT NULL);")?;
    Ok(c)
}
fn read_only(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}
pub type Receipt = (String, Option<String>, Option<String>);
fn read_receipt(c: &Connection, command: &str) -> Result<Receipt> {
    Ok(c.query_row(
        "SELECT status,error,native_turn FROM commands WHERE id=?",
        [command],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}
pub fn receipt(
    c: &mut Connection,
    command: &str,
    status: &str,
    error: Option<&str>,
    turn: Option<&str>,
) -> Result<()> {
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let old: Option<String> = tx
        .query_row("SELECT status FROM commands WHERE id=?", [command], |r| {
            r.get(0)
        })
        .optional()?;
    if old.as_deref() == Some("codex_accepted") && status != "codex_accepted" {
        return Ok(());
    }
    tx.execute(
        "UPDATE commands SET status=?,error=?,native_turn=coalesce(?,native_turn) WHERE id=?",
        params![status, error, turn, command],
    )?;
    tx.execute(
        "UPDATE resume_dispatch_ledger SET status=?,error=?,turn=coalesce(?,turn) WHERE id=?",
        params![status, error, turn, command],
    )?;
    tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,? FROM commands WHERE id=?",params![now(),command])?;
    tx.commit()?;
    Ok(())
}
struct Protocol {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    buffer: Vec<u8>,
    bytes: usize,
    identity: u64,
    thread: String,
    turn: Option<String>,
    completed: Vec<Value>,
    finals: Vec<(Option<String>, String)>,
}
impl Protocol {
    async fn spawn(codex: &Path, native: &str) -> Result<Self> {
        let mut child = Command::new(codex)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Self {
            input: child.stdin.take().unwrap(),
            output: child.stdout.take().unwrap(),
            child,
            buffer: vec![],
            bytes: 0,
            identity: 0,
            thread: native.into(),
            turn: None,
            completed: vec![],
            finals: vec![],
        })
    }
    async fn send(&mut self, row: Value) -> Result<()> {
        let mut data = serde_json::to_vec(&row)?;
        data.push(b'\n');
        ensure!(data.len() <= FRAME, "request_limit");
        timeout(Duration::from_secs(12), async {
            self.input.write_all(&data).await?;
            self.input.flush().await
        })
        .await
        .map_err(|_| anyhow::anyhow!("upstream_timeout"))??;
        Ok(())
    }
    async fn receive(&mut self, deadline: Instant) -> Result<Value> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line = self.buffer.drain(..=end).collect::<Vec<_>>();
                return Ok(serde_json::from_slice(&line)?);
            }
            let mut chunk = vec![0; 65536];
            let left = deadline.saturating_duration_since(Instant::now());
            let size = timeout(left, self.output.read(&mut chunk))
                .await
                .map_err(|_| anyhow::anyhow!("upstream_timeout"))??;
            ensure!(size > 0, "upstream_disconnected");
            self.bytes += size;
            self.buffer.extend_from_slice(&chunk[..size]);
            ensure!(
                self.buffer.len() <= FRAME && self.bytes <= 8 * FRAME,
                "upstream_output_limit"
            );
        }
    }
    fn handle(&mut self, row: Value) -> Result<()> {
        let method = row["method"].as_str().unwrap_or("");
        let p = &row["params"];
        if row.get("id").is_some() && !method.is_empty() {
            if method == "item/tool/call" {
                bail!("desktop_dynamic_tool_unavailable")
            }
            if method.contains("requestApproval")
                || matches!(
                    method,
                    "tool/requestUserInput" | "mcpServer/elicitation/request"
                )
            {
                bail!("user_action_required")
            }
            bail!("unsupported_server_request")
        }
        if method == "item/completed"
            && p["item"]["type"] == "agentMessage"
            && p["item"]["phase"] == "final_answer"
        {
            ensure!(
                p["threadId"].as_str() == Some(&self.thread),
                "completion_thread_mismatch"
            );
            if let Some(text) = p["item"]["text"].as_str().filter(|s| !s.is_empty()) {
                self.finals
                    .push((p["turnId"].as_str().map(str::to_owned), text.into()))
            }
        }
        if method == "turn/completed" {
            ensure!(
                p["threadId"].as_str() == Some(&self.thread),
                "completion_thread_mismatch"
            );
            self.completed.push(p["turn"].clone())
        }
        Ok(())
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        self.identity += 1;
        let id = self.identity;
        self.send(json!({"id":id,"method":method,"params":params}))
            .await?;
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let row = self.receive(deadline).await?;
            if row["id"].as_u64() == Some(id) && row.get("method").is_none() {
                ensure!(row.get("error").is_none(), "upstream_rejected");
                return row
                    .get("result")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("upstream_response_invalid"));
            }
            self.handle(row)?;
        }
    }
    async fn wait_completed(&mut self, wait: Duration) -> Result<()> {
        let deadline = Instant::now() + wait;
        while self.completed.is_empty() {
            let row = self.receive(deadline).await?;
            self.handle(row)?
        }
        let turn = self.completed.remove(0);
        ensure!(
            turn["id"].as_str() == self.turn.as_deref(),
            "completion_turn_mismatch"
        );
        ensure!(turn["status"] == "completed", "turn_not_completed");
        Ok(())
    }
    async fn close(&mut self) {
        let _ = self.child.kill().await;
        let _ = timeout(Duration::from_secs(3), self.child.wait()).await;
    }
}
fn require_thread(row: &Value, grant: &Grant) -> Result<()> {
    let t = &row["thread"];
    ensure!(t["id"] == grant.native_id, "original_id_mismatch");
    ensure!(
        t["cwd"].as_str() == grant.cwd.to_str(),
        "working_directory_mismatch"
    );
    ensure!(
        (t["ephemeral"].is_null() || t["ephemeral"] == false)
            && (t["forkedFromId"].is_null() || t["forkedFromId"] == ""),
        "unsupported_thread_identity"
    );
    ensure!(
        matches!(t["status"]["type"].as_str(), Some("idle" | "notLoaded")),
        "thread_not_idle"
    );
    Ok(())
}
async fn version(codex: &Path, grant: &Grant) -> Result<()> {
    let mut child = Command::new(codex)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let output = child.stdout.take().unwrap();
    let mut bytes = Vec::new();
    let operation = async {
        output
            .take((FRAME + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(bytes.len() <= FRAME, "upstream_output_limit");
        ensure!(child.wait().await?.success(), "cli_version_changed");
        Ok::<_, anyhow::Error>(())
    };
    let result = timeout(Duration::from_secs(5), operation)
        .await
        .map_err(|_| anyhow::anyhow!("upstream_timeout"))?;
    if result.is_err() {
        let _ = child.kill().await;
    }
    result?;
    ensure!(
        String::from_utf8(bytes)?.trim() == grant.verified_cli_version,
        "cli_version_changed"
    );
    Ok(())
}
async fn restore(
    p: &mut Protocol,
    grant: &Grant,
    revision: &str,
    preflight: bool,
) -> Result<Value> {
    let name = if preflight {
        "threadbridge-resume-preflight"
    } else {
        "threadbridge-resume-candidate"
    };
    p.rpc(
        "initialize",
        json!({"clientInfo":{"name":name,"version":"0.1"},"capabilities":{"experimentalApi":true}}),
    )
    .await?;
    p.send(json!({"method":"initialized","params":{}})).await?;
    require_thread(
        &p.rpc(
            "thread/read",
            json!({"threadId":grant.native_id,"includeTurns":false}),
        )
        .await?,
        grant,
    )?;
    let row=p.rpc("thread/resume",json!({"threadId":grant.native_id,"excludeTurns":true,"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":grant.sandbox_mode.as_deref().unwrap_or("read-only"),"cwd":grant.cwd})).await?;
    require_thread(&row, grant)?;
    ensure!(
        row["approvalPolicy"] == "on-request" && row["approvalsReviewer"] == "user",
        "approval_policy_mismatch"
    );
    ensure!(
        row["thread"]["canAcceptDirectInput"] == true,
        "direct_input_capability_unconfirmed"
    );
    let expected = if grant.sandbox_mode.as_deref() == Some("workspace-write") {
        "workspaceWrite"
    } else {
        "readOnly"
    };
    ensure!(
        row["sandbox"]["type"] == expected
            && (row["sandbox"].get("networkAccess").is_none()
                || row["sandbox"]["networkAccess"] == false),
        "sandbox_policy_mismatch"
    );
    if let Some(value) = row["sandbox"].get("writableRoots") {
        let roots = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("writable_scope_mismatch"))?;
        ensure!(
            roots.iter().all(|r| r.as_str() == grant.cwd.to_str()),
            "writable_scope_mismatch"
        )
    }
    let page=p.rpc("thread/turns/list",json!({"threadId":grant.native_id,"limit":1,"itemsView":"summary","sortDirection":"desc"})).await?;
    ensure!(
        page["data"].as_array().is_some_and(|d| !d.is_empty())
            && page["data"][0]["id"] == revision
            && matches!(
                page["data"][0]["status"].as_str(),
                Some("completed" | "failed" | "interrupted")
            ),
        "original_context_revision_mismatch"
    );
    Ok(row)
}
fn prepare(c: &mut Connection, grant: &Grant, command_id: &str) -> Result<Option<Value>> {
    let thread = key(&grant.host_id, &grant.native_id);
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let old: Option<String> = tx
        .query_row(
            "SELECT status FROM resume_dispatch_ledger WHERE id=?",
            [command_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(status) = old {
        drop(tx);
        if matches!(status.as_str(), "intent" | "dispatching") {
            receipt(
                c,
                command_id,
                "unknown",
                Some("interrupted_resume_no_retry"),
                None,
            )?
        } else if status == "preparing" {
            receipt(
                c,
                command_id,
                "rejected",
                Some("interrupted_preparation_no_generation"),
                None,
            )?
        }
        return Ok(None);
    }
    ensure!(
        tx.query_row(
            "SELECT count(*) FROM devices WHERE id=? AND role='agent' AND revoked=0 AND expires>?",
            params![grant.host_id, now()],
            |r| r.get::<_, i64>(0)
        )? > 0,
        "host_not_authorized"
    );
    ensure!(tx.query_row("SELECT count(*) FROM capture_queue_ledger q JOIN commands c ON c.id=q.id WHERE c.thread=? AND q.status<>'codex_accepted'",[&thread],|r|r.get::<_,i64>(0))?==0,"legacy_queue_unresolved");
    let row: Option<(String, String, String, String, i64)> = tx
        .query_row(
            "SELECT host,thread,status,payload,expires FROM commands WHERE id=?",
            [command_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let row = row.ok_or_else(|| anyhow::anyhow!("command_scope_or_state_mismatch"))?;
    ensure!(
        row.0 == grant.host_id && row.1 == thread && row.2 == "accepted",
        "command_scope_or_state_mismatch"
    );
    ensure!(row.4 > now(), "command_expired");
    let mut command: Value = serde_json::from_str(&row.3)?;
    ensure!(
        command["native_id"] == grant.native_id
            && command["thread_id"] == thread
            && command["id"] == command_id
            && command["kind"] == "send",
        "command_identity_mismatch"
    );
    ensure!(tx.query_row("SELECT count(*) FROM commands WHERE thread=? AND id<>? AND status IN ('accepted','dispatching','upstream_queued','unknown')",params![thread,command_id],|r|r.get::<_,i64>(0))?==0,"thread_command_pending");
    let revision: Option<String> = tx
        .query_row("SELECT revision FROM threads WHERE id=?", [&thread], |r| {
            r.get(0)
        })
        .optional()?;
    ensure!(
        revision.as_deref() == command["expected_revision"].as_str() && revision.is_some(),
        "revision_conflict"
    );
    let owner = tx
        .query_row(
            "SELECT grant_hash,expires FROM resume_owners WHERE thread=?",
            [&thread],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|_| anyhow::anyhow!("resume_owner_not_provisioned"))?;
    ensure!(
        owner.is_some_and(|o| o.0 == grant_hash(grant).unwrap_or_default() && o.1 > now()),
        "resume_owner_not_authorized"
    );
    tx.execute("INSERT INTO resume_dispatch_ledger(id,host,native,status,created) VALUES(?,?,?,'preparing',?)",params![command_id,grant.host_id,grant.native_id,now()])?;
    tx.execute(
        "UPDATE commands SET status='dispatching' WHERE id=?",
        [command_id],
    )?;
    tx.execute(
        "INSERT INTO events(kind,thread,created) VALUES('command',?,?)",
        params![thread, now()],
    )?;
    tx.commit()?;
    if command.get("expires_at").is_none() {
        command["expires_at"] = json!(row.4)
    }
    Ok(Some(command))
}
fn captured(
    capture_db: &Path,
    native: &str,
    turn: &str,
    command: &str,
    created: Option<i64>,
) -> bool {
    (|| -> Result<bool> {
        let source = read_only(capture_db)?;
        let captured: Option<i64> = source
            .query_row(
                "SELECT captured_at FROM captured_replies WHERE thread_id=? AND turn_id=?",
                params![native, turn],
                |r| r.get(0),
            )
            .optional()?;
        let marker: Option<String> = source
            .query_row(
                "SELECT request_id FROM captured_request_ids WHERE thread_id=? AND turn_id=?",
                params![native, turn],
                |r| r.get(0),
            )
            .optional()?;
        Ok(captured.is_some_and(|t| created.is_none_or(|min| t >= min))
            && marker.as_deref().is_none_or(|m| m == command))
    })()
    .unwrap_or(false)
}
fn require_capture_policy(capture_db: &Path, native: &str) -> Result<()> {
    if let Some(policy) = crate::collection::read_policy(capture_db)? {
        ensure!(policy["enabled"] != false, "collection_disabled");
        ensure!(
            !policy["excluded"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == native),
            "collection_target_excluded"
        );
    }
    Ok(())
}

// The collection writer and final capture use the same lock. A turn already sent
// can become unknown on exclusion, but its durable intent can never become retryable.
fn check_capture_policy(capture_db: &Path, native: &str) -> Result<()> {
    let _guard = crate::collection::lock(capture_db)?;
    require_capture_policy(capture_db, native)
}

fn known_reason(error: &anyhow::Error) -> &str {
    let reason = error.to_string();
    const KNOWN: &[&str] = &[
        "command_expired",
        "grant_expired",
        "cli_version_changed",
        "original_id_mismatch",
        "working_directory_mismatch",
        "unsupported_thread_identity",
        "thread_not_idle",
        "approval_policy_mismatch",
        "direct_input_capability_unconfirmed",
        "sandbox_policy_mismatch",
        "writable_scope_mismatch",
        "original_context_revision_mismatch",
        "user_action_required",
        "desktop_dynamic_tool_unavailable",
        "unsupported_server_request",
        "upstream_timeout",
        "upstream_disconnected",
        "upstream_rejected",
        "upstream_response_invalid",
        "upstream_output_limit",
        "completion_thread_mismatch",
        "completion_turn_mismatch",
        "turn_not_completed",
        "final_capture_not_confirmed",
        "collection_disabled",
        "collection_target_excluded",
        "collection_policy_missing",
        "collection_policy_invalid",
    ];
    KNOWN
        .iter()
        .copied()
        .find(|s| *s == reason)
        .unwrap_or("resume_dispatch_failed")
}
#[allow(clippy::too_many_arguments)]
pub async fn dispatch(
    database: &Path,
    capture_db: &Path,
    codex: &Path,
    grant: &Grant,
    command_id: &str,
    execute: bool,
    turn_timeout: Duration,
    capture_timeout: Duration,
) -> Result<Receipt> {
    validate_grant(grant)?;
    ensure!(execute, "execution_not_enabled");
    uuid::Uuid::parse_str(command_id)?;
    let mut c = connect(database)?;
    let Some(command) = prepare(&mut c, grant, command_id)? else {
        return read_receipt(&c, command_id);
    };
    let mut intent = false;
    let mut turn = None;
    let mut protocol = None;
    let outcome=async {
        check_capture_policy(capture_db,&grant.native_id)?;
        version(codex,grant).await?;protocol=Some(Protocol::spawn(codex,&grant.native_id).await?);let p=protocol.as_mut().unwrap();
        restore(p,grant,command["expected_revision"].as_str().unwrap_or(""),false).await?;
        validate_grant(grant)?;ensure!(command["expires_at"].as_i64().unwrap_or(0)>now(),"command_expired");
        let text=command["text"].as_str().ok_or_else(||anyhow::anyhow!("invalid_input"))?;
        c.execute("UPDATE resume_dispatch_ledger SET status='intent' WHERE id=?",[command_id])?;intent=true;
        let sandbox=if grant.sandbox_mode.as_deref()==Some("workspace-write"){json!({"type":"workspaceWrite","writableRoots":[grant.cwd],"networkAccess":false})}else{json!({"type":"readOnly","networkAccess":false})};
        let response=p.rpc("turn/start",json!({"threadId":grant.native_id,"input":[{"type":"text","text":text}],"cwd":grant.cwd,"approvalPolicy":"on-request","approvalsReviewer":"user","sandboxPolicy":sandbox})).await?;
        let native_turn=response["turn"]["id"].as_str().ok_or_else(||anyhow::anyhow!("invalid_turn_id"))?.to_owned();uuid::Uuid::parse_str(&native_turn)?;turn=Some(native_turn.clone());
        let tx=c.transaction()?;tx.execute("UPDATE resume_dispatch_ledger SET status='dispatching',turn=? WHERE id=?",params![native_turn,command_id])?;tx.execute("UPDATE commands SET native_turn=? WHERE id=?",params![native_turn,command_id])?;tx.commit()?;
        p.turn=Some(native_turn.clone());p.wait_completed(turn_timeout).await?;
        let finals=p.finals.iter().filter(|(t,_)|t.as_deref()==Some(&native_turn)).map(|(_,s)|s).collect::<Vec<_>>();
        if finals.len()==1 {
            let _capture_guard=crate::collection::lock(capture_db)?;
            require_capture_policy(capture_db,&grant.native_id)?;
            let has_title=c.prepare("PRAGMA table_info(threads)")?.query_map([],|r|r.get::<_,String>(1))?.any(|r|r.as_deref()==Ok("title"));
            let title:Option<String>=if has_title{c.query_row("SELECT title FROM threads WHERE id=?",[key(&grant.host_id,&grant.native_id)],|r|r.get(0)).optional()?}else{None};
            crate::native_capture::capture_completion(&json!({"type":"agent-turn-complete","thread-id":grant.native_id,"turn-id":native_turn,"last-assistant-message":finals[0],"input-messages":[]}).to_string(),&grant.native_id,capture_db,title.as_deref(),None,None,64*1024*1024,true)?;
        }
        let deadline=Instant::now()+capture_timeout;while !captured(capture_db,&grant.native_id,&native_turn,command_id,None){ensure!(Instant::now()<deadline,"final_capture_not_confirmed");tokio::time::sleep(Duration::from_millis(100)).await;}
        receipt(&mut c,command_id,"codex_accepted",None,Some(&native_turn))?;Ok::<_,anyhow::Error>(())
    }.await;
    if let Some(p) = protocol.as_mut() {
        p.close().await
    }
    if let Err(error) = outcome {
        receipt(
            &mut c,
            command_id,
            if intent { "unknown" } else { "rejected" },
            Some(known_reason(&error)),
            turn.as_deref(),
        )?;
    }
    read_receipt(&c, command_id)
}
pub async fn preflight(
    database: &Path,
    codex: &Path,
    grant: &Grant,
    execute: bool,
) -> Result<Value> {
    validate_grant(grant)?;
    ensure!(execute, "execution_not_enabled");
    let c = read_only(database)?;
    let thread = key(&grant.host_id, &grant.native_id);
    ensure!(c.query_row("SELECT count(*) FROM commands WHERE thread=? AND status IN ('accepted','dispatching','upstream_queued','unknown')",[&thread],|r|r.get::<_,i64>(0))?==0,"unresolved_request_before_handoff");
    let revision: String = c
        .query_row("SELECT revision FROM threads WHERE id=?", [thread], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("existing_captured_target_required"))?;
    drop(c);
    version(codex, grant).await?;
    let mut p = Protocol::spawn(codex, &grant.native_id).await?;
    let result = restore(&mut p, grant, &revision, true).await;
    p.close().await;
    Ok(
        json!({"native_id":grant.native_id,"cwd":grant.cwd,"revision":revision,"no_turn_started":true,"instruction_sources":result?["instructionSources"].as_array().cloned().unwrap_or_default()}),
    )
}
pub fn reconcile_completed(database: &Path, capture_db: &Path, grant: &Grant) -> Result<()> {
    let mut c = connect(database)?;
    let rows=c.prepare("SELECT l.id,l.turn,x.created FROM resume_dispatch_ledger l JOIN commands x ON x.id=l.id WHERE l.host=? AND l.native=? AND l.turn IS NOT NULL AND x.status IN ('dispatching','unknown')")?.query_map(params![grant.host_id,grant.native_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (command, turn, created) in rows {
        if captured(capture_db, &grant.native_id, &turn, &command, Some(created)) {
            receipt(&mut c, &command, "codex_accepted", None, Some(&turn))?
        }
    }
    Ok(())
}
async fn competitor_inactive() -> Result<bool> {
    let status = timeout(
        Duration::from_secs(3),
        Command::new("systemctl")
            .args([
                "--user",
                "is-active",
                "--quiet",
                "threadbridge-phone-capture.service",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await??;
    Ok(status.code() == Some(3))
}
#[cfg(unix)]
fn owner_lock(database: &Path, thread: &str) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = PathBuf::from(format!("{}.{thread}.resume.lock", database.display()));
    // Linux deployment: refuse symlink lock paths, as the prior O_NOFOLLOW lock did.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("resume_owner_running"))?;
    Ok(file)
}
pub async fn worker(
    database: &Path,
    capture_db: &Path,
    codex: &Path,
    grant: &Grant,
    execute: bool,
) -> Result<()> {
    worker_inner(
        database,
        capture_db,
        codex,
        grant,
        execute,
        None,
        None,
        Duration::from_secs(2),
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn worker_inner(
    database: &Path,
    capture_db: &Path,
    codex: &Path,
    grant: &Grant,
    execute: bool,
    competitor: Option<bool>,
    stop_after: Option<usize>,
    poll: Duration,
) -> Result<()> {
    validate_grant(grant)?;
    ensure!(execute, "execution_not_enabled");
    check_capture_policy(capture_db, &grant.native_id)?;
    ensure!(
        match competitor {
            Some(v) => v,
            None => competitor_inactive().await?,
        },
        "old_queue_worker_not_stopped"
    );
    let thread = key(&grant.host_id, &grant.native_id);
    let digest = grant_hash(grant)?;
    let _lock = owner_lock(database, &thread)?;
    let mut c = connect(database)?;
    let outcome=async {
        reconcile_completed(database,capture_db,grant)?;let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(tx.query_row("SELECT count(*) FROM devices WHERE id=? AND role='agent' AND revoked=0 AND expires>?",params![grant.host_id,now()],|r|r.get::<_,i64>(0))?>0,"host_not_authorized");
        ensure!(tx.query_row("SELECT count(*) FROM capture_queue_ledger q JOIN commands x ON x.id=q.id WHERE x.thread=? AND q.status<>'codex_accepted'",[&thread],|r|r.get::<_,i64>(0))?==0,"legacy_queue_unresolved");
        ensure!(tx.query_row("SELECT count(*) FROM commands WHERE thread=? AND status IN ('dispatching','upstream_queued','unknown')",[&thread],|r|r.get::<_,i64>(0))?==0,"unresolved_request_before_handoff");
        ensure!(tx.query_row("SELECT count(*) FROM capture_targets WHERE thread=? AND host=? AND native=?",params![thread,grant.host_id,grant.native_id],|r|r.get::<_,i64>(0))?>0,"existing_captured_target_required");
        tx.execute_batch("CREATE TABLE IF NOT EXISTS resume_owners(thread TEXT PRIMARY KEY,grant_hash TEXT NOT NULL,expires INTEGER NOT NULL,last_seen INTEGER NOT NULL DEFAULT 0);")?;
        let owner:Option<(String,i64)>=tx.query_row("SELECT grant_hash,expires FROM resume_owners WHERE thread=?",[&thread],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        ensure!(!owner.is_some_and(|o|o.0!=digest&&o.1>now()),"another_resume_owner_active");
        tx.execute("INSERT INTO resume_owners(thread,grant_hash,expires,last_seen) VALUES(?,?,?,?) ON CONFLICT(thread) DO UPDATE SET grant_hash=excluded.grant_hash,expires=excluded.expires,last_seen=excluded.last_seen",params![thread,digest,grant.expires_at,now()])?;tx.commit()?;
        let mut iterations=0;loop {
            validate_grant(grant)?;reconcile_completed(database,capture_db,grant)?;ensure!(match competitor{Some(v)=>v,None=>competitor_inactive().await?},"competing_queue_worker_started");
            {
                // Serialize the policy check and lease publication with collection mutation.
                let _publication_guard=crate::collection::lock(capture_db)?;
                require_capture_policy(capture_db,&grant.native_id)?;
                let tx=c.transaction()?;
                tx.execute("UPDATE devices SET last_seen=? WHERE id=?",params![now(),grant.host_id])?;
                tx.execute("UPDATE resume_owners SET last_seen=? WHERE thread=? AND grant_hash=?",params![now(),thread,digest])?;
                tx.execute("UPDATE capture_targets SET queue_enabled=1,last_seen=? WHERE thread=?",params![now(),thread])?;
                tx.execute("UPDATE threads SET can_send=1,status='resume_ready' WHERE id=?",[&thread])?;
                tx.commit()?;
            }
            let pending:Option<String>=c.query_row("SELECT id FROM commands WHERE thread=? AND status='accepted' AND expires>? ORDER BY created LIMIT 1",params![thread,now()],|r|r.get(0)).optional()?;
            if let Some(command)=pending{dispatch(database,capture_db,codex,grant,&command,true,Duration::from_secs((grant.expires_at-now()).clamp(1,900) as u64),Duration::from_secs(10)).await?;}
            iterations+=1;if stop_after.is_some_and(|n|iterations>=n){return Ok(())}
            tokio::select!{_=tokio::time::sleep(poll)=>{},_=tokio::signal::ctrl_c()=>{return Ok(())}}
        }
    }.await;
    let cleanup = (|| -> Result<()> {
        let tx = c.transaction()?;
        let owned: Option<String> = tx
            .query_row(
                "SELECT grant_hash FROM resume_owners WHERE thread=?",
                [&thread],
                |r| r.get(0),
            )
            .optional()?;
        if owned.as_deref() == Some(&digest) {
            tx.execute(
                "UPDATE resume_owners SET last_seen=0 WHERE thread=?",
                [&thread],
            )?;
            tx.execute(
                "UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE thread=?",
                [&thread],
            )?;
            tx.execute(
                "UPDATE threads SET can_send=0,status='capture_only' WHERE id=?",
                [&thread],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();
    // Preserve the original refusal if setup failed; shutdown errors must remain visible after success.
    outcome?;
    cleanup?;
    Ok(())
}
pub async fn run(args: ResumeArgs) -> Result<()> {
    ensure!(args.execute, "execution_not_enabled");
    let grant: Grant = serde_json::from_slice(&std::fs::read(&args.grant)?)?;
    if args.preflight {
        check_capture_policy(&args.capture_db, &grant.native_id)?;
        println!("{}", preflight(&args.db, &args.codex, &grant, true).await?)
    } else if args.worker {
        worker(&args.db, &args.capture_db, &args.codex, &grant, true).await?
    } else {
        let command = args
            .command
            .ok_or_else(|| anyhow::anyhow!("mode_required"))?;
        println!(
            "{}",
            json!({"receipt":dispatch(&args.db,&args.capture_db,&args.codex,&grant,&command,true,Duration::from_secs(900),Duration::from_secs(10)).await?})
        )
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, sync::OnceLock};
    const HOST: &str = "00000000-0000-4000-8000-000000000051";
    const NATIVE: &str = "00000000-0000-4000-8000-000000000052";
    const PRIOR: &str = "00000000-0000-4000-8000-000000000053";
    const TURN: &str = "00000000-0000-4000-8000-000000000054";
    const MOCK: &str = r#"
use serde_json::{json,Value};
use std::{io::{self,BufRead,Write},fs::{self,OpenOptions},path::PathBuf};
fn emit(row:Value){println!("{}",row);io::stdout().flush().unwrap();}
fn main(){
 let args:Vec<_>=std::env::args().collect();
 if args.get(1).map(String::as_str)==Some("--version"){println!("codex-cli fixture");return}
 if args.get(1).map(String::as_str)==Some("--hold-lock") {
 let file=OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&args[2]).unwrap();
 fs2::FileExt::lock_exclusive(&file).unwrap();println!("locked");io::stdout().flush().unwrap();std::thread::sleep(std::time::Duration::from_secs(30));return
 }
 assert_eq!(&args[1..],["app-server"]);
 let root=PathBuf::from(&args[0]).parent().unwrap().to_path_buf();
 let cfg:Value=serde_json::from_str(&fs::read_to_string(root.join("fixture.json")).unwrap()).unwrap();
 let mode=fs::read_to_string(root.join("mode")).unwrap();let native=cfg["native"].as_str().unwrap();let prior=cfg["prior"].as_str().unwrap();let turn=cfg["turn"].as_str().unwrap();
 for line in io::stdin().lock().lines(){let r:Value=serde_json::from_str(&line.unwrap()).unwrap();let method=r["method"].as_str().unwrap();let p=&r["params"];
 writeln!(OpenOptions::new().create(true).append(true).open(root.join("calls.jsonl")).unwrap(),"{}",json!({"method":method,"thread":p["threadId"],"text":p["input"][0]["text"]})).unwrap();
 if r.get("id").is_none(){continue}let id=r["id"].clone();
 let result=match method {
 "initialize"=>json!({}),
 "thread/read"|"thread/resume"=>{let original=if mode=="wrong_id"&&method=="thread/resume"{"00000000-0000-4000-8000-000000000055"}else{native};
 let mut result=json!({"thread":{"id":original,"cwd":root,"ephemeral":false,"forkedFromId":null,"status":{"type":"idle"},"canAcceptDirectInput":true},"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"readOnly","writableRoots":[],"networkAccess":false}});
 if method=="thread/resume" {match mode.as_str(){"bad_reviewer"=>result["approvalsReviewer"]=json!("auto_review"),"no_direct_input"=>result["thread"]["canAcceptDirectInput"]=json!(false),"bad_policy"=>result["sandbox"]["networkAccess"]=json!(true),_=>{}}}result},
 "thread/turns/list"=>{assert_eq!(p["itemsView"],"summary");assert_eq!(p["limit"],1);json!({"data":[{"id":if mode=="stale"{"stale"}else{prior},"status":"completed"}]})},
 "turn/start"=>{
 assert_eq!(p["threadId"],native);assert_eq!(p["approvalPolicy"],"on-request");assert_eq!(p["approvalsReviewer"],"user");assert_eq!(p["sandboxPolicy"]["networkAccess"],false);assert_eq!(p["input"][0]["text"],"synthetic phone message");
 if mode=="approval"||mode=="tool"{emit(json!({"id":"callback","method":if mode=="approval"{"item/commandExecution/requestApproval"}else{"item/tool/call"},"params":{"threadId":native}}));continue}
 emit(json!({"id":id,"result":{"turn":{"id":turn,"status":"inProgress"}}}));
 if mode=="hang"{std::thread::sleep(std::time::Duration::from_secs(30));continue}
 if mode=="wait_before_final" {
 fs::write(root.join("started"),b"yes").unwrap();
 while !root.join("continue").exists(){std::thread::sleep(std::time::Duration::from_millis(5));}
 }
 if mode=="client_final_only"||mode=="legacy_phase"||mode=="wait_before_final" {
 emit(json!({"method":"item/completed","params":{"threadId":native,"turnId":turn,"item":{"type":"agentMessage","phase":if mode!="legacy_phase"{json!("final_answer")}else{Value::Null},"text":"synthetic final"}}}));
 }else{
 let c=rusqlite::Connection::open(root.join("capture.sqlite")).unwrap();
 c.execute_batch("CREATE TABLE captured_replies(thread_id TEXT,turn_id TEXT,reply TEXT,utf8_bytes INTEGER,captured_at INTEGER DEFAULT (unixepoch()));CREATE TABLE captured_request_ids(thread_id TEXT,turn_id TEXT,request_id TEXT);").unwrap();
 c.execute("INSERT INTO captured_replies(thread_id,turn_id,reply,utf8_bytes) VALUES(?,?,'synthetic final',15)",rusqlite::params![native,turn]).unwrap();
 }
 emit(json!({"method":"turn/completed","params":{"threadId":native,"turn":{"id":if mode=="wrong_turn"{"00000000-0000-4000-8000-000000000055"}else{turn},"status":"completed"}}}));continue},
 _=>panic!("unexpected method"),};
 emit(json!({"id":id,"result":result}));
 }
}
"#;
    // Compile an immutable mock once; each fixture symlinks to it and stores only data.
    // Parallel tests never execute a just-written script or mutate process-global env.
    fn mock_binary() -> &'static Path {
        static MOCK_BINARY: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
        &MOCK_BINARY
            .get_or_init(|| {
                let dir = tempfile::tempdir().unwrap();
                let source = dir.path().join("fixture.rs");
                fs::write(&source, MOCK).unwrap();
                let binary = dir.path().join("fixture");
                let deps = std::env::current_exe()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .to_path_buf();
                let rlib = |name: &str| {
                    fs::read_dir(&deps)
                        .unwrap()
                        .map(|e| e.unwrap().path())
                        .find(|p| {
                            p.file_name()
                                .unwrap()
                                .to_string_lossy()
                                .starts_with(&format!("lib{name}-"))
                                && p.extension().is_some_and(|s| s == "rlib")
                        })
                        .unwrap()
                };
                let result = std::process::Command::new("rustc")
                    .arg("--edition=2021")
                    .arg(&source)
                    .arg("-o")
                    .arg(&binary)
                    .arg("-L")
                    .arg(format!("dependency={}", deps.display()))
                    .arg("--extern")
                    .arg(format!("serde_json={}", rlib("serde_json").display()))
                    .arg("--extern")
                    .arg(format!("rusqlite={}", rlib("rusqlite").display()))
                    .arg("--extern")
                    .arg(format!("fs2={}", rlib("fs2").display()))
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                (dir, binary)
            })
            .1
    }
    struct Fixture {
        root: tempfile::TempDir,
        db: PathBuf,
        capture: PathBuf,
        binary: PathBuf,
        command: String,
        thread: String,
        grant: Grant,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let db = root.path().join("hub.sqlite");
            let capture = root.path().join("capture.sqlite");
            let binary = root.path().join("mock-codex");
            let command = uuid::Uuid::new_v4().to_string();
            let thread = key(HOST, NATIVE);
            let grant = Grant {
                native_id: NATIVE.into(),
                host_id: HOST.into(),
                cwd: root.path().into(),
                verified_cli_version: "codex-cli fixture".into(),
                expires_at: now() + 120,
                tool_compatibility_confirmed: true,
                handoff_confirmed: true,
                approval_policy: "on-request".into(),
                sandbox_mode: Some("read-only".into()),
            };
            let c = Connection::open(&db).unwrap();
            c.execute_batch("CREATE TABLE devices(id TEXT PRIMARY KEY,role TEXT,revoked INTEGER,expires INTEGER,last_seen INTEGER DEFAULT 0);CREATE TABLE threads(id TEXT PRIMARY KEY,revision TEXT,can_send INTEGER DEFAULT 0,status TEXT);CREATE TABLE commands(id TEXT PRIMARY KEY,host TEXT,thread TEXT,status TEXT,payload TEXT,expires INTEGER,error TEXT,native_turn TEXT,created INTEGER NOT NULL DEFAULT 0);CREATE TABLE capture_queue_ledger(id TEXT PRIMARY KEY,status TEXT);CREATE TABLE events(kind TEXT,thread TEXT,created INTEGER);CREATE TABLE resume_owners(thread TEXT PRIMARY KEY,grant_hash TEXT,expires INTEGER,last_seen INTEGER DEFAULT 0);CREATE TABLE capture_targets(thread TEXT PRIMARY KEY,host TEXT,native TEXT,queue_enabled INTEGER DEFAULT 0,last_seen INTEGER DEFAULT 0);").unwrap();
            c.execute(
                "INSERT INTO devices(id,role,revoked,expires) VALUES(?,'agent',0,?)",
                params![HOST, now() + 120],
            )
            .unwrap();
            c.execute(
                "INSERT INTO threads(id,revision) VALUES(?,?)",
                params![thread, PRIOR],
            )
            .unwrap();
            let payload = json!({"id":command,"native_id":NATIVE,"thread_id":thread,"expected_revision":PRIOR,"text":"synthetic phone message","kind":"send"});
            c.execute("INSERT INTO commands(id,host,thread,status,payload,expires,created) VALUES(?,?,?,'accepted',?,?,?)",params![command,HOST,thread,payload.to_string(),now()+60,now()]).unwrap();
            c.execute(
                "INSERT INTO resume_owners(thread,grant_hash,expires) VALUES(?,?,?)",
                params![thread, grant_hash(&grant).unwrap(), now() + 120],
            )
            .unwrap();
            c.execute(
                "INSERT INTO capture_targets(thread,host,native) VALUES(?,?,?)",
                params![thread, HOST, NATIVE],
            )
            .unwrap();
            std::os::unix::fs::symlink(mock_binary(), &binary).unwrap();
            fs::write(
                root.path().join("fixture.json"),
                json!({"native":NATIVE,"prior":PRIOR,"turn":TURN}).to_string(),
            )
            .unwrap();
            fs::write(root.path().join("mode"), "ok").unwrap();
            Self {
                root,
                db,
                capture,
                binary,
                command,
                thread,
                grant,
            }
        }
        fn mode(&self, mode: &str) {
            fs::write(self.root.path().join("mode"), mode).unwrap()
        }
        fn sql(&self, sql: &str) {
            Connection::open(&self.db)
                .unwrap()
                .execute_batch(sql)
                .unwrap()
        }
        fn rows(&self) -> Vec<Value> {
            fs::read_to_string(self.root.path().join("calls.jsonl"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
        fn starts(&self) -> usize {
            self.rows()
                .iter()
                .filter(|r| r["method"] == "turn/start")
                .count()
        }
        async fn run(&self) -> Result<Receipt> {
            dispatch(
                &self.db,
                &self.capture,
                &self.binary,
                &self.grant,
                &self.command,
                true,
                Duration::from_millis(100),
                Duration::from_millis(100),
            )
            .await
        }
        async fn worker(&self, competitor: bool) -> Result<()> {
            worker_inner(
                &self.db,
                &self.capture,
                &self.binary,
                &self.grant,
                true,
                Some(competitor),
                Some(1),
                Duration::ZERO,
            )
            .await
        }
    }
    fn accepted() -> Receipt {
        ("codex_accepted".into(), None, Some(TURN.into()))
    }
    fn state(status: &str, reason: &str, receipt: Receipt) {
        assert_eq!(receipt.0, status);
        assert_eq!(receipt.1.as_deref(), Some(reason))
    }
    #[tokio::test]
    async fn preflight_never_generates_or_forks() {
        let f = Fixture::new();
        f.sql("UPDATE commands SET status='cancelled'");
        let result = preflight(&f.db, &f.binary, &f.grant, true).await.unwrap();
        assert_eq!(result["native_id"], NATIVE);
        assert_eq!(result["no_turn_started"], true);
        assert_eq!(f.starts(), 0);
        assert!(f.rows().iter().all(|r| r["method"] != "thread/fork"))
    }
    #[tokio::test]
    async fn late_final_settles_without_relaunch() {
        let f = Fixture::new();
        f.run().await.unwrap();
        f.sql("UPDATE commands SET status='unknown';UPDATE resume_dispatch_ledger SET status='unknown'");
        let calls = f.rows().len();
        reconcile_completed(&f.db, &f.capture, &f.grant).unwrap();
        assert_eq!(
            read_receipt(&Connection::open(&f.db).unwrap(), &f.command).unwrap(),
            accepted()
        );
        assert_eq!(calls, f.rows().len())
    }
    #[test]
    fn legacy_queue_turn_is_never_guessed() {
        let f = Fixture::new();
        f.sql(&format!("UPDATE commands SET status='upstream_queued';INSERT INTO capture_queue_ledger VALUES('{}','upstream_queued')",f.command));
        reconcile_completed(&f.db, &f.capture, &f.grant).unwrap();
        assert_eq!(
            read_receipt(&Connection::open(&f.db).unwrap(), &f.command).unwrap(),
            ("upstream_queued".into(), None, None)
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn active_competitor_refuses_before_spawn() {
        let f = Fixture::new();
        assert_eq!(
            f.worker(false).await.unwrap_err().to_string(),
            "old_queue_worker_not_stopped"
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn exclusive_owner_lock_refuses_second_owner() {
        let f = Fixture::new();
        let _lock = owner_lock(&f.db, &f.thread).unwrap();
        assert_eq!(
            f.worker(true).await.unwrap_err().to_string(),
            "resume_owner_running"
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn exclusive_owner_is_enforced_across_processes() {
        use std::io::{BufRead, BufReader};
        let f = Fixture::new();
        let lock_path = format!("{}.{thread}.resume.lock", f.db.display(), thread = f.thread);
        let mut child = std::process::Command::new(mock_binary())
            .args(["--hold-lock", &lock_path])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "locked");
        let result = f.worker(true).await;
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(result.unwrap_err().to_string(), "resume_owner_running");
        assert!(f.rows().is_empty());
    }

    #[tokio::test]
    async fn unresolved_request_blocks_owner_provisioning() {
        let f = Fixture::new();
        f.sql("UPDATE commands SET status='unknown'");
        assert_eq!(
            f.worker(true).await.unwrap_err().to_string(),
            "unresolved_request_before_handoff"
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn original_id_turn_input_and_idempotence() {
        let f = Fixture::new();
        assert_eq!(f.run().await.unwrap(), accepted());
        assert_eq!(f.run().await.unwrap(), accepted());
        assert_eq!(f.starts(), 1);
        assert!(f
            .rows()
            .iter()
            .all(|r| r["method"] != "thread/start" && r["method"] != "thread/fork"));
        assert_eq!(
            f.rows()
                .into_iter()
                .find(|r| r["method"] == "turn/start")
                .unwrap()["text"],
            "synthetic phone message"
        );
        let c = Connection::open(&f.capture).unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM captured_request_ids", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        )
    }
    #[tokio::test]
    async fn authoritative_turn_without_input_marker() {
        let f = Fixture::new();
        f.mode("without_marker");
        assert_eq!(f.run().await.unwrap(), accepted());
    }
    #[tokio::test]
    async fn final_phase_capture_without_notify() {
        let f = Fixture::new();
        f.mode("client_final_only");
        assert_eq!(f.run().await.unwrap(), accepted());
        assert_eq!(
            Connection::open(&f.capture)
                .unwrap()
                .query_row("SELECT reply FROM captured_replies", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "synthetic final"
        )
    }
    #[tokio::test]
    async fn unknown_phase_never_promoted() {
        let f = Fixture::new();
        f.mode("legacy_phase");
        state(
            "unknown",
            "final_capture_not_confirmed",
            f.run().await.unwrap(),
        );
        assert!(!f.capture.exists())
    }
    #[tokio::test]
    async fn identity_and_policy_gates_never_generate() {
        for (mode, reason) in [
            ("wrong_id", "original_id_mismatch"),
            ("bad_policy", "sandbox_policy_mismatch"),
            ("bad_reviewer", "approval_policy_mismatch"),
            ("no_direct_input", "direct_input_capability_unconfirmed"),
            ("stale", "original_context_revision_mismatch"),
        ] {
            let f = Fixture::new();
            f.mode(mode);
            state("rejected", reason, f.run().await.unwrap());
            assert_eq!(f.starts(), 0)
        }
    }
    #[tokio::test]
    async fn approval_and_dynamic_tool_requests_never_retry() {
        for (mode, reason) in [
            ("approval", "user_action_required"),
            ("tool", "desktop_dynamic_tool_unavailable"),
        ] {
            let f = Fixture::new();
            f.mode(mode);
            state("unknown", reason, f.run().await.unwrap());
            f.run().await.unwrap();
            assert_eq!(f.starts(), 1)
        }
    }
    #[tokio::test]
    async fn wrong_completion_is_unknown() {
        let f = Fixture::new();
        f.mode("wrong_turn");
        state(
            "unknown",
            "completion_turn_mismatch",
            f.run().await.unwrap(),
        )
    }
    #[tokio::test]
    async fn timeout_restart_never_sends_again() {
        let f = Fixture::new();
        f.mode("hang");
        state("unknown", "upstream_timeout", f.run().await.unwrap());
        f.run().await.unwrap();
        assert_eq!(f.starts(), 1)
    }
    #[tokio::test]
    async fn old_queue_unresolved_blocks_all_spawn() {
        let f = Fixture::new();
        f.sql(&format!(
            "INSERT INTO capture_queue_ledger VALUES('{}','upstream_queued')",
            f.command
        ));
        assert_eq!(
            f.run().await.unwrap_err().to_string(),
            "legacy_queue_unresolved"
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn disabled_and_unconfirmed_tool_gates() {
        let mut f = Fixture::new();
        assert_eq!(
            dispatch(
                &f.db,
                &f.capture,
                &f.binary,
                &f.grant,
                &f.command,
                false,
                Duration::ZERO,
                Duration::ZERO
            )
            .await
            .unwrap_err()
            .to_string(),
            "execution_not_enabled"
        );
        f.grant.tool_compatibility_confirmed = false;
        assert_eq!(
            f.run().await.unwrap_err().to_string(),
            "tool_compatibility_not_confirmed"
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn another_pending_command_blocks() {
        let f = Fixture::new();
        f.sql(&format!(
            "INSERT INTO commands(id,host,thread,status) VALUES('{}','{}','{}','unknown')",
            uuid::Uuid::new_v4(),
            HOST,
            f.thread
        ));
        assert_eq!(
            f.run().await.unwrap_err().to_string(),
            "thread_command_pending"
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn interrupted_intent_is_unknown_without_spawn() {
        let f = Fixture::new();
        let c = connect(&f.db).unwrap();
        c.execute("INSERT INTO resume_dispatch_ledger(id,host,native,status,created) VALUES(?,?,?,'intent',?)",params![f.command,HOST,NATIVE,now()]).unwrap();
        drop(c);
        state(
            "unknown",
            "interrupted_resume_no_retry",
            f.run().await.unwrap(),
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn grant_owner_required() {
        let f = Fixture::new();
        f.sql("DELETE FROM resume_owners");
        assert_eq!(
            f.run().await.unwrap_err().to_string(),
            "resume_owner_not_authorized"
        );
        assert!(f.rows().is_empty())
    }
    #[tokio::test]
    async fn worker_dispatches_and_releases_lease() {
        let f = Fixture::new();
        f.worker(true).await.unwrap();
        assert_eq!(f.starts(), 1);
        let c = Connection::open(&f.db).unwrap();
        assert_eq!(
            c.query_row("SELECT can_send,status FROM threads", [], |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?
            )))
            .unwrap(),
            (0, "capture_only".into())
        );
        assert_eq!(
            c.query_row(
                "SELECT queue_enabled,last_seen FROM capture_targets",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            (0, 0)
        )
    }
    #[test]
    fn grant_rejects_unknown_fields_and_permissions() {
        let f = Fixture::new();
        let mut v = serde_json::to_value(&f.grant).unwrap();
        v["extra"] = json!(true);
        assert!(serde_json::from_value::<Grant>(v).is_err());
        for (field, value, reason) in [
            (
                "sandbox_mode",
                json!("danger-full-access"),
                "unsupported_sandbox_mode",
            ),
            (
                "handoff_confirmed",
                json!(false),
                "execution_handoff_not_confirmed",
            ),
            ("expires_at", json!(0), "grant_expired"),
            (
                "approval_policy",
                json!("never"),
                "unsupported_approval_policy",
            ),
        ] {
            let mut v = serde_json::to_value(&f.grant).unwrap();
            v[field] = value;
            assert_eq!(
                validate_grant(&serde_json::from_value(v).unwrap())
                    .unwrap_err()
                    .to_string(),
                reason
            )
        }
    }
    async fn late_policy_change(disable: bool) {
        let f = Fixture::new();
        f.mode("wait_before_final");
        let initial = json!({"schema":1,"generation":uuid::Uuid::new_v4().to_string(),"cutoff_ms":1,"excluded":[]});
        crate::collection::write_policy(&f.capture, &initial).unwrap();
        let operation = dispatch(
            &f.db,
            &f.capture,
            &f.binary,
            &f.grant,
            &f.command,
            true,
            Duration::from_secs(3),
            Duration::from_millis(100),
        );
        let change = async {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !f.root.path().join("started").exists() {
                assert!(Instant::now() < deadline, "mock did not start turn");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            // Change the actual collection boundary after the authoritative turn acknowledgement.
            if disable {
                let mut policy = initial;
                policy["enabled"] = json!(false);
                crate::collection::write_policy(&f.capture, &policy).unwrap();
            } else {
                crate::collection::purge(&f.capture, &[NATIVE.into()]).unwrap();
            }
            fs::write(f.root.path().join("continue"), b"yes").unwrap();
        };
        let (result, _) = tokio::join!(operation, change);
        let value = result.unwrap();
        assert_eq!(value.2.as_deref(), Some(TURN));
        state(
            "unknown",
            if disable {
                "collection_disabled"
            } else {
                "collection_target_excluded"
            },
            value,
        );
        if f.capture.exists() {
            let c = Connection::open(&f.capture).unwrap();
            let tables: i64 = c
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='captured_replies'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if tables > 0 {
                assert_eq!(
                    c.query_row("SELECT count(*) FROM captured_replies", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                )
            }
        }
        assert!(!crate::health::path(&f.capture).exists());
        f.run().await.unwrap();
        assert_eq!(f.starts(), 1);
    }
    #[tokio::test]
    async fn deleted_target_final_never_recreates_capture_or_retries() {
        late_policy_change(false).await
    }
    #[tokio::test]
    async fn disabled_collection_final_never_saves_or_retries() {
        late_policy_change(true).await
    }
    #[tokio::test]
    async fn already_excluded_target_never_launches() {
        let f = Fixture::new();
        let policy = json!({"schema":1,"generation":uuid::Uuid::new_v4().to_string(),"cutoff_ms":1,"excluded":[NATIVE]});
        crate::collection::write_policy(&f.capture, &policy).unwrap();
        state(
            "rejected",
            "collection_target_excluded",
            f.run().await.unwrap(),
        );
        assert!(f.rows().is_empty());
        assert_eq!(
            f.worker(true).await.unwrap_err().to_string(),
            "collection_target_excluded"
        );
    }

    #[tokio::test]
    async fn policy_disable_stops_worker_and_releases_advertised_lease() {
        let f = Fixture::new();
        let policy = json!({"schema":1,"generation":uuid::Uuid::new_v4().to_string(),"cutoff_ms":1,"excluded":[]});
        crate::collection::write_policy(&f.capture, &policy).unwrap();
        let operation = worker_inner(
            &f.db,
            &f.capture,
            &f.binary,
            &f.grant,
            true,
            Some(true),
            None,
            Duration::from_millis(10),
        );
        let disable = async {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let c = Connection::open(&f.db).unwrap();
                if read_receipt(&c, &f.command).unwrap().0 == "codex_accepted" {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "worker did not dispatch fixture command"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let mut policy = policy;
            policy["enabled"] = json!(false);
            crate::collection::write_policy(&f.capture, &policy).unwrap();
        };
        let (result, _) = tokio::time::timeout(Duration::from_secs(4), async {
            tokio::join!(operation, disable)
        })
        .await
        .expect("disabled worker must stop promptly");
        assert_eq!(result.unwrap_err().to_string(), "collection_disabled");
        assert_eq!(f.starts(), 1);
        let c = Connection::open(&f.db).unwrap();
        assert_eq!(
            c.query_row("SELECT can_send,status FROM threads", [], |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?
            )))
            .unwrap(),
            (0, "capture_only".into())
        );
        assert_eq!(
            c.query_row(
                "SELECT queue_enabled,last_seen FROM capture_targets",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            (0, 0)
        );
        assert_eq!(
            c.query_row("SELECT last_seen FROM resume_owners", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn cli_preflight_refuses_excluded_handoff_before_launch() {
        let f = Fixture::new();
        let policy = json!({"schema":1,"generation":uuid::Uuid::new_v4().to_string(),"cutoff_ms":1,"excluded":[NATIVE]});
        crate::collection::write_policy(&f.capture, &policy).unwrap();
        let grant_path = f.root.path().join("grant.json");
        fs::write(&grant_path, serde_json::to_vec(&f.grant).unwrap()).unwrap();
        let args = ResumeArgs {
            db: f.db.clone(),
            capture_db: f.capture.clone(),
            codex: f.binary.clone(),
            grant: grant_path,
            command: None,
            worker: false,
            preflight: true,
            execute: true,
        };
        assert_eq!(
            run(args).await.unwrap_err().to_string(),
            "collection_target_excluded"
        );
        assert!(f.rows().is_empty());
    }

    #[test]
    fn canonical_grant_hash_matches_existing_unicode_grants() {
        let f = Fixture::new();
        let mut grant = f.grant;
        grant.cwd = PathBuf::from("/tmp/续桥😀");
        grant.expires_at = 123;
        assert_eq!(
            grant_hash(&grant).unwrap(),
            "924b194fd7a41b76fbdaed7a034b2f52552bab4d96f0ca302f8238747bab3797"
        );
    }

    #[test]
    fn accepted_receipt_cannot_downgrade() {
        let f = Fixture::new();
        let mut c = connect(&f.db).unwrap();
        receipt(&mut c, &f.command, "codex_accepted", None, Some(TURN)).unwrap();
        receipt(&mut c, &f.command, "unknown", Some("late_error"), None).unwrap();
        assert_eq!(read_receipt(&c, &f.command).unwrap(), accepted())
    }
}
