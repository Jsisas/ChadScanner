//! ChadScanner ↔ tarkovtracker.org bridge: the injected page script, the calls into it, and its
//! messages back.
use crate::needs::{Popup, PopupRow, Target, TargetKind};
use crate::progress::{ObjectiveProgress, PartProgress};
use std::collections::HashMap;

pub const START_URL: &str = "https://tarkovtracker.org/needed-items";
const SCRIPT: &str = include_str!("tracker.js");
const ICON_SVG: &str = include_str!("../assets/icon.svg");
/// Posted by tracker.js once a document has `window.__chad`; the window then replays the raid in progress.
pub const READY: &str = "chad-ready";

/// Injected into every main-frame document (defines `window.__chad` on tarkovtracker.org only), with the exe icon
/// filled in for the Chad Fill button.
pub fn script() -> String {
    SCRIPT.replacen("CHAD_ICON_SVG", &js(ICON_SVG), 1)
}

fn js(s: &str) -> String {
    serde_json::to_string(s).expect("string serialises")
}

pub fn open_script(id: &str, name: &str) -> String {
    format!("window.__chad&&__chad.open({},{})", js(id), js(name))
}

fn targets_json(targets: &[Target]) -> serde_json::Value {
    serde_json::Value::Array(
        targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "kind": if t.kind == TargetKind::Task { "task" } else { "hideout" },
                    "id": t.id,
                    "find": t.find_id,
                    "required": t.required,
                    "fir": t.fir,
                    "label": t.label,
                })
            })
            .collect(),
    )
}

/// One Arrow key press: `up` raises the first of `targets` with room, else undoes; `targets` in fill order.
pub fn step_script(id: &str, name: &str, up: bool, targets: &[Target]) -> String {
    format!("window.__chad&&__chad.step({},{},{up},{})", js(id), js(name), targets_json(targets))
}

/// Chad Fill: sets each of `targets` to the matching `counts` entry; `fir_left`/`non_fir_left` are the
/// collected items no target took.
pub fn fill_script(id: &str, name: &str, targets: &[Target], counts: &[u32], fir_left: u32, non_fir_left: u32) -> String {
    format!(
        "window.__chad&&__chad.fill({},{},{},{},{fir_left},{non_fir_left})",
        js(id),
        js(name),
        targets_json(targets),
        serde_json::json!(counts)
    )
}

/// Switches the site to game mode `mode` (`Mode::key()`); a no-op when it already shows it.
pub fn mode_script(mode: &str) -> String {
    format!("window.__chad&&__chad.mode({})", js(mode))
}

/// Raises trader loyalty levels from `(trader id, name, level)`.
pub fn traders_script(list: &[(String, String, u8)]) -> String {
    let items: Vec<_> = list.iter().map(|(id, name, level)| serde_json::json!({"id": id, "name": name, "level": level})).collect();
    format!("window.__chad&&__chad.traders({})", serde_json::Value::Array(items))
}

/// Raises the player level.
pub fn level_script(level: u8) -> String {
    format!("window.__chad&&__chad.level({level})")
}

/// Raises hideout station levels from `(station id, name, level)`.
pub fn hideout_script(list: &[(String, String, u8)]) -> String {
    let items: Vec<_> = list.iter().map(|(id, name, level)| serde_json::json!({"id": id, "name": name, "level": level})).collect();
    format!("window.__chad&&__chad.hideout({})", serde_json::Value::Array(items))
}

/// Opens the tarkovtracker.org map of the tarkov.dev map `map_id` for a raid; `rotation` is its `coordinateRotation`.
pub fn raid_script(map_id: &str, rotation: f64) -> String {
    format!("window.__chad&&__chad.raid({},{rotation})", js(map_id))
}

/// Shows only the extracts named in `names` on the raid map; `side` ("pmc"/"scav") picks the Chad-mode layers.
pub fn raid_extracts_script(names: &[String], side: Option<&str>) -> String {
    format!(
        "window.__chad&&__chad.raidExtracts({},{})",
        serde_json::to_string(names).expect("strings serialise"),
        serde_json::to_string(&side).expect("option serialises")
    )
}

/// Draws the player's arrow; `position_json` is `{"pos":[z,x],"yaw":degrees}`.
pub fn here_script(position_json: &str) -> String {
    format!("window.__chad&&__chad.here({position_json})")
}

/// Brings the raid map back (after an item page replaced it).
pub fn raid_show_script() -> &'static str {
    "window.__chad&&__chad.raidShow()"
}

