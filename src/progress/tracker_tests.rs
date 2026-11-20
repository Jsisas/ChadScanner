//! `TrackerSource` and `test_token` against a scripted local API: every status the real gateway documents
//! (200, 304, 400, 401, 403, 429, 5xx, no connection) and what each does to the snapshot, the retry queue and the toasts.
use super::*;
use crate::testutil::{MockServer, Reply};
use std::collections::VecDeque;

type Toasts = Arc<Mutex<Vec<(String, String)>>>;

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("chadscanner-test-{}-tt-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A source that does not poll, the toasts it raised, and its pending file.
fn source_in(url: &str, mode: Mode, dir: &Path) -> (TrackerSource, Toasts) {
    let toasts = Toasts::default();
    let sink = toasts.clone();
    let notify: Notify = Arc::new(move |title, body| sink.lock().unwrap().push((title.to_owned(), body.to_owned())));
    (TrackerSource::new(url, mode, "PVE_testtoken".into(), dir.join("pending.json"), notify).unwrap(), toasts)
}

fn source(url: &str, name: &str) -> (TrackerSource, Toasts) {
    source_in(url, Mode::Pve, &tmp_dir(name))
}

/// Replies in order; 418 once the script runs out, so an unexpected request shows up as a failure.
fn scripted(replies: Vec<Reply>) -> MockServer {
    let queue = Mutex::new(VecDeque::from(replies));
    MockServer::start(move |_| queue.lock().unwrap().pop_front().unwrap_or_else(|| Reply::new(418, "script exhausted")))
}

/// A URL nothing listens on.
fn dead_url() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", l.local_addr().unwrap())
}

fn ok() -> Reply {
    Reply::json(200, &json!({"success": true}))
}

fn progress_reply() -> Reply {
    Reply::json(
        200,
        &json!({"success": true, "data": {
            "tasksProgress": [{"id": "t1", "complete": true, "failed": false, "invalid": false}],
            "taskObjectivesProgress": [{"id": "o1", "complete": false, "count": 2}],
            "hideoutModulesProgress": [{"id": "s-1", "complete": true}],
            "hideoutPartsProgress": [{"id": "p1", "complete": false, "count": 3}],
            "playerLevel": 17, "pmcFaction": "BEAR"}}),
    )
}

fn pending_for(mode: Mode, what: &str) -> Pending {
    Pending { mode, path: "/progress/level/9".into(), body: json!({}), what: what.into() }
}

fn seed_queue(src: &TrackerSource, items: &[Pending]) {
    src.save_queue(items);
}

fn toast_titles(t: &Toasts) -> Vec<String> {
    t.lock().unwrap().iter().map(|(title, _)| title.clone()).collect()
}

fn post_paths(s: &MockServer) -> Vec<String> {
    s.requests().iter().filter(|r| r.method == "POST").map(|r| r.path.clone()).collect()
}

// ---------------------------------------------------------------- test_token

#[test]
fn test_token_reports_permissions_and_game_mode() {
    let s = scripted(vec![Reply::json(200, &json!({"success": true, "permissions": ["GP", "WP"], "gameMode": "pve"}))]);
    let (perms, mode) = test_token_at(&s.url, "PVE_abc").unwrap();
    assert_eq!((perms, mode.as_str()), (vec!["GP".to_owned(), "WP".to_owned()], "pve"));
    let r = &s.requests()[0];
    assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/token"));
    assert_eq!(r.header("authorization"), Some("Bearer PVE_abc"));
    // The gateway rejects requests without a 5-200 character User-Agent.
    assert!(
        r.header("user-agent").is_some_and(|ua| ua.starts_with("ChadScanner/") && (5..=200).contains(&ua.len())),
        "{:?}",
        r.header("user-agent")
    );
}

#[test]
fn test_token_tolerates_missing_fields() {
    let s = scripted(vec![Reply::json(200, &json!({"success": true}))]);
    assert_eq!(test_token_at(&s.url, "t").unwrap(), (vec![], "?".to_owned()));
}

