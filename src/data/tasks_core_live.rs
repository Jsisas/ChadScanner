//! Smoke tests against the live TarkovTracker `tasks-core` endpoint (public, no account) and the full catalog load that
//! merges it. `#[ignore]`d so `cargo test` stays offline; the daily pipeline runs them with `cargo test -- --ignored live_`.
use super::*;
use std::collections::HashSet;

fn tasks_core(mode: Mode) -> Vec<u8> {
    let url = tracker_tasks_url(&Hosts::default().tracker, mode);
    let mut body = Vec::new();
    std::io::Read::read_to_end(
        &mut agent().unwrap().get(&url).call().unwrap_or_else(|e| panic!("GET {url}: {e}")).into_reader(),
        &mut body,
    )
    .unwrap();
    body
}

#[test]
#[ignore = "live network"]
fn live_tasks_core_parses_completely_in_every_game_mode() {
    for mode in Mode::ALL {
        let body = tasks_core(mode);
        let raw = data(&body).unwrap_or_else(|e| panic!("{mode:?}: {e:#}"));
        let listed = raw["tasks"].as_array().unwrap_or_else(|| panic!("{mode:?}: `tasks` is not a list")).len();
        let tasks = tracker_tasks(&body);
        assert!(tasks.len() > 100, "{mode:?}: only {} tasks", tasks.len());
        assert_eq!(tasks.len(), listed, "{mode:?}: {} of {listed} tasks no longer parse", listed - tasks.len());
        let ids: HashSet<_> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), tasks.len(), "{mode:?}: duplicate task ids");
        assert!(tasks.iter().all(|t| !t.name.is_empty()), "{mode:?}: a task without a name");
        // The parts of a task ChadScanner reads must still be populated for at least some tasks.
        assert!(tasks.iter().any(|t| !t.objectives.is_empty()), "{mode:?}: no objectives");
        assert!(tasks.iter().any(|t| !t.task_requirements.is_empty()), "{mode:?}: no task requirements");
        assert!(tasks.iter().any(|t| t.min_player_level.is_some_and(|l| l > 1)), "{mode:?}: no level requirements");
        assert!(tasks.iter().any(|t| t.kappa_required == Some(true)), "{mode:?}: no Kappa tasks");
    }
}

#[test]
#[ignore = "live network"]
fn live_full_catalog_loads_with_tasks_core_merged_and_caches() {
    let dir = std::env::temp_dir().join(format!("chadscanner-live-{}-catalog", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let c = fetch_all(&agent().unwrap(), &dir, Mode::Pve, &Hosts::default()).expect("the live catalog loads");
    assert!(
        c.items.len() > 1000 && c.tasks.len() > 100 && c.stations.len() > 5 && c.traders.len() > 5,
        "{} items, {} tasks",
        c.items.len(),
        c.tasks.len()
    );
    assert!(!c.task_unlocks.is_empty() && !c.level_unlocks.is_empty());
    assert!(cache_file(&dir, Mode::Pve, TRACKER_TASKS).is_file(), "tasks-core was cached");
    let bundle = std::fs::metadata(cache_file(&dir, Mode::Pve, TRACKER_STRINGS)).expect("the strings bundle was cached");
    assert!(bundle.len() < 200_000, "strings bundle not pruned: {} bytes", bundle.len());
    assert!(
        c.tasks.values().all(|t| !t.name.starts_with("@tt:")),
        "unresolved name: {:?}",
        c.tasks.values().find(|t| t.name.starts_with("@tt:")).map(|t| &t.id)
    );
    assert_eq!(c.tasks["placeholder_pay_the_fare"].name, "Pay the Fare!");
    let again = load_cached(&dir, Mode::Pve).expect("what was cached loads offline");
    assert_eq!(again.tasks.len(), c.tasks.len());
}
