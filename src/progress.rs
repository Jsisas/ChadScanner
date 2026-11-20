//! Player progress: a local TarkovTracker-backup file or the TarkovTracker API.
use crate::config::{write_atomic, Mode};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TaskProgress {
    pub complete: bool,
    pub failed: bool,
    pub invalid: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ObjectiveProgress {
    pub complete: bool,
    pub count: u32,
    pub invalid: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PartProgress {
    pub complete: bool,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TraderStanding {
    pub level: Option<u32>,
    pub reputation: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub tasks: HashMap<String, TaskProgress>,
    pub objectives: HashMap<String, ObjectiveProgress>,
    /// Built station levels, `"<station>-<level>"`.
    pub hideout_modules: HashSet<String>,
    pub hideout_parts: HashMap<String, PartProgress>,
    pub level: Option<u8>,
    pub faction: Option<String>,
    pub traders: HashMap<String, TraderStanding>,
    /// Quests hidden on tarkovtracker.org (account-wide task ids): left out of needs, so off the card and never
    /// raised or filled. Set by the app from the TarkovTracker window; progress sources never fill it.
    pub hidden_tasks: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Completed,
    Failed,
}

impl TaskState {
    fn api(self) -> &'static str {
        match self {
            TaskState::Completed => "completed",
            TaskState::Failed => "failed",
        }
    }
}

pub trait ProgressSource: Send + Sync {
    fn snapshot(&self) -> Arc<Progress>;
    fn set_task(&self, id: &str, state: TaskState) -> anyhow::Result<()>;
    fn set_objective(&self, id: &str, complete: bool, count: u32) -> anyhow::Result<()>;
    fn set_hideout_level(&self, station_id: &str, level: u8) -> anyhow::Result<()>;
    fn set_level(&self, level: u8) -> anyhow::Result<()>;
    /// Whether objectives must be written alongside a completed task (the tracker marks them itself).
    fn writes_objectives(&self) -> bool;
    /// Re-reads remote progress when the snapshot is older than a few seconds; called before a scan.
    fn refresh_if_stale(&self) {}
    /// Merges progress observed outside ChadScanner (the TarkovTracker window) into the shown snapshot, without writing.
    fn observe(
        &self,
        _objectives: &HashMap<String, ObjectiveProgress>,
        _parts: &HashMap<String, PartProgress>,
        _modules: &HashMap<String, bool>,
    ) {
    }
}

/// Toast sink: (title, body).
pub type Notify = Arc<dyn Fn(&str, &str) + Send + Sync>;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------------------------------------------------------------- local

const FORMAT: &str = "tarkovtracker-backup";
/// TarkovTracker rejects sections without a valid faction; written when the faction is unknown.
const PLACEHOLDER_FACTION: &str = "USEC";
/// Our own marker (TarkovTracker strips unknown keys on import) that `pmcFaction` is a placeholder.
const FACTION_UNSET: &str = "chadFactionUnset";

fn empty_section() -> Value {
    json!({
        "level": null,
        "pmcFaction": PLACEHOLDER_FACTION,
        FACTION_UNSET: true,
        "xpOffset": 0,
        "traders": {},
        "taskObjectives": {},
        "taskCompletions": {},
        "hideoutModules": {},
        "hideoutParts": {},
    })
}

fn empty_doc(mode: Mode) -> Value {
    json!({
        "_format": FORMAT,
        "_version": 2,
        "exportedAt": now_ms(),
        "appVersion": "ChadScanner 0.1",
        "currentGameMode": mode.key(),
        "gameEdition": 1,
        "tarkovUid": null,
        "seasonNumber": 1,
        "pvp": empty_section(),
        "pve": empty_section(),
        "seasonal": empty_section(),
    })
}

fn obj<'a>(section: &'a Value, key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    section.get(key).and_then(Value::as_object).into_iter().flatten()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn count(v: &Value) -> u32 {
    v.get("count").and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as u32
}

fn parse_section(s: &Value) -> Progress {
    let mut p = Progress::default();
    for (id, v) in obj(s, "taskCompletions") {
        p.tasks.insert(id.clone(), TaskProgress { complete: flag(v, "complete"), failed: flag(v, "failed"), invalid: false });
    }
    for (id, v) in obj(s, "taskObjectives") {
        p.objectives.insert(id.clone(), ObjectiveProgress { complete: flag(v, "complete"), count: count(v), invalid: false });
    }
    for (id, v) in obj(s, "hideoutModules") {
        if flag(v, "complete") {
            p.hideout_modules.insert(id.clone());
        }
    }
    for (id, v) in obj(s, "hideoutParts") {
        p.hideout_parts.insert(id.clone(), PartProgress { complete: flag(v, "complete"), count: count(v) });
    }
    for (id, v) in obj(s, "traders") {
        p.traders.insert(
            id.clone(),
            TraderStanding {
                level: v.get("level").and_then(Value::as_u64).map(|l| l as u32),
                reputation: v.get("reputation").and_then(Value::as_f64),
            },
        );
    }
    p.level = s.get("level").and_then(Value::as_u64).filter(|l| (1..=255).contains(l)).map(|l| l as u8);
    if !flag(s, FACTION_UNSET) {
        p.faction = s.get("pmcFaction").and_then(Value::as_str).filter(|f| !f.is_empty()).map(str::to_owned);
    }
    p
}

struct LocalState {
    doc: Value,
    snapshot: Arc<Progress>,
    /// mtime of the file as last read or written by this process.
    seen: Option<SystemTime>,
}

/// `progress.json` in TarkovTracker backup v2 format; one section per game mode.
pub struct LocalSource {
    path: PathBuf,
    mode: Mode,
    state: Mutex<LocalState>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl LocalSource {
    pub fn open(path: PathBuf, mode: Mode) -> LocalSource {
        let doc = Self::read(&path, mode);
        let snapshot = Arc::new(parse_section(&doc[mode.key()]));
        let seen = mtime(&path);
        LocalSource { path, mode, state: Mutex::new(LocalState { doc, snapshot, seen }) }
    }

    fn read(path: &Path, mode: Mode) -> Value {
        let Ok(text) = std::fs::read_to_string(path) else { return empty_doc(mode) };
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.get("_format").and_then(Value::as_str) == Some(FORMAT) => v,
            other => {
                let why = other.err().map(|e| e.to_string()).unwrap_or_else(|| "not a tarkovtracker-backup".into());
                let bad = path.with_file_name(format!("progress.json.bad-{}", now_ms()));
                log::warn!("progress.json unreadable ({why}); moved to {} and starting empty", bad.display());
                let _ = std::fs::rename(path, &bad);
                empty_doc(mode)
            }
        }
    }

    /// Picks up edits made by another process (the Settings window).
    pub fn reload_if_changed(&self) {
        let mut st = self.state.lock().unwrap();
        let m = mtime(&self.path);
        if m != st.seen {
            st.doc = Self::read(&self.path, self.mode);
            st.snapshot = Arc::new(parse_section(&st.doc[self.mode.key()]));
            st.seen = mtime(&self.path);
        }
    }

    /// Replaces the store with a TarkovTracker export.
    pub fn import(&self, file: &Path) -> anyhow::Result<()> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(file)?)?;
        anyhow::ensure!(v.get("_format").and_then(Value::as_str) == Some(FORMAT), "not a TarkovTracker backup");
        let mut st = self.state.lock().unwrap();
        st.doc = v;
        self.commit(&mut st)
    }

    fn edit(&self, f: impl FnOnce(&mut Map<String, Value>)) -> anyhow::Result<()> {
        let mut st = self.state.lock().unwrap();
        let key = self.mode.key();
        if !st.doc.get(key).is_some_and(Value::is_object) {
            st.doc[key] = empty_section();
        }
        f(st.doc[key].as_object_mut().expect("section object"));
        self.commit(&mut st)
    }

    fn commit(&self, st: &mut LocalState) -> anyhow::Result<()> {
        st.doc["exportedAt"] = json!(now_ms());
        st.doc["currentGameMode"] = json!(self.mode.key());
        for m in Mode::ALL {
            if !st.doc.get(m.key()).is_some_and(Value::is_object) {
                st.doc[m.key()] = empty_section();
            }
        }
        write_atomic(&self.path, serde_json::to_string_pretty(&st.doc)?.as_bytes())?;
        st.seen = mtime(&self.path);
        st.snapshot = Arc::new(parse_section(&st.doc[self.mode.key()]));
        Ok(())
    }

    pub fn set_faction(&self, faction: Option<&str>) -> anyhow::Result<()> {
        self.edit(|s| {
            s.insert("pmcFaction".into(), json!(faction.unwrap_or(PLACEHOLDER_FACTION)));
            s.insert(FACTION_UNSET.into(), json!(faction.is_none()));
        })
    }
}

