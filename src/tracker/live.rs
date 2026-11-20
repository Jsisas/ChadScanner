//! Smoke tests against the live tarkovtracker.org site the injected script drives. `#[ignore]`d so `cargo test` stays
//! offline; the daily pipeline runs them with `cargo test -- --ignored live_`. They only fetch public pages.
use super::*;

fn page(url: &str) -> (u16, String) {
    let r = crate::data::agent().unwrap().get(url).call().unwrap_or_else(|e| panic!("GET {url}: {e}"));
    let status = r.status();
    (status, r.into_string().unwrap_or_else(|e| panic!("GET {url}: {e}")))
}

#[test]
#[ignore = "live network"]
fn live_the_window_start_page_is_served_by_the_nuxt_app() {
    let (status, html) = page(START_URL);
    assert_eq!(status, 200);
    // `tracker.js` reaches the router and the progress store through `#__nuxt`'s Vue app.
    assert!(html.contains(r#"id="__nuxt""#), "the page no longer mounts a Nuxt app at #__nuxt");
    assert!(html.contains("TarkovTracker"));
}

#[test]
#[ignore = "live network"]
fn live_the_raid_map_route_exists() {
    // Customs, as ChadScanner opens it at raid start.
    let (status, html) = page("https://tarkovtracker.org/tasks/maps?map=55f2d3fd4bdc2d5f408b4567");
    assert_eq!(status, 200);
    assert!(html.contains(r#"id="__nuxt""#));
}

#[test]
#[ignore = "live network"]
fn live_the_script_acts_on_the_host_the_window_opens() {
    let host = START_URL.strip_prefix("https://").and_then(|r| r.split('/').next()).expect("an https start URL");
    assert_eq!(host, "tarkovtracker.org");
    assert!(script().contains(r"/(^|\.)tarkovtracker\.org$/"), "the injected script no longer guards on this host");
}
