//! ChadScanner ↔ tarkovtracker.org bridge: the injected page script, the calls into it, and its
//! messages back.
use crate::needs::{Target, TargetKind};
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

fn targets_json(targets: &[Target]) -> serde_json::Value {
    serde_json::Value::Array(
        targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "kind": if t.kind == TargetKind::Task { "task" } else { "hideout" },
                    "id": t.id,
                    "find": t.find_id,
                    "required": t.required,
                    "fir": t.fir,
                    "label": t.label,
                })
            })
            .collect(),
    )
}

/// One Arrow key press: `up` raises the first of `targets` with room, else undoes; `targets` in fill order.
pub fn step_script(id: &str, name: &str, up: bool, targets: &[Target]) -> String {
    format!("window.__chad&&__chad.step({},{},{up},{})", js(id), js(name), targets_json(targets))
}

/// ChadScanner Fill: sets each of `targets` to the matching `counts` entry; `fir_left`/`non_fir_left` are the
/// collected items no target took.
pub fn fill_script(id: &str, name: &str, targets: &[Target], counts: &[u32], fir_left: u32, non_fir_left: u32) -> String {
    format!(
        "window.__chad&&__chad.fill({},{},{},{},{fir_left},{non_fir_left})",
        js(id),
        js(name),
        targets_json(targets),
        serde_json::json!(counts)
    )
}

#[derive(Debug, PartialEq)]
pub enum Message {
    Toast { title: String, body: String, warn: bool },
    Log(String),
    /// ChadScanner Fill clicked in the item popup: the popup's collected FIR / non-FIR totals for item `id`.
    Fill { id: String, fir: u32, non_fir: u32 },
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

/// A page message: `{"toast":{"title","body","warn"}}`, `{"progress":{"mode","objectives","parts"}}`,
/// `{"fill":{"id","fir","nonFir"}}` or `{"log":"…"}`; anything else → None.
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
    if let Some(f) = v.get("fill") {
        let n = |k: &str| f.get(k).and_then(|c| c.as_f64()).unwrap_or(0.0).max(0.0) as u32;
        return Some(Message::Fill { id: f.get("id")?.as_str()?.to_owned(), fir: n("fir"), non_fir: n("nonFir") });
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
        let t = Target {
            kind: TargetKind::Hideout,
            id: "p1".into(),
            label: "Security 2".into(),
            required: 1,
            count: 0,
            remaining: 1,
            fir: false,
            find_id: None,
            find_count: 0,
        };
        assert_eq!(
            step_script("a", "O'Neil \"x\"", true, &[t]),
            r#"window.__chad&&__chad.step("a","O'Neil \"x\"",true,[{"find":null,"fir":false,"id":"p1","kind":"hideout","label":"Security 2","required":1}])"#
        );
    }

    #[test]
    fn parses_fill() {
        assert_eq!(
            parse(r#"{"fill":{"id":"i","fir":2,"nonFir":1}}"#),
            Some(Message::Fill { id: "i".into(), fir: 2, non_fir: 1 })
        );
        assert_eq!(parse(r#"{"fill":{"id":"i"}}"#), Some(Message::Fill { id: "i".into(), fir: 0, non_fir: 0 }));
        assert_eq!(parse(r#"{"fill":{}}"#), None);
    }

    #[test]
    fn fill_script_carries_targets_and_counts() {
        let t = Target {
            kind: TargetKind::Hideout,
            id: "p1".into(),
            label: "Security 2".into(),
            required: 1,
            count: 0,
            remaining: 1,
            fir: false,
            find_id: None,
            find_count: 0,
        };
        assert_eq!(
            fill_script("a", "Pliers", &[t], &[1], 0, 2),
            r#"window.__chad&&__chad.fill("a","Pliers",[{"find":null,"fir":false,"id":"p1","kind":"hideout","label":"Security 2","required":1}],[1],0,2)"#
        );
    }
}
