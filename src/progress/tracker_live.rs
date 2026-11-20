//! Smoke tests against the live TarkovTracker API (https://api.tarkovtracker.org). They are `#[ignore]`d so `cargo test`
//! stays offline; the daily pipeline runs them with `cargo test -- --ignored live_`.
//!
//! No real account is involved: every request carries a well-formed token that belongs to nobody, so the gateway answers
//! 401 and nothing can be read or written. What they prove is that the API is up, still has the endpoints, headers and
//! fields ChadScanner relies on, and that the client turns the gateway's real answers into the right behaviour.
use super::*;

/// A token with the right prefix for `mode` that no account owns.
fn nonexistent_token(mode: Mode) -> String {
    let prefix = match mode {
        Mode::Pvp => "PVP",
        Mode::Pve => "PVE",
        Mode::Seasonal => "SZN",
    };
    format!("{prefix}_chadscanner-ci-nonexistent-account")
}

fn get_json(path: &str) -> Value {
    let r = tracker_agent().unwrap().get(&format!("{API}{path}")).call().unwrap_or_else(|e| panic!("GET {path}: {e}"));
    r.into_json().unwrap_or_else(|e| panic!("GET {path}: not JSON: {e}"))
}

fn toasts_and_source(mode: Mode, name: &str) -> (TrackerSource, Arc<Mutex<Vec<(String, String)>>>) {
    let toasts: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let sink = toasts.clone();
    let notify: Notify = Arc::new(move |t, b| sink.lock().unwrap().push((t.to_owned(), b.to_owned())));
    let dir = std::env::temp_dir().join(format!("chadscanner-live-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (TrackerSource::new(API, mode, nonexistent_token(mode), dir.join("pending.json"), notify).unwrap(), toasts)
}

fn rejected_toast(mode: Mode) -> (String, String) {
    ("TarkovTracker rejected the token".to_owned(), mode.label().to_owned())
}

#[test]
#[ignore = "live network"]
fn live_api_is_healthy() {
    let v = get_json("/health");
    assert_eq!(v["success"], true, "{v}");
    assert_eq!(v["data"]["status"], "healthy", "{v}");
}

#[test]
#[ignore = "live network"]
fn live_openapi_still_has_every_call_chadscanner_makes() {
    let spec = get_json("/openapi.json");
    let ops = [
        ("/token", "get"),
        ("/progress", "get"),
        ("/progress/task/{taskId}", "post"),
        ("/progress/task/objective/{objectiveId}", "post"),
        ("/progress/level/{level}", "post"),
    ];
    for (path, method) in ops {
        assert!(spec["paths"][path][method].is_object(), "{method} {path} is gone from the API");
    }
    // What `refresh` reacts to.
    let statuses: Vec<&str> = spec["paths"]["/progress"]["get"]["responses"].as_object().unwrap().keys().map(String::as_str).collect();
    for code in ["200", "304", "401", "403", "429"] {
        assert!(statuses.contains(&code), "GET /progress no longer documents {code}: {statuses:?}");
    }
    // The headers `refresh` reads for the daily quota and the revalidation it sends.
    let headers = &spec["paths"]["/progress"]["get"]["responses"]["200"]["headers"];
    for h in ["ETag", "X-RateLimit-Remaining"] {
        assert!(headers[h].is_object(), "GET /progress 200 no longer sends {h}");
    }
    // The fields `parse_tracker` reads.
    let fields = &spec["components"]["schemas"]["ProgressData"]["properties"];
    for f in ["tasksProgress", "taskObjectivesProgress", "hideoutModulesProgress", "hideoutPartsProgress", "playerLevel", "pmcFaction"] {
        assert!(fields[f].is_object(), "ProgressData lost {f}");
    }
    let task = &spec["components"]["schemas"]["ProgressTask"]["properties"];
    for f in ["id", "complete", "failed", "invalid"] {
        assert!(task[f].is_object(), "ProgressTask lost {f}");
    }
    // The values `set_task` / `set_objective` send.
    let states =
        |schema: &Value| -> Vec<String> { schema["enum"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect() };
    let task_states = states(&spec["components"]["schemas"]["TaskState"]);
    for s in [TaskState::Completed.api(), TaskState::Failed.api()] {
        assert!(task_states.iter().any(|t| t == s), "task state {s} is gone: {task_states:?}");
    }
    let objective_states = states(&spec["components"]["schemas"]["ObjectiveUpdateRequest"]["properties"]["state"]);
    assert_eq!(objective_states, ["completed", "uncompleted"]);
    // The fields `test_token` reads.
    let token = &spec["components"]["schemas"]["TokenInfoResponse"]["properties"];
    assert!(token["permissions"].is_object() && token["gameMode"].is_object());
}

#[test]
#[ignore = "live network"]
fn live_test_token_rejects_a_nonexistent_account_in_every_game_mode() {
    for mode in Mode::ALL {
        let e = test_token(&nonexistent_token(mode)).expect_err("a nonexistent token must not validate");
        match e.downcast::<ureq::Error>() {
            Ok(ureq::Error::Status(401, r)) => {
                let body: Value = r.into_json().expect("the gateway answers errors in JSON");
                assert_eq!(body["success"], false, "{mode:?}: {body}");
            }
            other => panic!("{mode:?}: expected a 401 from the gateway, got {other:?}"),
        }
    }
}

#[test]
#[ignore = "live network"]
fn live_legacy_and_malformed_tokens_are_rejected() {
    for token in ["tt_legacytokenformat", "x", "PVE_"] {
        let e = test_token(token).expect_err(token);
        assert!(matches!(e.downcast_ref::<ureq::Error>(), Some(ureq::Error::Status(401, _))), "{token}: {e:#}");
    }
}

#[test]
#[ignore = "live network"]
fn live_a_nonexistent_account_reads_nothing_and_raises_one_toast_per_read() {
    for mode in Mode::ALL {
        let (src, toasts) = toasts_and_source(mode, &format!("read-{}", mode.key()));
        src.refresh();
        assert_eq!(*toasts.lock().unwrap(), vec![rejected_toast(mode)], "{mode:?}");
        assert_eq!(*src.snapshot(), Progress::default(), "{mode:?}: nothing to read");
    }
}

#[test]
#[ignore = "live network"]
fn live_a_nonexistent_account_cannot_write_and_nothing_is_queued() {
    let (src, toasts) = toasts_and_source(Mode::Pve, "write");
    for (what, result) in [
        ("task", src.set_task("nonexistent-task", TaskState::Completed)),
        ("failed task", src.set_task("nonexistent-task", TaskState::Failed)),
        ("objective", src.set_objective("nonexistent-objective", true, 1)),
        ("level", src.set_level(15)),
    ] {
        let e = result.expect_err(what);
        assert!(e.to_string().contains("401"), "{what}: {e:#}");
    }
    assert_eq!(toasts.lock().unwrap().len(), 4, "every refused write says so");
    assert!(toasts.lock().unwrap().iter().all(|t| *t == rejected_toast(Mode::Pve)));
    assert_eq!(*src.snapshot(), Progress::default(), "refused writes are not shown");
    assert!(src.load_queue().is_empty(), "a rejected token is not retried");
    assert!(src.set_hideout_level("5d484fc0654e76006657e0ab", 1).is_err());
}

#[test]
#[ignore = "live network"]
fn live_a_scan_and_the_poll_tick_survive_a_nonexistent_account() {
    let (src, toasts) = toasts_and_source(Mode::Seasonal, "scan");
    src.refresh_if_stale();
    assert_eq!(toasts.lock().unwrap().len(), 1, "the scan read once");
    let mut last = None;
    src.poll(&mut last);
    assert!(last.is_some());
    assert_eq!(toasts.lock().unwrap().len(), 2);
    assert!(src.snapshot().tasks.is_empty());
}

#[test]
#[ignore = "live network"]
fn live_the_background_thread_reports_a_nonexistent_account() {
    let toasts: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let sink = toasts.clone();
    let notify: Notify = Arc::new(move |t, b| sink.lock().unwrap().push((t.to_owned(), b.to_owned())));
    let dir = std::env::temp_dir().join(format!("chadscanner-live-{}-thread", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src =
        TrackerSource::start_at(API, Mode::Pvp, nonexistent_token(Mode::Pvp), dir.join("pending.json"), notify, Duration::from_millis(50))
            .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while toasts.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the polling thread never reached the API");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(toasts.lock().unwrap()[0], rejected_toast(Mode::Pvp));
    drop(src);
}