#[test]
fn test_token_rejection_is_an_error_naming_the_status() {
    let s = scripted(vec![Reply::json(401, &json!({"success": false, "error": "Invalid token"}))]);
    let e = test_token_at(&s.url, "bad").unwrap_err();
    assert!(e.to_string().contains("401"), "{e:#}");
}

#[test]
fn test_token_without_a_connection_is_an_error() {
    assert!(test_token_at(&dead_url(), "t").is_err());
}

#[test]
fn test_token_rejects_a_non_json_answer() {
    let s = scripted(vec![Reply::new(200, "<html>maintenance</html>")]);
    assert!(test_token_at(&s.url, "t").is_err());
}

// ---------------------------------------------------------------- reads

#[test]
fn refresh_reads_progress_then_revalidates_with_the_etag() {
    let s = MockServer::start(|req| match req.header("if-none-match") {
        Some(_) => Reply::new(304, "").header("X-RateLimit-Remaining", "41"),
        None => progress_reply().header("ETag", "W/\"v1\"").header("X-RateLimit-Remaining", "42"),
    });
    let (src, toasts) = source(&s.url, "etag");
    src.refresh();
    let first = src.snapshot();
    assert!(first.tasks["t1"].complete);
    assert_eq!(first.objectives["o1"].count, 2);
    assert!(first.hideout_modules.contains("s-1"));
    assert_eq!(first.hideout_parts["p1"], PartProgress { complete: false, count: 3 });
    assert_eq!((first.level, first.faction.as_deref()), (Some(17), Some("BEAR")));
    assert_eq!(src.state.lock().unwrap().reads_left, Some(42));

    src.refresh();
    assert!(Arc::ptr_eq(&first, &src.snapshot()), "a 304 keeps the snapshot");
    assert_eq!(src.state.lock().unwrap().reads_left, Some(41));

    let reqs = s.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].path, "/progress");
    assert_eq!(reqs[0].header("authorization"), Some("Bearer PVE_testtoken"));
    assert_eq!(reqs[0].header("if-none-match"), None);
    assert_eq!(reqs[1].header("if-none-match"), Some("W/\"v1\""));
    assert!(toasts.lock().unwrap().is_empty());
}

#[test]
fn refresh_ignores_an_unparseable_quota_header() {
    let s = scripted(vec![progress_reply().header("X-RateLimit-Remaining", "lots")]);
    let (src, _) = source(&s.url, "quota");
    src.refresh();
    assert_eq!(src.state.lock().unwrap().reads_left, None);
    assert!(src.snapshot().tasks.contains_key("t1"));
}

#[test]
fn refresh_keeps_the_snapshot_when_the_body_is_not_json() {
    let s = scripted(vec![progress_reply().header("ETag", "W/\"v1\""), Reply::new(200, "<html>").header("ETag", "W/\"v2\"")]);
    let (src, _) = source(&s.url, "badbody");
    src.refresh();
    src.refresh();
    assert!(src.snapshot().tasks.contains_key("t1"));
    assert_eq!(src.state.lock().unwrap().etag.as_deref(), Some("W/\"v1\""), "an unreadable body must not advance the etag");
}

#[test]
fn refresh_429_pauses_reads_for_retry_after() {
    let s = scripted(vec![Reply::json(429, &json!({"success": false})).header("Retry-After", "120").header("X-RateLimit-Remaining", "0")]);
    let (src, toasts) = source(&s.url, "r429");
    src.refresh();
    {
        let st = src.state.lock().unwrap();
        let until = st.reads_paused_until.expect("paused");
        assert!(
            until > std::time::Instant::now() + Duration::from_secs(100) && until <= std::time::Instant::now() + Duration::from_secs(120)
        );
        assert_eq!(st.reads_left, Some(0));
    }
    src.refresh();
    assert_eq!(s.requests().len(), 1, "no read while paused");
    assert!(toasts.lock().unwrap().is_empty());
}

