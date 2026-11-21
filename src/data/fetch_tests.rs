//! Loading the catalog over HTTP: json.tarkov.dev's endpoints plus TarkovTracker's `tasks-core`, the on-disk cache
//! and the background `Store`, against a scripted local server.
use super::tests::base;
use super::*;
use crate::testutil::{MockServer, Reply};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("chadscanner-test-{}-data-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const TRACKER_BODY: &str = r#"{"data":{"tasks":[{"id":"overlay_only","name":"@tt:overlay:/tasks/overlay_only/name","minPlayerLevel":3}]}}"#;
/// Raw `strings` response: task names plus an unrelated entry that pruning must drop.
const STRINGS_BODY: &str = r#"{"data":{"strings":{"@tt:overlay:/tasks/overlay_only/name":"Overlay Only","@tt:regular:maps:x Name":"Customs"}},"dataOverlay":{"version":"1"}}"#;

/// Serves the `base()` catalog for `mode` on json.tarkov.dev's paths, `tracker` for `tasks-core` and `strings` for the
/// `strings` bundle.
fn catalog_server(mode: Mode, tracker: impl Fn() -> Reply + Send + 'static, strings: impl Fn() -> Reply + Send + 'static) -> MockServer {
    let bodies = base();
    MockServer::start(move |req| {
        let path = req.path.split('?').next().unwrap_or("");
        if path == "/api/tarkov/tasks-core" {
            return tracker();
        }
        if path == "/api/tarkov/strings" {
            return strings();
        }
        match path.strip_prefix(&format!("/{}/", mode.api())).and_then(|ep| bodies.get(ep)) {
            Some(body) => Reply::new(200, body.clone()),
            None => Reply::new(404, "no such endpoint"),
        }
    })
}

fn hosts(s: &MockServer) -> Hosts {
    Hosts { tarkov_dev: s.url.clone(), tracker: s.url.clone() }
}

fn agent_for_tests() -> ureq::Agent {
    agent().unwrap()
}

fn cached(dir: &Path, mode: Mode, ep: &str) -> Option<Vec<u8>> {
    std::fs::read(cache_file(dir, mode, ep)).ok()
}

#[test]
fn tasks_core_url_names_the_game_mode_and_language() {
    assert_eq!(
        tracker_tasks_url("https://tarkovtracker.org", Mode::Pvp),
        "https://tarkovtracker.org/api/tarkov/tasks-core?gameMode=regular&lang=en"
    );
    assert_eq!(tracker_tasks_url("https://x", Mode::Pve), "https://x/api/tarkov/tasks-core?gameMode=pve&lang=en");
    assert_eq!(tracker_tasks_url("https://x", Mode::Seasonal), "https://x/api/tarkov/tasks-core?gameMode=pvp-season&lang=en");
}

#[test]
fn the_default_hosts_are_the_real_services() {
    let h = Hosts::default();
    assert_eq!((h.tarkov_dev.as_str(), h.tracker.as_str()), ("https://json.tarkov.dev", "https://tarkovtracker.org"));
}

#[test]
fn fetch_all_merges_tracker_tasks_and_caches_every_body() {
    let s = catalog_server(Mode::Pve, || Reply::new(200, TRACKER_BODY).header("x-overlay-version", "1"), || Reply::new(200, STRINGS_BODY));
    let dir = tmp_dir("fetch");
    let c = fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).unwrap();
    assert!(c.tasks.contains_key("q"), "json.tarkov.dev tasks");
    assert_eq!(c.tasks["overlay_only"].min_player_level, 3, "TarkovTracker's overlay task");
    assert_eq!(c.tasks["overlay_only"].name, "Overlay Only", "`@tt:` name resolved");
    for ep in ENDPOINTS.iter().copied().chain([TRACKER_TASKS, TRACKER_STRINGS]) {
        assert!(cached(&dir, Mode::Pve, ep).is_some(), "{ep} cached");
    }
    let strings: Value = serde_json::from_slice(&cached(&dir, Mode::Pve, TRACKER_STRINGS).unwrap()).unwrap();
    let keys: Vec<_> = strings["data"]["strings"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["@tt:overlay:/tasks/overlay_only/name"], "the cached bundle is pruned to task names");
    let tt = s.requests().into_iter().find(|r| r.path.starts_with("/api/tarkov/tasks-core")).expect("tasks-core requested");
    assert_eq!(tt.path, "/api/tarkov/tasks-core?gameMode=pve&lang=en");
    assert!(tt.header("user-agent").is_some_and(|ua| ua.starts_with("ChadScanner/")));
    let st = s.requests().into_iter().find(|r| r.path.starts_with("/api/tarkov/strings")).expect("strings requested");
    assert_eq!(st.path, "/api/tarkov/strings?lang=en");
    // And what was cached rebuilds the same catalog offline.
    assert_eq!(load_cached(&dir, Mode::Pve).unwrap().tasks["overlay_only"].name, "Overlay Only");
}