/// Ends the raid: arrow and extract filter go, the page returns to where it was before the raid.
pub fn raid_end_script() -> &'static str {
    "window.__chad&&__chad.raidEnd()"
}

#[derive(Debug, PartialEq)]
pub enum Message {
    Toast { title: String, body: String, warn: bool },
    Log(String),
    /// Chad Fill clicked in the item popup for item `id`: what the popup showed.
    Fill { id: String, popup: Popup },
    /// The site's own progress for its current game mode (`pvp`/`pve`/`seasonal`), by objective, hideout-part and hideout-module id.
    Progress {
        mode: String,
        objectives: HashMap<String, ObjectiveProgress>,
        parts: HashMap<String, PartProgress>,
        /// hideout module id → complete
        modules: HashMap<String, bool>,
    },
}

/// `{id: {count, complete}}` → `id → make(complete, count)`; missing fields read as 0 / false, a non-object as empty.
fn counts<T>(v: Option<&serde_json::Value>, make: impl Fn(bool, u32) -> T) -> HashMap<String, T> {
    let Some(serde_json::Value::Object(m)) = v else { return HashMap::new() };
    m.iter()
        .map(|(id, e)| {
            let count = e.get("count").and_then(|c| c.as_f64()).unwrap_or(0.0).max(0.0) as u32;
            (id.clone(), make(e.get("complete").and_then(|c| c.as_bool()).unwrap_or(false), count))
        })
        .collect()
}

/// One `{"kind":"task"|"hideout","group","fir","count"}` popup row; malformed rows → None.
fn popup_row(v: &serde_json::Value) -> Option<PopupRow> {
    let kind = match v.get("kind")?.as_str()? {
        "task" => TargetKind::Task,
        "hideout" => TargetKind::Hideout,
        _ => return None,
    };
    Some(PopupRow {
        kind,
        group: v.get("group")?.as_str()?.to_owned(),
        fir: v.get("fir").and_then(|b| b.as_bool()).unwrap_or(false),
        count: v.get("count").and_then(|c| c.as_f64()).unwrap_or(0.0).max(0.0) as u32,
    })
}

