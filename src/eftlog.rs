//! EFT log tracking: task started/failed/finished, flea sales and session mode.
use crate::config::Mode;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TaskStarted(String),
    TaskFailed(String),
    TaskFinished(String),
    /// `soldItem` template id and count from `systemData`, when present.
    FleaSold { item: Option<String>, count: Option<u64> },
    SessionMode(Mode),
}

const FLEA_TEMPLATE: &str = "5bdabfb886f7743e152e867e 0";

/// TarkovMonitor's GameWatcher.cs line regex.
static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^(?P<date>\d{4}-\d{2}-\d{2}) (?P<time>\d{2}:\d{2}:\d{2}\.\d{3})( [+-]\d{2}:\d{2})?\|(?P<message>.+$)\s*(?P<json>^\{[\s\S]+?^\})?",
    )
    .unwrap()
});
static SESSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Session mode: (?P<mode>[^\s|]+)").unwrap());
static ENTRY_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}").unwrap());
static SESSION_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^log_(\d{4})\.(\d{2})\.(\d{2})_(\d{1,2})-(\d{2})-(\d{2})").unwrap());

fn chat_event(json: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(json).ok()?;
    let m = v.get("message")?;
    let kind = match &m["type"] {
        Value::Number(n) => n.as_i64()?,
        Value::String(s) => s.parse().ok()?,
        _ => return None,
    };
    let template = m["templateId"].as_str().unwrap_or_default();
    if kind == 4 && template == FLEA_TEMPLATE {
        log::debug!("flea sale notification: {json}");
        let sd = &m["systemData"];
        return Some(Event::FleaSold { item: sd["soldItem"].as_str().map(str::to_owned), count: sd["itemCount"].as_u64() });
    }
    let task = template.split(' ').next().filter(|t| !t.is_empty())?.to_owned();
    match kind {
        10 => Some(Event::TaskStarted(task)),
        11 => Some(Event::TaskFailed(task)),
        12 => Some(Event::TaskFinished(task)),
        _ => None,
    }
}

/// Events in a chunk of complete log entries, in order.
pub fn parse(chunk: &str) -> Vec<Event> {
    let mut out = Vec::new();
    for c in LINE.captures_iter(chunk) {
        let message = &c["message"];
        if let Some(s) = SESSION.captures(message) {
            if let Some(mode) = Mode::from_session(&s["mode"]) {
                out.push(Event::SessionMode(mode));
            }
            continue;
        }
        if message.contains("ChatMessageReceived") {
            if let Some(e) = c.name("json").and_then(|j| chat_event(j.as_str())) {
                out.push(e);
            }
        }
    }
    out
}

/// Splits buffered text into (complete entries, still-open tail). An entry is complete once
/// the next entry has started, or when it ends with a newline and any JSON body has closed.
pub fn split_complete(buf: &str) -> (&str, &str) {
    let last = ENTRY_START.find_iter(buf).last().map_or(0, |m| m.start());
    let (head, tail) = buf.split_at(last);
    let opened = tail.lines().any(|l| l.starts_with('{'));
    let closed = tail.lines().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| l.trim_end() == "}");
    if !tail.is_empty() && tail.ends_with('\n') && (!opened || closed) {
        (buf, "")
    } else {
        (head, tail)
    }
}

/// `log_YYYY.MM.DD_H-MM-SS…` → sortable key.
pub fn session_key(name: &str) -> Option<(u32, u32, u32, u32, u32, u32)> {
    let c = SESSION_DIR.captures(name)?;
    let n = |i: usize| c[i].parse::<u32>().ok();
    Some((n(1)?, n(2)?, n(3)?, n(4)?, n(5)?, n(6)?))
}

fn tracked(file: &Path) -> bool {
    let name = file.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    name.ends_with(".log") && (name.contains("push-notifications") || name.contains("application"))
}

/// Session folders under `logs`, oldest first.
pub fn sessions(logs: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(logs)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            session_key(&name).map(|_| (name, e.path()))
        })
        .collect();
    v.sort_by_key(|(n, _)| session_key(n));
    v
}