/// tasks-core answering with a switchable `x-overlay-version` header (none when `None`).
fn versioned_server(version: Arc<Mutex<Option<&'static str>>>) -> MockServer {
    catalog_server(
        Mode::Pve,
        move || {
            let r = Reply::new(200, TRACKER_BODY);
            match *version.lock().unwrap() {
                Some(v) => r.header("x-overlay-version", v),
                None => r,
            }
        },
        || Reply::new(200, STRINGS_BODY),
    )
}

#[test]
fn strings_are_refetched_only_when_the_overlay_version_changes() {
    let dir = tmp_dir("strings-version");
    let version = Arc::new(Mutex::new(Some("1")));
    let s = versioned_server(version.clone());
    let fetch = || fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).unwrap();
    fetch();
    assert_eq!(s.count("GET", "/api/tarkov/strings"), 1, "first run downloads the bundle");
    fetch();
    assert_eq!(s.count("GET", "/api/tarkov/strings"), 1, "same overlay version: cached bundle reused");
    *version.lock().unwrap() = Some("2");
    fetch();
    assert_eq!(s.count("GET", "/api/tarkov/strings"), 2, "new overlay version: bundle downloaded again");
    *version.lock().unwrap() = None;
    fetch();
    assert_eq!(s.count("GET", "/api/tarkov/strings"), 2, "no version header and a cache: no refetch");
}

#[test]
fn a_failing_strings_fetch_keeps_the_cached_names() {
    let dir = tmp_dir("strings-fail");
    std::fs::write(cache_file(&dir, Mode::Pve, TRACKER_STRINGS), prune_strings(STRINGS_BODY.as_bytes()).unwrap()).unwrap();
    for bad in [
        Reply::new(500, "boom"),
        Reply::new(200, "<html>Cloudflare</html>"),
        Reply::new(200, r#"{"data":{"strings":{"@tt:regular:x":"y"}}}"#),
    ] {
        let bad = Mutex::new(Some(bad));
        let s = catalog_server(
            Mode::Pve,
            || Reply::new(200, TRACKER_BODY).header("x-overlay-version", "2"),
            move || bad.lock().unwrap().take().unwrap_or_else(|| Reply::new(500, "")),
        );
        let c = fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).unwrap();
        assert_eq!(s.count("GET", "/api/tarkov/strings"), 1, "the bundle was attempted");
        assert_eq!(c.tasks["overlay_only"].name, "Overlay Only");
    }
}

