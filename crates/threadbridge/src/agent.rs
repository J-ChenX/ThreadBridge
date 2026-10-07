use crate::{adapter::Adapter, model::*, store::Store};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, OptionalExtension};
use std::{path::PathBuf, time::Duration};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Message},
};

#[derive(Clone)]
pub struct AgentConfig {
    pub hub: String,
    pub token: String,
    pub host: String,
    pub db: PathBuf,
    pub codex: PathBuf,
    pub socket: Option<PathBuf>,
    pub verified_version: Option<String>,
    pub demo: bool,
    pub local: Option<Store>,
}
fn demo(host: &str) -> Snapshot {
    let text="欢迎使用续桥 · ThreadBridge\n\n这是隔离的演示任务，用于验证 APK、同步和发送账本。它不会运行 Codex，也不会修改你的项目。\n\n正式任务需要电脑端连接到已有 Codex 服务。";
    Snapshot {
        thread: Thread {
            id: key(host, "demo"),
            native_id: "demo".into(),
            host_id: host.into(),
            title: "演示 · 手机接着聊".into(),
            status: "idle".into(),
            revision: "demo-0".into(),
            updated_at: now(),
            can_send: true,
            history_cursor: None,
            project: String::new(),
        },
        messages: vec![ChatMessage {
            id: "welcome".into(),
            turn_id: "demo-0".into(),
            role: "assistant".into(),
            text: text.into(),
            version: hash(text),
            ordinal: 1,
        }],
        initial: true,
        history: false,
    }
}
type Remote =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
enum Link {
    Remote(Box<Remote>),
    Local(Store, String),
}
impl Link {
    async fn next(&mut self) -> Result<Option<HubFrame>> {
        match self {
            Self::Remote(ws) => loop {
                let Some(value) = ws.next().await else {
                    return Ok(None);
                };
                let value = value?;
                if value.is_close() {
                    return Ok(None);
                }
                if value.is_text() {
                    return Ok(Some(serde_json::from_str(value.to_text()?)?));
                }
            },
            Self::Local(db, host) => loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if !db.valid_device(host) {
                    return Ok(None);
                }
                if let Some(command) = db.claim(host)? {
                    return Ok(Some(HubFrame::Command { command }));
                }
            },
        }
    }
}
async fn emit(link: &mut Link, frame: AgentFrame) -> Result<()> {
    match link {
        Link::Remote(ws) => {
            let data = serde_json::to_string(&frame)?;
            anyhow::ensure!(data.len() <= FRAME_LIMIT, "frame_limit");
            tokio::time::timeout(Duration::from_secs(5), ws.send(Message::Text(data.into())))
                .await??;
        }
        Link::Local(db, host) => {
            anyhow::ensure!(db.valid_device(host), "agent_revoked");
            match frame {
                AgentFrame::Snapshot { snapshot } => db.snapshot(host, &snapshot)?,
                AgentFrame::Receipt {
                    id,
                    status,
                    native_turn_id,
                    error,
                } => db.receipt(
                    host,
                    &id,
                    &status,
                    native_turn_id.as_deref(),
                    error.as_deref(),
                )?,
                AgentFrame::Deleted {
                    native_id,
                    message_id,
                } => db.delete_source(host, &native_id, message_id.as_deref())?,
                AgentFrame::Heartbeat | AgentFrame::Hello { .. } => db.heartbeat(host)?,
            }
        }
    }
    Ok(())
}
pub async fn run(config: AgentConfig) -> Result<()> {
    let url = reqwest::Url::parse(&config.hub)?;
    anyhow::ensure!(
        url.scheme() == "https"
            || url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        "agent requires HTTPS except loopback"
    );
    let ledger = if let Some(db) = &config.local {
        db.clone()
    } else {
        Store::open(&config.db)?
    };
    ledger.0.lock().unwrap().execute(
        "UPDATE agent_ledger SET status='unknown' WHERE status='intent'",
        [],
    )?;
    let mut delay = 1;
    loop {
        if let Err(e) = session(&config, &ledger).await {
            eprintln!(
                "Agent disconnected: {}",
                e.to_string().chars().take(120).collect::<String>()
            )
        }
        if let Some(db) = &config.local {
            let _ = db.disconnected(&config.host);
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}
async fn session(config: &AgentConfig, ledger: &Store) -> Result<()> {
    let mut adapter = if config.demo {
        None
    } else {
        Some(
            Adapter::connect(
                &config.codex,
                config.socket.as_deref(),
                config.verified_version.as_deref(),
            )
            .await?,
        )
    };
    let mut ws = if let Some(db) = &config.local {
        anyhow::ensure!(
            db.authenticate(&config.token, "agent")? == config.host,
            "agent_identity_mismatch"
        );
        Link::Local(db.clone(), config.host.clone())
    } else {
        let endpoint = format!("{}/v1/agent/ws", config.hub.trim_end_matches('/'))
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let mut req = endpoint.into_client_request()?;
        req.headers_mut()
            .insert("Authorization", format!("Bearer {}", config.token).parse()?);
        let limits = WebSocketConfig::default()
            .max_message_size(Some(FRAME_LIMIT))
            .max_frame_size(Some(FRAME_LIMIT));
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(12),
            connect_async_with_config(req, Some(limits), false),
        )
        .await??;
        Link::Remote(Box::new(socket))
    };
    emit(
        &mut ws,
        AgentFrame::Hello {
            protocol: PROTOCOL,
            adapter: adapter
                .as_ref()
                .map(|a| a.version.clone())
                .unwrap_or("demo (no Codex)".into()),
        },
    )
    .await?;
    if matches!(ws, Link::Remote(_)) {
        let welcome = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await??
            .context("no_welcome")?;
        anyhow::ensure!(
            matches!(welcome, HubFrame::Welcome { protocol: PROTOCOL }),
            "protocol_mismatch"
        );
    }
    // A interrupted intent is never re-executed, including reconnects in this process.
    ledger.0.lock().unwrap().execute(
        "UPDATE agent_ledger SET status='unknown' WHERE status='intent'",
        [],
    )?;
    // Replay every receipt in bounded batches; old unresolved receipts must not starve.
    let mut receipt_cursor = 0;
    loop {
        let receipts = {
            let c = ledger.0.lock().unwrap();
            let mut q=c.prepare("SELECT id,status,native_turn,error,rowid FROM agent_ledger WHERE status!='intent' AND rowid>?1 ORDER BY rowid LIMIT 128")?;
            let rows = q
                .query_map([receipt_cursor], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        if receipts.is_empty() {
            break;
        }
        for (id, status, native_turn_id, error, rowid) in receipts {
            receipt_cursor = rowid;
            emit(
                &mut ws,
                AgentFrame::Receipt {
                    id,
                    status,
                    native_turn_id,
                    error,
                },
            )
            .await?;
        }
    }
    let mut demo_snapshot = demo(&config.host);
    let mut list_cursor: Option<String> = None;
    let mut observed = std::collections::HashMap::<String, String>::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
         _=tick.tick()=>{
          emit(&mut ws,AgentFrame::Heartbeat).await?;
          if let Some(adapter)=adapter.as_mut(){let list=adapter.list(list_cursor.as_deref()).await?;list_cursor=list["nextCursor"].as_str().map(str::to_string);
           for t in list["data"].as_array().into_iter().flatten(){if let Some(id)=t["id"].as_str(){let stamp=format!("{}:{}",t["updatedAt"],t["status"]);if observed.get(id)==Some(&stamp){continue}emit(&mut ws,AgentFrame::Heartbeat).await?;let s=adapter.snapshot(&config.host,id,None,!observed.contains_key(id)).await?;emit(&mut ws,AgentFrame::Snapshot{snapshot:s}).await?;if observed.len()>=200{observed.clear();}observed.insert(id.into(),stamp);}}
          }else{emit(&mut ws,AgentFrame::Snapshot{snapshot:demo_snapshot.clone()}).await?;}
          demo_snapshot.initial=false;
         }
         message=ws.next()=>{let Some(HubFrame::Command{command:cmd})=message? else{break};
          let old=ledger.0.lock().unwrap().query_row("SELECT status,native_turn,error FROM agent_ledger WHERE id=?1",[&cmd.id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
          let (status,native_turn_id,error)=if let Some(old)=old{old}else if cmd.expires_at<now(){("rejected".into(),None,Some("expired".into()))}else{
           let current=if let Some(a)=adapter.as_mut(){a.snapshot(&config.host,&cmd.native_id,None,false).await?}else{demo_snapshot.clone()};
           if cmd.kind=="history"{
            if let Some(a)=adapter.as_mut(){let mut s=a.snapshot(&config.host,&cmd.native_id,cmd.cursor.as_deref(),true).await?;s.thread.revision=current.thread.revision;s.thread.status=current.thread.status;s.thread.can_send=current.thread.can_send;emit(&mut ws,AgentFrame::Snapshot{snapshot:s}).await?;}("history_loaded".into(),None,None)
           }else if !current.thread.can_send||current.thread.revision!=cmd.expected_revision||!["idle","completed"].contains(&current.thread.status.as_str()){
            ("rejected".into(),None,Some("state_changed_or_unverified".into()))
           }else{
            ledger.0.lock().unwrap().execute("INSERT INTO agent_ledger(id,status) VALUES(?1,'intent')",[&cmd.id])?;
            if let Some(a)=adapter.as_mut(){match a.send(&cmd).await{Ok(id)=>("codex_accepted".into(),Some(id),None),Err(crate::adapter::SendError::NotDispatched(_))=>("rejected".into(),None,Some("upstream_preflight_rejected".into())),Err(crate::adapter::SendError::Unknown(_))=>("unknown".into(),None,Some("upstream_result_unknown".into()))}}else{
             let turn=format!("demo-{}",cmd.id);let answer=format!("演示已收到：{}\n\n发送、落盘与同步链路正常。这是演示回执，未调用 Codex。",cmd.text);let n=now()*1000;demo_snapshot.messages=vec![ChatMessage{id:format!("u-{}",cmd.id),turn_id:turn.clone(),role:"user".into(),version:hash(&cmd.text),text:cmd.text.clone(),ordinal:n},ChatMessage{id:format!("a-{}",cmd.id),turn_id:turn.clone(),role:"assistant".into(),version:hash(&answer),text:answer,ordinal:n+1}];demo_snapshot.thread.revision=turn.clone();demo_snapshot.thread.status="completed".into();demo_snapshot.thread.updated_at=now();demo_snapshot.initial=false;emit(&mut ws,AgentFrame::Snapshot{snapshot:demo_snapshot.clone()}).await?;("codex_accepted".into(),Some(turn),None)
            }
           }
          };
          ledger.0.lock().unwrap().execute("INSERT INTO agent_ledger(id,status,native_turn,error) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET status=excluded.status,native_turn=excluded.native_turn,error=excluded.error",params![cmd.id,status,native_turn_id,error])?;
          emit(&mut ws,AgentFrame::Receipt{id:cmd.id,status,native_turn_id,error}).await?;
         }
        }
    }
    Ok(())
}
