//! Raid map data: tarkov.dev spawns/extracts joined with tarkov.dev's interactive-map display config.
use crate::config::Mode;
use anyhow::Context;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};
use std::time::{Duration, SystemTime};

const TARKOV_DEV_MAPS: &str = "https://raw.githubusercontent.com/the-hideout/tarkov-dev/main/src/data/maps.json";
const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
const RETRY: Duration = Duration::from_secs(600);
/// Minimum `lexicon::score` for an OCR'd extract name to count as a given extract.
const MATCH_MIN: f32 = 0.75;

/// Which side the player raids as; told apart by extracts only one side has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Pmc,
    Scav,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Pmc => "PMC",
            Side::Scav => "Scav",
        }
    }

    /// Stable identifier the map script uses for this side.
    pub fn key(self) -> &'static str {
        match self {
            Side::Pmc => "pmc",
            Side::Scav => "scav",
        }
    }
}

/// One raid location: the tarkovtracker.org map to open and the extracts the exfil panel is matched against.
#[derive(Debug, Clone)]
pub struct MapView {
    /// tarkov.dev map id; tarkovtracker.org's `/tasks/maps?map=` takes it.
    pub id: String,
    pub name: String,
    /// tarkov.dev's `coordinateRotation` for this map (0 when absent); turns the position arrow.
    pub rotation: f64,
    /// English extract names, each once.
    pub extracts: Vec<String>,
    /// Extract names only PMCs have (every tarkov.dev entry of the name is `faction: "pmc"`).
    pub pmc_only: Vec<String>,
    /// Extract names only Scavs have.
    pub scav_only: Vec<String>,
}

impl MapView {
    /// The side `names` (matched extract names) prove: Some only when they include one side's own extracts and none of the other's.
    pub fn side_of(&self, names: &[String]) -> Option<Side> {
        let p = names.iter().any(|n| self.pmc_only.contains(n));
        let s = names.iter().any(|n| self.scav_only.contains(n));
        match (p, s) {
            (true, false) => Some(Side::Pmc),
            (false, true) => Some(Side::Scav),
            _ => None,
        }
    }

    /// Whether a player on `side` can have extract `name` (it is not the other side's own).
    pub fn allows(&self, side: Side, name: &str) -> bool {
        let other = match side {
            Side::Pmc => &self.scav_only,
            Side::Scav => &self.pmc_only,
        };
        !other.iter().any(|n| n == name)
    }
}

#[derive(Debug, Default)]
pub struct Maps {
    by_scene: HashMap<String, MapView>,
}

impl Maps {
    pub fn for_scene(&self, scene: &str) -> Option<&MapView> {
        self.by_scene.get(scene)
    }
}

fn data(body: &[u8]) -> anyhow::Result<Value> {
    let mut v: Value = serde_json::from_slice(body)?;
    Ok(v.get_mut("data").map(Value::take).context("response has no `data`")?)
}

/// The interactive config of the tarkov-dev group that covers `normalized` (as its key or an alt map).
fn interactive_config<'a>(tarkov_dev: &'a Value, normalized: &str) -> Option<&'a Value> {
    tarkov_dev.as_array()?.iter().flat_map(|g| g["maps"].as_array().into_iter().flatten()).find(|m| {
        m["projection"] == "interactive"
            && (m["key"] == normalized || m["altMaps"].as_array().is_some_and(|a| a.iter().any(|k| k == normalized)))
    })
}

/// Builds the scene → view table from the three downloaded documents.
pub fn build(maps: &[u8], maps_en: &[u8], tarkov_dev: &[u8]) -> anyhow::Result<Maps> {
    let maps = data(maps)?;
    let en = data(maps_en)?;
    let tarkov_dev: Value = serde_json::from_slice(tarkov_dev).context("tarkov-dev maps.json")?;
    let text = |key: &str| en[key].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let mut by_scene = HashMap::new();
    for m in maps["maps"].as_object().context("no `data.maps`")?.values() {
        let (Some(id), Some(scene), Some(normalized)) =
            (m["id"].as_str(), m["scenePath"].as_str(), m["normalizedName"].as_str())
        else {
            continue;
        };
        let Some(config) = interactive_config(&tarkov_dev, normalized) else {
            log::info!("map {}: no interactive map", m["nameId"].as_str().unwrap_or(normalized));
            continue;
        };
        let mut extracts: Vec<String> = Vec::new();
        // Per name: (has a pmc entry, has a scav entry, has any other entry).
        let mut factions: HashMap<String, (bool, bool, bool)> = HashMap::new();
        for e in m["extracts"].as_array().into_iter().flatten() {
            let Some(raw) = e["name"].as_str() else { continue };
            let name = text(raw).unwrap_or_else(|| raw.to_owned());
            if !extracts.contains(&name) {
                extracts.push(name.clone());
            }
            let f = factions.entry(name).or_default();
            match e["faction"].as_str() {
                Some("pmc") => f.0 = true,
                Some("scav") => f.1 = true,
                _ => f.2 = true,
            }
        }
        let only = |want: (bool, bool, bool)| -> Vec<String> {
            extracts.iter().filter(|n| factions[*n] == want).cloned().collect()
        };
        let (pmc_only, scav_only) = (only((true, false, false)), only((false, true, false)));
        let name = m["name"].as_str().and_then(text).unwrap_or_else(|| normalized.to_owned());
        let rotation = config["coordinateRotation"].as_f64().unwrap_or(0.0);
        by_scene.insert(scene.to_owned(), MapView { id: id.to_owned(), name, rotation, extracts, pmc_only, scav_only });
    }
    Ok(Maps { by_scene })
}

