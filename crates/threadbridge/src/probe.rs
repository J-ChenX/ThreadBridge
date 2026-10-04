use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::Command,
};

const MAX_FRAME: usize = 256 * 1024;
const MAX_NOTIFICATIONS: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(3);

// Delegate control-socket transport to the installed official proxy.
// Do not assume the daemon control socket and --listen unix:// are identical.
async fn response<R: AsyncBufRead + Unpin>(reader: &mut R, id: u64) -> Result<Value, String> {
    for _ in 0..=MAX_NOTIFICATIONS {
        let mut bytes = Vec::new();
        let count = (&mut *reader)
            .take((MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| "response read failed")?;
        if count > MAX_FRAME {
            return Err("response exceeds 256 KiB; no content imported".into());
        }
        if bytes.last() != Some(&b'\n') {
            return Err("proxy closed before response; existing endpoint unavailable".into());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid JSON response")?;
        if value.get("id") == Some(&json!(id)) {
            if value.get("error").is_some() {
                return Err("upstream rejected read-only request".into());
            }
            return value.get("result").cloned().ok_or("missing result".into());
        }
        // Requests from the upstream need a client response and must not be
        // silently treated as notifications or answered with invented identity.
        if value.get("id").is_some() || !value.get("method").is_some_and(Value::is_string) {
            return Err("unexpected RPC envelope".into());
        }
    }
    Err("notification budget exceeded".into())
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, value: Value) -> Result<(), String> {
    let mut data = serde_json::to_vec(&value).map_err(|_| "cannot encode request")?;
    data.push(b'\n');
    writer
        .write_all(&data)
        .await
        .map_err(|_| "request write failed".into())
}

async fn request<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    tokio::time::timeout(TIMEOUT, async {
        send(writer, json!({"id":id,"method":method,"params":params})).await?;
        response(reader, id).await
    })
    .await
    .map_err(|_| "request deadline exceeded".to_string())?
}

async fn inspect<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    thread: Option<&str>,
) -> Result<Value, String> {
    let init = request(
        reader,
        writer,
        1,
        "initialize",
        json!({
            "clientInfo":{"name":"threadbridge_probe","version":env!("CARGO_PKG_VERSION")},
            "capabilities":{"experimentalApi":true}
        }),
    )
    .await?;
    if !init.is_object() {
        return Err("invalid initialization result".into());
    }
    tokio::time::timeout(
        TIMEOUT,
        send(writer, json!({"method":"initialized","params":{}})),
    )
    .await
    .map_err(|_| "request deadline exceeded".to_string())??;
    let list = request(
        reader,
        writer,
        2,
        "thread/list",
        json!({
            "limit":1,"useStateDbOnly":true,"sourceKinds":["appServer","cli","vscode"]
        }),
    )
    .await?;
    let count = list
        .get("data")
        .and_then(Value::as_array)
        .ok_or("missing thread list")?
        .len();
    let mut target_verified = false;
    let mut target_loaded = None;
    if let Some(id) = thread {
        let read = request(
            reader,
            writer,
            3,
            "thread/read",
            json!({"threadId":id,"includeTurns":false}),
        )
        .await?;
        if read.pointer("/thread/id").and_then(Value::as_str) != Some(id) {
            return Err("target identity mismatch".into());
        }
        target_verified = true;
        let loaded = request(reader, writer, 4, "thread/loaded/list", json!({})).await?;
        let ids = loaded
            .get("data")
            .and_then(Value::as_array)
            .ok_or("missing loaded thread list")?;
        if !ids.iter().all(Value::is_string) {
            return Err("invalid loaded thread list".into());
        }
        target_loaded = Some(ids.iter().any(|v| v.as_str() == Some(id)));
    }
    Ok(json!({
        "initialize":true,"list_page_count":count,"transport":"official_proxy",
        "target_identity_verified":target_verified,"target_loaded":target_loaded,
        "writes_enabled":false,
        "remaining":["desktop_executor_identity","full_reply_pagination","same_thread_continuation","desktop_restart","other_hosts"]
    }))
}

