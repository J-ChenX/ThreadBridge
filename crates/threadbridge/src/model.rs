use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROTOCOL: u32 = 1;
pub const FRAME_LIMIT: usize = 1024 * 1024;
pub const BODY_CHUNK: usize = 16000;
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
pub fn secret() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn key(host: &str, native: &str) -> String {
    hash(&format!("{host}\0default\0{native}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub native_id: String,
    pub host_id: String,
    pub title: String,
    pub status: String,
    pub revision: String,
    pub updated_at: i64,
    pub can_send: bool,
    #[serde(default)]
    pub history_cursor: Option<String>,
    #[serde(default)]
    pub project: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub turn_id: String,
    pub role: String,
    pub text: String,
    pub version: String,
    pub ordinal: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub thread: Thread,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub initial: bool,
    #[serde(default)]
    pub history: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub thread_id: String,
    pub native_id: String,
    pub text: String,
    pub expected_revision: String,
    pub kind: String,
    pub cursor: Option<String>,
    pub expires_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentFrame {
    Hello {
        protocol: u32,
        adapter: String,
    },
    Snapshot {
        snapshot: Snapshot,
    },
    Receipt {
        id: String,
        status: String,
        native_turn_id: Option<String>,
        error: Option<String>,
    },
    Deleted {
        native_id: String,
        message_id: Option<String>,
    },
    Heartbeat,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubFrame {
    Command { command: Command },
    Welcome { protocol: u32 },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submit {
    pub request_id: String,
    pub thread_id: String,
    pub text: String,
    pub expected_revision: String,
    pub created_at: i64,
    #[serde(default = "send_kind")]
    pub kind: String,
    pub cursor: Option<String>,
}
fn send_kind() -> String {
    "send".into()
}
