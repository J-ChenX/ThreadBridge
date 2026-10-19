//! The only tool cards imported into visible history are explicit human questions.
use anyhow::{ensure, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const REPLY_START: &str = "<send_user_message_question_reply>";
const REPLY_END: &str = "</send_user_message_question_reply>";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OptionLabel {
    pub label: String,
    pub description: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Question {
    pub schema: u32,
    pub id: String,
    pub kind: String,
    pub title: String,
    pub options: Vec<OptionLabel>,
    pub status: String,
    pub answer: Option<String>,
}
impl Question {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1
                && !self.id.is_empty()
                && self.id.len() <= 256
                && ["async", "blocking"].contains(&self.kind.as_str())
                && !self.title.trim().is_empty()
                && self.title.len() <= 4000
                && self.options.len() <= 12
                && self.options.iter().all(|o| !o.label.trim().is_empty()
                    && o.label.len() <= 512
                    && o.description.len() <= 1024)
                && ["pending", "answered", "closed", "failed"].contains(&self.status.as_str())
                && self.answer.as_ref().is_none_or(|a| a.len() <= 8000)
                && serde_json::to_vec(self)?.len() <= 12000,
            "invalid_question"
        );
        Ok(())
    }
    pub fn message_id(&self) -> String {
        format!("question:{}", crate::model::hash(&self.id))
    }
}