/// Byte offsets already consumed, persisted in `logstate.json` so a restart resumes where it left off.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LogState {
    /// Newest session folder seen.
    pub last_seen_session: Option<String>,
    /// File name → bytes consumed in `last_seen_session`.
    pub offsets: HashMap<String, u64>,
}

impl LogState {
    pub fn load(path: &Path) -> LogState {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn save(&self, path: &Path) {
        if let Err(e) = serde_json::to_vec(self).map_err(anyhow::Error::from).and_then(|b| crate::config::write_atomic(path, &b)) {
            log::warn!("logstate.json: {e:#}");
        }
    }
}

/// Follows the newest session folder; reads only new bytes and holds back unfinished entries.
pub struct Tailer {
    logs: PathBuf,
    pub state: LogState,
    pending: HashMap<String, String>,
}

impl Tailer {
    /// Catch-up: replays sessions newer than `state.last_seen_session` (and the unread rest of
    /// that one) and returns their events. With no saved state, starts at the end of everything.
    pub fn start(logs: PathBuf, state: LogState) -> (Tailer, Vec<Event>) {
        let mut t = Tailer { logs, state, pending: HashMap::new() };
        let all = sessions(&t.logs);
        let Some((newest, _)) = all.last().cloned() else { return (t, Vec::new()) };
        let mut events = Vec::new();
        match t.state.last_seen_session.clone() {
            None => {
                t.state.offsets = Self::sizes(&all.last().unwrap().1);
                t.state.last_seen_session = Some(newest);
            }
            Some(seen) => {
                let seen_key = session_key(&seen);
                for (name, dir) in all.iter().filter(|(n, _)| session_key(n) >= seen_key) {
                    let mut offsets = if *name == seen { std::mem::take(&mut t.state.offsets) } else { HashMap::new() };
                    for file in Self::files(dir) {
                        let fname = file.file_name().unwrap().to_string_lossy().into_owned();
                        let from = offsets.get(&fname).copied().unwrap_or(0);
                        let (evs, consumed) = Self::read_from(&file, from, t.pending.remove(&fname).unwrap_or_default(), &mut t.pending, &fname);
                        events.extend(evs);
                        offsets.insert(fname, consumed);
                    }
                    t.state.offsets = offsets;
                    t.state.last_seen_session = Some(name.clone());
                }
            }
        }
        (t, events)
    }

    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| tracked(p)).collect();
        v.sort();
        v
    }

    fn sizes(dir: &Path) -> HashMap<String, u64> {
        Self::files(dir)
            .into_iter()
            .filter_map(|f| Some((f.file_name()?.to_string_lossy().into_owned(), std::fs::metadata(&f).ok()?.len())))
            .collect()
    }

    /// Reads `file` from byte `from`; returns events and the new offset. Unfinished text goes to `pending`.
    fn read_from(file: &Path, from: u64, carry: String, pending: &mut HashMap<String, String>, key: &str) -> (Vec<Event>, u64) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(file) else { return (Vec::new(), from) };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        // A shrunk file was replaced: start over.
        let (from, carry) = if len < from { (0, String::new()) } else { (from, carry) };
        let mut bytes = Vec::new();
        if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut bytes).is_err() {
            return (Vec::new(), from);
        }
        let mut buf = carry;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        let (done, rest) = split_complete(&buf);
        let events = parse(done);
        if !rest.is_empty() {
            pending.insert(key.to_owned(), rest.to_owned());
        }
        (events, from + bytes.len() as u64)
    }

    /// New events since the last call. Switches to a newer session folder when one appears.
    pub fn poll(&mut self) -> Vec<Event> {
        let Some((newest, dir)) = sessions(&self.logs).pop() else { return Vec::new() };
        if self.state.last_seen_session.as_deref() != Some(newest.as_str()) {
            self.state.last_seen_session = Some(newest);
            self.state.offsets.clear();
            self.pending.clear();
        }
        let mut events = Vec::new();
        for file in Self::files(&dir) {
            let fname = file.file_name().unwrap().to_string_lossy().into_owned();
            let from = self.state.offsets.get(&fname).copied().unwrap_or(0);
            let carry = self.pending.remove(&fname).unwrap_or_default();
            let (evs, consumed) = Self::read_from(&file, from, carry, &mut self.pending, &fname);
            events.extend(evs);
            self.state.offsets.insert(fname, consumed);
        }
        events
    }
}

