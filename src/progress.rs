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
        "appVersion": "ChadOverlay 0.1",
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
    /// Earliest time the pending queue may be retried.
    retry_at: Option<std::time::Instant>,
}

pub struct TrackerSource {
    mode: Mode,
    token: String,
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
        .user_agent("ChadOverlay/0.1")
        .build())
}

/// `GET /token`: (permissions, game mode) for a token, for the Settings `Test` button.
pub fn test_token(token: &str) -> anyhow::Result<(Vec<String>, String)> {
    let v: Value = tracker_agent()?.get(&format!("{API}/token")).set("Authorization", &format!("Bearer {token}")).call()?.into_json()?;
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
        let src = Arc::new(TrackerSource {
            mode,
            token,
            agent: tracker_agent()?,
            pending_path,
            notify,
            state: Mutex::new(TrackerState { snapshot: Arc::new(Progress::default()), etag: None, retry_at: None }),
            queue_lock: Mutex::new(()),
        });
        let weak: Weak<TrackerSource> = Arc::downgrade(&src);
        std::thread::Builder::new()
            .name(format!("tracker-{}", mode.key()))
            .spawn(move || {
                let mut last_poll: Option<std::time::Instant> = None;
                loop {
                    let Some(src) = weak.upgrade() else { return };
                    if last_poll.is_none_or(|t| t.elapsed() >= POLL) {
                        src.refresh();
                        last_poll = Some(std::time::Instant::now());
                    }
                    src.flush_pending();
                    drop(src);
                    std::thread::sleep(Duration::from_secs(5));
                }
            })?;
        Ok(src)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// `GET /progress` with `If-None-Match`.
    pub fn refresh(&self) {
        let etag = self.state.lock().unwrap().etag.clone();
        let mut req = self.agent.get(&format!("{API}/progress")).set("Authorization", &self.auth());
        if let Some(e) = &etag {
            req = req.set("If-None-Match", e);
        }
        match req.call() {
            Ok(r) if r.status() == 304 => {}
            Ok(r) => {
                let tag = r.header("ETag").map(str::to_owned);
                match r.into_json::<Value>() {
                    Ok(v) => {
                        let mut st = self.state.lock().unwrap();
                        st.snapshot = Arc::new(parse_tracker(&v));
                        st.etag = tag;
                    }
                    Err(e) => log::warn!("TarkovTracker {}: progress unreadable: {e}", self.mode.key()),
                }
            }
            Err(ureq::Error::Status(code @ (401 | 403), _)) => {
                log::warn!("TarkovTracker {}: GET /progress: {code}", self.mode.key());
                (self.notify)("TarkovTracker rejected the token", self.mode.label());
            }
            Err(e) => log::warn!("TarkovTracker {}: GET /progress failed: {e}", self.mode.key()),
        }
    }

    fn post(&self, path: &str, body: &Value, what: &str) -> Result<(), WriteError> {
        let res = self.agent.post(&format!("{API}{path}")).set("Authorization", &self.auth()).send_json(body.clone());
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

    fn write(&self, path: String, body: Value, what: String) -> anyhow::Result<()> {
        let queued = {
            let _g = self.queue_lock.lock().unwrap();
            self.load_queue().iter().any(|p| p.mode == self.mode)
        };
        // Keep order: while older writes wait, queue behind them.
        if queued {
            self.enqueue(Pending { mode: self.mode, path, body, what: what.clone() }, None);
            return Ok(());
        }
        match self.post(&path, &body, &what) {
            Ok(()) => {
                self.refresh();
                Ok(())
            }
            Err(WriteError::Retry(wait, why)) => {
                self.enqueue(Pending { mode: self.mode, path, body, what }, Some(wait));
                log::info!("queued for retry ({why})");
                Ok(())
            }
            Err(WriteError::Fatal(why)) => {
                if why.starts_with("401") || why.starts_with("403") {
                    (self.notify)("TarkovTracker rejected the token", self.mode.label());
                }
                anyhow::bail!("TarkovTracker {what}: {why}")
            }
        }
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
            self.refresh();
        }
    }
}

impl ProgressSource for TrackerSource {
    fn snapshot(&self) -> Arc<Progress> {
        self.state.lock().unwrap().snapshot.clone()
    }

    fn set_task(&self, id: &str, state: TaskState) -> anyhow::Result<()> {
        self.write(format!("/progress/task/{id}"), json!({"state": state.api()}), format!("task {id} -> {}", state.api()))
    }

    fn set_objective(&self, id: &str, complete: bool, count: u32) -> anyhow::Result<()> {
        let state = if complete { "completed" } else { "uncompleted" };
        self.write(
            format!("/progress/task/objective/{id}"),
            json!({"state": state, "count": count}),
            format!("objective {id} -> {state} ({count})"),
        )
    }

    fn set_hideout_level(&self, _station_id: &str, _level: u8) -> anyhow::Result<()> {
        anyhow::bail!("TarkovTracker API has no hideout write")
    }

    fn set_level(&self, level: u8) -> anyhow::Result<()> {
        self.write(format!("/progress/level/{level}"), json!({}), format!("level -> {level}"))
    }

    fn writes_objectives(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chadoverlay-test-{}-{name}", std::process::id()));
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
        assert!(p.tasks["t1"].complete && !p.tasks["t1"].failed);
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
}