fn dict<'a>(s: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let v = s.entry(key).or_insert_with(|| json!({}));
    if !v.is_object() {
        *v = json!({});
    }
    v.as_object_mut().unwrap()
}

impl ProgressSource for LocalSource {
    fn snapshot(&self) -> Arc<Progress> {
        self.state.lock().unwrap().snapshot.clone()
    }

    fn set_task(&self, id: &str, state: TaskState) -> anyhow::Result<()> {
        self.edit(|s| {
            let t = dict(s, "taskCompletions");
            match state {
                // Failed is stored as complete plus a flag, as TarkovTracker does.
                TaskState::Completed => t.insert(id.into(), json!({"complete": true, "failed": false, "timestamp": now_ms()})),
                TaskState::Failed => t.insert(id.into(), json!({"complete": true, "failed": true, "timestamp": now_ms()})),
            };
        })
    }

    fn set_objective(&self, id: &str, complete: bool, count: u32) -> anyhow::Result<()> {
        self.edit(|s| {
            dict(s, "taskObjectives").insert(id.into(), json!({"complete": complete, "count": count, "timestamp": now_ms()}));
        })
    }

    fn set_hideout_level(&self, station_id: &str, level: u8) -> anyhow::Result<()> {
        self.edit(|s| {
            let m = dict(s, "hideoutModules");
            let prefix = format!("{station_id}-");
            m.retain(|k, _| !k.strip_prefix(&prefix).is_some_and(|l| l.parse::<u8>().is_ok_and(|l| l > level)));
            for l in 1..=level {
                m.insert(format!("{station_id}-{l}"), json!({"complete": true, "timestamp": now_ms()}));
            }
        })
    }