#[test]
fn refresh_429_without_retry_after_pauses_ten_minutes() {
    let s = scripted(vec![Reply::json(429, &json!({"success": false}))]);
    let (src, _) = source(&s.url, "r429b");
    src.refresh();
    let until = src.state.lock().unwrap().reads_paused_until.expect("paused");
    assert!(until > std::time::Instant::now() + Duration::from_secs(590));
}

#[test]
fn refresh_reads_again_once_the_pause_is_over() {
    let s = scripted(vec![progress_reply()]);
    let (src, _) = source(&s.url, "resume");
    src.state.lock().unwrap().reads_paused_until = Some(std::time::Instant::now());
    src.refresh();
    assert_eq!(s.requests().len(), 1);
    assert!(src.snapshot().tasks.contains_key("t1"));
}

#[test]
fn refresh_toasts_a_rejected_token() {
    for code in [401, 403] {
        let s = scripted(vec![Reply::json(code, &json!({"success": false, "error": "Invalid token"}))]);
        let (src, toasts) = source(&s.url, &format!("rej{code}"));
        src.refresh();
        assert_eq!(*toasts.lock().unwrap(), vec![("TarkovTracker rejected the token".to_owned(), "PvE".to_owned())], "{code}");
        assert!(src.snapshot().tasks.is_empty());
    }
}

#[test]
fn refresh_survives_server_errors_and_a_dead_connection() {
    let s = scripted(vec![Reply::json(503, &json!({"success": false}))]);
    let (src, toasts) = source(&s.url, "r503");
    src.refresh();
    let (dead, dead_toasts) = source(&dead_url(), "rdead");
    dead.refresh();
    for t in [&toasts, &dead_toasts] {
        assert!(t.lock().unwrap().is_empty(), "transient failures only go to the log");
    }
    assert!(src.snapshot().tasks.is_empty() && dead.snapshot().tasks.is_empty());
}

#[test]
fn refresh_reports_what_the_server_did_not_keep() {
    let s = MockServer::start(|req| if req.method == "POST" { ok() } else { progress_reply() });
    let (src, toasts) = source(&s.url, "unkept");
    src.set_objective("o1", true, 5).unwrap();
    src.set_task("t2", TaskState::Completed).unwrap();
    // The server never heard of o1=5 or t2.
    src.refresh();
    assert_eq!(*toasts.lock().unwrap(), vec![("TarkovTracker did not keep 2 change(s)".to_owned(), "see log.txt".to_owned())]);
    let snap = src.snapshot();
    assert_eq!(snap.objectives["o1"].count, 2, "the server wins");
    assert!(!snap.tasks.contains_key("t2"));
    let st = src.state.lock().unwrap();
    assert!(st.written_objectives.is_empty() && st.written_tasks.is_empty());
    assert!(st.sync_at.is_none(), "a read clears the pending confirmation");
}