/// A page message: `{"toast":{"title","body","warn"}}`, `{"progress":{"mode","objectives","parts","modules"}}`,
/// `{"fill":{"id","fir","nonFir","rows":[{"kind","group","fir","count"}]}}` or `{"log":"…"}`; anything else → None.
pub fn parse(msg: &str) -> Option<Message> {
    let v: serde_json::Value = serde_json::from_str(msg).ok()?;
    if let Some(t) = v.get("toast") {
        return Some(Message::Toast {
            title: t.get("title")?.as_str()?.to_owned(),
            body: t.get("body")?.as_str()?.to_owned(),
            warn: t.get("warn").and_then(|w| w.as_bool()).unwrap_or(false),
        });
    }
    if let Some(p) = v.get("progress") {
        return Some(Message::Progress {
            mode: p.get("mode")?.as_str()?.to_owned(),
            objectives: counts(p.get("objectives"), |complete, count| ObjectiveProgress { complete, count, invalid: false }),
            parts: counts(p.get("parts"), |complete, count| PartProgress { complete, count }),
            modules: counts(p.get("modules"), |complete, _| complete),
        });
    }
    if let Some(f) = v.get("fill") {
        let n = |k: &str| f.get(k).and_then(|c| c.as_f64()).unwrap_or(0.0).max(0.0) as u32;
        let rows = f.get("rows").and_then(serde_json::Value::as_array).map_or_else(Vec::new, |a| a.iter().filter_map(popup_row).collect());
        return Some(Message::Fill { id: f.get("id")?.as_str()?.to_owned(), popup: Popup { fir: n("fir"), non_fir: n("nonFir"), rows } });
    }
    Some(Message::Log(v.get("log")?.as_str()?.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_messages() {
        assert_eq!(
            parse(r#"{"toast":{"title":"T","body":"B","warn":true}}"#),
            Some(Message::Toast { title: "T".into(), body: "B".into(), warn: true })
        );
        assert_eq!(
            parse(r#"{"toast":{"title":"T","body":"B"}}"#),
            Some(Message::Toast { title: "T".into(), body: "B".into(), warn: false })
        );
        assert_eq!(parse(r#"{"log":"x"}"#), Some(Message::Log("x".into())));
        assert_eq!(parse("\"nope\""), None);
        assert_eq!(parse("{}"), None);
        assert_eq!(parse(r#"{"toast":{"body":"B"}}"#), None);
    }

    #[test]
    fn mode_script_passes_the_key() {
        assert_eq!(mode_script("pve"), r#"window.__chad&&__chad.mode("pve")"#);
    }

    #[test]
    fn menu_scripts_carry_ids_and_levels() {
        assert_eq!(level_script(26), "window.__chad&&__chad.level(26)");
        assert_eq!(
            traders_script(&[("t1".into(), "Prapor".into(), 2)]),
            r#"window.__chad&&__chad.traders([{"id":"t1","level":2,"name":"Prapor"}])"#
        );
        assert_eq!(
            hideout_script(&[("s1".into(), "Vents".into(), 2)]),
            r#"window.__chad&&__chad.hideout([{"id":"s1","level":2,"name":"Vents"}])"#
        );
    }

    #[test]
    fn raid_extracts_script_passes_the_side() {
        assert_eq!(
            raid_extracts_script(&["A".into()], Some("pmc")),
            r#"window.__chad&&__chad.raidExtracts(["A"],"pmc")"#
        );
        assert_eq!(raid_extracts_script(&[], None), r#"window.__chad&&__chad.raidExtracts([],null)"#);
    }

    #[test]
    fn parses_progress() {
        let m = parse(
            r#"{"progress":{"mode":"pvp","objectives":{"o":{"count":2,"complete":true}},"parts":{"p":{"count":1}},"modules":{"s-1":{"complete":true},"s-2":{}}}}"#,
        );
        let Some(Message::Progress { mode, objectives, parts, modules }) = m else { panic!("not a progress message: {m:?}") };
        assert_eq!(mode, "pvp");
        assert_eq!(objectives["o"], ObjectiveProgress { complete: true, count: 2, invalid: false });
        assert_eq!(parts["p"], PartProgress { complete: false, count: 1 });
        assert!(modules["s-1"]);
        assert!(!modules["s-2"]);
        assert_eq!(parse(r#"{"progress":{"objectives":{}}}"#), None);
        assert!(matches!(parse(r#"{"progress":{"mode":"pve"}}"#), Some(Message::Progress { objectives, .. }) if objectives.is_empty()));
    }

    #[test]
    fn step_script_quotes_names() {
        let t = Target {
            kind: TargetKind::Hideout,
            id: "p1".into(),
            label: "Security 2".into(),
            required: 1,
            count: 0,
            remaining: 1,
            fir: false,
            find_id: None,
            find_count: 0,
        };
        assert_eq!(
            step_script("a", "O'Neil \"x\"", true, &[t]),
            r#"window.__chad&&__chad.step("a","O'Neil \"x\"",true,[{"find":null,"fir":false,"id":"p1","kind":"hideout","label":"Security 2","required":1}])"#
        );
    }

    #[test]
    fn parses_fill() {
        let row = |kind, group: &str, fir, count| PopupRow { kind, group: group.into(), fir, count };
        assert_eq!(
            parse(
                r#"{"fill":{"id":"i","fir":2,"nonFir":1,"rows":[{"kind":"task","group":"t1","fir":true,"count":2},
                {"kind":"hideout","group":"h1"},{"kind":"x","group":"g"},{"kind":"task"}]}}"#
            ),
            Some(Message::Fill {
                id: "i".into(),
                popup: Popup {
                    fir: 2,
                    non_fir: 1,
                    rows: vec![row(TargetKind::Task, "t1", true, 2), row(TargetKind::Hideout, "h1", false, 0)],
                },
            })
        );
        assert_eq!(parse(r#"{"fill":{"id":"i"}}"#), Some(Message::Fill { id: "i".into(), popup: Popup::default() }));
        assert_eq!(parse(r#"{"fill":{}}"#), None);
    }

    #[test]
    fn fill_script_carries_targets_and_counts() {
        let t = Target {
            kind: TargetKind::Hideout,
            id: "p1".into(),
            label: "Security 2".into(),
            required: 1,
            count: 0,
            remaining: 1,
            fir: false,
            find_id: None,
            find_count: 0,
        };
        assert_eq!(
            fill_script("a", "Pliers", &[t], &[1], 0, 2),
            r#"window.__chad&&__chad.fill("a","Pliers",[{"find":null,"fir":false,"id":"p1","kind":"hideout","label":"Security 2","required":1}],[1],0,2)"#
        );
    }
}