    fn set_level(&self, level: u8) -> anyhow::Result<()> {
        self.edit(|s| {
            s.insert("level".into(), json!(level));
        })
    }

    fn writes_objectives(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------- TarkovTracker

const API: &str = "https://api.tarkovtracker.org";
const POLL: Duration = Duration::from_secs(300);
/// A scan re-reads progress when it is older than this: the API asks for ≥ 60 s between polls,
/// and a 304 still counts against the daily read quota (1000 on the free tier).
const SCAN_FRESH: Duration = Duration::from_secs(60);
/// Scan-triggered reads stop when fewer reads than this are left today; the 5-min poll continues.
const READ_RESERVE: u32 = 100;
/// A scan waits at most this long for the re-read; otherwise the last snapshot is shown.
const SCAN_TIMEOUT: Duration = Duration::from_secs(2);
/// Pause between ticks of the background thread.
const TICK: Duration = Duration::from_secs(5);

/// A write is confirmed against the server this long after the latest write.
const SYNC_AFTER_WRITE: Duration = Duration::from_secs(30);

/// Outcome of `TrackerSource::write`: on the server already, or waiting in the retry queue.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sent {
    Now,
    Queued,
}

/// What the server reports differently from what was written to it, one line each.
fn unsynced(objs: &HashMap<String, ObjectiveProgress>, tasks: &HashMap<String, TaskProgress>, server: &Progress) -> Vec<String> {
    let mut out = Vec::new();
    for (id, w) in objs {
        let s = server.objectives.get(id).filter(|e| !e.invalid).copied().unwrap_or_default();
        if s.complete != w.complete || s.count != w.count {
            out.push(format!(
                "objective {id}: wrote complete={} count={}, server has complete={} count={}",
                w.complete, w.count, s.complete, s.count
            ));
        }
    }
    for (id, w) in tasks {
        let s = server.tasks.get(id).copied().unwrap_or_default();
        if s.complete != w.complete || s.failed != w.failed {
            out.push(format!(
                "task {id}: wrote complete={} failed={}, server has complete={} failed={}",
                w.complete, w.failed, s.complete, s.failed
            ));
        }
    }
    out
}

/// A write that failed with a retryable error, persisted across restarts.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Pending {
    mode: Mode,
    path: String,
    body: Value,
    what: String,
}

struct TrackerState {
    snapshot: Arc<Progress>,
    etag: Option<String>,
    last_refresh: Option<std::time::Instant>,
    /// `X-RateLimit-Remaining` from the last read, when the gateway sent it.
    reads_left: Option<u32>,
    /// No reads before this (daily quota hit, `Retry-After`).
    reads_paused_until: Option<std::time::Instant>,
    /// Earliest time the pending queue may be retried.
    retry_at: Option<std::time::Instant>,
    /// Objectives / tasks the server accepted since the last read, to compare against its next answer.
    written_objectives: HashMap<String, ObjectiveProgress>,
    written_tasks: HashMap<String, TaskProgress>,
    /// When the confirming read is due; set by every write, cleared by a read.
    sync_at: Option<std::time::Instant>,
}

pub struct TrackerSource {
    mode: Mode,
    token: String,
    /// API root without a trailing slash.
    api: String,
    agent: ureq::Agent,
    pending_path: PathBuf,
    notify: Notify,
    state: Mutex<TrackerState>,
    /// Serialises pending.json read-modify-write.
    queue_lock: Mutex<()>,
}

pub fn tracker_agent() -> anyhow::Result<ureq::Agent> {
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(native_tls::TlsConnector::new()?))
        .timeout(Duration::from_secs(20))
        .user_agent("ChadScanner/0.1 (+https://github.com/Jsisas/ChadScanner)")
        .build())
}

/// `GET /token`: (permissions, game mode) for a token, for the Settings `Test` button.
pub fn test_token(token: &str) -> anyhow::Result<(Vec<String>, String)> {
    test_token_at(API, token)
}