struct Captured {
    turn: String,
    call: String,
    answer_key: String,
    stamp: i64,
    question: Question,
}
#[derive(Default)]
pub struct Scan {
    rows: BTreeMap<String, Captured>,
}
impl Scan {
    pub fn record(&mut self, record: &Value) -> Result<()> {
        if record["type"] != "response_item" {
            return Ok(());
        }
        let p = &record["payload"];
        if p["type"] == "function_call"
            && matches!(
                p["name"].as_str(),
                Some("request_user_input_async" | "request_user_input")
            )
        {
            let (Some(call), Some(turn), Some(args)) = (
                p["call_id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 128),
                p["internal_chat_message_metadata_passthrough"]["turn_id"].as_str(),
                p["arguments"].as_str(),
            ) else {
                return Ok(());
            };
            ensure!(args.len() <= 64000, "question_input_limit");
            uuid::Uuid::parse_str(turn)?;
            let Ok(args) = serde_json::from_str::<Value>(args) else {
                return Ok(());
            };
            let Some(questions) = args["questions"].as_array() else {
                return Ok(());
            };
            ensure!(questions.len() <= 12, "question_count_limit");
            let stamp =
                chrono::DateTime::parse_from_rfc3339(record["timestamp"].as_str().unwrap_or(""))?
                    .timestamp_millis();
            let asynchronous = p["name"] == "request_user_input_async";
            for (index, q) in questions.iter().enumerate() {
                // Secret prompts require the native UI; never create a reply form for them.
                if q["isSecret"] == true {
                    continue;
                }
                let Some(title) = q[if asynchronous { "title" } else { "question" }].as_str()
                else {
                    continue;
                };
                let answer_key = q["id"].as_str().unwrap_or("").to_owned();
                if !asynchronous && (answer_key.is_empty() || answer_key.len() > 128) {
                    continue;
                }
                let id = json!([
                    p["name"],
                    call,
                    if asynchronous {
                        json!(index)
                    } else {
                        json!(answer_key)
                    }
                ])
                .to_string();
                let options = q["options"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| OptionLabel {
                                label: item
                                    .as_str()
                                    .or_else(|| item["label"].as_str())
                                    .unwrap_or("")
                                    .into(),
                                description: item["description"].as_str().unwrap_or("").into(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let question = Question {
                    schema: 1,
                    id: id.clone(),
                    kind: if asynchronous { "async" } else { "blocking" }.into(),
                    title: title.into(),
                    options,
                    status: "pending".into(),
                    answer: None,
                };
                question.validate()?;
                self.rows.entry(id).or_insert(Captured {
                    turn: turn.into(),
                    call: call.into(),
                    answer_key,
                    stamp,
                    question,
                });
            }
            ensure!(self.rows.len() <= 1000, "question_count_limit");
        } else if p["type"] == "function_call_output" {
            let Some(call) = p["call_id"].as_str() else {
                return Ok(());
            };
            let output = p["output"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok());
            for row in self.rows.values_mut().filter(|row| row.call == call) {
                if row.question.kind == "blocking" {
                    let answer = output
                        .as_ref()
                        .and_then(|v| v["answers"][&row.answer_key]["answers"].as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("\n")
                        });
                    row.question.status = if answer.is_some() {
                        "answered"
                    } else {
                        "closed"
                    }
                    .into();
                    row.question.answer = answer.filter(|s| s.len() <= 8000);
                    if row.question.validate().is_err() {
                        row.question.answer = None;
                    }
                } else if output.as_ref().is_some_and(|v| {
                    v["accepted"] == false || v.get("error").is_some_and(|e| !e.is_null())
                }) {
                    row.question.status = "failed".into();
                }
            }
        } else if p["type"] == "message" && p["role"] == "user" {
            let kinds =
                p["internal_chat_message_metadata_passthrough"]["content_item_kinds"].as_array();
            let content = p["content"].as_array();
            let human = kinds.is_some_and(|k| !k.is_empty() && k.iter().all(|v| v == "user.text"))
                && content.is_some_and(|c| {
                    !c.is_empty()
                        && c.iter()
                            .all(|v| v["type"] == "input_text" && v["text"].is_string())
                });
            if !human {
                return Ok(());
            }
            let text = content
                .unwrap()
                .iter()
                .filter_map(|v| v["text"].as_str())
                .collect::<String>();
            if let Some(replies) = replies(&text) {
                for reply in replies {
                    if let Some(row) = self
                        .rows
                        .get_mut(reply["questionItemId"].as_str().unwrap_or(""))
                    {
                        if row.question.kind == "async"
                            && row.question.status == "pending"
                            && reply["question"] == row.question.title
                        {
                            if let Some(answer) = reply["answer"].as_str() {
                                row.question.status = "answered".into();
                                row.question.answer = (answer.len() <= 8000).then(|| answer.into());
                                if row.question.validate().is_err() {
                                    row.question.answer = None;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn save(&self, db: &mut Connection, native: &str) -> Result<()> {
        initialize(db)?;
        let tx = db.transaction()?;
        let mut used: i64 = tx.query_row(
            "SELECT coalesce(sum(length(CAST(text AS BLOB))),0) FROM captured_questions",
            [],
            |r| r.get(0),
        )?;
        for row in self.rows.values() {
            let body = serde_json::to_string(&row.question)?;
            row.question.validate()?;
            let old_size: i64 = tx.query_row("SELECT coalesce((SELECT length(CAST(text AS BLOB)) FROM captured_questions WHERE thread_id=?1 AND message_id=?2),0)",params![native,row.question.message_id()],|r|r.get(0))?;
            ensure!(
                used - old_size + body.len() as i64 <= 32 * 1024 * 1024,
                "question_storage_limit"
            );
            let changed = tx.execute("INSERT INTO captured_questions(thread_id,turn_id,message_id,text,created_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(thread_id,message_id) DO UPDATE SET text=excluded.text WHERE captured_questions.text<>excluded.text AND NOT (json_extract(captured_questions.text,'$.status')<>'pending' AND json_extract(excluded.text,'$.status')='pending')", params![native,row.turn,row.question.message_id(),body,row.stamp])?;
            if changed > 0 {
                used = used - old_size + body.len() as i64;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS captured_questions(thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,message_id TEXT NOT NULL,text TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(thread_id,message_id))")?;
    Ok(())
}
pub fn replies(text: &str) -> Option<Vec<Value>> {
    let text = text.trim();
    let body = text
        .strip_prefix(REPLY_START)?
        .strip_suffix(REPLY_END)?
        .trim();
    serde_json::from_str::<Vec<Value>>(body)
        .ok()
        .filter(|r| !r.is_empty() && r.len() <= 12)
}
/// Render before Hub pagination; splitting the native JSON wrapper would expose it as raw text.
pub fn visible_reply(text: &str) -> String {
    let Some(rows) = replies(text) else {
        return text.into();
    };
    rows.iter()
        .map(|r| {
            Some(format!(
                "{}\n\n回答：{}",
                r["question"].as_str()?,
                r["answer"].as_str()?
            ))
        })
        .collect::<Option<Vec<_>>>()
        .map(|rows| rows.join("\n\n"))
        .unwrap_or_else(|| text.into())
}
/// Used at submission/dispatch, so a stale phone card cannot answer an already resolved question.
pub fn validate_reply(db: &Connection, table: &str, thread: &str, text: &str) -> Result<()> {
    if !text.trim_start().starts_with(REPLY_START) {
        return Ok(());
    }
    let replies = replies(text).ok_or_else(|| anyhow::anyhow!("invalid_question_reply"))?;
    ensure!(replies.len() == 1, "invalid_question_reply");
    let reply = &replies[0];
    let id = reply["questionItemId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("invalid_question_reply"))?;
    let message = format!("question:{}", crate::model::hash(id));
    let (body_col, thread_col, id_col) = match table {
        "messages" => ("body", "thread", "id"),
        "captured_questions" => ("text", "thread_id", "message_id"),
        _ => anyhow::bail!("invalid_question_source"),
    };
    let body: String = db
        .query_row(
            &format!("SELECT {body_col} FROM {table} WHERE {thread_col}=?1 AND {id_col}=?2"),
            params![thread, message],
            |r| r.get(0),
        )
        .map_err(|_| anyhow::anyhow!("question_no_longer_pending"))?;
    let q: Question = serde_json::from_str(&body)?;
    q.validate()?;
    ensure!(
        q.id == id && q.kind == "async" && q.status == "pending" && reply["question"] == q.title,
        "question_no_longer_pending"
    );
    ensure!(
        reply["answer"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 8000),
        "invalid_question_reply"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const TURN: &str = "00000000-0000-4000-8000-000000000001";
    fn call(name: &str, call: &str, questions: Value) -> Value {
        json!({"type":"response_item","timestamp":"2026-10-19T01:00:00Z","payload":{"type":"function_call","name":name,"call_id":call,"arguments":json!({"questions":questions}).to_string(),"internal_chat_message_metadata_passthrough":{"turn_id":TURN}}})
    }
    fn user(text: &str, kind: &str) -> Value {
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":[kind]}}})
    }
    fn output(call: &str, output: Value) -> Value {
        json!({"type":"response_item","payload":{"type":"function_call_output","call_id":call,"output":output.to_string()}})
    }
    fn reply(q: &Question, answer: &str) -> String {
        format!(
            "{REPLY_START}\n{}\n{REPLY_END}",
            json!([{"questionItemId":q.id,"question":q.title,"answer":answer}])
        )
    }
    #[test]
    fn async_ack_does_not_resolve_and_replies_use_exact_question_identity() {
        let mut scan = Scan::default();
        scan.record(&call(
            "request_user_input_async",
            "call_a",
            json!([{"title":"双 WAN？","options":["是","否"]},{"title":"其他问题"}]),
        ))
        .unwrap();
        scan.record(&output("call_a", json!({"accepted":true,"error":null})))
            .unwrap();
        assert!(scan.rows.values().all(|r| r.question.status == "pending"));
        let id = json!(["request_user_input_async", "call_a", 0]).to_string();
        let question = scan.rows[&id].question.clone();
        let answer = reply(&question, "否\n带有引号\"和 emoji 🐈");
        scan.record(&user(&answer, "additional_content.environment"))
            .unwrap();
        assert_eq!(scan.rows[&id].question.status, "pending");
        scan.record(&user(&answer, "user.text")).unwrap();
        assert_eq!(scan.rows[&id].question.status, "answered");
        assert_eq!(
            scan.rows[&id].question.answer.as_deref(),
            Some("否\n带有引号\"和 emoji 🐈")
        );
        assert_eq!(
            scan.rows
                .values()
                .filter(|r| r.question.status == "pending")
                .count(),
            1
        );
        let mut db = Connection::open_in_memory().unwrap();
        scan.save(&mut db, "native").unwrap();
        // A partial live source cannot downgrade an already captured answer.
        let mut partial = Scan::default();
        partial
            .record(&call(
                "request_user_input_async",
                "call_a",
                json!([{"title":"双 WAN？","options":["是","否"]}]),
            ))
            .unwrap();
        partial.save(&mut db, "native").unwrap();
        assert!(validate_reply(&db, "captured_questions", "native", &answer).is_err());
        assert_eq!(
            db.query_row(
                "SELECT json_extract(text,'$.status') FROM captured_questions WHERE message_id=?1",
                [question.message_id()],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "answered"
        );
    }
    #[test]
    fn allowlist_secret_and_failed_questions() {
        let mut scan = Scan::default();
        scan.record(&call(
            "unrelated_tool",
            "other",
            json!([{"title":"TOOL_SECRET"}]),
        ))
        .unwrap();
        scan.record(&call("request_user_input","blocking",json!([{"id":"choice","question":"选择","options":[{"label":"A","description":"说明"}]},{"id":"password","question":"SECRET","isSecret":true}]))).unwrap();
        assert_eq!(scan.rows.len(), 1);
        scan.record(&output(
            "blocking",
            json!({"answers":{"choice":{"answers":["A"]}}}),
        ))
        .unwrap();
        assert_eq!(
            scan.rows
                .values()
                .next()
                .unwrap()
                .question
                .answer
                .as_deref(),
            Some("A")
        );
        scan.record(&call(
            "request_user_input_async",
            "fail",
            json!([{"title":"失败的问题"}]),
        ))
        .unwrap();
        scan.record(&output("fail", json!({"accepted":false})))
            .unwrap();
        assert!(scan.rows.values().any(|r| r.question.status == "failed"));
    }
    #[test]
    fn stale_wrong_thread_and_malformed_answers_are_refused() {
        let mut scan = Scan::default();
        scan.record(&call(
            "request_user_input_async",
            "live",
            json!([{"title":"选择网络"}]),
        ))
        .unwrap();
        let q = &scan.rows.values().next().unwrap().question;
        let mut db = Connection::open_in_memory().unwrap();
        scan.save(&mut db, "native").unwrap();
        assert!(validate_reply(&db, "captured_questions", "native", &reply(q, "公网")).is_ok());
        assert!(validate_reply(&db, "captured_questions", "other", &reply(q, "公网")).is_err());
        assert!(validate_reply(&db, "captured_questions", "native", &reply(q, " ")).is_err());
        let mut wrong = q.clone();
        wrong.title = "其他问题".into();
        assert!(
            validate_reply(&db, "captured_questions", "native", &reply(&wrong, "公网")).is_err()
        );
        assert!(validate_reply(&db, "captured_questions", "native", REPLY_START).is_err());
        assert!(validate_reply(&db, "captured_questions", "native", "ordinary text").is_ok());
    }
    #[test]
    fn oversized_questions_fail_and_large_desktop_answers_still_resolve() {
        let mut scan = Scan::default();
        assert!(scan
            .record(&call(
                "request_user_input_async",
                "oversized",
                json!([{"title":"x".repeat(4001)}])
            ))
            .is_err());
        scan.record(&call(
            "request_user_input_async",
            "long_answer",
            json!([{"title":"请提供说明"}]),
        ))
        .unwrap();
        let q = scan.rows.values().next().unwrap().question.clone();
        scan.record(&user(&reply(&q, &"x".repeat(9000)), "user.text"))
            .unwrap();
        let q = &scan.rows.values().next().unwrap().question;
        assert_eq!(q.status, "answered");
        assert!(q.answer.is_none());
        q.validate().unwrap();
        assert_eq!(
            visible_reply(&reply(q, &"x".repeat(20000))),
            format!("{}\n\n回答：{}", q.title, "x".repeat(20000))
        );
    }
}
