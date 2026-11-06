//! The raid map page: `map.html` with the vendored Leaflet inlined, loaded as an in-memory page.
//! `window.__map.show(view)` (a `maps::MapView` as JSON) and `window.__map.only(names)` drive it.
const PAGE: &str = include_str!("map.html");
const LEAFLET_JS: &str = include_str!("../assets/leaflet/leaflet.js");
const LEAFLET_CSS: &str = include_str!("../assets/leaflet/leaflet.css");

/// Posted to the app (`window.ipc`) once the page's script has run; `map.html` posts the same text.
pub const READY: &str = "map-ready";

pub fn page() -> String {
    PAGE.replacen("/*LEAFLET_CSS*/", LEAFLET_CSS, 1).replacen("/*LEAFLET_JS*/", LEAFLET_JS, 1)
}
