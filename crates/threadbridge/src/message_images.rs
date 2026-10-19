//! Copy images explicitly embedded in visible assistant text. Native text stays immutable.
use crate::{
    model::{hash, ChatMessage, Snapshot, Thread},
    store::Store,
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use pulldown_cmark::{Event, Parser, Tag};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{io::Read, ops::Range, path::Path};
const IMAGE_BYTES: usize = crate::native_input::MAX_IMAGE_BYTES;

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS captured_assistant_images(thread_id TEXT NOT NULL,message_id TEXT NOT NULL,source TEXT NOT NULL,image_id TEXT NOT NULL,mime TEXT NOT NULL,bytes BLOB NOT NULL,PRIMARY KEY(thread_id,message_id,source))")?;
    Ok(())
}
fn references(text: &str) -> Vec<(Range<usize>, String)> {
    Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            if let Event::Start(Tag::Image { dest_url, .. }) = event {
                Some((range, dest_url.into_string()))
            } else {
                None
            }
        })
        .collect()
}
fn image(source: &str) -> Result<Option<(String, Vec<u8>)>> {
    let bytes = if let Some(encoded) = source.strip_prefix("data:image/") {
        let Some((kind, value)) = encoded.split_once(";base64,") else {
            return Ok(None);
        };
        if !["png", "jpeg", "webp"].contains(&kind) || value.len() > IMAGE_BYTES.div_ceil(3) * 4 {
            return Ok(None);
        }
        let Ok(bytes) = STANDARD.decode(value) else {
            return Ok(None);
        };
        bytes
    } else {
        let path = Path::new(source);
        if !path.is_absolute() || source.len() > 2048 {
            return Ok(None);
        }
        if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
            return Ok(None);
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let Ok(mut file) = options.open(path) else {
            return Ok(None);
        };
        if !file.metadata()?.is_file() || file.metadata()?.len() > IMAGE_BYTES as u64 {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take((IMAGE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        bytes
    };
    if bytes.is_empty() || bytes.len() > IMAGE_BYTES {
        return Ok(None);
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return Ok(None);
    };
    Ok(Some((mime.into(), bytes)))
}
pub fn capture(db: &mut Connection, native: &str, message: &str, text: &str) -> Result<()> {
    let refs = references(text);
    if refs.is_empty() {
        return Ok(());
    }
    initialize(db)?;
    let tx = db.transaction()?;
    let user_used: i64 = if tx.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE name='captured_images'",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        tx.query_row(
            "SELECT coalesce(sum(length(bytes)),0) FROM captured_images",
            [],
            |r| r.get(0),
        )?
    } else {
        0
    };
    let mut used: i64 = user_used
        + tx.query_row(
            "SELECT coalesce(sum(length(bytes)),0) FROM captured_assistant_images",
            [],
            |r| r.get::<_, i64>(0),
        )?;
    for (_, source) in refs.into_iter().take(16) {
        let saved: bool = tx.query_row("SELECT count(*)>0 FROM captured_assistant_images WHERE thread_id=?1 AND message_id=?2 AND source=?3", params![native,message,source], |r|r.get(0))?;
        if saved {
            continue;
        } // Replays keep the originally captured bytes, even if the file changes.
        let Some((mime, bytes)) = image(&source)? else {
            continue;
        };
        ensure!(
            used + bytes.len() as i64 <= 64 * 1024 * 1024,
            "image_storage_budget"
        );
        let id = format!("{:x}", Sha256::digest(&bytes));
        tx.execute(
            "INSERT INTO captured_assistant_images VALUES(?1,?2,?3,?4,?5,?6)",
            params![native, message, source, id, mime, bytes],
        )?;
        used += bytes.len() as i64;
    }
    tx.commit()?;
    Ok(())
}
pub fn render(db: &Connection, native: &str, message: &str, text: &str) -> Result<String> {
    if !db.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE name='captured_assistant_images'",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(text.into());
    }
    let mut rendered = text.to_owned();
    for (range, source) in references(text).into_iter().rev() {
        let id: Option<String> = db.query_row("SELECT image_id FROM captured_assistant_images WHERE thread_id=?1 AND message_id=?2 AND source=?3",params![native,message,source], |r|r.get(0)).optional()?;
        if let Some(id) = id {
            rendered.replace_range(range, &format!("![图片](threadbridge-image:{id})"));
        }
    }
    ensure!(
        rendered.len() <= crate::native_input::MAX_INPUT,
        "visible_history_message_limit"
    );
    Ok(rendered)
}
pub fn import(
    db: &Store,
    source: &Connection,
    host: &str,
    thread: &Thread,
    title: &str,
) -> Result<()> {
    if !source.query_row(
        "SELECT count(*)>0 FROM sqlite_master WHERE name='captured_assistant_images'",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let native = &thread.native_id;
    let mut q = source.prepare("SELECT message_id,source,image_id,mime,length(bytes) FROM captured_assistant_images WHERE thread_id=?1 ORDER BY message_id,source")?;
    let mut messages = std::collections::BTreeSet::new();
    for row in q.query_map([native], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, usize>(4)?,
        ))
    })? {
        let (message, reference, id, mime, size) = row?;
        ensure!(
            size <= IMAGE_BYTES
                && ["image/png", "image/jpeg", "image/webp"].contains(&mime.as_str()),
            "invalid_image"
        );
        messages.insert(message.clone());
        let c = db.0.lock().unwrap();
        let exists: bool = c.query_row(
            "SELECT count(*)>0 FROM attachments WHERE thread=?1 AND message=?2 AND image=?3",
            params![thread.id, message, id],
            |r| r.get(0),
        )?;
        let visible: bool = c.query_row("SELECT count(*)>0 FROM messages WHERE thread=?1 AND id=?2 AND role='assistant' AND NOT EXISTS(SELECT 1 FROM tombstones WHERE thread=?1 AND (message='' OR message=?2))",params![thread.id,message],|r|r.get(0))?;
        if exists || !visible {
            continue;
        }
        let bytes:Vec<u8> = source.query_row("SELECT bytes FROM captured_assistant_images WHERE thread_id=?1 AND message_id=?2 AND source=?3",params![native,message,reference],|r|r.get(0))?;
        ensure!(
            id == format!("{:x}", Sha256::digest(&bytes)),
            "invalid_image"
        );
        let used: i64 = c.query_row(
            "SELECT coalesce(sum(length(bytes)),0) FROM attachments",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            used + bytes.len() as i64 <= 64 * 1024 * 1024,
            "image_storage_budget"
        );
        c.execute(
            "INSERT INTO attachments VALUES(?1,?2,?3,?4,?5)",
            params![thread.id, message, id, mime, bytes],
        )?;
    }
    // Upgrade previously imported text as well; old import cursors must not hide images.
    for message in messages {
        let current: Option<(String, i64)> = db
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT turn_id,ordinal FROM messages WHERE thread=?1 AND id=?2 AND role='assistant'",
                params![thread.id, message],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((turn, ordinal)) = current else {
            continue;
        };
        let original: String = if let Some(id) = message.strip_prefix("native:") {
            source.query_row(
                "SELECT text FROM captured_visible_messages WHERE thread_id=?1 AND message_id=?2",
                params![native, id],
                |r| r.get(0),
            )?
        } else if message == format!("notify:{turn}") {
            source.query_row(
                "SELECT reply FROM captured_replies WHERE thread_id=?1 AND turn_id=?2",
                params![native, turn],
                |r| r.get(0),
            )?
        } else {
            anyhow::bail!("invalid_assistant_image_identity")
        };
        let text = render(source, native, &message, &original)?;
        db.capture_snapshot(
            host,
            &Snapshot {
                thread: thread.clone(),
                messages: vec![ChatMessage {
                    id: message,
                    turn_id: turn,
                    role: "assistant".into(),
                    version: hash(&text),
                    text,
                    ordinal,
                }],
                initial: true,
                history: false,
            },
            title,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_markdown_only_and_replay_keeps_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("an image.png");
        let png = b"\x89PNG\r\n\x1a\nfixture";
        std::fs::write(&path, png).unwrap();
        // Markdown treats backslashes before punctuation as escapes. Use forward
        // slashes so Windows temporary paths such as Temp\.tmp... stay intact.
        let markdown_path = path.to_string_lossy().replace('\\', "/");
        let text=format!("before ![result](<{markdown_path}>) after\n`![code]({markdown_path})`\n```\n![code]({markdown_path})\n```\n![remote](https://example.test/private.png)");
        let mut db = Connection::open_in_memory().unwrap();
        capture(&mut db, "n", "native:m", &text).unwrap();
        let rendered = render(&db, "n", "native:m", &text).unwrap();
        assert!(rendered.starts_with("before ![图片](threadbridge-image:"));
        assert!(rendered.contains("`![code]("));
        assert!(rendered.ends_with("![remote](https://example.test/private.png)"));
        std::fs::write(&path, b"changed").unwrap();
        capture(&mut db, "n", "native:m", &text).unwrap();
        assert_eq!(rendered, render(&db, "n", "native:m", &text).unwrap());
        assert_eq!(
            db.query_row("SELECT bytes FROM captured_assistant_images", [], |r| r
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            png
        );
    }
    #[test]
    fn unsupported_missing_and_oversized_images_keep_text() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("oversized.png");
        std::fs::write(&path, vec![0; IMAGE_BYTES + 1]).unwrap();
        let markdown_path = path.to_string_lossy().replace('\\', "/");
        let text = format!(
            "![large](<{markdown_path}>)\n![missing](/missing.png)\n![svg](data:image/svg+xml;base64,PHN2Zz4=)"
        );
        let mut db = Connection::open_in_memory().unwrap();
        capture(&mut db, "n", "notify:t", &text).unwrap();
        assert_eq!(render(&db, "n", "notify:t", &text).unwrap(), text);
    }
}