#[test]
fn refresh_is_silent_when_the_server_kept_the_writes() {
    let s = MockServer::start(|req| {
        if req.method == "POST" {
            ok()
        } else {
            Reply::json(
                200,
                &json!({"data": {"tasksProgress": [{"id": "t2", "complete": true, "failed": true}],
                    "taskObjectivesProgress": [{"id": "o1", "complete": true, "count": 5}]}}),
            )
        }
    });
    let (src, toasts) = source(&s.url, "kept");
    src.set_objective("o1", true, 5).unwrap();
    src.set_task("t2", TaskState::Failed).unwrap();
    src.refresh();
    assert!(toasts.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------- writes

#[test]
fn set_task_posts_the_state_and_shows_it_at_once() {
    let s = scripted(vec![ok(), ok()]);
    let (src, _) = source(&s.url, "settask");
    src.set_task("t1", TaskState::Completed).unwrap();
    src.set_task("t2", TaskState::Failed).unwrap();
    let reqs = s.requests();
    assert_eq!((reqs[0].path.as_str(), reqs[0].json()), ("/progress/task/t1", json!({"state": "completed"})));
    assert_eq!((reqs[1].path.as_str(), reqs[1].json()), ("/progress/task/t2", json!({"state": "failed"})));
    assert_eq!(reqs[0].header("authorization"), Some("Bearer PVE_testtoken"));
    assert_eq!(reqs[0].header("content-type"), Some("application/json"));
    let snap = src.snapshot();
    assert_eq!(snap.tasks["t1"], TaskProgress { complete: true, failed: false, invalid: false });
    assert_eq!(
        snap.tasks["t2"],
        TaskProgress { complete: true, failed: true, invalid: false },
        "the API stores failed as complete + failed"
    );
    let st = src.state.lock().unwrap();
    assert!(st.etag.is_none(), "a 304 would restore the pre-write copy");
    assert!(st.sync_at.is_some_and(|t| t > std::time::Instant::now() + Duration::from_secs(25)));
    assert_eq!(st.written_tasks.len(), 2);
}

#[test]
fn set_objective_posts_state_and_count() {
    let s = scripted(vec![ok(), ok()]);
    let (src, _) = source(&s.url, "setobj");
    src.set_objective("o1", true, 4).unwrap();
    src.set_objective("o2", false, 1).unwrap();
    let reqs = s.requests();
    assert_eq!((reqs[0].path.as_str(), reqs[0].json()), ("/progress/task/objective/o1", json!({"state": "completed", "count": 4})));
    assert_eq!((reqs[1].path.as_str(), reqs[1].json()), ("/progress/task/objective/o2", json!({"state": "uncompleted", "count": 1})));
    let snap = src.snapshot();
    assert_eq!(snap.objectives["o1"], ObjectiveProgress { complete: true, count: 4, invalid: false });
    assert_eq!(snap.objectives["o2"], ObjectiveProgress { complete: false, count: 1, invalid: false });
}

#[test]
fn set_level_posts_the_level() {
    let s = scripted(vec![ok()]);
    let (src, _) = source(&s.url, "setlevel");
    src.set_level(26).unwrap();
    let r = &s.requests()[0];
    assert_eq!((r.method.as_str(), r.path.as_str(), r.json()), ("POST", "/progress/level/26", json!({})));
    assert_eq!(src.snapshot().level, Some(26));
}

#[test]
fn hideout_levels_cannot_be_written_through_the_api() {
    let s = scripted(vec![]);
    let (src, _) = source(&s.url, "hideout");
    let e = src.set_hideout_level("st", 2).unwrap_err();
    assert!(e.to_string().contains("no hideout write"), "{e:#}");
    assert!(s.requests().is_empty());
    assert!(!src.writes_objectives());
}

#[test]
fn a_rejected_write_is_an_error_and_a_toast_and_changes_nothing() {
    for code in [401, 403] {
        let s = scripted(vec![Reply::json(code, &json!({"success": false, "error": "Invalid token"}))]);
        let (src, toasts) = source(&s.url, &format!("wrej{code}"));
        let e = src.set_task("t1", TaskState::Completed).unwrap_err();
        assert!(e.to_string().contains(&code.to_string()), "{e:#}");
        assert_eq!(toast_titles(&toasts), vec!["TarkovTracker rejected the token"]);
        assert!(src.snapshot().tasks.is_empty(), "a refused write is not shown");
        assert!(src.load_queue().is_empty(), "and not retried");
    }
}

#[test]
fn a_bad_request_is_an_error_with_the_servers_reason() {
    let s = scripted(vec![Reply::json(400, &json!({"success": false, "error": "Unknown task"}))]);
    let (src, toasts) = source(&s.url, "w400");
    let e = src.set_task("nope", TaskState::Completed).unwrap_err();
    assert!(e.to_string().contains("400") && e.to_string().contains("Unknown task"), "{e:#}");
    assert!(toasts.lock().unwrap().is_empty(), "only a rejected token toasts");
    assert!(src.load_queue().is_empty());
}

#[test]
fn a_retryable_failure_is_queued_and_shown_but_not_confirmed() {
    let s = scripted(vec![Reply::json(503, &json!({"success": false})).header("Retry-After", "7")]);
    let (src, toasts) = source(&s.url, "w503");
    src.set_objective("o1", true, 2).unwrap();
    let q = src.load_queue();
    assert_eq!(q.len(), 1);
    assert_eq!((q[0].mode, q[0].path.as_str()), (Mode::Pve, "/progress/task/objective/o1"));
    assert_eq!(q[0].body, json!({"state": "completed", "count": 2}));
    assert_eq!(src.snapshot().objectives["o1"].count, 2);
    let st = src.state.lock().unwrap();
    assert!(st.written_objectives.is_empty(), "the server has not accepted it yet");
    let wait = st.retry_at.expect("retry scheduled") - std::time::Instant::now();
    assert!(wait > Duration::from_secs(5) && wait <= Duration::from_secs(7), "{wait:?}");
    assert!(toasts.lock().unwrap().is_empty());
}

#[test]
fn rate_limited_and_unreachable_writes_retry_after_a_minute() {
    let s = scripted(vec![Reply::json(429, &json!({"success": false}))]);
    let (src, _) = source(&s.url, "w429");
    src.set_task("t1", TaskState::Completed).unwrap();
    assert_eq!(src.load_queue().len(), 1);
    assert!(src.state.lock().unwrap().retry_at.unwrap() > std::time::Instant::now() + Duration::from_secs(55));

    let (dead, _) = source(&dead_url(), "wdead");
    dead.set_level(5).unwrap();
    assert_eq!(dead.load_queue().len(), 1);
    assert!(dead.state.lock().unwrap().retry_at.unwrap() > std::time::Instant::now() + Duration::from_secs(55));
    assert_eq!(dead.snapshot().level, Some(5));
}

#[test]
fn writes_queue_behind_older_ones_to_keep_their_order() {
    let s = scripted(vec![]);
    let (src, _) = source(&s.url, "order");
    seed_queue(&src, &[pending_for(Mode::Pve, "older")]);
    src.set_task("t1", TaskState::Completed).unwrap();
    assert!(s.requests().is_empty(), "nothing is sent past a queued write");
    let q = src.load_queue();
    assert_eq!(q.iter().map(|p| p.what.as_str()).collect::<Vec<_>>(), ["older", "task t1 -> completed"]);
    assert!(src.snapshot().tasks["t1"].complete);
    assert!(src.state.lock().unwrap().written_tasks.is_empty());
}

#[test]
fn another_modes_queue_does_not_block_this_mode() {
    let s = scripted(vec![ok()]);
    let (src, _) = source(&s.url, "othermode");
    seed_queue(&src, &[pending_for(Mode::Pvp, "pvp write")]);
    src.set_task("t1", TaskState::Completed).unwrap();
    assert_eq!(post_paths(&s), ["/progress/task/t1"]);
    assert_eq!(src.load_queue().len(), 1, "the other mode's entry stays");
}

// ---------------------------------------------------------------- queue

#[test]
fn flush_sends_queued_writes_in_order_and_schedules_a_confirming_read() {
    let s = scripted(vec![ok(), ok()]);
    let (src, _) = source(&s.url, "flush");
    seed_queue(&src, &[pending_for(Mode::Pve, "one"), pending_for(Mode::Pvp, "theirs"), pending_for(Mode::Pve, "two")]);
    src.state.lock().unwrap().etag = Some("W/\"x\"".into());
    src.flush_pending();
    assert_eq!(s.requests().len(), 2);
    let left = src.load_queue();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].mode, Mode::Pvp);
    let st = src.state.lock().unwrap();
    assert!(st.etag.is_none());
    assert!(st.sync_at.is_some());
}

