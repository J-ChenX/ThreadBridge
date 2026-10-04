use crate::{model::*, store::Store};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct Hub {
    pub db: Store,
    connections: Arc<Semaphore>,
    hosts: Arc<Mutex<HashSet<String>>>,
    pair_rate: Arc<Mutex<(i64, u32)>>,
}
type ApiResult = Result<Json<Value>, ApiError>;
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn bad(e: anyhow::Error) -> ApiError {
    let text = e.to_string();
    let known = [
        "invalid_request",
        "invalid_pairing",
        "idempotency_conflict",
        "restore_read_only",
        "host_offline",
        "not_ready",
        "revision_conflict",
        "queue_full",
        "thread_command_pending",
    ];
    if known.contains(&text.as_str()) {
        ApiError(StatusCode::CONFLICT, text)
    } else {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_or_resource_error".into(),
        )
    }
}
fn auth(h: &HeaderMap, s: &Hub, role: &str) -> Result<String, ApiError> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized".into()))?;
    s.db.authenticate(token, role)
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "unauthorized".into()))
}
#[derive(Deserialize)]
struct Pair {
    code: String,
    name: String,
}
async fn pair(State(s): State<Hub>, Json(p): Json<Pair>) -> ApiResult {
    let mut rate = s.pair_rate.lock().unwrap();
    if now() - rate.0 > 60 {
        *rate = (now(), 0)
    }
    rate.1 += 1;
    if rate.1 > 20 {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited".into(),
        ));
    }
    drop(rate);
    if p.code.len() != 64 || p.name.len() > 100 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_pairing".into()));
    }
    s.db.pair(&p.code, &p.name).map(Json).map_err(bad)
}
async fn health() -> Json<Value> {
    Json(json!({"status":"ok","protocol":PROTOCOL}))
}
#[derive(Deserialize, Default)]
struct Paging {
    offset: Option<i64>,
    before: Option<i64>,
    before_id: Option<String>,
    after: Option<i64>,
    version: Option<String>,
}
async fn hosts(State(s): State<Hub>, h: HeaderMap) -> ApiResult {
    auth(&h, &s, "phone")?;
    s.db.hosts().map(Json).map_err(bad)
}
async fn capture_health(State(s): State<Hub>, h: HeaderMap) -> ApiResult {
    auth(&h, &s, "phone")?;
    let c = s.db.0.lock().unwrap();
    let server_time = now();
    let mut q=c.prepare("SELECT h.host,h.status,h.checked_at,d.last_seen,d.expires FROM capture_health h JOIN devices d ON d.id=h.host WHERE d.role='agent' AND d.revoked=0").map_err(|e|bad(e.into()))?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| bad(e.into()))?;
    let mut hosts = Vec::new();
    let mut offline_hosts = Vec::new();
    for row in rows {
        let (host, status, checked_at, last_seen, expires) = row.map_err(|e| bad(e.into()))?;
        let status: Value = serde_json::from_str(&status).map_err(|e| bad(e.into()))?;
        let online = last_seen > server_time - 15 && expires > server_time;
        let unresolved = status["failures"]
            .as_object()
            .is_none_or(|failures| !failures.is_empty())
            || status["overflow"] == true
            || status.get("projection_error").is_some();
        // A stopped remote monitor's last healthy snapshot is an offline device,
        // not an unconfirmed capture on another active computer. Keep its original
        // timestamp and state separately; unresolved failures are always visible.
        let row = json!({"host_id":host,"status":status,"checked_at":checked_at,"online":online});
        if !online && server_time - checked_at > 30 && !unresolved {
            offline_hosts.push(row);
        } else {
            hosts.push(row);
        }
    }
    Ok(Json(
        json!({"hosts":hosts,"offline_hosts":offline_hosts,"server_time":server_time}),
    ))
}
async fn threads(State(s): State<Hub>, h: HeaderMap, Query(q): Query<Paging>) -> ApiResult {
    auth(&h, &s, "phone")?;
    s.db.threads(q.offset.unwrap_or(0).clamp(0, 100000))
        .map(Json)
        .map_err(bad)
}
async fn messages(
    State(s): State<Hub>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Paging>,
) -> ApiResult {
    auth(&h, &s, "phone")?;
    s.db.messages_page(&id, q.before.unwrap_or(i64::MAX), q.before_id.as_deref())
        .map(Json)
        .map_err(bad)
}
async fn chunk(
    State(s): State<Hub>,
    h: HeaderMap,
    Path((id, msg)): Path<(String, String)>,
    Query(q): Query<Paging>,
) -> ApiResult {
    auth(&h, &s, "phone")?;
    s.db.chunk(
        &id,
        &msg,
        q.version.as_deref().unwrap_or(""),
        q.offset.unwrap_or(0).clamp(0, 10 * 1024 * 1024),
    )
    .map(Json)
    .map_err(|_| ApiError(StatusCode::CONFLICT, "message_version_changed".into()))
}
async fn image(
    State(s): State<Hub>,
    h: HeaderMap,
    Path((id, msg, image)): Path<(String, String, String)>,
) -> ApiResult {
    auth(&h, &s, "phone")?;
    s.db.image(&id, &msg, &image)
        .map(Json)
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "image_unavailable".into()))
}
async fn submit(State(s): State<Hub>, h: HeaderMap, Json(p): Json<Submit>) -> ApiResult {
    let d = auth(&h, &s, "phone")?;
    s.db.submit(&d, &p).map(Json).map_err(bad)
}
async fn delete_copy(State(s): State<Hub>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    auth(&h, &s, "phone")?;
    s.db.delete_copy(&id).map(Json).map_err(bad)
}
async fn command(State(s): State<Hub>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    let d = auth(&h, &s, "phone")?;
    s.db.command(&d, &id)
        .map(Json)
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "command_not_found".into()))
}
async fn cancel(State(s): State<Hub>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    let d = auth(&h, &s, "phone")?;
    s.db.cancel(&d, &id)
        .map(Json)
        .map_err(|_| ApiError(StatusCode::CONFLICT, "cannot_cancel_dispatched".into()))
}
async fn events(State(s): State<Hub>, h: HeaderMap, Query(q): Query<Paging>) -> ApiResult {
    auth(&h, &s, "phone")?;
    s.db.events(q.after.unwrap_or(0)).map(Json).map_err(bad)
}
async fn event_socket(
    State(s): State<Hub>,
    h: HeaderMap,
    ws: WebSocketUpgrade,
    Query(q): Query<Paging>,
) -> Result<Response, ApiError> {
    let device = auth(&h, &s, "phone")?;
    let permit = s
        .connections
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "connections_full".into()))?;
    Ok(ws.max_message_size(FRAME_LIMIT).max_frame_size(FRAME_LIMIT).on_upgrade(move|mut socket|async move{let _permit=permit;let mut cursor=q.after.unwrap_or(0);let mut tick=tokio::time::interval(Duration::from_secs(2));loop{tokio::select!{_=tick.tick()=>{if !s.db.valid_device(&device){break}let Ok(e)=s.db.events(cursor)else{break};cursor=e["cursor"].as_i64().unwrap_or(cursor);if !matches!(tokio::time::timeout(Duration::from_secs(3),socket.send(Message::Text(e.to_string().into()))).await,Ok(Ok(()))){break}},v=socket.recv()=>{if !matches!(v,Some(Ok(Message::Ping(_)|Message::Pong(_)|Message::Text(_)))){break}}}}}))
}
async fn agent_socket(
    State(s): State<Hub>,
    h: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let host = auth(&h, &s, "agent")?;
    let permit = s
        .connections
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "connections_full".into()))?;
    {
        let mut hosts = s.hosts.lock().unwrap();
        if hosts.len() >= 4 || !hosts.insert(host.clone()) {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "host_already_connected".into(),
            ));
        }
    }
    Ok(ws
        .max_message_size(FRAME_LIMIT)
        .max_frame_size(FRAME_LIMIT)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            agent_session(s.clone(), host.clone(), socket).await;
            let _ = s.db.disconnected(&host);
            s.hosts.lock().unwrap().remove(&host);
        }))
}
async fn agent_session(s: Hub, host: String, mut socket: WebSocket) {
    let hello = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
    if !matches!(hello,Ok(Some(Ok(Message::Text(ref v)))) if matches!(serde_json::from_str::<AgentFrame>(v),Ok(AgentFrame::Hello{protocol:PROTOCOL,..})))
    {
        return;
    }
    let _ = socket
        .send(Message::Text(
            serde_json::to_string(&HubFrame::Welcome { protocol: PROTOCOL })
                .unwrap()
                .into(),
        ))
        .await;
    let _ = s.db.heartbeat(&host);
    let mut last = now();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
         _=interval.tick()=>{
          if !s.db.valid_device(&host)||now()-last>45{break}
          if let Ok(Some(command))=s.db.claim(&host){let frame=serde_json::to_string(&HubFrame::Command{command}).unwrap();if !matches!(tokio::time::timeout(Duration::from_secs(3),socket.send(Message::Text(frame.into()))).await,Ok(Ok(()))){break}}
         }
         msg=socket.recv()=>{let Some(Ok(Message::Text(v)))=msg else{break};let Ok(frame)=serde_json::from_str::<AgentFrame>(&v)else{break};last=now();let _=s.db.heartbeat(&host);
          let result=match frame{AgentFrame::Snapshot{snapshot}=>s.db.snapshot(&host,&snapshot),AgentFrame::Receipt{id,status,native_turn_id,error}=>s.db.receipt(&host,&id,&status,native_turn_id.as_deref(),error.as_deref()),AgentFrame::Deleted{native_id,message_id}=>s.db.delete_source(&host,&native_id,message_id.as_deref()),AgentFrame::Heartbeat=>Ok(()),_=>break};if result.is_err(){break}
         }
        }
    }
}
pub async fn run(db: Store, listen: &str) -> anyhow::Result<()> {
    let s = Hub {
        db,
        connections: Arc::new(Semaphore::new(12)),
        hosts: Default::default(),
        pair_rate: Arc::new(Mutex::new((now(), 0))),
    };
    let router = Router::new()
        .route("/health", get(health))
        .route("/v1/pair", post(pair))
        .route("/v1/hosts", get(hosts))
        .route("/v1/threads", get(threads))
        .route("/v1/threads/{id}/delete", post(delete_copy))
        .route("/v1/threads/{id}/messages/{msg}/images/{image}", get(image))
        .route("/v1/capture-health", get(capture_health))
        .route("/v1/threads/{id}/messages", get(messages))
        .route("/v1/threads/{id}/messages/{msg}/body", get(chunk))
        .route("/v1/commands", post(submit))
        .route("/v1/commands/{id}", get(command))
        .route("/v1/commands/{id}/cancel", post(cancel))
        .route("/v1/events", get(events))
        .route("/v1/events/ws", get(event_socket))
        .route("/v1/agent/ws", get(agent_socket))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(tower::limit::ConcurrencyLimitLayer::new(32))
        .with_state(s);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!("ThreadBridge Hub listening on {listen}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
