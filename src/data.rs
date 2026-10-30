//! Item, task, hideout and trader data from json.tarkov.dev, cached on disk and refreshed in the background.
use crate::config::Mode;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;

pub const ENDPOINTS: [&str; 10] =
    ["items", "items_en", "tasks", "tasks_en", "hideout", "hideout_en", "traders", "traders_en", "barters", "crafts"];
const REFRESH: Duration = Duration::from_secs(600);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default)]
pub struct TraderPrice {
    pub trader: String,
    pub price_rub: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Item {
    pub id: String,
    pub short_name: String,
    pub width: i64,
    pub height: i64,
    pub avg24h: Option<i64>,
    pub sell_to_trader: Vec<TraderPrice>,
}

impl Item {
    /// avg24hPrice per inventory slot (integer division, as RatScanner).
    pub fn price_per_slot(&self) -> Option<i64> {
        self.avg24h.map(|p| p / (self.width * self.height).max(1))
    }

    pub fn best_trader(&self) -> Option<&TraderPrice> {
        self.sell_to_trader.iter().max_by_key(|t| t.price_rub)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskRequirement {
    pub task: String,
    pub status: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TraderRequirement {
    pub trader: String,
    pub requirement_type: String,
    pub compare_method: String,
    pub value: f64,
}

#[derive(Debug, Clone, Default)]
pub struct OtherRequirement {
    pub kind: String,
    pub variable_id: String,
    pub compare_method: String,
    pub value: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Objective {
    pub id: String,
    pub kind: String,
    pub count: u32,
    pub optional: bool,
    pub found_in_raid: bool,
    pub items: Vec<String>,
    /// `mark` objectives.
    pub marker_item: Option<String>,
    /// `buildWeapon` objectives.
    pub build_item: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub min_player_level: u32,
    pub task_requirements: Vec<TaskRequirement>,
    pub trader_requirements: Vec<TraderRequirement>,
    pub other_requirements: Vec<OtherRequirement>,
    pub faction_name: String,
    pub kappa_required: bool,
    pub lightkeeper_required: bool,
    pub objectives: Vec<Objective>,
}

#[derive(Debug, Clone, Default)]
pub struct HideoutRequirement {
    pub id: String,
    pub item: String,
    pub count: u32,
    pub fir: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StationLevel {
    /// `"<station>-<level>"`, TarkovTracker's hideout module id.
    pub id: String,
    pub level: u8,
    pub item_requirements: Vec<HideoutRequirement>,
    /// (station id, level) that must be built first.
    pub station_requirements: Vec<(String, u8)>,
    pub trader_requirements: Vec<TraderRequirement>,
}

#[derive(Debug, Clone, Default)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub levels: Vec<StationLevel>,
}

/// Barter or craft: only the inputs matter here.
#[derive(Debug, Clone, Default)]
pub struct Recipe {
    pub required_items: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Catalog {
    pub items: HashMap<String, Item>,
    pub tasks: HashMap<String, Task>,
    pub stations: Vec<Station>,
    pub barters: Vec<Recipe>,
    pub crafts: Vec<Recipe>,
    /// Trader id → name.
    pub traders: HashMap<String, String>,
    /// Story-chain counter id → (required value, task id) of every task it gates.
    pub chains: HashMap<String, Vec<(f64, String)>>,
    /// Task id → number of tasks that (transitively) require it.
    pub task_unlocks: HashMap<String, u32>,
    /// Station level id → number of station levels that (transitively) require it.
    pub level_unlocks: HashMap<String, u32>,
}

/// For each node, how many nodes are reachable through `edges` (node → direct dependents).
fn reach_counts(edges: &HashMap<String, Vec<String>>) -> HashMap<String, u32> {
    edges
        .keys()
        .map(|start| {
            let mut seen = std::collections::HashSet::new();
            let mut stack: Vec<&String> = edges[start].iter().collect();
            while let Some(n) = stack.pop() {
                if seen.insert(n) {
                    stack.extend(edges.get(n).into_iter().flatten());
                }
            }
            (start.clone(), seen.len() as u32)
        })
        .collect()
}

impl Catalog {
    fn compute_unlocks(&mut self) {
        let mut task_edges: HashMap<String, Vec<String>> = HashMap::new();
        for t in self.tasks.values() {
            for r in &t.task_requirements {
                task_edges.entry(r.task.clone()).or_default().push(t.id.clone());
            }
        }
        self.task_unlocks = reach_counts(&task_edges);

        let mut level_edges: HashMap<String, Vec<String>> = HashMap::new();
        let id_of = |station: &str, level: u8| {
            self.stations.iter().find(|s| s.id == station)?.levels.iter().find(|l| l.level == level).map(|l| l.id.clone())
        };
        for s in &self.stations {
            for (i, l) in s.levels.iter().enumerate() {
                if let Some(next) = s.levels.get(i + 1) {
                    level_edges.entry(l.id.clone()).or_default().push(next.id.clone());
                }
                for (rs, rl) in &l.station_requirements {
                    if let Some(req) = id_of(rs, *rl) {
                        level_edges.entry(req).or_default().push(l.id.clone());
                    }
                }
            }
        }
        self.level_unlocks = reach_counts(&level_edges);
    }
}

// ---- raw json.tarkov.dev shapes (only the fields used) ----

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawTraderPrice {
    trader: String,
    #[serde(rename = "priceRUB")]
    price_rub: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawItem {
    id: String,
    short_name: String,
    width: i64,
    height: i64,
    avg24h_price: Option<f64>,
    sell_to_trader: Vec<RawTraderPrice>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawTaskReq {
    task: String,
    status: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawTraderReq {
    trader: String,
    requirement_type: String,
    compare_method: String,
    value: f64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawOtherReq {
    #[serde(rename = "type")]
    kind: String,
    variable_id: Option<String>,
    compare_method: Option<String>,
    value: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawObjective {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    count: Option<f64>,
    optional: bool,
    found_in_raid: bool,
    items: Vec<Value>,
    marker_item: Option<Value>,
    item: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawTask {
    id: String,
    name: String,
    min_player_level: Option<u32>,
    task_requirements: Vec<RawTaskReq>,
    trader_requirements: Vec<RawTraderReq>,
    other_requirements: Vec<RawOtherReq>,
    faction_name: Option<String>,
    kappa_required: Option<bool>,
    lightkeeper_required: Option<bool>,
    objectives: Vec<RawObjective>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAttributes {
    #[serde(rename = "foundInRaid")]
    found_in_raid: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawHideoutReq {
    id: String,
    item: Option<Value>,
    count: Option<f64>,
    attributes: Option<RawAttributes>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawLevel {
    id: String,
    level: u8,
    item_requirements: Vec<RawHideoutReq>,
    station_level_requirements: Vec<RawStationReq>,
    trader_requirements: Vec<RawTraderReq>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStationReq {
    station: Option<Value>,
    level: u8,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStation {
    id: String,
    name: String,
    levels: Vec<RawLevel>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawCount {
    item: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawRecipe {
    required_items: Vec<RawCount>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTrader {
    id: String,
    name: String,
}

/// An item reference: either an id string or an object with an `id`.
fn ref_id(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("id").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn data(body: &[u8]) -> anyhow::Result<Value> {
    let mut v: Value = serde_json::from_slice(body)?;
    Ok(v.get_mut("data").map(Value::take).ok_or_else(|| anyhow::anyhow!("no `data` field"))?)
}

fn translations(body: &[u8]) -> anyhow::Result<HashMap<String, String>> {
    Ok(serde_json::from_value(data(body)?)?)
}

fn tr(map: &HashMap<String, String>, key: &str) -> String {
    map.get(key).cloned().unwrap_or_else(|| key.to_owned())
}

impl Catalog {
    /// `bodies`: raw response per endpoint name in `ENDPOINTS`.
    pub fn build(bodies: &HashMap<&str, Vec<u8>>) -> anyhow::Result<Catalog> {
        let body = |name: &str| bodies.get(name).map(Vec::as_slice).ok_or_else(|| anyhow::anyhow!("missing {name}"));
        let items_en = translations(body("items_en")?)?;
        let tasks_en = translations(body("tasks_en")?)?;
        let hideout_en = translations(body("hideout_en")?)?;
        let traders_en = translations(body("traders_en")?)?;

        let mut items_data = data(body("items")?)?;
        let raw_items: HashMap<String, RawItem> =
            serde_json::from_value(items_data.get_mut("items").map(Value::take).unwrap_or_default())?;
        let items = raw_items
            .into_values()
            .map(|r| {
                let item = Item {
                    short_name: tr(&items_en, &r.short_name),
                    width: r.width,
                    height: r.height,
                    avg24h: r.avg24h_price.filter(|p| *p > 0.0).map(|p| p as i64),
                    sell_to_trader: r
                        .sell_to_trader
                        .into_iter()
                        .map(|t| TraderPrice { trader: t.trader, price_rub: t.price_rub.unwrap_or(0.0) as i64 })
                        .collect(),
                    id: r.id,
                };
                (item.id.clone(), item)
            })
            .collect();

        let mut tasks_data = data(body("tasks")?)?;
        let raw_tasks: HashMap<String, RawTask> =
            serde_json::from_value(tasks_data.get_mut("tasks").map(Value::take).unwrap_or_default())?;
        let mut chains: HashMap<String, Vec<(f64, String)>> = HashMap::new();
        let tasks = raw_tasks
            .into_values()
            .map(|r| {
                let other_requirements: Vec<OtherRequirement> = r
                    .other_requirements
                    .into_iter()
                    .map(|o| OtherRequirement {
                        kind: o.kind,
                        variable_id: o.variable_id.unwrap_or_default(),
                        compare_method: o.compare_method.unwrap_or_default(),
                        value: o.value.unwrap_or(0.0),
                    })
                    .collect();
                for o in &other_requirements {
                    if !o.variable_id.is_empty() && o.kind.eq_ignore_ascii_case("globalVariable") {
                        chains.entry(o.variable_id.clone()).or_default().push((o.value, r.id.clone()));
                    }
                }
                let task = Task {
                    name: tr(&tasks_en, &r.name),
                    min_player_level: r.min_player_level.unwrap_or(0),
                    task_requirements: r
                        .task_requirements
                        .into_iter()
                        .map(|t| TaskRequirement { task: t.task, status: t.status })
                        .collect(),
                    trader_requirements: r
                        .trader_requirements
                        .into_iter()
                        .map(|t| TraderRequirement {
                            trader: t.trader,
                            requirement_type: t.requirement_type,
                            compare_method: t.compare_method,
                            value: t.value,
                        })
                        .collect(),
                    other_requirements,
                    faction_name: r.faction_name.unwrap_or_default(),
                    kappa_required: r.kappa_required.unwrap_or(false),
                    lightkeeper_required: r.lightkeeper_required.unwrap_or(false),
                    objectives: r
                        .objectives
                        .into_iter()
                        .map(|o| Objective {
                            id: o.id,
                            kind: o.kind,
                            count: o.count.unwrap_or(0.0).max(0.0) as u32,
                            optional: o.optional,
                            found_in_raid: o.found_in_raid,
                            items: o.items.iter().filter_map(ref_id).collect(),
                            marker_item: o.marker_item.as_ref().and_then(ref_id),
                            build_item: o.item.as_ref().and_then(ref_id),
                        })
                        .collect(),
                    id: r.id,
                };
                (task.id.clone(), task)
            })
            .collect();

        let raw_stations: HashMap<String, RawStation> = serde_json::from_value(data(body("hideout")?)?)?;
        let mut stations: Vec<Station> = raw_stations
            .into_values()
            .map(|s| Station {
                name: tr(&hideout_en, &s.name),
                levels: s
                    .levels
                    .into_iter()
                    .map(|l| StationLevel {
                        id: l.id,
                        level: l.level,
                        item_requirements: l
                            .item_requirements
                            .into_iter()
                            .filter_map(|q| {
                                Some(HideoutRequirement {
                                    item: q.item.as_ref().and_then(ref_id)?,
                                    id: q.id,
                                    count: q.count.unwrap_or(0.0).max(0.0) as u32,
                                    fir: q.attributes.and_then(|a| a.found_in_raid).unwrap_or(false),
                                })
                            })
                            .collect(),
                        station_requirements: l
                            .station_level_requirements
                            .iter()
                            .filter_map(|r| Some((r.station.as_ref().and_then(ref_id)?, r.level)))
                            .collect(),
                        trader_requirements: l
                            .trader_requirements
                            .into_iter()
                            .map(|t| TraderRequirement {
                                trader: t.trader,
                                requirement_type: t.requirement_type,
                                compare_method: t.compare_method,
                                value: t.value,
                            })
                            .collect(),
                    })
                    .collect(),
                id: s.id,
            })
            .collect();
        stations.sort_by(|a, b| a.name.cmp(&b.name));
        for s in &mut stations {
            s.levels.sort_by_key(|l| l.level);
        }

        let recipes = |name: &str| -> anyhow::Result<Vec<Recipe>> {
            let raw: Vec<RawRecipe> = serde_json::from_value(data(body(name)?)?)?;
            Ok(raw
                .into_iter()
                .map(|r| Recipe { required_items: r.required_items.iter().filter_map(|c| c.item.as_ref().and_then(ref_id)).collect() })
                .collect())
        };

        let raw_traders: HashMap<String, RawTrader> = serde_json::from_value(data(body("traders")?)?)?;
        let traders = raw_traders.into_values().map(|t| (t.id, tr(&traders_en, &t.name))).collect();

        let mut c = Catalog {
            items,
            tasks,
            stations,
            barters: recipes("barters")?,
            crafts: recipes("crafts")?,
            traders,
            chains,
            ..Default::default()
        };
        c.compute_unlocks();
        Ok(c)
    }
}

/// The active mode's catalog; swapped whole on refresh. `None` until the first load succeeds.
pub struct Store {
    current: RwLock<Option<Arc<Catalog>>>,
}

fn cache_file(dir: &Path, mode: Mode, endpoint: &str) -> PathBuf {
    dir.join(format!("{}_{endpoint}.json", mode.api()))
}

/// Catalog from the on-disk cache only (no network); used by the Settings window.
pub fn load_cached(cache_dir: &Path, mode: Mode) -> Option<Catalog> {
    let mut bodies = HashMap::new();
    for ep in ENDPOINTS {
        bodies.insert(ep, std::fs::read(cache_file(cache_dir, mode, ep)).ok()?);
    }
    Catalog::build(&bodies).map_err(|e| log::warn!("cached {} data unusable: {e:#}", mode.api())).ok()
}

fn agent() -> anyhow::Result<ureq::Agent> {
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(native_tls::TlsConnector::new()?))
        .timeout(Duration::from_secs(60))
        .user_agent("ChadOverlay/0.1")
        .build())
}

fn fetch_all(agent: &ureq::Agent, cache_dir: &Path, mode: Mode) -> anyhow::Result<Catalog> {
    let mut bodies = HashMap::new();
    for ep in ENDPOINTS {
        let url = format!("https://json.tarkov.dev/{}/{ep}", mode.api());
        let mut body = Vec::new();
        std::io::Read::read_to_end(&mut agent.get(&url).call()?.into_reader(), &mut body)?;
        bodies.insert(ep, body);
    }
    let catalog = Catalog::build(&bodies)?;
    // Cache only a set that parsed, so a bad response never replaces a good cache.
    for (ep, body) in &bodies {
        if let Err(e) = crate::config::write_atomic(&cache_file(cache_dir, mode, ep), body) {
            log::warn!("caching {ep}: {e}");
        }
    }
    Ok(catalog)
}

impl Store {
    /// Loads the cache now, then fetches in the background every 10 min (60 s after a failure).
    /// `on_missing` is called once when there is neither cache nor a successful first fetch.
    pub fn start(mode: Mode, cache_dir: PathBuf, on_missing: impl Fn() + Send + 'static) -> Arc<Store> {
        let cached = load_cached(&cache_dir, mode).map(Arc::new);
        if cached.is_some() {
            log::info!("{} data loaded from cache", mode.api());
        }
        let store = Arc::new(Store { current: RwLock::new(cached) });
        let weak: Weak<Store> = Arc::downgrade(&store);
        std::thread::Builder::new()
            .name(format!("data-{}", mode.api()))
            .spawn(move || {
                let agent = match agent() {
                    Ok(a) => a,
                    Err(e) => return log::error!("TLS setup failed: {e:#}"),
                };
                let mut warned = false;
                loop {
                    let Some(store) = weak.upgrade() else { return };
                    let wait = match fetch_all(&agent, &cache_dir, mode) {
                        Ok(c) => {
                            log::info!("{} data refreshed: {} items, {} tasks", mode.api(), c.items.len(), c.tasks.len());
                            *store.current.write().unwrap() = Some(Arc::new(c));
                            REFRESH
                        }
                        Err(e) => {
                            log::warn!("{} data fetch failed: {e:#}; retrying in 60 s", mode.api());
                            if store.get().is_none() && !warned {
                                warned = true;
                                on_missing();
                            }
                            RETRY
                        }
                    };
                    drop(store);
                    std::thread::sleep(wait);
                }
            })
            .expect("spawn data thread");
        store
    }

    pub fn get(&self) -> Option<Arc<Catalog>> {
        self.current.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_from_tarkov_dev_shapes() {
        let mut b: HashMap<&str, Vec<u8>> = HashMap::new();
        b.insert("items", br#"{"data":{"items":{"a":{"id":"a","name":"a Name","shortName":"a ShortName","width":1,"height":2,"avg24hPrice":1001,"sellToTrader":[{"trader":"t1","priceRUB":10},{"trader":"t2","priceRUB":30}]}},"fleaMarket":{}}}"#.to_vec());
        b.insert("items_en", br#"{"data":{"a Name":"Alpha","a ShortName":"A"}}"#.to_vec());
        b.insert("tasks", br#"{"data":{"tasks":{"q":{"id":"q","name":"q name","trader":"t1","minPlayerLevel":5,"taskRequirements":[],"traderRequirements":[],"otherRequirements":[{"id":"x","type":"globalVariable","variableId":"v","compareMethod":">=","value":3}],"factionName":"Any","kappaRequired":true,"lightkeeperRequired":false,"objectives":[{"id":"o","type":"mark","count":1,"optional":false,"markerItem":"a"}]}}}}"#.to_vec());
        b.insert("tasks_en", br#"{"data":{"q name":"Quest"}}"#.to_vec());
        b.insert("hideout", br#"{"data":{"s":{"id":"s","name":"s_name","levels":[{"id":"s-1","level":1,"itemRequirements":[{"id":"r","item":"a","count":2,"attributes":{"foundInRaid":true}}]}]}}}"#.to_vec());
        b.insert("hideout_en", br#"{"data":{"s_name":"Stash"}}"#.to_vec());
        b.insert("traders", br#"{"data":{"t1":{"id":"t1","name":"t1 Nickname"}}}"#.to_vec());
        b.insert("traders_en", br#"{"data":{"t1 Nickname":"Prapor"}}"#.to_vec());
        b.insert("barters", br#"{"data":[{"trader":"t1","requiredItems":[{"item":"a","count":155.1}],"offeredItem":{"item":"b","count":1}}]}"#.to_vec());
        b.insert("crafts", br#"{"data":[{"requiredItems":[{"item":"a","count":1}],"productItem":{"item":"b"}}]}"#.to_vec());
        let c = Catalog::build(&b).unwrap();
        let a = &c.items["a"];
        assert_eq!(a.short_name, "A");
        assert_eq!(a.price_per_slot(), Some(500));
        assert_eq!(a.best_trader().unwrap().trader, "t2");
        assert_eq!(c.tasks["q"].name, "Quest");
        assert_eq!(c.tasks["q"].objectives[0].marker_item.as_deref(), Some("a"));
        assert_eq!(c.stations[0].name, "Stash");
        assert!(c.stations[0].levels[0].item_requirements[0].fir);
        assert_eq!(c.traders["t1"], "Prapor");
        assert_eq!(c.barters[0].required_items, vec!["a"]);
        assert_eq!(c.chains["v"], vec![(3.0, "q".to_owned())]);
    }

    #[test]
    fn unlock_counts_are_transitive() {
        use crate::data::{Station, StationLevel, Task, TaskRequirement};
        let mut c = Catalog::default();
        let t = |id: &str, pre: &[&str]| Task {
            id: id.into(),
            task_requirements: pre.iter().map(|p| TaskRequirement { task: (*p).into(), status: vec![] }).collect(),
            ..Default::default()
        };
        for task in [t("a", &[]), t("b", &["a"]), t("c", &["b"]), t("d", &["a"])] {
            c.tasks.insert(task.id.clone(), task);
        }
        let lvl = |id: &str, level, req: Vec<(String, u8)>| StationLevel { id: id.into(), level, station_requirements: req, ..Default::default() };
        c.stations = vec![
            Station { id: "g".into(), levels: vec![lvl("g-1", 1, vec![]), lvl("g-2", 2, vec![])], ..Default::default() },
            Station { id: "w".into(), levels: vec![lvl("w-1", 1, vec![("g".into(), 2)])], ..Default::default() },
        ];
        c.compute_unlocks();
        assert_eq!((c.task_unlocks["a"], c.task_unlocks["b"]), (3, 1));
        // g-1 → g-2 → w-1
        assert_eq!((c.level_unlocks["g-1"], c.level_unlocks["g-2"]), (2, 1));
    }
}
