//! ChadScanner ↔ tarkovtracker.org bridge: the injected page script, the calls into it, its
//! messages back, and the Smart Fill debounce.
use std::time::{Duration, Instant};

pub const START_URL: &str = "https://tarkovtracker.org/needed-items";
/// Injected into every main-frame document; defines `window.__chad` on tarkovtracker.org only.
pub const SCRIPT: &str = include_str!("tracker.js");
/// Smart Fill runs this long after the last count key.
pub const FILL_DELAY: Duration = Duration::from_millis(1500);
pub const FILL_SCRIPT: &str = "window.__chad&&__chad.smartFill()";

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
}

/// A page message: `{"toast":{"title","body","warn"}}` or `{"log":"…"}`; anything else → None.
pub fn parse(msg: &str) -> Option<Message> {
    let v: serde_json::Value = serde_json::from_str(msg).ok()?;
    if let Some(t) = v.get("toast") {
        return Some(Message::Toast {
            title: t.get("title")?.as_str()?.to_owned(),
            body: t.get("body")?.as_str()?.to_owned(),
            warn: t.get("warn").and_then(|w| w.as_bool()).unwrap_or(false),
        });
    }
    Some(Message::Log(v.get("log")?.as_str()?.to_owned()))
}

/// Pending Smart Fill: `bumped` (re)arms it FILL_DELAY ahead; `due` reports it once when the
/// deadline passed; `take` flushes it early (another item opens, window hides).
#[derive(Default)]
pub struct FillTimer {
    due: Option<Instant>,
}

impl FillTimer {
    pub fn bumped(&mut self, now: Instant) {
        self.due = Some(now + FILL_DELAY);
    }

    pub fn due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(t) if now >= t => {
                self.due = None;
                true
            }
            _ => false,
        }
    }

    pub fn take(&mut self) -> bool {
        self.due.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_fires_once_after_last_bump() {
        let t0 = Instant::now();
        let mut f = FillTimer::default();
        f.bumped(t0);
        f.bumped(t0 + Duration::from_secs(1));
        assert!(!f.due(t0 + Duration::from_millis(2400)));
        assert!(f.due(t0 + Duration::from_millis(2500)));
        assert!(!f.due(t0 + Duration::from_secs(3)));
    }

    #[test]
    fn take_flushes_pending_fill() {
        let t0 = Instant::now();
        let mut f = FillTimer::default();
        f.bumped(t0);
        assert!(f.take());
        assert!(!f.due(t0 + Duration::from_secs(10)));
        assert!(!f.take());
    }

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
    fn step_script_quotes_names() {
        assert_eq!(
            step_script("a", "O'Neil \"x\"", true, false),
            r#"window.__chad&&__chad.step("a","O'Neil \"x\"",true,false)"#
        );
    }
}