pub async fn probe(socket: &Path, thread: Option<&str>) -> Result<Value, String> {
    // Attach only. Neither proxy nor this probe starts a server, resumes a
    // persisted task, submits a turn, or uses the desktop's private tool pipe.
    if !socket.exists() {
        return Err(
            "existing endpoint unavailable: socket does not exist; no daemon started".into(),
        );
    }
    let mut child = Command::new("codex")
        .args(["app-server", "proxy", "--sock"])
        .arg(socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "cannot launch installed codex proxy")?;
    let mut input = child.stdin.take().ok_or("missing proxy input")?;
    let mut output = BufReader::new(child.stdout.take().ok_or("missing proxy output")?);
    let result = inspect(&mut output, &mut input, thread).await;
    // This is only our short-lived proxy, never the upstream app-server.
    let _ = child.start_kill();
    let _ = tokio::time::timeout(TIMEOUT, child.wait()).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn receive(bytes: &[u8]) -> Result<Value, String> {
        let mut reader = BufReader::new(bytes);
        response(&mut reader, 7).await
    }
    #[tokio::test]
    async fn skips_notification_and_reads_matching_response() {
        assert_eq!(
            receive(b"{\"method\":\"notice\"}\n{\"id\":7,\"result\":{\"ok\":true}}\n")
                .await
                .unwrap(),
            json!({"ok":true})
        );
    }
    #[tokio::test]
    async fn does_not_leak_upstream_error() {
        assert!(!receive(b"{\"id\":7,\"error\":{\"message\":\"secret\"}}\n")
            .await
            .unwrap_err()
            .contains("secret"));
    }
    #[tokio::test]
    async fn rejects_oversize_incomplete_and_invalid_frames() {
        assert!(receive(&vec![b'x'; MAX_FRAME + 1])
            .await
            .unwrap_err()
            .contains("exceeds"));
        for bytes in [
            b"{}".as_slice(),
            b"not json\n",
            b"{\"id\":8,\"result\":{}}\n",
        ] {
            assert!(receive(bytes).await.is_err());
        }
    }
    #[tokio::test]
    async fn rejects_notification_flood() {
        assert!(
            receive(&b"{\"method\":\"notice\"}\n".repeat(MAX_NOTIFICATIONS + 1))
                .await
                .unwrap_err()
                .contains("budget")
        );
    }
    #[tokio::test]
    async fn silent_peer_times_out() {
        let (client, _peer) = tokio::io::duplex(64);
        let (read, mut write) = tokio::io::split(client);
        let mut read = BufReader::new(read);
        let result = request(&mut read, &mut write, 1, "initialize", json!({})).await;
        assert_eq!(result.unwrap_err(), "request deadline exceeded");
    }
    #[tokio::test]
    async fn missing_endpoint_does_not_start_daemon() {
        let dir = tempfile::tempdir().unwrap();
        assert!(probe(&dir.path().join("absent.sock"), None)
            .await
            .unwrap_err()
            .contains("no daemon started"));
    }
    #[tokio::test]
    async fn handshake_checks_loaded_identity_without_mutations() {
        let replies = b"{\"id\":1,\"result\":{}}\n{\"id\":2,\"result\":{\"data\":[]}}\n{\"id\":3,\"result\":{\"thread\":{\"id\":\"task\"}}}\n{\"id\":4,\"result\":{\"data\":[\"task\"]}}\n";
        let mut reader = BufReader::new(replies.as_slice());
        let mut writes = Vec::new();
        let result = inspect(&mut reader, &mut writes, Some("task"))
            .await
            .unwrap();
        assert_eq!(result["target_loaded"], true);
        assert_eq!(result["writes_enabled"], false);
        let methods: Vec<String> = std::str::from_utf8(&writes)
            .unwrap()
            .lines()
            .map(|s| {
                serde_json::from_str::<Value>(s).unwrap()["method"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "initialized",
                "thread/list",
                "thread/read",
                "thread/loaded/list"
            ]
        );
    }
    #[tokio::test]
    async fn rejects_wrong_target_identity() {
        let mut reader = BufReader::new(b"{\"id\":1,\"result\":{}}\n{\"id\":2,\"result\":{\"data\":[]}}\n{\"id\":3,\"result\":{\"thread\":{\"id\":\"other\"}}}\n".as_slice());
        assert_eq!(
            inspect(&mut reader, &mut Vec::new(), Some("task"))
                .await
                .unwrap_err(),
            "target identity mismatch"
        );
    }
    #[tokio::test]
    #[ignore = "requires the installed Codex CLI; uses only an isolated control fixture socket"]
    async fn installed_proxy_attaches_to_isolated_control_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let mut methods = Vec::new();
            loop {
                let mut text = String::new();
                if reader.read_line(&mut text).await.unwrap() == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(&text).unwrap();
                let method = request["method"].as_str().unwrap().to_string();
                methods.push(method.clone());
                let result = match method.as_str() {
                    "initialize" => json!({"userAgent":"fixture"}),
                    "initialized" => continue,
                    "thread/list" => json!({"data":[]}),
                    "thread/read" => json!({"thread":{"id":"fixture-task"}}),
                    "thread/loaded/list" => json!({"data":["fixture-task"]}),
                    _ => panic!("unexpected non-read operation"),
                };
                send(&mut write, json!({"id":request["id"],"result":result}))
                    .await
                    .unwrap();
                if method == "thread/loaded/list" {
                    break;
                }
            }
            methods
        });
        let result = probe(&path, Some("fixture-task")).await.unwrap();
        assert_eq!(result["target_loaded"], true);
        assert_eq!(result["writes_enabled"], false);
        assert_eq!(server.await.unwrap().len(), 5);
    }
}
