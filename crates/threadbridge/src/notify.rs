use crate::{model::now, store::Store};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use std::time::Duration;
pub fn validate_url(url: &str) -> Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url)?;
    anyhow::ensure!(
        parsed.scheme() == "https"
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && !parsed.path().trim_matches('/').is_empty(),
        "ntfy requires an HTTPS topic URL without embedded credentials or query"
    );
    Ok(parsed)
}
pub async fn run(db: Store, url: String, token: Option<String>) -> Result<()> {
    let _ = validate_url(&url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    loop {
        let row = {
            let c = db.0.lock().unwrap();
            c.query_row("SELECT id,thread,status,attempts FROM outbox WHERE sent=0 AND attempts<5 AND next_at<=?1 AND expires>?1 ORDER BY next_at LIMIT 1",[now()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?))).optional()?
        };
        if let Some((id, thread, _status, attempt)) = row {
            let mut request = client
                .post(&url)
                .header("Title", "ThreadBridge")
                .header("Click", format!("threadbridge://thread/{thread}"))
                .body("A task has an update. Open ThreadBridge to view it.");
            if let Some(token) = &token {
                request = request.bearer_auth(token)
            }
            let ok = request.send().await.is_ok_and(|r| r.status().is_success());
            db.0.lock().unwrap().execute(
                "UPDATE outbox SET sent=?2,attempts=attempts+1,next_at=?3 WHERE id=?1",
                params![id, ok, now() + (1i64 << attempt.min(5)) * 5],
            )?;
        } else {
            tokio::time::sleep(Duration::from_secs(2)).await
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn notification_endpoint_must_be_a_private_https_topic() {
        for invalid in [
            "http://example.com/topic",
            "https://example.com",
            "https://user:secret@example.com/topic",
            "https://example.com/topic?token=secret",
            "https://example.com/topic#fragment",
        ] {
            assert!(super::validate_url(invalid).is_err());
        }
        assert!(super::validate_url("https://example.com/topic").is_ok());
    }
}