/// Extract names from `view` that the OCR'd panel lines name, limited to `side`'s extracts when known (several factions can share one name).
pub fn match_extracts(view: &MapView, ocr: &[String], side: Option<Side>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in ocr {
        let best = view
            .extracts
            .iter()
            .filter(|n| side.map_or(true, |s| view.allows(s, n)))
            .map(|n| (n, chadvision::lexicon::score(line, n)))
            .filter(|(_, s)| *s >= MATCH_MIN)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((n, _)) = best {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
    }
    out
}

pub struct MapStore {
    current: RwLock<Option<Arc<Maps>>>,
}

fn files(cache_dir: &Path, mode: Mode) -> [PathBuf; 3] {
    [
        cache_dir.join(format!("maps-{}.json", mode.api())),
        cache_dir.join(format!("maps_en-{}.json", mode.api())),
        cache_dir.join("tarkov-dev-maps.json"),
    ]
}

fn urls(mode: Mode) -> [String; 3] {
    [
        format!("https://json.tarkov.dev/{}/maps", mode.api()),
        format!("https://json.tarkov.dev/{}/maps_en", mode.api()),
        TARKOV_DEV_MAPS.to_owned(),
    ]
}

fn load_cached(cache_dir: &Path, mode: Mode) -> Option<Maps> {
    let [a, b, c] = files(cache_dir, mode).map(|p| std::fs::read(p).ok());
    build(&a?, &b?, &c?).map_err(|e| log::warn!("cached map data unusable: {e:#}")).ok()
}

/// Age of the oldest cache file; None when any is missing.
fn cache_age(cache_dir: &Path, mode: Mode) -> Option<Duration> {
    files(cache_dir, mode)
        .iter()
        .map(|p| {
            let modified = std::fs::metadata(p).ok()?.modified().ok()?;
            Some(SystemTime::now().duration_since(modified).unwrap_or_default())
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()
}

fn fetch(agent: &ureq::Agent, cache_dir: &Path, mode: Mode) -> anyhow::Result<Maps> {
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    for url in urls(mode) {
        let mut body = Vec::new();
        std::io::Read::read_to_end(&mut agent.get(&url).call().with_context(|| url.clone())?.into_reader(), &mut body)?;
        bodies.push(body);
    }
    let maps = build(&bodies[0], &bodies[1], &bodies[2])?;
    // Cache only a set that parsed, so a bad response never replaces a good cache.
    for (path, body) in files(cache_dir, mode).iter().zip(&bodies) {
        if let Err(e) = crate::config::write_atomic(path, body) {
            log::warn!("caching {}: {e}", path.display());
        }
    }
    Ok(maps)
}

impl MapStore {
    /// Loads the cache now, then refreshes in the background when it is missing or older than 24 h
    /// (retrying every 10 min after a failure).
    pub fn start(mode: Mode, cache_dir: PathBuf) -> Arc<MapStore> {
        let cached = load_cached(&cache_dir, mode).map(Arc::new);
        if cached.is_some() {
            log::info!("{} map data loaded from cache", mode.api());
        }
        let store = Arc::new(MapStore { current: RwLock::new(cached) });
        let weak: Weak<MapStore> = Arc::downgrade(&store);
        std::thread::Builder::new()
            .name(format!("maps-{}", mode.api()))
            .spawn(move || {
                let agent = match crate::data::agent() {
                    Ok(a) => a,
                    Err(e) => return log::error!("TLS setup failed: {e:#}"),
                };
                loop {
                    let Some(store) = weak.upgrade() else { return };
                    let have = store.get().is_some();
                    let wait = match cache_age(&cache_dir, mode).filter(|a| have && *a < MAX_AGE) {
                        Some(age) => (MAX_AGE - age).max(Duration::from_secs(60)),
                        None => match fetch(&agent, &cache_dir, mode) {
                            Ok(m) => {
                                log::info!("{} map data refreshed: {} locations", mode.api(), m.by_scene.len());
                                *store.current.write().unwrap() = Some(Arc::new(m));
                                MAX_AGE
                            }
                            Err(e) => {
                                log::warn!("{} map data fetch failed: {e:#}; retrying in 10 min", mode.api());
                                RETRY
                            }
                        },
                    };
                    drop(store);
                    std::thread::sleep(wait);
                }
            })
            .expect("spawn maps thread");
        store
    }

    pub fn get(&self) -> Option<Arc<Maps>> {
        self.current.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPS: &str = r#"{"data":{"maps":{
        "m1":{"id":"m1","name":"m1 Name","normalizedName":"customs","nameId":"bigmap","scenePath":"maps/customs_preset.bundle",
          "spawns":[
            {"position":{"x":1,"y":0,"z":2},"sides":["pmc"],"categories":["player"]},
            {"position":{"x":3,"y":0,"z":4},"sides":["scav"],"categories":["player","bot"]},
            {"position":{"x":5,"y":0,"z":6},"sides":["all"],"categories":["player"]},
            {"position":{"x":7,"y":0,"z":8},"sides":["pmc"],"categories":["boss"]}],
          "extracts":[
            {"name":"ex_old_gas","faction":"pmc","position":{"x":10,"y":0,"z":11}},
            {"name":"ex_old_gas","faction":"scav","position":{"x":12,"y":0,"z":13}},
            {"name":"RUAF Roadblock","faction":null,"position":{"x":14,"y":0,"z":15}},
            {"name":"ex_trailer","faction":"pmc","position":{"x":16,"y":0,"z":17}},
            {"name":"ex_admin","faction":"scav","position":{"x":18,"y":0,"z":19}},
            {"name":"ex_boiler","faction":"shared","position":{"x":20,"y":0,"z":21}}]},
        "m2":{"id":"m2","name":"m2 Name","normalizedName":"night-factory","nameId":"f4n","scenePath":"maps/factory_night_preset.bundle","spawns":[],"extracts":[]},
        "m3":{"id":"m3","name":"m3 Name","normalizedName":"tutorial","nameId":"tut","scenePath":"maps/tutorial.bundle","spawns":[],"extracts":[]}}}}"#;
    const EN: &str = r#"{"data":{"m1 Name":"Customs","ex_old_gas":"Old Gas Station","ex_trailer":"Trailer Park","ex_admin":"Administration Gate","ex_boiler":"Boiler Room"}}"#;
    const TD: &str = r#"[
 {"normalizedName":"customs","maps":[{"key":"customs","projection":"interactive","minZoom":2,"coordinateRotation":180},{"key":"customs-2d","projection":"2D"}]},
 {"normalizedName":"factory","maps":[{"key":"factory","altMaps":["night-factory"],"projection":"interactive","minZoom":1}]}]"#;

    fn built() -> Maps {
        build(MAPS.as_bytes(), EN.as_bytes(), TD.as_bytes()).unwrap()
    }

    #[test]
    fn extracts_are_translated_and_unique() {
        let m = built();
        let v = m.for_scene("maps/customs_preset.bundle").unwrap();
        assert_eq!(v.extracts, vec!["Old Gas Station", "RUAF Roadblock", "Trailer Park", "Administration Gate", "Boiler Room"]);
    }

    #[test]
    fn alt_map_resolves_parent_config_and_unmapped_is_skipped() {
        let m = built();
        let night = m.for_scene("maps/factory_night_preset.bundle").unwrap();
        assert_eq!(night.id, "m2");
        assert_eq!(night.rotation, 0.0);
        assert_eq!(m.for_scene("maps/customs_preset.bundle").unwrap().rotation, 180.0);
        assert!(m.for_scene("maps/tutorial.bundle").is_none());
    }

    #[test]
    fn map_name_falls_back_to_normalized_name() {
        let m = built();
        assert_eq!(m.for_scene("maps/customs_preset.bundle").unwrap().name, "Customs");
        assert_eq!(m.for_scene("maps/factory_night_preset.bundle").unwrap().name, "night-factory");
    }

    #[test]
    fn matches_ocr_names_to_extracts() {
        let m = built();
        let v = m.for_scene("maps/customs_preset.bundle").unwrap();
        assert_eq!(match_extracts(v, &["Old Gas Statlon".into(), "Nonsense".into()], None), vec!["Old Gas Station"]);
        assert_eq!(match_extracts(v, &["Old Gas Station".into(), "Old Gas Station".into()], None), vec!["Old Gas Station"]);
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn side_comes_from_one_side_extracts() {
        let m = built();
        let v = m.for_scene("maps/customs_preset.bundle").unwrap();
        assert_eq!(v.pmc_only, vec!["Trailer Park"]);
        assert_eq!(v.scav_only, vec!["Administration Gate"]);
        assert_eq!(v.side_of(&names(&["Trailer Park", "Old Gas Station"])), Some(Side::Pmc));
        assert_eq!(v.side_of(&names(&["Administration Gate", "Boiler Room"])), Some(Side::Scav));
        assert_eq!(v.side_of(&names(&["Old Gas Station", "RUAF Roadblock"])), None);
        assert_eq!(v.side_of(&names(&["Trailer Park", "Administration Gate"])), None);
    }

    #[test]
    fn side_filters_matching() {
        let m = built();
        let v = m.for_scene("maps/customs_preset.bundle").unwrap();
        let gate = names(&["Administration Gate"]);
        assert!(match_extracts(v, &gate, Some(Side::Pmc)).is_empty());
        assert_eq!(match_extracts(v, &gate, Some(Side::Scav)), gate);
        let gas = names(&["Old Gas Station"]);
        assert_eq!(match_extracts(v, &gas, Some(Side::Scav)), gas);
    }
}
