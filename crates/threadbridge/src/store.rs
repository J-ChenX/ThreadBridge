use crate::model::*;
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

// Keep this width folding and per-code-point lowercase identical to ConversationSearch.kt.
fn normalize_search(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap(),
            '\u{3000}' => ' ',
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Clone)]
pub struct Store(pub Arc<Mutex<Connection>>);
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            for suffix in ["-wal", "-shm"] {
                let side = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
                if side.exists() {
                    std::fs::set_permissions(side, std::fs::Permissions::from_mode(0o600))?;
                }
            }
        }
        let c = Connection::open(path)?;
        c.busy_timeout(std::time::Duration::from_secs(3))?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA cache_size=-2048; PRAGMA max_page_count=4294967294;
CREATE TABLE IF NOT EXISTS settings(k TEXT PRIMARY KEY,v TEXT NOT NULL);
INSERT OR IGNORE INTO settings VALUES('writes','on');
INSERT OR IGNORE INTO settings VALUES('collection_generation','initial');
CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY, token TEXT UNIQUE, role TEXT NOT NULL,name TEXT NOT NULL,expires INTEGER NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,last_seen INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS capture_targets(thread TEXT PRIMARY KEY,host TEXT NOT NULL,native TEXT NOT NULL,queue_enabled INTEGER NOT NULL DEFAULT 0,last_seen INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS resume_owners(thread TEXT PRIMARY KEY,grant_hash TEXT NOT NULL,expires INTEGER NOT NULL,last_seen INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS capture_health(host TEXT PRIMARY KEY,status TEXT NOT NULL,checked_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS capture_import_positions(thread TEXT PRIMARY KEY,last_row INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS capture_user_import_positions(thread TEXT PRIMARY KEY,last_row INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS attachments(thread TEXT NOT NULL,message TEXT NOT NULL,image TEXT NOT NULL,mime TEXT NOT NULL,bytes BLOB NOT NULL,PRIMARY KEY(thread,message,image));
CREATE TABLE IF NOT EXISTS capture_visible_import_positions(thread TEXT PRIMARY KEY,last_row INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS thread_projects(thread TEXT PRIMARY KEY,project TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS creation_results(command TEXT PRIMARY KEY,thread TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS project_names(host TEXT NOT NULL,project TEXT NOT NULL,name TEXT NOT NULL,PRIMARY KEY(host,project));
CREATE TABLE IF NOT EXISTS host_projects(host TEXT NOT NULL,project TEXT NOT NULL,PRIMARY KEY(host,project));
CREATE TABLE IF NOT EXISTS creation_hosts(host TEXT PRIMARY KEY,last_seen INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS thread_message_state(thread TEXT PRIMARY KEY,revision INTEGER NOT NULL,activity_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS completion_metadata(thread TEXT NOT NULL,turn TEXT NOT NULL,title TEXT NOT NULL,recorded_at INTEGER NOT NULL,PRIMARY KEY(thread,turn));
CREATE TABLE IF NOT EXISTS capture_queue_ledger(id TEXT PRIMARY KEY,status TEXT NOT NULL,queue_id TEXT);
CREATE TABLE IF NOT EXISTS capture_queue_send_times(id TEXT PRIMARY KEY,started_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pairings(code TEXT PRIMARY KEY,expires INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS threads(id TEXT PRIMARY KEY,host TEXT NOT NULL,native TEXT NOT NULL,title TEXT NOT NULL,status TEXT NOT NULL,revision TEXT NOT NULL,updated INTEGER NOT NULL,can_send INTEGER NOT NULL,cursor TEXT);
CREATE TABLE IF NOT EXISTS messages(thread TEXT NOT NULL,id TEXT NOT NULL,turn_id TEXT NOT NULL,role TEXT NOT NULL,body TEXT NOT NULL,version TEXT NOT NULL,ordinal INTEGER NOT NULL,PRIMARY KEY(thread,id));
CREATE INDEX IF NOT EXISTS messages_page ON messages(thread,ordinal,id);
CREATE TABLE IF NOT EXISTS commands(id TEXT PRIMARY KEY,device TEXT NOT NULL,request TEXT NOT NULL,digest TEXT NOT NULL,host TEXT NOT NULL,thread TEXT NOT NULL,payload TEXT NOT NULL,status TEXT NOT NULL,created INTEGER NOT NULL,expires INTEGER NOT NULL,native_turn TEXT,error TEXT,UNIQUE(device,request));
CREATE INDEX IF NOT EXISTS commands_dispatch ON commands(host,status,created);
CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,kind TEXT NOT NULL,thread TEXT NOT NULL,created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS outbox(id TEXT PRIMARY KEY,thread TEXT NOT NULL,status TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,next_at INTEGER NOT NULL,sent INTEGER NOT NULL DEFAULT 0,expires INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS history_positions(thread TEXT PRIMARY KEY,cursor TEXT);
CREATE TABLE IF NOT EXISTS tombstones(thread TEXT NOT NULL,message TEXT NOT NULL,PRIMARY KEY(thread,message));
CREATE TABLE IF NOT EXISTS agent_ledger(id TEXT PRIMARY KEY,status TEXT NOT NULL,native_turn TEXT,error TEXT);")?;
        let initialized: bool = c.query_row(
            "SELECT count(*)>0 FROM settings WHERE k='message_state_initialized'",
            [],
            |r| r.get(0),
        )?;
        if !initialized {
            // Existing bodies establish activity, not a new unread mutation.
            c.execute_batch("BEGIN IMMEDIATE; INSERT OR IGNORE INTO thread_message_state SELECT thread,0,max(ordinal) FROM messages GROUP BY thread; INSERT OR IGNORE INTO settings VALUES('message_state_initialized','1'); COMMIT;")?;
        }
        let has_expiry = {
            let mut columns = c.prepare("PRAGMA table_info(outbox)")?;
            let names = columns
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            names.iter().any(|name| name == "expires")
        };
        if !has_expiry {
            c.execute(
                "ALTER TABLE outbox ADD COLUMN expires INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self(Arc::new(Mutex::new(c))))
    }
    pub fn credential(&self, role: &str, name: &str) -> Result<(String, String)> {
        let id = uuid::Uuid::new_v4().to_string();
        let token = secret();
        self.0.lock().unwrap().execute(
            "INSERT INTO devices(id,token,role,name,expires) VALUES(?1,?2,?3,?4,?5)",
            params![id, hash(&token), role, name, now() + 90 * 86400],
        )?;
        Ok((id, token))
    }
    pub fn authenticate(&self, token: &str, role: &str) -> Result<String> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT id FROM devices WHERE token=?1 AND role=?2 AND revoked=0 AND expires>?3",
                params![hash(token), role, now()],
                |r| r.get(0),
            )
            .map_err(Into::into)
    }
    pub fn valid_device(&self, id: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM devices WHERE id=?1 AND revoked=0 AND expires>?2",
                params![id, now()],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 1
    }
    pub fn pair_code(&self) -> Result<String> {
        let code = secret();
        let c = self.0.lock().unwrap();
        c.execute("DELETE FROM pairings WHERE expires<?1", [now()])?;
        c.execute(
            "INSERT INTO pairings VALUES(?1,?2)",
            params![hash(&code), now() + 300],
        )?;
        Ok(code)
    }
    pub fn pair(&self, code: &str, name: &str) -> Result<Value> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        if tx.execute(
            "DELETE FROM pairings WHERE code=?1 AND expires>?2",
            params![hash(code), now()],
        )? != 1
        {
            bail!("invalid_pairing")
        }
        let id = uuid::Uuid::new_v4().to_string();
        let token = secret();
        tx.execute(
            "INSERT INTO devices(id,token,role,name,expires) VALUES(?1,?2,'phone',?3,?4)",
            params![id, hash(&token), name, now() + 90 * 86400],
        )?;
        tx.commit()?;
        Ok(json!({"device_id":id,"token":token,"expires_at":now()+90*86400,"protocol":PROTOCOL}))
    }
    pub fn revoke(&self, id: &str) -> Result<()> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("UPDATE devices SET revoked=1,last_seen=0 WHERE id=?1", [id])?;
        tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?2 FROM commands WHERE device=?1 AND status='accepted'",params![id,now()])?;
        tx.execute("UPDATE commands SET status='cancelled',error='device_revoked' WHERE device=?1 AND status='accepted'",[id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn heartbeat(&self, id: &str) -> Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE devices SET last_seen=?2 WHERE id=?1",
            params![id, now()],
        )?;
        Ok(())
    }
    pub fn hosts(&self) -> Result<Value> {
        let c = self.0.lock().unwrap();
        let mut q = c.prepare(
            "SELECT id,name,last_seen,revoked,expires FROM devices WHERE role='agent' LIMIT 4",
        )?;
        let mut rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"last_seen":r.get::<_,i64>(2)?,"online":r.get::<_,i64>(2)?>now()-15 && r.get::<_,i64>(3)?==0 && r.get::<_,i64>(4)?>now()})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for row in &mut rows {
            let id = row["id"].as_str().unwrap_or("");
            let mut projects = c
                .prepare(
                    "SELECT project FROM host_projects WHERE host=?1 ORDER BY project LIMIT 1000",
                )?
                .query_map([id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            projects.truncate(100);
            while serde_json::to_vec(&projects)?.len() > 128 * 1024 {
                projects.pop();
            }
            let mut details = projects
                .iter()
                .map(|path| {
                    let name: String = c
                        .query_row(
                            "SELECT name FROM project_names WHERE host=?1 AND project=?2",
                            params![id, path],
                            |r| r.get(0),
                        )
                        .optional()
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    json!({"path":path,"name":name})
                })
                .collect::<Vec<_>>();
            while serde_json::to_vec(&json!({"projects":projects,"project_details":details}))?.len()
                > 128 * 1024
            {
                projects.pop();
                details.pop();
            }
            row["project_details"] = json!(details);
            row["projects"] = json!(projects);
        }
        Ok(json!({"hosts":rows,"collection_generation":generation(&c)?}))
    }
    pub fn snapshot(&self, host: &str, s: &Snapshot) -> Result<()> {
        self.snapshot_source(host, s, None)
    }
    pub fn capture_snapshot(&self, host: &str, s: &Snapshot, title: &str) -> Result<()> {
        self.snapshot_source(host, s, Some(title))
    }
    fn snapshot_source(&self, host: &str, s: &Snapshot, capture_title: Option<&str>) -> Result<()> {
        let t = &s.thread;
        let owned: bool = self.0.lock().unwrap().query_row(
            "SELECT count(*)>0 FROM capture_targets WHERE thread=?1",
            [&t.id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(!owned || capture_title.is_some(), "capture_source_owned");
        {
            let c = self.0.lock().unwrap();
            let dead: i64 = c.query_row(
                "SELECT count(*) FROM tombstones WHERE thread=?1 AND message=''",
                [&t.id],
                |r| r.get(0),
            )?;
            if dead > 0 {
                return Ok(());
            }
        }
        if t.id != key(host, &t.native_id)
            || t.host_id != host
            || s.messages.len() > 100
            || s.messages.iter().map(|m| m.text.len()).sum::<usize>() > FRAME_LIMIT
            || t.title.len() > 1000
            || t.native_id.len() > 256
        {
            bail!("invalid_snapshot")
        }
        for m in &s.messages {
            if !["user", "assistant"].contains(&m.role.as_str())
                || m.text.len() > 256 * 1024
                || m.id.len() > 256
            {
                bail!("invalid_message")
            }
        }
        let mut c = self.0.lock().unwrap();
        // Reserve the writer before reading: a deferred WAL read transaction
        // cannot upgrade while another process commits, even with busy_timeout.
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if tx.query_row(
            "SELECT count(*)>0 FROM tombstones WHERE thread=?1 AND message=''",
            [&t.id],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(());
        }
        let old: Option<(String, String, String)> = tx
            .query_row(
                "SELECT status,revision,title FROM threads WHERE id=?1",
                [&t.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let history_cursor = if s.history
            || old
                .as_ref()
                .is_some_and(|(_, revision, _)| revision != &t.revision)
        {
            tx.execute("INSERT INTO history_positions VALUES(?1,?2) ON CONFLICT(thread) DO UPDATE SET cursor=excluded.cursor",params![t.id,t.history_cursor])?;
            t.history_cursor.clone()
        } else {
            tx.query_row(
                "SELECT cursor FROM history_positions WHERE thread=?1",
                [&t.id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .unwrap_or(t.history_cursor.clone())
        };
        if !t.project.is_empty() {
            anyhow::ensure!(
                t.project.len() <= 2048 && !t.project.contains(['\0', '\n', '\r']),
                "invalid_project"
            );
            tx.execute("INSERT INTO thread_projects VALUES(?1,?2) ON CONFLICT(thread) DO UPDATE SET project=excluded.project", params![t.id,t.project])?;
        }
        tx.execute("INSERT INTO threads VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(id) DO UPDATE SET title=excluded.title,status=excluded.status,revision=excluded.revision,updated=excluded.updated,can_send=excluded.can_send,cursor=excluded.cursor",params![t.id,host,t.native_id,t.title,t.status,t.revision,t.updated_at,t.can_send,history_cursor])?;
        let mut changed = old
            .as_ref()
            .is_none_or(|x| x.0 != t.status || x.1 != t.revision);
        let mut message_changed = false;
        let mut activity_at = 0;
        for m in &s.messages {
            let dead: i64 = tx.query_row(
                "SELECT count(*) FROM tombstones WHERE thread=?1 AND message=?2",
                params![t.id, m.id],
                |r| r.get(0),
            )?;
            if dead > 0 {
                continue;
            }
            let n=tx.execute("INSERT INTO messages VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(thread,id) DO UPDATE SET body=excluded.body,version=excluded.version,ordinal=excluded.ordinal WHERE messages.version<>excluded.version OR messages.ordinal<>excluded.ordinal",params![t.id,m.id,m.turn_id,m.role,m.text,m.version,m.ordinal])?;
            changed |= n > 0;
            message_changed |= n > 0;
            if n > 0 {
                activity_at = activity_at.max(m.ordinal);
            }
            if let Some(title) = capture_title {
                changed |= tx.execute("INSERT INTO completion_metadata(thread,turn,title,recorded_at) VALUES(?1,?2,?3,?4) ON CONFLICT(thread,turn) DO UPDATE SET title=excluded.title WHERE completion_metadata.title<>excluded.title",params![t.id,m.turn_id,title,m.ordinal/1000])?>0;
            }
        }
        if message_changed {
            tx.execute("INSERT INTO thread_message_state VALUES(?1,1,?2) ON CONFLICT(thread) DO UPDATE SET revision=revision+1,activity_at=max(activity_at,excluded.activity_at)",params![t.id,activity_at])?;
        }
        if changed || old.as_ref().is_some_and(|x| x.2 != t.title) {
            tx.execute(
                "INSERT INTO events(kind,thread,created) VALUES('thread',?1,?2)",
                params![t.id, now()],
            )?;
        }
        if old.is_some()
            && !s.initial
            && !s.history
            && changed
            && ["completed", "failed", "waiting_input"].contains(&t.status.as_str())
        {
            let id = hash(&format!("{}:{}:{}", t.id, t.revision, t.status));
            tx.execute(
                "INSERT OR IGNORE INTO outbox(id,thread,status,next_at,expires) VALUES(?1,?2,?3,?4,?5)",
                params![id, t.id, t.status, now(), now() + 86400],
            )?;
        }
        tx.execute(
            "DELETE FROM events WHERE seq < (SELECT COALESCE(MAX(seq),0)-10000 FROM events)",
            [],
        )?;
        tx.execute(
            "DELETE FROM outbox WHERE next_at<?1 AND (sent=1 OR attempts>=5)",
            [now() - 7 * 86400],
        )?;
        tx.commit()?;
        Ok(())
    }
    #[cfg(test)]
    pub fn threads(&self, offset: i64) -> Result<Value> {
        self.search_threads(offset, None)
    }
    pub fn search_threads(&self, offset: i64, query: Option<&str>) -> Result<Value> {
        anyhow::ensure!(
            query.is_none_or(|q| q.chars().count() <= 64),
            "invalid_request"
        );
        let needle = normalize_search(query.unwrap_or(""))
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<Vec<_>>();
        let searching = !needle.is_empty();
        let c = self.0.lock().unwrap();
        let mut q=c.prepare("SELECT t.id,t.native,t.host,t.title,t.status,t.revision,t.updated,CASE WHEN t.status='resume_ready' THEN t.can_send AND EXISTS(SELECT 1 FROM resume_owners ro WHERE ro.thread=t.id AND ro.expires>strftime('%s','now') AND ro.last_seen>strftime('%s','now')-15) AND EXISTS(SELECT 1 FROM capture_targets ct WHERE ct.thread=t.id AND ct.queue_enabled=1 AND ct.last_seen>strftime('%s','now')-15) WHEN EXISTS(SELECT 1 FROM capture_targets ct WHERE ct.thread=t.id) THEN t.can_send AND EXISTS(SELECT 1 FROM capture_targets ct WHERE ct.thread=t.id AND ct.queue_enabled=1 AND ct.last_seen>strftime('%s','now')-15) ELSE t.can_send AND NOT EXISTS(SELECT 1 FROM capture_targets ct WHERE ct.host=t.host) END,t.cursor,CASE WHEN d.revoked=0 AND d.expires>strftime('%s','now') THEN COALESCE(d.last_seen,0) ELSE 0 END,coalesce(ms.revision,0),coalesce(ms.activity_at,0),coalesce(p.project,''),coalesce(pn.name,''),p.thread IS NOT NULL FROM threads t LEFT JOIN devices d ON d.id=t.host LEFT JOIN thread_message_state ms ON ms.thread=t.id LEFT JOIN thread_projects p ON p.thread=t.id LEFT JOIN project_names pn ON pn.host=t.host AND pn.project=p.project ORDER BY max(t.updated*1000,coalesce(ms.activity_at,0)) DESC,t.id LIMIT ?2 OFFSET ?1")?;
        // Stream title matching in Rust: SQLite's lower() only folds ASCII.
        // Search never imports message bodies, and follows the same bounded catalogue as sync.
        let mut iter = q.query(params![
            if searching { 0 } else { offset },
            if searching { 100001 } else { 50 }
        ])?;
        let mut rows = Vec::new();
        let mut matched = 0i64;
        while let Some(r) = iter.next()? {
            let title = r.get::<_, String>(3)?;
            if searching {
                let mut next = 0;
                for ch in normalize_search(&title).chars() {
                    if ch == needle[next] {
                        next += 1;
                        if next == needle.len() {
                            break;
                        }
                    }
                }
                if next != needle.len() {
                    continue;
                }
                matched += 1;
                if matched <= offset {
                    continue;
                }
            }
            rows.push(json!({"id":r.get::<_,String>(0)?,"native_id":r.get::<_,String>(1)?,"host_id":r.get::<_,String>(2)?,"title":title,"status":r.get::<_,String>(4)?,"revision":r.get::<_,String>(5)?,"updated_at":r.get::<_,i64>(6)?,"can_send":r.get::<_,bool>(7)? && r.get::<_,i64>(9)?>now()-15,"history_cursor":r.get::<_,Option<String>>(8)?,"host_online":r.get::<_,i64>(9)?>now()-15,"message_revision":r.get::<_,i64>(10)?,"message_activity_at":r.get::<_,i64>(11)?,"project":r.get::<_,String>(12)?,"project_name":r.get::<_,String>(13)?,"project_known":r.get::<_,bool>(14)?}));
            if rows.len() == 50 {
                break;
            }
        }
        let cursor: i64 =
            c.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))?;
        Ok(
            json!({"threads":rows,"collection_generation":generation(&c)?,"cursor":cursor,"next_offset":if rows.len()==50{Some(offset+50)}else{None}}),
        )
    }
    #[cfg(test)]
    pub fn messages(&self, thread: &str, before: i64) -> Result<Value> {
        self.messages_page(thread, before, None)
    }
    pub fn messages_page(
        &self,
        thread: &str,
        before: i64,
        before_id: Option<&str>,
    ) -> Result<Value> {
        let c = self.0.lock().unwrap();
        let mut q = c.prepare("SELECT m.id,m.turn_id,m.role,substr(m.body,1,?3),m.version,m.ordinal,length(m.body),cm.title,cm.recorded_at FROM messages m LEFT JOIN completion_metadata cm ON cm.thread=m.thread AND cm.turn=m.turn_id WHERE m.thread=?1 AND (m.ordinal<?2 OR (m.ordinal=?2 AND ?4 IS NOT NULL AND m.id<?4)) ORDER BY m.ordinal DESC,m.id DESC LIMIT 31")?;
        let mut iter = q.query(params![thread, before, BODY_CHUNK as i64, before_id])?;
        let mut rows = Vec::new();
        let mut bytes = 0;
        let mut more = false;
        while let Some(r) = iter.next()? {
            let row = json!({"id":r.get::<_,String>(0)?,"turn_id":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?,"text":r.get::<_,String>(3)?,"version":r.get::<_,String>(4)?,"ordinal":r.get::<_,i64>(5)?,"timestamp_ms":r.get::<_,i64>(5)?,"characters":r.get::<_,i64>(6)?,"completion_title":r.get::<_,Option<String>>(7)?,"recorded_at":r.get::<_,Option<i64>>(8)?});
            let size = serde_json::to_vec(&row)?.len();
            if rows.len() == 30 || (!rows.is_empty() && bytes + size > 256 * 1024) {
                more = true;
                break;
            }
            bytes += size;
            rows.push(row);
        }
        let last = if more { rows.last() } else { None };
        Ok(
            json!({"messages":rows,"collection_generation":generation(&c)?,"next_before":last.map(|v| &v["ordinal"]),"next_before_id":last.map(|v| &v["id"])}),
        )
    }
    pub fn chunk(&self, thread: &str, id: &str, version: &str, offset: i64) -> Result<Value> {
        let c = self.0.lock().unwrap();
        let (text,len):(String,i64)=c.query_row("SELECT substr(body,?4+1,?5),length(body) FROM messages WHERE thread=?1 AND id=?2 AND version=?3",params![thread,id,version,offset,BODY_CHUNK as i64],|r|Ok((r.get(0)?,r.get(1)?)))?;
        Ok(
            json!({"text":text,"collection_generation":generation(&c)?,"offset":offset,"next_offset":if offset+BODY_CHUNK as i64>=len{None}else{Some(offset+BODY_CHUNK as i64)}}),
        )
    }
    pub fn submit(&self, device: &str, s: &Submit) -> Result<Value> {
        if uuid::Uuid::parse_str(&s.request_id).is_err()
            || s.text.len() > 32000
            || s.created_at < now() - 120
            || s.created_at > now() + 30
            || !["send", "history"].contains(&s.kind.as_str())
            || s.kind == "send" && s.text.trim().is_empty()
        {
            bail!("invalid_request")
        }
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
            if old != digest {
                bail!("idempotency_conflict")
            }
            drop(tx);
            drop(c);
            return self.command(device, &id);
        }
        let writes: String =
            tx.query_row("SELECT v FROM settings WHERE k='writes'", [], |r| r.get(0))?;
        if writes != "on" {
            bail!("restore_read_only")
        }
        let (host,native,status,revision,can_send,seen):(String,String,String,String,bool,i64)=tx.query_row("SELECT t.host,t.native,t.status,t.revision,t.can_send,d.last_seen FROM threads t JOIN devices d ON d.id=t.host AND d.revoked=0 AND d.expires>?2 WHERE t.id=?1",params![s.thread_id,now()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        if seen < now() - 15 {
            bail!("host_offline")
        }
        if s.kind == "send"
            && (!can_send
                || !["idle", "completed", "queue_ready", "resume_ready"].contains(&status.as_str()))
        {
            bail!("not_ready")
        }
        if status == "resume_ready" {
            let owned:bool=tx.query_row("SELECT count(*)>0 FROM resume_owners WHERE thread=?1 AND expires>?2 AND last_seen>?3",params![s.thread_id,now(),now()-15],|r|r.get(0))?;
            if !owned {
                bail!("not_ready")
            }
        }
        if revision != s.expected_revision {
            bail!("revision_conflict")
        }
        let capture_ready: bool=tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM capture_targets WHERE host=(SELECT host FROM threads WHERE id=?1)) OR EXISTS(SELECT 1 FROM capture_targets WHERE thread=?1 AND queue_enabled=1 AND last_seen>?2)", rusqlite::params![s.thread_id,now()-15], |r|r.get(0))?;
        if s.kind == "send" && !capture_ready {
            bail!("not_ready")
        }
        let pending: i64 = tx.query_row(
            "SELECT COUNT(*) FROM commands WHERE status IN ('accepted','dispatching','unknown','upstream_queued')",
            [],
            |r| r.get(0),
        )?;
        if pending >= 128 {
            bail!("queue_full")
        }
        let active:i64=tx.query_row("SELECT COUNT(*) FROM commands WHERE thread=?1 AND status IN ('accepted','dispatching','unknown','upstream_queued')",[&s.thread_id],|r|r.get(0))?;
        if active > 0 {
            bail!("thread_command_pending")
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cmd = Command {
            id: id.clone(),
            thread_id: s.thread_id.clone(),
            native_id: native,
            text: s.text.clone(),
            expected_revision: s.expected_revision.clone(),
            kind: s.kind.clone(),
            cursor: s.cursor.clone(),
            expires_at: now() + 60,
        };
        tx.execute("INSERT INTO commands(id,device,request,digest,host,thread,payload,status,created,expires) VALUES(?1,?2,?3,?4,?5,?6,?7,'accepted',?8,?9)",params![id,device,s.request_id,digest,host,s.thread_id,serde_json::to_string(&cmd)?,now(),cmd.expires_at])?;
        tx.execute(
            "INSERT INTO events(kind,thread,created) VALUES('command',?1,?2)",
            params![s.thread_id, now()],
        )?;
        tx.commit()?;
        drop(c);
        self.command(device, &id)
    }
    fn expire_queued(tx: &rusqlite::Transaction<'_>) -> Result<()> {
        tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?1 FROM commands WHERE status='accepted' AND expires<=?1",[now()])?;
        tx.execute("UPDATE commands SET status='expired',error='expired_before_dispatch' WHERE status='accepted' AND expires<=?1",[now()])?;
        Ok(())
    }
    pub fn command(&self, device: &str, id: &str) -> Result<Value> {
        {
            let mut c = self.0.lock().unwrap();
            let tx = c.transaction()?;
            Self::expire_queued(&tx)?;
            tx.commit()?;
        }
        self.0.lock().unwrap().query_row("SELECT id,request,status,native_turn,error,(SELECT thread FROM creation_results WHERE command=commands.id) FROM commands WHERE device=?1 AND (id=?2 OR request=?2)",params![device,id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"request_id":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?,"native_turn_id":r.get::<_,Option<String>>(3)?,"error":r.get::<_,Option<String>>(4)?,"result_thread_id":r.get::<_,Option<String>>(5)?}))).map_err(Into::into)
    }
    pub fn claim(&self, host: &str) -> Result<Option<Command>> {
        self.claim_source(host, None)
    }
    pub fn claim_capture(&self, host: &str, thread: &str) -> Result<Option<Command>> {
        self.claim_source(host, Some(thread))
    }
    fn claim_source(&self, host: &str, target: Option<&str>) -> Result<Option<Command>> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        Self::expire_queued(&tx)?;
        let row: Option<(String, String)> = if let Some(thread) = target {
            tx.query_row("SELECT id,payload FROM commands WHERE host=?1 AND thread=?2 AND status='accepted' ORDER BY created LIMIT 1",rusqlite::params![host,thread],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
        } else {
            tx.query_row("SELECT id,payload FROM commands WHERE host=?1 AND status='accepted' AND json_extract(payload,'$.kind')<>'create' AND thread NOT IN (SELECT thread FROM capture_targets) ORDER BY created LIMIT 1",[host],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
        };
        let Some((id, payload)) = row else {
            tx.commit()?;
            return Ok(None);
        };
        let cmd: Command = serde_json::from_str(&payload)?;
        tx.execute(
            "UPDATE commands SET status='dispatching' WHERE id=?1",
            [&id],
        )?;
        tx.execute(
            "INSERT INTO events(kind,thread,created) VALUES('command',?1,?2)",
            params![cmd.thread_id, now()],
        )?;
        tx.commit()?;
        Ok(Some(cmd))
    }
    pub fn receipt(
        &self,
        host: &str,
        id: &str,
        status: &str,
        turn: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        if ![
            "codex_accepted",
            "rejected",
            "unknown",
            "history_loaded",
            "upstream_queued",
        ]
        .contains(&status)
            || status == "codex_accepted" && turn.is_none()
        {
            bail!("invalid_receipt")
        }
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?3 FROM commands WHERE id=?1 AND host=?2 AND status IN ('dispatching','unknown','upstream_queued') AND (status<>?4 OR native_turn IS NOT ?5 OR error IS NOT ?6)",params![id,host,now(),status,turn,error])?;
        tx.execute("UPDATE commands SET status=?3,native_turn=?4,error=?5 WHERE id=?1 AND host=?2 AND status IN ('dispatching','unknown','upstream_queued')",params![id,host,status,turn,error])?;
        tx.commit()?;
        Ok(())
    }
    pub fn disconnected(&self, host: &str) -> Result<()> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("UPDATE devices SET last_seen=0 WHERE id=?1", [host])?;
        tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?2 FROM commands WHERE host=?1 AND status='dispatching'",params![host,now()])?;
        tx.execute("UPDATE commands SET status='unknown',error='connection_lost' WHERE host=?1 AND status='dispatching'",[host])?;
        tx.commit()?;
        Ok(())
    }
    pub fn recover(&self) -> Result<()> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?1 FROM commands WHERE status='dispatching'",[now()])?;
        tx.execute_batch("UPDATE devices SET last_seen=0;UPDATE commands SET status='unknown',error='hub_restarted' WHERE status='dispatching';UPDATE agent_ledger SET status='unknown' WHERE status='intent';")?;
        Self::expire_queued(&tx)?;
        tx.commit()?;
        Ok(())
    }
    pub fn events(&self, after: i64) -> Result<Value> {
        let c = self.0.lock().unwrap();
        let (min, max): (i64, i64) = c.query_row(
            "SELECT COALESCE(MIN(seq),0),COALESCE(MAX(seq),0) FROM events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if after > max || after > 0 && after < min - 1 {
            return Ok(
                json!({"snapshot_required":true,"collection_generation":generation(&c)?,"cursor":max}),
            );
        }
        let mut q =
            c.prepare("SELECT seq,kind,thread FROM events WHERE seq>?1 ORDER BY seq LIMIT 100")?;
        let rows=q.query_map([after],|r|Ok(json!({"seq":r.get::<_,i64>(0)?,"kind":r.get::<_,String>(1)?,"thread_id":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"events":rows,"collection_generation":generation(&c)?,"cursor":rows.last().and_then(|x|x["seq"].as_i64()).unwrap_or(after)}),
        )
    }
    pub fn cancel(&self, device: &str, id: &str) -> Result<Value> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("INSERT INTO events(kind,thread,created) SELECT 'command',thread,?3 FROM commands WHERE id=?1 AND device=?2 AND status='accepted'",params![id,device,now()])?;
        if tx.execute("UPDATE commands SET status='cancelled',error='cancelled_before_dispatch' WHERE id=?1 AND device=?2 AND status='accepted'",params![id,device])?!=1{bail!("cannot_cancel_dispatched")};
        tx.commit()?;
        drop(c);
        self.command(device, id)
    }
    /// Deletes the shared phone replica, never sends a native delete to Codex.
    pub fn image(&self, thread: &str, message: &str, image: &str) -> Result<Value> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let c = self.0.lock().unwrap();
        let (mime,bytes):(String,Vec<u8>)=c.query_row("SELECT a.mime,a.bytes FROM attachments a JOIN messages m ON m.thread=a.thread AND m.id=a.message WHERE a.thread=?1 AND a.message=?2 AND a.image=?3",params![thread,message,image],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let generation: String = c.query_row(
            "SELECT v FROM settings WHERE k='collection_generation'",
            [],
            |r| r.get(0),
        )?;
        Ok(json!({"mime":mime,"base64":STANDARD.encode(bytes),"collection_generation":generation}))
    }
    pub fn delete_copy(&self, thread: &str) -> Result<Value> {
        anyhow::ensure!(
            thread.len() == 64 && thread.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid_request"
        );
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("INSERT OR IGNORE INTO tombstones VALUES(?1,'')", [thread])?;
        for table in [
            "messages",
            "completion_metadata",
            "history_positions",
            "capture_import_positions",
            "capture_user_import_positions",
            "capture_visible_import_positions",
            "attachments",
            "thread_message_state",
            "thread_projects",
            "resume_owners",
            "outbox",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE thread=?1"), [thread])?;
        }
        tx.execute("DELETE FROM threads WHERE id=?1", [thread])?;
        // Keep the native routing identity solely for source exclusion propagation.
        tx.execute(
            "UPDATE capture_targets SET queue_enabled=0,last_seen=0 WHERE thread=?1",
            [thread],
        )?;
        tx.execute("UPDATE commands SET status=CASE WHEN status='accepted' THEN 'cancelled' WHEN status IN ('dispatching','upstream_queued') THEN 'unknown' ELSE status END,payload='{}',error='copy_deleted' WHERE thread=?1",[thread])?;
        tx.execute(
            "INSERT INTO events(kind,thread,created) VALUES('delete_thread',?1,?2)",
            params![thread, now()],
        )?;
        tx.commit()?;
        Ok(json!({"deleted":true}))
    }
    pub fn delete_source(&self, host: &str, native: &str, message: Option<&str>) -> Result<()> {
        let owned: bool = self.0.lock().unwrap().query_row(
            "SELECT count(*)>0 FROM capture_targets WHERE thread=?1",
            [key(host, native)],
            |r| r.get(0),
        )?;
        anyhow::ensure!(!owned, "capture_source_owned");
        let thread = key(host, native);
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO tombstones VALUES(?1,?2)",
            params![thread, message.unwrap_or("")],
        )?;
        tx.execute(
            "DELETE FROM attachments WHERE thread=?1 AND (?2 IS NULL OR message=?2)",
            params![thread, message],
        )?;
        let kind = if let Some(id) = message {
            let removed = tx.execute(
                "DELETE FROM messages WHERE thread=?1 AND id=?2",
                params![thread, id],
            )?;
            if removed > 0 {
                tx.execute("INSERT INTO thread_message_state VALUES(?1,1,0) ON CONFLICT(thread) DO UPDATE SET revision=revision+1",[&thread])?;
            }
            format!("delete_message:{id}")
        } else {
            tx.execute("DELETE FROM messages WHERE thread=?1", [&thread])?;
            tx.execute("DELETE FROM threads WHERE id=?1", [&thread])?;
            tx.execute(
                "DELETE FROM thread_message_state WHERE thread=?1",
                [&thread],
            )?;
            tx.execute("UPDATE commands SET status='rejected',error='source_deleted' WHERE thread=?1 AND status='accepted'",[&thread])?;
            "delete_thread".into()
        };
        tx.execute(
            "INSERT INTO events(kind,thread,created) VALUES(?1,?2,?3)",
            params![kind, thread, now()],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn backup(&self, path: &Path) -> Result<()> {
        if path.exists() {
            bail!("backup destination already exists")
        };
        self.0
            .lock()
            .unwrap()
            .backup(rusqlite::DatabaseName::Main, path, None)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

fn generation(c: &Connection) -> Result<String> {
    Ok(c.query_row(
        "SELECT v FROM settings WHERE k='collection_generation'",
        [],
        |r| r.get(0),
    )?)
}