/// Default EFT logs folder: override, launcher install, Steam install, then the stock path.
pub fn default_logs_dir(override_dir: &str) -> Option<PathBuf> {
    if !override_dir.trim().is_empty() {
        let p = PathBuf::from(override_dir.trim());
        return p.is_dir().then_some(p);
    }
    #[cfg(windows)]
    for key in [
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\EscapeFromTarkov",
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Steam App 3932890",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Steam App 3932890",
    ] {
        if let Some(dir) = crate::win::registry::install_location(key) {
            let logs = PathBuf::from(dir).join("Logs");
            if logs.is_dir() {
                return Some(logs);
            }
        }
    }
    let stock = PathBuf::from(r"C:\Battlestate Games\Escape from Tarkov\Logs");
    stock.is_dir().then_some(stock)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "2026-10-30 14:52:11.158 +02:00|1.0.0|Info|push-notifications|Got notification | ChatMessageReceived\n{\n  \"type\": \"new_message\",\n  \"message\": {\n    \"type\": 12,\n    \"templateId\": \"5967530a86f77462ba22226b successMessageText\"\n  }\n}\n2026-10-30 14:52:12.000|1.0.0|Info|push-notifications|Got notification | ChatMessageReceived\n{\n  \"message\": {\n    \"type\": 4,\n    \"templateId\": \"5bdabfb886f7743e152e867e 0\",\n    \"systemData\": {\"soldItem\": \"abc\", \"itemCount\": 2}\n  }\n}\n2026-10-30 14:52:13.000|1.0.0|Info|application|Session mode: Pve\n";

    #[test]
    fn parses_task_flea_and_mode() {
        assert_eq!(
            parse(LOG),
            vec![
                Event::TaskFinished("5967530a86f77462ba22226b".into()),
                Event::FleaSold { item: Some("abc".into()), count: Some(2) },
                Event::SessionMode(Mode::Pve),
            ]
        );
    }

    #[test]
    fn unfinished_entry_is_held_back() {
        let cut = LOG.find("    \"type\": 12").unwrap();
        let (done, rest) = split_complete(&LOG[..cut]);
        assert!(done.is_empty() && !rest.is_empty());
        let (done, rest) = split_complete(LOG);
        assert_eq!((done.len(), rest.len()), (LOG.len(), 0));
    }

    #[test]
    fn catch_up_then_live_tail() {
        let root = std::env::temp_dir().join(format!("chadscanner-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let s1 = root.join("log_2026.10.30_9-00-00_1.0");
        std::fs::create_dir_all(&s1).unwrap();
        let file = s1.join("log_2026.10.30_9-00-00_1.0 push-notifications_000.log");
        std::fs::write(&file, LOG).unwrap();

        // First run ever: no replay, start at the end.
        let (mut t, ev) = Tailer::start(root.clone(), LogState::default());
        assert!(ev.is_empty());
        let saved = t.state.clone();

        // App not running: a new session appears with a completion.
        let s2 = root.join("log_2026.10.30_10-00-00_1.0");
        std::fs::create_dir_all(&s2).unwrap();
        std::fs::write(s2.join("x push-notifications_000.log"), &LOG[..LOG.find("2026-10-30 14:52:12").unwrap()]).unwrap();
        let (mut t2, ev) = Tailer::start(root.clone(), saved);
        assert_eq!(ev, vec![Event::TaskFinished("5967530a86f77462ba22226b".into())]);
        assert!(t2.poll().is_empty());

        // Live: appended entry is reported once.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(s2.join("x push-notifications_000.log")).unwrap();
        f.write_all(b"2026-10-30 14:53:00.000|1.0.0|Info|push-notifications|Got notification | ChatMessageReceived\n{\n  \"message\": {\"type\": 11, \"templateId\": \"aaa x\"}\n}\n").unwrap();
        assert_eq!(t2.poll(), vec![Event::TaskFailed("aaa".into())]);
        assert!(t2.poll().is_empty());
        assert!(t.poll().len() <= 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}
