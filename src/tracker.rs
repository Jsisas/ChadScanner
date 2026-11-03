//! ChadScanner ↔ tarkovtracker.org bridge: the injected page script, the calls into it, and its
//! messages back.
use crate::progress::{ObjectiveProgress, PartProgress};
use std::collections::HashMap;

pub const START_URL: &str = "https://tarkovtracker.org/needed-items";
/// Injected into every main-frame document; defines `window.__chad` on tarkovtracker.org only.
pub const SCRIPT: &str = include_str!("tracker.js");

fn js(s: &str) -> String {
    serde_json::to_string(s).expect("string serialises")
}

pub fn open_script(id: &str, name: &str) -> String {
    format!("window.__chad&&__chad.open({},{})", js(id), js(name))
}

/// One Arrow key press: `up` raises (FIR first; Non-FIR when FIR is full and `overfill`, or when the item needs no FIR), else undoes.
pub fn step_script(id: &str, name: &str, up: bool, overfill: bool) -> String {
    format!("window.__chad&&__chad.step({},{},{up},{overfill})", js(id), js(name))
}

#[derive(Debug, PartialEq)]
pub enum Message {
    Toast { title: String, body: String, warn: bool },
    Log(String),
    /// The site's own progress for its current game mode (`pvp`/`pve`/`seasonal`), by objective and hideout-part id.
    Progress { mode: String, objectives: HashMap<String, ObjectiveProgress>, parts: HashMap<String, PartProgress> },
}

/// `{id: {count, complete}}` → `id → make(complete, count)`; missing fields read as 0 / false, a non-object as empty.
fn counts<T>(v: Option<&serde_json::Value>, make: impl Fn(bool, u32) -> T) -> HashMap<String, T> {
    let Some(serde_json::Value::Object(m)) = v else { return HashMap::new() };
    m.iter()
        .map(|(id, e)| {
            let count = e.get("count").and_then(|c| c.as_f64()).unwrap_or(0.0).max(0.0) as u32;
            (id.clone(), make(e.get("complete").and_then(|c| c.as_bool()).unwrap_or(false), count))
        })
        .collect()
}

/// A page message: `{"toast":{"title","body","warn"}}`, `{"progress":{"mode","objectives","parts"}}` or `{"log":"…"}`;
/// anything else → None.
pub fn parse(msg: &str) -> Option<Message> {
    let v: serde_json::Value = serde_json::from_str(msg).ok()?;
    if let Some(t) = v.get("toast") {
        return Some(Message::Toast {
            title: t.get("title")?.as_str()?.to_owned(),
            body: t.get("body")?.as_str()?.to_owned(),
            warn: t.get("warn").and_then(|w| w.as_bool()).unwrap_or(false),
        });
    }
    if let Some(p) = v.get("progress") {
        return Some(Message::Progress {
            mode: p.get("mode")?.as_str()?.to_owned(),
            objectives: counts(p.get("objectives"), |complete, count| ObjectiveProgress { complete, count, invalid: false }),
            parts: counts(p.get("parts"), |complete, count| PartProgress { complete, count }),
        });
    }
    Some(Message::Log(v.get("log")?.as_str()?.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_messages() {
        assert_eq!(
            parse(r#"{"toast":{"title":"T","body":"B","warn":true}}"#),
            Some(Message::Toast { title: "T".into(), body: "B".into(), warn: true })
        );
        assert_eq!(
            parse(r#"{"toast":{"title":"T","body":"B"}}"#),
            Some(Message::Toast { title: "T".into(), body: "B".into(), warn: false })
        );
        assert_eq!(parse(r#"{"log":"x"}"#), Some(Message::Log("x".into())));
        assert_eq!(parse("\"nope\""), None);
        assert_eq!(parse("{}"), None);
        assert_eq!(parse(r#"{"toast":{"body":"B"}}"#), None);
    }

    #[test]
    fn parses_progress() {
        let m = parse(r#"{"progress":{"mode":"pvp","objectives":{"o":{"count":2,"complete":true}},"parts":{"p":{"count":1}}}}"#);
        let Some(Message::Progress { mode, objectives, parts }) = m else { panic!("not a progress message: {m:?}") };
        assert_eq!(mode, "pvp");
        assert_eq!(objectives["o"], ObjectiveProgress { complete: true, count: 2, invalid: false });
        assert_eq!(parts["p"], PartProgress { complete: false, count: 1 });
        assert_eq!(parse(r#"{"progress":{"objectives":{}}}"#), None);
        assert!(matches!(parse(r#"{"progress":{"mode":"pve"}}"#), Some(Message::Progress { objectives, .. }) if objectives.is_empty()));
    }

    #[test]
    fn step_script_quotes_names() {
        assert_eq!(
            step_script("a", "O'Neil \"x\"", true, false),
            r#"window.__chad&&__chad.step("a","O'Neil \"x\"",true,false)"#
        );
    }
}