/// `test_token` against another API root (the tests' local server).
fn test_token_at(api: &str, token: &str) -> anyhow::Result<(Vec<String>, String)> {
    let v: Value = tracker_agent()?.get(&format!("{api}/token")).set("Authorization", &format!("Bearer {token}")).call()?.into_json()?;
    let perms = v["permissions"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    Ok((perms, v["gameMode"].as_str().unwrap_or("?").to_owned()))
}

fn parse_tracker(v: &Value) -> Progress {
    let d = &v["data"];
    let list = |k: &str| d[k].as_array().cloned().unwrap_or_default();
    let id = |e: &Value| e["id"].as_str().unwrap_or_default().to_owned();
    let mut p = Progress::default();
    for e in list("tasksProgress") {
        p.tasks.insert(id(&e), TaskProgress { complete: flag(&e, "complete"), failed: flag(&e, "failed"), invalid: flag(&e, "invalid") });
    }
    for e in list("taskObjectivesProgress") {
        p.objectives.insert(id(&e), ObjectiveProgress { complete: flag(&e, "complete"), count: count(&e), invalid: flag(&e, "invalid") });
    }
    for e in list("hideoutModulesProgress") {
        if flag(&e, "complete") {
            p.hideout_modules.insert(id(&e));
        }
    }
    for e in list("hideoutPartsProgress") {
        p.hideout_parts.insert(id(&e), PartProgress { complete: flag(&e, "complete"), count: count(&e) });
    }
    p.level = d["playerLevel"].as_u64().filter(|l| (1..=255).contains(l)).map(|l| l as u8);
    p.faction = d["pmcFaction"].as_str().filter(|f| !f.is_empty()).map(str::to_owned);
    p
}

enum WriteError {
    Retry(Duration, String),
    Fatal(String),
}

impl TrackerSource {
    pub fn start(mode: Mode, token: String, pending_path: PathBuf, notify: Notify) -> anyhow::Result<Arc<TrackerSource>> {
        Self::start_at(API, mode, token, pending_path, notify, TICK)
    }

    /// `start` against another API root, polling every `tick`.
    fn start_at(
        api: &str,
        mode: Mode,
        token: String,
        pending_path: PathBuf,
        notify: Notify,
        tick: Duration,
    ) -> anyhow::Result<Arc<TrackerSource>> {
        let src = Arc::new(Self::new(api, mode, token, pending_path, notify)?);
        let weak: Weak<TrackerSource> = Arc::downgrade(&src);
        std::thread::Builder::new().name(format!("tracker-{}", mode.key())).spawn(move || {
            let mut last_poll: Option<std::time::Instant> = None;
            loop {
                let Some(src) = weak.upgrade() else { return };
                src.poll(&mut last_poll);
                drop(src);
                std::thread::sleep(tick);
            }
        })?;
        Ok(src)
    }

    /// A source that does not poll by itself.
    fn new(api: &str, mode: Mode, token: String, pending_path: PathBuf, notify: Notify) -> anyhow::Result<TrackerSource> {
        Ok(TrackerSource {
            mode,
            token,
            api: api.trim_end_matches('/').to_owned(),
            agent: tracker_agent()?,
            pending_path,
            notify,
            state: Mutex::new(TrackerState {
                snapshot: Arc::new(Progress::default()),
                etag: None,
                last_refresh: None,
                reads_left: None,
                reads_paused_until: None,
                retry_at: None,
                written_objectives: HashMap::new(),
                written_tasks: HashMap::new(),
                sync_at: None,
            }),
            queue_lock: Mutex::new(()),
        })
    }

    /// One tick of the background thread: reads when the poll interval or the post-write sync is due, then sends queued writes.
    fn poll(&self, last_poll: &mut Option<std::time::Instant>) {
        let sync_due = self.state.lock().unwrap().sync_at.is_some_and(|t| std::time::Instant::now() >= t);
        if sync_due || last_poll.is_none_or(|t| t.elapsed() >= POLL) {
            self.refresh();
            *last_poll = Some(std::time::Instant::now());
        }
        self.flush_pending();
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    pub fn refresh(&self) {
        self.refresh_within(Duration::from_secs(20));
    }

    /// `GET /progress` with `If-None-Match`.
    fn refresh_within(&self, timeout: Duration) {
        let etag = {
            let mut st = self.state.lock().unwrap();
            if st.reads_paused_until.is_some_and(|t| std::time::Instant::now() < t) {
                return;
            }
            st.last_refresh = Some(std::time::Instant::now());
            st.sync_at = None;
            st.etag.clone()
        };
        let note_quota = |r: &ureq::Response| {
            if let Some(n) = r.header("X-RateLimit-Remaining").and_then(|v| v.trim().parse().ok()) {
                self.state.lock().unwrap().reads_left = Some(n);
            }
        };
        let mut req = self.agent.get(&self.url("/progress")).set("Authorization", &self.auth()).timeout(timeout);
        if let Some(e) = &etag {
            req = req.set("If-None-Match", e);
        }
        match req.call() {
            Ok(r) if r.status() == 304 => note_quota(&r),
            Ok(r) => {
                note_quota(&r);
                let tag = r.header("ETag").map(str::to_owned);
                match r.into_json::<Value>() {
                    Ok(v) => {
                        let parsed = parse_tracker(&v);
                        // The server wins; whatever it did not keep is reported.
                        let bad = {
                            let mut st = self.state.lock().unwrap();
                            let bad = unsynced(&st.written_objectives, &st.written_tasks, &parsed);
                            st.written_objectives.clear();
                            st.written_tasks.clear();
                            st.snapshot = Arc::new(parsed);
                            st.etag = tag;
                            bad
                        };
                        for line in &bad {
                            log::warn!("TarkovTracker {}: server disagrees with {line}", self.mode.key());
                        }
                        if !bad.is_empty() {
                            (self.notify)(&format!("TarkovTracker did not keep {} change(s)", bad.len()), "see log.txt");
                        }
                    }
                    Err(e) => log::warn!("TarkovTracker {}: progress unreadable: {e}", self.mode.key()),
                }
            }
            Err(ureq::Error::Status(429, r)) => {
                note_quota(&r);
                let wait = r.header("Retry-After").and_then(|v| v.trim().parse().ok()).unwrap_or(600);
                log::warn!("TarkovTracker {}: GET /progress: 429, pausing reads for {wait} s", self.mode.key());
                self.state.lock().unwrap().reads_paused_until = Some(std::time::Instant::now() + Duration::from_secs(wait));
            }
            Err(ureq::Error::Status(code @ (401 | 403), _)) => {
                log::warn!("TarkovTracker {}: GET /progress: {code}", self.mode.key());
                (self.notify)("TarkovTracker rejected the token", self.mode.label());
            }
            Err(e) => log::warn!("TarkovTracker {}: GET /progress failed: {e}", self.mode.key()),
        }
    }

    fn post(&self, path: &str, body: &Value, what: &str) -> Result<(), WriteError> {
        let res = self.agent.post(&self.url(path)).set("Authorization", &self.auth()).send_json(body.clone());
        let tag = format!("TarkovTracker {}: {what}", self.mode.key());
        match res {
            Ok(r) => {
                let status = r.status();
                log::info!("{tag}: {status} {}", r.into_string().unwrap_or_default());
                Ok(())
            }
            Err(ureq::Error::Status(code, r)) => {
                let retry_after = r.header("Retry-After").and_then(|s| s.trim().parse::<u64>().ok());
                let text = r.into_string().unwrap_or_default();
                log::warn!("{tag}: {code} {text}");
                match code {
                    401 | 403 => Err(WriteError::Fatal(format!("{code}"))),
                    429 | 500..=599 => Err(WriteError::Retry(Duration::from_secs(retry_after.unwrap_or(60)), format!("{code}"))),
                    _ => Err(WriteError::Fatal(format!("{code} {text}"))),
                }
            }
            Err(e) => {
                log::warn!("{tag}: {e}");
                Err(WriteError::Retry(Duration::from_secs(60), e.to_string()))
            }
        }
    }

    fn load_queue(&self) -> Vec<Pending> {
        std::fs::read_to_string(&self.pending_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save_queue(&self, q: &[Pending]) {
        if let Err(e) = serde_json::to_vec_pretty(q).map_err(anyhow::Error::from).and_then(|b| write_atomic(&self.pending_path, &b)) {
            log::warn!("pending.json: {e:#}");
        }
    }

    fn write(&self, path: String, body: Value, what: String) -> anyhow::Result<Sent> {
        let queued = {
            let _g = self.queue_lock.lock().unwrap();
            self.load_queue().iter().any(|p| p.mode == self.mode)
        };
        // Keep order: while older writes wait, queue behind them.
        if queued {
            self.enqueue(Pending { mode: self.mode, path, body, what: what.clone() }, None);
            return Ok(Sent::Queued);
        }
        match self.post(&path, &body, &what) {
            Ok(()) => Ok(Sent::Now),
            Err(WriteError::Retry(wait, why)) => {
                self.enqueue(Pending { mode: self.mode, path, body, what }, Some(wait));
                log::info!("queued for retry ({why})");
                Ok(Sent::Queued)
            }
            Err(WriteError::Fatal(why)) => {
                if why.starts_with("401") || why.starts_with("403") {
                    (self.notify)("TarkovTracker rejected the token", self.mode.label());
                }
                anyhow::bail!("TarkovTracker {what}: {why}")
            }
        }
    }

    /// Shows a write at once: edits a copy of the snapshot, drops the ETag (a 304 would restore the
    /// pre-write copy) and schedules the confirming read `SYNC_AFTER_WRITE` after the latest write.
    fn apply(&self, f: impl FnOnce(&mut Progress, &mut TrackerState)) {
        let mut st = self.state.lock().unwrap();
        let mut snap = (*st.snapshot).clone();
        f(&mut snap, &mut st);
        st.snapshot = Arc::new(snap);
        st.etag = None;
        st.sync_at = Some(std::time::Instant::now() + SYNC_AFTER_WRITE);
    }

    fn enqueue(&self, p: Pending, wait: Option<Duration>) {
        let _g = self.queue_lock.lock().unwrap();
        let mut q = self.load_queue();
        q.push(p);
        self.save_queue(&q);
        if let Some(w) = wait {
            self.state.lock().unwrap().retry_at = Some(std::time::Instant::now() + w);
        }
    }

    fn flush_pending(&self) {
        if self.state.lock().unwrap().retry_at.is_some_and(|t| std::time::Instant::now() < t) {
            return;
        }
        let _g = self.queue_lock.lock().unwrap();
        let mut q = self.load_queue();
        let mut sent = false;
        while let Some(i) = q.iter().position(|p| p.mode == self.mode) {
            let p = q[i].clone();
            match self.post(&p.path, &p.body, &p.what) {
                Ok(()) => {
                    q.remove(i);
                    sent = true;
                }
                Err(WriteError::Retry(wait, _)) => {
                    self.state.lock().unwrap().retry_at = Some(std::time::Instant::now() + wait);
                    break;
                }
                Err(WriteError::Fatal(why)) => {
                    log::warn!("dropping queued {}: {why}", p.what);
                    q.remove(i);
                    if why.starts_with("401") || why.starts_with("403") {
                        (self.notify)("TarkovTracker rejected the token", self.mode.label());
                        break;
                    }
                }
            }
        }
        self.save_queue(&q);
        drop(_g);
        if sent {
            let mut st = self.state.lock().unwrap();
            st.etag = None;
            st.sync_at = Some(std::time::Instant::now() + SYNC_AFTER_WRITE);
        }
    }
}

impl ProgressSource for TrackerSource {
    fn snapshot(&self) -> Arc<Progress> {
        self.state.lock().unwrap().snapshot.clone()
    }

    fn set_task(&self, id: &str, state: TaskState) -> anyhow::Result<()> {
        let sent = self.write(format!("/progress/task/{id}"), json!({"state": state.api()}), format!("task {id} -> {}", state.api()))?;
        self.apply(|p, st| {
            // The API marks a failed task complete + failed.
            let v = TaskProgress { complete: true, failed: state == TaskState::Failed, invalid: false };
            p.tasks.insert(id.into(), v);
            if sent == Sent::Now {
                st.written_tasks.insert(id.into(), v);
            }
        });
        Ok(())
    }

    fn set_objective(&self, id: &str, complete: bool, count: u32) -> anyhow::Result<()> {
        let state = if complete { "completed" } else { "uncompleted" };
        let sent = self.write(
            format!("/progress/task/objective/{id}"),
            json!({"state": state, "count": count}),
            format!("objective {id} -> {state} ({count})"),
        )?;
        self.apply(|p, st| {
            let v = ObjectiveProgress { complete, count, invalid: false };
            p.objectives.insert(id.into(), v);
            if sent == Sent::Now {
                st.written_objectives.insert(id.into(), v);
            }
        });
        Ok(())
    }

    fn set_hideout_level(&self, _station_id: &str, _level: u8) -> anyhow::Result<()> {
        anyhow::bail!("TarkovTracker API has no hideout write")
    }

    fn set_level(&self, level: u8) -> anyhow::Result<()> {
        self.write(format!("/progress/level/{level}"), json!({}), format!("level -> {level}"))?;
        self.apply(|p, _| p.level = Some(level));
        Ok(())
    }

    fn writes_objectives(&self) -> bool {
        false
    }

    fn observe(
        &self,
        objectives: &HashMap<String, ObjectiveProgress>,
        parts: &HashMap<String, PartProgress>,
        modules: &HashMap<String, bool>,
    ) {
        self.apply(|p, _| {
            for (id, o) in objectives {
                let invalid = p.objectives.get(id).is_some_and(|old| old.invalid);
                p.objectives.insert(id.clone(), ObjectiveProgress { invalid, ..*o });
            }
            p.hideout_parts.extend(parts.iter().map(|(id, v)| (id.clone(), *v)));
            for (id, built) in modules {
                if *built {
                    p.hideout_modules.insert(id.clone());
                } else {
                    p.hideout_modules.remove(id);
                }
            }
        });
    }

    fn refresh_if_stale(&self) {
        let due = {
            let st = self.state.lock().unwrap();
            // Inside the post-write window the optimistic snapshot is shown and no read is spent.
            st.sync_at.is_none()
                && st.last_refresh.is_none_or(|t| t.elapsed() >= SCAN_FRESH)
                && st.reads_left.is_none_or(|n| n > READ_RESERVE)
        };
        if due {
            self.refresh_within(SCAN_TIMEOUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chadscanner-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("progress.json")
    }

    #[test]
    fn local_round_trip_in_backup_format() {
        let path = tmp("rt");
        let s = LocalSource::open(path.clone(), Mode::Pve);
        s.set_task("t1", TaskState::Completed).unwrap();
        s.set_task("t2", TaskState::Failed).unwrap();
        s.set_objective("o1", false, 3).unwrap();
        s.set_hideout_level("st", 3).unwrap();
        s.set_hideout_level("st", 2).unwrap();
        s.set_level(42).unwrap();

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["_format"], FORMAT);
        assert_eq!(v["_version"], 2);
        assert_eq!(v["pve"]["taskCompletions"]["t1"]["complete"], true);
        assert_eq!(v["pve"]["taskCompletions"]["t2"]["failed"], true);
        assert!(v["pve"]["hideoutModules"].get("st-3").is_none());
        // Every section importable: TarkovTracker requires a faction everywhere.
        for m in Mode::ALL {
            assert!(["USEC", "BEAR"].contains(&v[m.key()]["pmcFaction"].as_str().unwrap()));
        }

        let p = LocalSource::open(path, Mode::Pve).snapshot();
        assert!(p.tasks["t1"].complete);
        assert!(!p.tasks["t1"].failed);
        assert!(p.tasks["t2"].failed);
        assert_eq!(p.objectives["o1"].count, 3);
        assert_eq!(p.hideout_modules, HashSet::from(["st-1".to_owned(), "st-2".to_owned()]));
        assert_eq!(p.level, Some(42));
        // A placeholder faction is not reported as known.
        assert_eq!(p.faction, None);
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let path = tmp("bad");
        std::fs::write(&path, "{not json").unwrap();
        let s = LocalSource::open(path.clone(), Mode::Pvp);
        assert!(s.snapshot().tasks.is_empty());
        let bad = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("progress.json.bad-"));
        assert!(bad);
    }

    #[test]
    fn tracker_flags_parsed() {
        let v = json!({"data": {"tasksProgress": [{"id": "a", "complete": true, "invalid": true}],
            "taskObjectivesProgress": [{"id": "o", "complete": false, "count": 2}],
            "hideoutModulesProgress": [{"id": "s-1", "complete": true}, {"id": "s-2", "complete": false}],
            "hideoutPartsProgress": [], "playerLevel": 17, "pmcFaction": "BEAR"}});
        let p = parse_tracker(&v);
        assert!(p.tasks["a"].invalid);
        assert_eq!(p.objectives["o"].count, 2);
        assert_eq!(p.hideout_modules, HashSet::from(["s-1".to_owned()]));
        assert_eq!((p.level, p.faction.as_deref()), (Some(17), Some("BEAR")));
    }

    #[test]
    fn unsynced_reports_only_disagreements() {
        let o = |count| ObjectiveProgress { complete: false, count, invalid: false };
        let written = HashMap::from([("o1".to_owned(), o(2)), ("o2".to_owned(), o(1))]);
        let mut server = Progress::default();
        server.objectives.insert("o1".into(), o(2));
        let bad = unsynced(&written, &HashMap::new(), &server);
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("objective o2: wrote complete=false count=1, server has complete=false count=0"), "{bad:?}");
    }

    fn read_doc(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn backup(sections: Value) -> Value {
        let mut doc = json!({"_format": FORMAT, "_version": 2});
        for (k, v) in sections.as_object().unwrap() {
            doc[k] = v.clone();
        }
        doc
    }

    #[test]
    fn a_section_is_read_field_by_field() {
        let p = parse_section(&json!({
            "level": 31, "pmcFaction": "BEAR",
            "traders": {"t": {"level": 3, "reputation": 0.25}, "u": {}},
            "taskObjectives": {"o": {"complete": true, "count": 7.0}, "n": {"count": -4}},
            "taskCompletions": {"c": {"complete": true, "failed": true}, "d": {}},
            "hideoutModules": {"s-1": {"complete": true}, "s-2": {"complete": false}},
            "hideoutParts": {"p": {"complete": true, "count": 2}},
        }));
        assert_eq!(p.traders["t"], TraderStanding { level: Some(3), reputation: Some(0.25) });
        assert_eq!(p.traders["u"], TraderStanding::default());
        assert_eq!(p.objectives["o"], ObjectiveProgress { complete: true, count: 7, invalid: false });
        assert_eq!(p.objectives["n"].count, 0, "a negative count reads as 0");
        assert_eq!(p.tasks["c"], TaskProgress { complete: true, failed: true, invalid: false });
        assert_eq!(p.tasks["d"], TaskProgress::default());
        assert_eq!(p.hideout_modules, HashSet::from(["s-1".to_owned()]));
        assert_eq!(p.hideout_parts["p"], PartProgress { complete: true, count: 2 });
        assert_eq!((p.level, p.faction.as_deref()), (Some(31), Some("BEAR")));
    }

    #[test]
    fn level_and_faction_are_only_reported_when_real() {
        for level in [json!(0), json!(256), json!(null), json!("9")] {
            assert_eq!(parse_section(&json!({ "level": level })).level, None, "{level}");
        }
        assert_eq!(parse_section(&json!({"level": 255})).level, Some(255));
        assert_eq!(parse_section(&json!({"pmcFaction": ""})).faction, None);
        assert_eq!(parse_section(&json!({"pmcFaction": "BEAR", FACTION_UNSET: true})).faction, None, "a placeholder faction is hidden");
        assert_eq!(parse_section(&json!({"pmcFaction": "BEAR", FACTION_UNSET: false})).faction.as_deref(), Some("BEAR"));
        assert_eq!(parse_section(&json!(null)), Progress::default());
    }

    #[test]
    fn a_json_file_in_another_format_is_moved_aside() {
        let path = tmp("otherfmt");
        std::fs::write(&path, r#"{"_format": "something-else"}"#).unwrap();
        let s = LocalSource::open(path.clone(), Mode::Pve);
        assert!(s.snapshot().tasks.is_empty());
        assert!(!path.exists());
        let moved = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("progress.json.bad-"));
        assert!(moved);
    }

    #[test]
    fn import_replaces_the_store_with_a_backup() {
        let path = tmp("import");
        let s = LocalSource::open(path.clone(), Mode::Pvp);
        s.set_task("old", TaskState::Completed).unwrap();
        let file = path.with_file_name("backup.json");
        let doc = backup(json!({"pvp": {"taskCompletions": {"new": {"complete": true}}, "level": 12}, "currentGameMode": "pve"}));
        std::fs::write(&file, doc.to_string()).unwrap();
        s.import(&file).unwrap();
        let p = s.snapshot();
        assert!(p.tasks.contains_key("new"));
        assert!(!p.tasks.contains_key("old"));
        assert_eq!(p.level, Some(12));
        let on_disk = read_doc(&path);
        assert_eq!(on_disk["currentGameMode"], "pvp", "the import is re-stamped for the mode in use");
        for m in Mode::ALL {
            assert!(on_disk[m.key()].is_object(), "{m:?} section is filled in so TarkovTracker accepts the file");
        }
    }

    #[test]
    fn import_refuses_what_is_not_a_backup_and_keeps_the_store() {
        let path = tmp("importbad");
        let s = LocalSource::open(path.clone(), Mode::Pve);
        s.set_task("keep", TaskState::Completed).unwrap();
        let file = path.with_file_name("backup.json");
        assert!(s.import(&file).is_err(), "missing file");
        std::fs::write(&file, "{nope").unwrap();
        assert!(s.import(&file).is_err(), "not json");
        std::fs::write(&file, r#"{"_format": "other"}"#).unwrap();
        let e = s.import(&file).unwrap_err();
        assert!(e.to_string().contains("not a TarkovTracker backup"), "{e:#}");
        assert!(s.snapshot().tasks.contains_key("keep"));
    }

    #[test]
    fn faction_is_stored_and_can_be_unset() {
        let path = tmp("faction");
        let s = LocalSource::open(path.clone(), Mode::Pve);
        s.set_faction(Some("BEAR")).unwrap();
        assert_eq!(s.snapshot().faction.as_deref(), Some("BEAR"));
        assert_eq!(read_doc(&path)["pve"]["chadFactionUnset"], false);
        s.set_faction(None).unwrap();
        assert_eq!(s.snapshot().faction, None);
        let sec = &read_doc(&path)["pve"];
        assert_eq!((sec["pmcFaction"].as_str(), sec["chadFactionUnset"].as_bool()), (Some("USEC"), Some(true)));
    }

    #[test]
    fn another_process_writing_the_file_is_picked_up_once() {
        let path = tmp("reload");
        let reader = LocalSource::open(path.clone(), Mode::Pve);
        assert!(reader.snapshot().tasks.is_empty());
        LocalSource::open(path.clone(), Mode::Pve).set_task("t", TaskState::Completed).unwrap();
        reader.reload_if_changed();
        let after = reader.snapshot();
        assert!(after.tasks["t"].complete);
        reader.reload_if_changed();
        assert!(Arc::ptr_eq(&after, &reader.snapshot()), "an unchanged file is not parsed again");
    }

    #[test]
    fn edits_repair_a_damaged_backup() {
        let path = tmp("damaged");
        std::fs::write(&path, backup(json!({"pve": 5, "pvp": {"taskCompletions": 3}})).to_string()).unwrap();
        let pve = LocalSource::open(path.clone(), Mode::Pve);
        assert_eq!(*pve.snapshot(), Progress::default());
        pve.set_task("a", TaskState::Completed).unwrap();
        let pvp = LocalSource::open(path.clone(), Mode::Pvp);
        pvp.set_task("b", TaskState::Failed).unwrap();
        let doc = read_doc(&path);
        assert_eq!(doc["pve"]["taskCompletions"]["a"]["complete"], true);
        assert_eq!(doc["pvp"]["taskCompletions"]["b"]["failed"], true, "a non-object collection is replaced");
        assert!(doc["seasonal"].is_object(), "a missing section is added");
    }

    #[test]
    fn an_objective_is_stored_with_its_count() {
        let path = tmp("objective");
        let s = LocalSource::open(path.clone(), Mode::Seasonal);
        s.set_objective("o", true, 3).unwrap();
        assert_eq!(s.snapshot().objectives["o"], ObjectiveProgress { complete: true, count: 3, invalid: false });
        assert!(s.writes_objectives());
        assert_eq!(read_doc(&path)["currentGameMode"], "seasonal");
    }

    #[test]
    fn hideout_level_marks_every_level_up_to_it_and_clears_higher_ones() {
        let path = tmp("hideout");
        let s = LocalSource::open(path.clone(), Mode::Pve);
        s.set_hideout_level("a", 3).unwrap();
        s.set_hideout_level("b", 1).unwrap();
        s.set_hideout_level("a", 1).unwrap();
        let modules = &read_doc(&path)["pve"]["hideoutModules"];
        let mut keys: Vec<_> = modules.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["a-1", "b-1"]);
    }
}

#[cfg(test)]
mod tracker_tests;

/// Smoke tests against the live TarkovTracker API; `#[ignore]`d, run by the daily pipeline.
#[cfg(test)]
mod tracker_live;
