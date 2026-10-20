//! Attach to the installed official proxy. New Unix listeners speak WebSocket;
//! older control endpoints speak JSONL. Negotiation never sends a model request.
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::{
    path::Path,
    pin::Pin,
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{
        AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
        DuplexStream, ReadBuf, ReadHalf, WriteHalf,
    },
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};
use tokio_tungstenite::{
    client_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    WebSocketStream,
};

const LIMIT: usize = crate::model::FRAME_LIMIT;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);

struct ProxyIo {
    input: ChildStdin,
    output: ChildStdout,
}
impl AsyncRead for ProxyIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.output).poll_read(cx, buf)
    }
}
impl AsyncWrite for ProxyIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.input).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.input).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.input).poll_shutdown(cx)
    }
}

pub struct Proxy {
    child: Child,
    bridge: JoinHandle<Result<()>>,
    pub input: WriteHalf<DuplexStream>,
    pub output: BufReader<ReadHalf<DuplexStream>>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub transport: &'static str,
}

fn spawn(codex: &Path, socket: Option<&Path>, home: Option<&Path>) -> Result<(Child, ProxyIo)> {
    let mut cmd = Command::new(codex);
    cmd.args(["app-server", "proxy"]);
    if let Some(socket) = socket {
        cmd.arg("--sock").arg(socket);
    }
    if let Some(home) = home {
        cmd.env("CODEX_HOME", home);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let io = ProxyIo {
        input: child.stdin.take().context("missing proxy input")?,
        output: child.stdout.take().context("missing proxy output")?,
    };
    Ok((child, io))
}

async fn websocket_bridge<S>(ws: WebSocketStream<S>, endpoint: DuplexStream) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut source) = ws.split();
    let (read, mut write) = tokio::io::split(endpoint);
    let outgoing = async {
        let mut reader = BufReader::new(read);
        loop {
            let mut line = Vec::new();
            let count = (&mut reader)
                .take((LIMIT + 1) as u64)
                .read_until(b'\n', &mut line)
                .await?;
            if count == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            if count > LIMIT || line.last() != Some(&b'\n') {
                bail!("invalid proxy request frame");
            }
            line.pop();
            sink.send(Message::Text(String::from_utf8(line)?.into()))
                .await?;
        }
    };
    let incoming = async {
        while let Some(message) = source.next().await {
            match message? {
                Message::Text(text) => {
                    if text.len() > LIMIT {
                        bail!("invalid proxy response frame");
                    }
                    // A WebSocket message can contain pretty-printed JSON.
                    // Normalize it to one line without splitting RPC envelopes.
                    let value: serde_json::Value = serde_json::from_str(&text)?;
                    let body = serde_json::to_vec(&value)?;
                    write.write_all(&body).await?;
                    write.write_all(b"\n").await?;
                }
                Message::Ping(_) | Message::Pong(_) => (),
                Message::Close(_) => return Ok::<_, anyhow::Error>(()),
                _ => bail!("unsupported proxy response frame"),
            }
        }
        Ok(())
    };
    tokio::select! { r = outgoing => r, r = incoming => r }
}

impl Proxy {
    pub async fn connect(codex: &Path, socket: Option<&Path>, home: Option<&Path>) -> Result<Self> {
        let (mut child, io) = spawn(codex, socket, home)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(LIMIT))
            .max_frame_size(Some(LIMIT));
        let handshake = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            client_async_with_config("ws://localhost/", io, Some(config)),
        )
        .await;
        let (client, mut endpoint) = tokio::io::duplex(64 * 1024);
        let (child, bridge, transport) = match handshake {
            Ok(Ok((ws, _))) => (
                child,
                tokio::spawn(websocket_bridge(ws, endpoint)),
                "official_proxy_websocket",
            ),
            _ => {
                // Only transport negotiation was attempted. Reconnect to the same
                // official endpoint for legacy JSONL; never replay an RPC.
                let _ = child.start_kill();
                let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, child.wait()).await;
                let (child, mut io) = spawn(codex, socket, home)?;
                let bridge = tokio::spawn(async move {
                    tokio::io::copy_bidirectional(&mut io, &mut endpoint).await?;
                    Ok(())
                });
                (child, bridge, "official_proxy_jsonl")
            }
        };
        let (read, input) = tokio::io::split(client);
        Ok(Self {
            child,
            bridge,
            input,
            output: BufReader::new(read),
            transport,
        })
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.bridge.abort();
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn bridge_fixture() -> (
        DuplexStream,
        WebSocketStream<DuplexStream>,
        JoinHandle<Result<()>>,
    ) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(tokio_tungstenite::accept_async(server));
        let (ws, _) = client_async_with_config(
            "ws://localhost/",
            client,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(LIMIT))
                    .max_frame_size(Some(LIMIT)),
            ),
        )
        .await
        .unwrap();
        let server = server.await.unwrap().unwrap();
        let (application, endpoint) = tokio::io::duplex(64 * 1024);
        (
            application,
            server,
            tokio::spawn(websocket_bridge(ws, endpoint)),
        )
    }

    #[tokio::test]
    async fn websocket_roundtrip_preserves_one_rpc_and_normalizes_pretty_json() {
        let (mut app, mut server, bridge) = bridge_fixture().await;
        let request = json!({"id":7,"method":"initialize","params":{}});
        app.write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let message = server.next().await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(message.to_text().unwrap()).unwrap(),
            request
        );
        let reply = json!({"id":7,"result":{"text":"first\nsecond"}});
        server.send(Message::Ping(vec![1, 2].into())).await.unwrap();
        server
            .send(Message::Text(
                serde_json::to_string_pretty(&reply).unwrap().into(),
            ))
            .await
            .unwrap();
        let mut reader = BufReader::new(&mut app);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            reply
        );
        drop(app);
        tokio::time::timeout(Duration::from_secs(2), bridge)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn oversized_or_binary_websocket_response_closes_without_forwarding() {
        for frame in [
            Message::Text("x".repeat(LIMIT + 1).into()),
            Message::Binary(vec![1, 2, 3].into()),
        ] {
            let (mut app, mut server, bridge) = bridge_fixture().await;
            let sender = tokio::spawn(async move {
                let _ = server.send(frame).await;
            });
            assert!(tokio::time::timeout(Duration::from_secs(2), bridge)
                .await
                .unwrap()
                .unwrap()
                .is_err());
            let mut byte = [0];
            assert_eq!(app.read(&mut byte).await.unwrap(), 0);
            sender.await.unwrap();
        }
    }
}