/// The reported bug: a tracker-only task showed `@tt:overlay:/tasks/...` on the card. Whatever the strings bundle does
/// (missing, down, garbage, lacking the key), no task name may be a raw reference, fetched or loaded from cache.
#[test]
fn no_strings_outcome_ever_shows_a_raw_tt_reference() {
    let outcomes: [(&str, &str, u16); 5] = [
        ("404", "", 404),
        ("500", "down", 500),
        ("html", "<html>", 200),
        ("no key", r#"{"data":{"strings":{"@tt:overlay:/tasks/other/name":"Other"}}}"#, 200),
        ("renamed format", r#"{"data":{"strings":{"@tt:overlay:/quests/overlay_only/title":"X"}}}"#, 200),
    ];
    for (what, body, status) in outcomes {
        let dir = tmp_dir(&format!("no-raw-{}", what.replace(' ', "-")));
        let s = catalog_server(Mode::Pve, || Reply::new(200, TRACKER_BODY), move || Reply::new(status, body));
        let fetched = fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).unwrap();
        let offline = load_cached(&dir, Mode::Pve).unwrap();
        for (src, c) in [("fetched", fetched), ("cached", offline)] {
            let raw: Vec<_> = c.tasks.values().filter(|t| t.name.contains("@tt:")).map(|t| &t.id).collect();
            assert!(raw.is_empty(), "{what}/{src}: raw reference shown for {raw:?}");
            assert_eq!(c.tasks["overlay_only"].name, "Overlay only", "{what}/{src}: readable id fallback");
        }
    }
}

#[test]
fn a_failing_tasks_core_falls_back_to_the_cached_copy() {
    let dir = tmp_dir("fallback");
    std::fs::write(cache_file(&dir, Mode::Pve, TRACKER_TASKS), TRACKER_BODY).unwrap();
    for bad in [Reply::new(500, "boom"), Reply::new(200, "<html>Cloudflare</html>"), Reply::new(200, r#"{"nodata":1}"#)] {
        let bad = Mutex::new(Some(bad));
        let s =
            catalog_server(Mode::Pve, move || bad.lock().unwrap().take().unwrap_or_else(|| Reply::new(500, "")), || Reply::new(404, ""));
        let c = fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).unwrap();
        assert!(c.tasks.contains_key("overlay_only"), "the cached overlay survives a bad answer");
    }
    assert_eq!(cached(&dir, Mode::Pve, TRACKER_TASKS).unwrap(), TRACKER_BODY.as_bytes(), "a bad answer never replaces the cache");
}

#[test]
fn tasks_core_is_optional() {
    let s = catalog_server(Mode::Pve, || Reply::new(503, "down"), || Reply::new(404, ""));
    let dir = tmp_dir("optional");
    let c = fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).unwrap();
    assert!(c.tasks.contains_key("q") && !c.tasks.contains_key("overlay_only"));
    assert!(cached(&dir, Mode::Pve, TRACKER_TASKS).is_none());
}

#[test]
fn a_missing_required_endpoint_fails_the_fetch_and_leaves_the_cache_alone() {
    let dir = tmp_dir("required");
    std::fs::write(cache_file(&dir, Mode::Pve, "items"), "good cache").unwrap();
    // Serves the Pvp paths only, so every Pve endpoint is a 404.
    let s = catalog_server(Mode::Pvp, || Reply::new(200, TRACKER_BODY), || Reply::new(404, ""));
    assert!(fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).is_err());
    assert_eq!(cached(&dir, Mode::Pve, "items").unwrap(), b"good cache");
}

#[test]
fn an_unparseable_catalog_is_not_cached() {
    let dir = tmp_dir("unparseable");
    let mut bodies = base();
    bodies.insert("items", b"{\"data\":[]}".to_vec());
    let s = MockServer::start(move |req| {
        let ep = req.path.rsplit('/').next().unwrap_or("").to_owned();
        match bodies.get(ep.as_str()) {
            Some(b) => Reply::new(200, b.clone()),
            None => Reply::new(404, ""),
        }
    });
    assert!(fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&s)).is_err());
    assert!(cached(&dir, Mode::Pve, "tasks").is_none(), "a set that did not parse is not cached");
}

#[test]
fn a_cache_that_cannot_be_written_does_not_fail_the_fetch() {
    let dir = tmp_dir("nocache");
    let blocker = dir.join("file");
    std::fs::write(&blocker, "x").unwrap();
    let s = catalog_server(Mode::Pve, || Reply::new(200, TRACKER_BODY), || Reply::new(404, ""));
    let c = fetch_all(&agent_for_tests(), &blocker.join("cache"), Mode::Pve, &hosts(&s)).unwrap();
    assert!(c.tasks.contains_key("q"));
}

#[test]
fn load_cached_needs_every_required_endpoint() {
    let dir = tmp_dir("loadcached");
    assert!(load_cached(&dir, Mode::Pve).is_none(), "nothing cached");
    for (ep, body) in base() {
        if ep != "crafts" {
            std::fs::write(cache_file(&dir, Mode::Pve, ep), body).unwrap();
        }
    }
    assert!(load_cached(&dir, Mode::Pve).is_none(), "one endpoint missing");
    std::fs::write(cache_file(&dir, Mode::Pve, "crafts"), "garbage").unwrap();
    assert!(load_cached(&dir, Mode::Pve).is_none(), "an unusable cache is ignored");
    std::fs::write(cache_file(&dir, Mode::Pve, "crafts"), base()["crafts"].clone()).unwrap();
    assert!(load_cached(&dir, Mode::Pve).unwrap().tasks.contains_key("q"));
    assert!(load_cached(&dir, Mode::Pvp).is_none(), "caches are per mode");
}