#[test]
fn flush_does_nothing_for_an_empty_queue() {
    let s = scripted(vec![]);
    let (src, _) = source(&s.url, "flushempty");
    src.flush_pending();
    assert!(s.requests().is_empty());
    assert!(src.state.lock().unwrap().sync_at.is_none());
}

#[test]
fn flush_waits_for_the_retry_time() {
    let s = scripted(vec![ok()]);
    let (src, _) = source(&s.url, "flushwait");
    seed_queue(&src, &[pending_for(Mode::Pve, "one")]);
    src.state.lock().unwrap().retry_at = Some(std::time::Instant::now() + Duration::from_secs(60));
    src.flush_pending();
    assert!(s.requests().is_empty());
    src.state.lock().unwrap().retry_at = Some(std::time::Instant::now());
    src.flush_pending();
    assert_eq!(s.requests().len(), 1);
    assert!(src.load_queue().is_empty());
}

#[test]
fn flush_stops_at_a_retryable_failure_and_keeps_the_rest() {
    let s = scripted(vec![ok(), Reply::json(500, &json!({})).header("Retry-After", "30")]);
    let (src, _) = source(&s.url, "flushretry");
    seed_queue(&src, &[pending_for(Mode::Pve, "a"), pending_for(Mode::Pve, "b"), pending_for(Mode::Pve, "c")]);
    src.flush_pending();
    assert_eq!(s.requests().len(), 2, "c is not tried after b failed");
    assert_eq!(src.load_queue().iter().map(|p| p.what.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    assert!(src.state.lock().unwrap().retry_at.unwrap() > std::time::Instant::now() + Duration::from_secs(25));
}

#[test]
fn flush_drops_a_write_the_server_refuses_and_goes_on() {
    let s = scripted(vec![Reply::json(400, &json!({"error": "Unknown task"})), ok()]);
    let (src, toasts) = source(&s.url, "flushdrop");
    seed_queue(&src, &[pending_for(Mode::Pve, "bad"), pending_for(Mode::Pve, "good")]);
    src.flush_pending();
    assert_eq!(s.requests().len(), 2);
    assert!(src.load_queue().is_empty());
    assert!(toasts.lock().unwrap().is_empty());
}

#[test]
fn flush_drops_and_stops_on_a_rejected_token() {
    for code in [401, 403] {
        let s = scripted(vec![Reply::json(code, &json!({"error": "Invalid token"}))]);
        let (src, toasts) = source(&s.url, &format!("flushrej{code}"));
        seed_queue(&src, &[pending_for(Mode::Pve, "a"), pending_for(Mode::Pve, "b")]);
        src.flush_pending();
        assert_eq!(s.requests().len(), 1, "{code}");
        assert_eq!(src.load_queue().iter().map(|p| p.what.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(toast_titles(&toasts), vec!["TarkovTracker rejected the token"]);
        assert!(src.state.lock().unwrap().sync_at.is_none(), "nothing was sent");
    }
}

#[test]
fn a_corrupt_queue_file_reads_as_empty() {
    let (src, _) = source("http://127.0.0.1:9", "corruptq");
    std::fs::write(&src.pending_path, "{not json").unwrap();
    assert!(src.load_queue().is_empty());
}

#[test]
fn a_queue_that_cannot_be_saved_is_logged_not_fatal() {
    let dir = tmp_dir("unsavable");
    let blocker = dir.join("file");
    std::fs::write(&blocker, "x").unwrap();
    let notify: Notify = Arc::new(|_: &str, _: &str| {});
    let src = TrackerSource::new("http://127.0.0.1:9", Mode::Pve, "t".into(), blocker.join("pending.json"), notify).unwrap();
    src.save_queue(&[pending_for(Mode::Pve, "x")]);
    assert!(src.load_queue().is_empty());
}

// ---------------------------------------------------------------- scans, observed progress, polling

#[test]
fn a_scan_reads_when_the_snapshot_is_stale_and_not_otherwise() {
    let s = scripted(vec![progress_reply(), progress_reply()]);
    let (src, _) = source(&s.url, "stale");
    src.refresh_if_stale();
    assert_eq!(s.requests().len(), 1, "never read yet");
    src.refresh_if_stale();
    assert_eq!(s.requests().len(), 1, "fresh");
    src.state.lock().unwrap().last_refresh = std::time::Instant::now().checked_sub(Duration::from_secs(61));
    src.refresh_if_stale();
    assert_eq!(s.requests().len(), 2, "older than a minute");
}

#[test]
fn a_scan_does_not_read_inside_the_post_write_window_or_on_a_low_quota() {
    let s = scripted(vec![progress_reply()]);
    let (src, _) = source(&s.url, "nostale");
    src.state.lock().unwrap().sync_at = Some(std::time::Instant::now() + Duration::from_secs(20));
    src.refresh_if_stale();
    {
        let mut st = src.state.lock().unwrap();
        st.sync_at = None;
        st.reads_left = Some(READ_RESERVE);
    }
    src.refresh_if_stale();
    assert!(s.requests().is_empty());
    src.state.lock().unwrap().reads_left = Some(READ_RESERVE + 1);
    src.refresh_if_stale();
    assert_eq!(s.requests().len(), 1);
}

#[test]
fn observed_progress_is_merged_without_writing() {
    let s = scripted(vec![progress_reply()]);
    let (src, _) = source(&s.url, "observe");
    src.refresh();
    {
        let mut st = src.state.lock().unwrap();
        let mut snap = (*st.snapshot).clone();
        snap.objectives.insert("o1".into(), ObjectiveProgress { complete: false, count: 2, invalid: true });
        snap.hideout_modules.insert("s-2".into());
        st.snapshot = Arc::new(snap);
        st.etag = Some("W/\"old\"".into());
    }
    let objectives = HashMap::from([
        ("o1".to_owned(), ObjectiveProgress { complete: true, count: 9, invalid: false }),
        ("o9".to_owned(), ObjectiveProgress { complete: false, count: 1, invalid: false }),
    ]);
    let parts = HashMap::from([("p1".to_owned(), PartProgress { complete: true, count: 5 })]);
    let modules = HashMap::from([("s-2".to_owned(), false), ("s-3".to_owned(), true)]);
    src.observe(&objectives, &parts, &modules);
    let snap = src.snapshot();
    assert_eq!(snap.objectives["o1"], ObjectiveProgress { complete: true, count: 9, invalid: true }, "invalid is the server's call");
    assert!(!snap.objectives["o9"].invalid);
    assert_eq!(snap.hideout_parts["p1"], PartProgress { complete: true, count: 5 });
    assert!(snap.hideout_modules.contains("s-1") && snap.hideout_modules.contains("s-3") && !snap.hideout_modules.contains("s-2"));
    assert_eq!(s.requests().len(), 1, "observing never writes");
    assert!(src.state.lock().unwrap().etag.is_none());
}

#[test]
fn poll_reads_on_the_first_tick_then_on_sync_or_interval() {
    let s = MockServer::start(|_| progress_reply());
    let (src, _) = source(&s.url, "poll");
    let mut last = None;
    src.poll(&mut last);
    assert_eq!(s.count("GET", "/progress"), 1);
    assert!(last.is_some());
    src.poll(&mut last);
    assert_eq!(s.count("GET", "/progress"), 1, "nothing due");
    src.state.lock().unwrap().sync_at = Some(std::time::Instant::now());
    src.poll(&mut last);
    assert_eq!(s.count("GET", "/progress"), 2, "confirming read after a write");
    last = std::time::Instant::now().checked_sub(POLL);
    src.poll(&mut last);
    assert_eq!(s.count("GET", "/progress"), 3, "poll interval elapsed");
}

#[test]
fn poll_also_sends_queued_writes() {
    let s = MockServer::start(|req| if req.method == "POST" { ok() } else { progress_reply() });
    let (src, _) = source(&s.url, "pollq");
    seed_queue(&src, &[pending_for(Mode::Pve, "queued")]);
    src.poll(&mut None);
    assert_eq!(post_paths(&s), ["/progress/level/9"]);
    assert!(src.load_queue().is_empty());
}

#[test]
fn the_background_thread_polls_until_the_source_is_dropped() {
    let s = MockServer::start(|_| progress_reply());
    let dir = tmp_dir("thread");
    let src = TrackerSource::start_at(
        &s.url,
        Mode::Pvp,
        "PVP_t".into(),
        dir.join("pending.json"),
        Arc::new(|_: &str, _: &str| {}),
        Duration::from_millis(10),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while src.snapshot().tasks.is_empty() {
        assert!(std::time::Instant::now() < deadline, "the thread never read progress");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(src);
    std::thread::sleep(Duration::from_millis(100));
    let seen = s.requests().len();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(s.requests().len(), seen, "the thread stops once nothing owns the source");
}

#[test]
fn the_api_root_may_end_in_a_slash() {
    let s = scripted(vec![progress_reply()]);
    let (src, _) = source(&format!("{}/", s.url), "slash");
    src.refresh();
    assert_eq!(s.requests()[0].path, "/progress");
}

#[test]
fn task_states_use_the_apis_names() {
    assert_eq!((TaskState::Completed.api(), TaskState::Failed.api()), ("completed", "failed"));
}

// ---------------------------------------------------------------- parsing

#[test]
fn parse_tracker_tolerates_missing_and_odd_fields() {
    let p = parse_tracker(&json!({"data": {"tasksProgress": [{"complete": true}], "playerLevel": 0, "pmcFaction": ""}}));
    assert!(p.tasks[""].complete, "an entry without an id lands under the empty id rather than failing the read");
    assert_eq!((p.level, p.faction), (None, None));
    assert_eq!(parse_tracker(&json!({"data": {"playerLevel": 256}})).level, None);
    assert_eq!(parse_tracker(&json!({"data": {"playerLevel": 255}})).level, Some(255));
    assert_eq!(parse_tracker(&json!({"data": {"tasksProgress": "nope"}})), Progress::default());
    assert_eq!(parse_tracker(&json!({})), Progress::default());
}

#[test]
fn unsynced_compares_tasks_and_ignores_invalid_server_objectives() {
    let t = |complete, failed| TaskProgress { complete, failed, invalid: false };
    let o = |complete, count| ObjectiveProgress { complete, count, invalid: false };
    let mut server = Progress::default();
    server.tasks.insert("same".into(), t(true, false));
    server.tasks.insert("diff".into(), t(true, false));
    server.objectives.insert("inv".into(), ObjectiveProgress { complete: true, count: 3, invalid: true });
    let tasks =
        HashMap::from([("same".to_owned(), t(true, false)), ("diff".to_owned(), t(true, true)), ("gone".to_owned(), t(true, false))]);
    let objs = HashMap::from([("inv".to_owned(), o(true, 3))]);
    let mut bad = unsynced(&objs, &tasks, &server);
    bad.sort();
    assert_eq!(bad.len(), 3, "{bad:?}");
    assert!(bad.iter().any(|l| l.starts_with("objective inv: wrote complete=true count=3, server has complete=false count=0")));
    assert!(bad.iter().any(|l| l.starts_with("task diff: wrote complete=true failed=true, server has complete=true failed=false")));
    assert!(bad.iter().any(|l| l.starts_with("task gone:")));
    assert!(unsynced(&HashMap::new(), &HashMap::new(), &server).is_empty());
}