#[test]
fn the_agent_identifies_itself() {
    let s = MockServer::start(|_| Reply::new(200, "{}"));
    agent_for_tests().get(&format!("{}/x", s.url)).call().unwrap();
    assert!(s.requests()[0].header("user-agent").is_some_and(|ua| ua.starts_with("ChadScanner/")));
}

// ---------------------------------------------------------------- tasks-core parsing

#[test]
fn tracker_tasks_skips_unusable_elements() {
    let tasks = tracker_tasks(
        br#"{"data":{"tasks":[
            {"id":"a","name":"A"},
            {"id":""},
            {"id":"b","objectives":"not a list"},
            {"id":"c","name":null,"taskRequirements":[{"task":null}, {"task":{"id":"a"},"status":["complete"]}]}
        ]}}"#,
    );
    let ids: Vec<_> = tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["a", "c"], "an empty id and an element of the wrong shape are dropped, the rest kept");
    assert_eq!(tasks[1].name, "c", "a missing name falls back to the id");
    assert_eq!(tasks[1].task_requirements.len(), 1, "a requirement without a task id is dropped");
}

#[test]
fn tracker_tasks_is_empty_for_an_unusable_response() {
    assert!(tracker_tasks(b"<html>").is_empty(), "not JSON");
    assert!(tracker_tasks(br#"{"nodata":[]}"#).is_empty(), "no data field");
    assert!(tracker_tasks(br#"{"data":{"tasks":{"a":1}}}"#).is_empty(), "tasks is not a list");
    assert!(tracker_tasks(br#"{"data":{}}"#).is_empty(), "no tasks");
}

// ---------------------------------------------------------------- Store

fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An `on_missing` that counts its calls.
fn counter() -> (Arc<AtomicUsize>, impl Fn() + Send + 'static) {
    let n = Arc::new(AtomicUsize::new(0));
    let seen = n.clone();
    (n, move || {
        seen.fetch_add(1, Ordering::SeqCst);
    })
}

const LONG: Duration = Duration::from_secs(600);

#[test]
fn the_store_swaps_in_the_first_fetched_catalog() {
    let s = catalog_server(Mode::Pve, || Reply::new(200, TRACKER_BODY), || Reply::new(404, ""));
    let (missing, on_missing) = counter();
    let store = Store::start_at(hosts(&s), Mode::Pve, tmp_dir("store"), on_missing, LONG, LONG);
    let c = wait_for("the first fetch", || store.get());
    assert!(c.tasks.contains_key("overlay_only"));
    assert_eq!(missing.load(Ordering::SeqCst), 0);
}

#[test]
fn the_store_says_once_that_there_is_no_data_and_keeps_retrying() {
    let s = MockServer::start(|_| Reply::new(500, "down"));
    let (missing, on_missing) = counter();
    let store = Store::start_at(hosts(&s), Mode::Pve, tmp_dir("missing"), on_missing, LONG, Duration::from_millis(10));
    wait_for("three failed rounds", || (s.count("GET", "/pve/items") >= 3).then_some(()));
    assert_eq!(missing.load(Ordering::SeqCst), 1, "on_missing fires once");
    assert!(store.get().is_none());
    drop(store);
    // The thread ends once the store is gone: no further requests after it has noticed.
    std::thread::sleep(Duration::from_millis(100));
    let n = s.requests().len();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(s.requests().len(), n);
}

#[test]
fn a_store_with_a_cache_serves_it_at_once_and_stays_quiet_when_the_fetch_fails() {
    let dir = tmp_dir("quiet");
    let good = catalog_server(Mode::Pve, || Reply::new(200, TRACKER_BODY), || Reply::new(404, ""));
    fetch_all(&agent_for_tests(), &dir, Mode::Pve, &hosts(&good)).unwrap();
    let down = MockServer::start(|_| Reply::new(500, "down"));
    let (missing, on_missing) = counter();
    let store = Store::start_at(hosts(&down), Mode::Pve, dir, on_missing, LONG, Duration::from_millis(10));
    assert!(store.get().unwrap().tasks.contains_key("overlay_only"), "the cache is served before any fetch finishes");
    wait_for("two failed rounds", || (down.count("GET", "/pve/items") >= 2).then_some(()));
    assert!(store.get().is_some());
    assert_eq!(missing.load(Ordering::SeqCst), 0);
}
