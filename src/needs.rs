//! Per-item quest and hideout needs, ported rule-for-rule from RatScanner's
//! `QuestNeedClassifier.cs` and `ItemExtensions.cs`.
use crate::data::{Catalog, Objective, OtherRequirement, Task, TaskRequirement, TraderRequirement};
use crate::progress::Progress;

/// Collector-style event tasks, excluded from every total.
const EXCLUDED_TASKS: [&str; 5] = [
    "61e6e5e0f5b9633f6719ed95",
    "61e6e60223374d168a4576a6",
    "61e6e621bfeab00251576265",
    "61e6e615eea2935bc018a2c5",
    "61e6e60c5ca3b3783662be27",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    NotApplicable,
    Applicable,
    Active,
    Available,
    Future,
    Conditional,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskNeed {
    pub task_id: String,
    pub name: String,
    pub gate: Gate,
    pub count: u32,
    pub fir: u32,
    /// Unfinished tasks in the prerequisite chain.
    pub missing_prereqs: u32,
    pub min_level: u32,
    pub kappa: bool,
}

impl TaskNeed {
    pub fn is_current(&self) -> bool {
        matches!(self.gate, Gate::Active | Gate::Available)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuestNeeds {
    /// Active + available.
    pub current: u32,
    pub current_fir: u32,
    pub future: u32,
    pub conditional: u32,
    pub kappa: u32,
    /// Lowest player level that unlocks a future need.
    pub unlock_level: Option<u32>,
    /// Active, then available, then the rest in unlock order (fewest missing prerequisites, lowest level).
    pub tasks: Vec<TaskNeed>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HideoutNeed {
    pub station: String,
    pub level: u8,
    pub remaining: u32,
    pub fir: bool,
    /// Next level of its station with every prerequisite met.
    pub buildable: bool,
    /// Unbuilt station levels (own and prerequisite) plus unmet trader gates before this one.
    pub distance: u32,
}

fn task_entry<'a>(p: &'a Progress, id: &str) -> Option<&'a crate::progress::TaskProgress> {
    p.tasks.get(id)
}

/// Started-but-unfinished tasks show up only as recorded objective progress.
fn task_shows_progress(task: &Task, p: &Progress) -> bool {
    let done = task_entry(p, &task.id).is_some_and(|e| e.complete || e.failed || e.invalid);
    !done && task.objectives.iter().any(|o| p.objectives.get(&o.id).is_some_and(|e| !e.invalid))
}

/// Tracker complete, or every non-optional objective complete; failure wins over completion.
fn task_complete(task: &Task, p: &Progress) -> bool {
    if let Some(e) = task_entry(p, &task.id).filter(|e| !e.invalid) {
        if e.failed {
            return false;
        }
        if e.complete {
            return true;
        }
    }
    let required: Vec<&Objective> = task.objectives.iter().filter(|o| !o.optional).collect();
    !required.is_empty() && required.iter().all(|o| p.objectives.get(&o.id).is_some_and(|e| !e.invalid && e.complete))
}

enum Prereq {
    Satisfied,
    LockedByKnownGate,
    UnverifiableActive,
    MismatchedEndState,
}

fn prerequisite(req: &TaskRequirement, cat: &Catalog, p: &Progress) -> Prereq {
    let e = task_entry(p, &req.task);
    let complete = e.is_some_and(|e| e.complete);
    let failed = e.is_some_and(|e| e.failed);
    let has = |s: &str| req.status.iter().any(|x| x.eq_ignore_ascii_case(s));
    let wants_active = has("active");
    if complete {
        if has("complete") {
            return Prereq::Satisfied;
        }
        return if wants_active { Prereq::UnverifiableActive } else { Prereq::MismatchedEndState };
    }
    if failed {
        return if has("failed") { Prereq::Satisfied } else { Prereq::MismatchedEndState };
    }
    if wants_active && !has("complete") && !has("failed") {
        if cat.tasks.get(&req.task).is_some_and(|t| task_shows_progress(t, p)) {
            Prereq::Satisfied
        } else {
            Prereq::UnverifiableActive
        }
    } else {
        Prereq::LockedByKnownGate
    }
}

enum Req {
    Met,
    Unmet,
    Unknown,
}

/// Unrecognised compare methods never pass.
fn compare(value: f64, method: &str, target: f64) -> bool {
    match method {
        ">=" => value >= target,
        ">" => value > target,
        "<=" => value <= target,
        "<" => value < target,
        "==" | "=" => value == target,
        _ => false,
    }
}

/// Unreported standing counts as met: it is not evidence the gate is shut.
fn trader_requirement(req: &TraderRequirement, p: &Progress) -> Req {
    let is_level = req.requirement_type.eq_ignore_ascii_case("level");
    let is_rep = req.requirement_type.eq_ignore_ascii_case("reputation");
    if !is_level && !is_rep {
        return Req::Unknown;
    }
    let standing = p.traders.iter().find(|(id, _)| id.eq_ignore_ascii_case(&req.trader)).map(|(_, s)| *s);
    let value = if is_level { standing.and_then(|s| s.level).map(f64::from) } else { standing.and_then(|s| s.reputation) };
    match value {
        None => Req::Met,
        Some(v) if compare(v, &req.compare_method, req.value) => Req::Met,
        Some(_) => Req::Unmet,
    }
}

/// A story counter only advances: reaching stage M proves it is ≥ M. Only `globalVariable >=` can be settled.
fn chain_met(req: &OtherRequirement, cat: &Catalog, p: &Progress) -> bool {
    if !req.kind.eq_ignore_ascii_case("globalVariable") || req.compare_method != ">=" {
        return false;
    }
    let reached = cat
        .chains
        .get(&req.variable_id)
        .into_iter()
        .flatten()
        .filter(|(_, tid)| cat.tasks.get(tid).is_some_and(|t| task_complete(t, p) || task_shows_progress(t, p)))
        .map(|(v, _)| *v)
        .reduce(f64::max);
    reached.is_some_and(|r| r >= req.value)
}

/// Gate without item context; second value is the level that unlocks a level-locked task.
pub fn classify_gate(task: &Task, cat: &Catalog, p: &Progress) -> (Gate, Option<u32>) {
    if task_entry(p, &task.id).is_some_and(|e| e.complete || e.failed || e.invalid) {
        return (Gate::NotApplicable, None);
    }
    let (mut conditional, mut future, mut unlock) = (false, false, None);

    if !task.faction_name.is_empty() && !task.faction_name.eq_ignore_ascii_case("Any") {
        match &p.faction {
            None => conditional = true,
            Some(f) if !f.eq_ignore_ascii_case(&task.faction_name) => return (Gate::NotApplicable, None),
            Some(_) => {}
        }
    }

    for req in &task.task_requirements {
        match prerequisite(req, cat, p) {
            Prereq::Satisfied | Prereq::UnverifiableActive => {}
            Prereq::MismatchedEndState => return (Gate::NotApplicable, None),
            Prereq::LockedByKnownGate => future = true,
        }
    }

    if task.min_player_level > 1 {
        match p.level {
            Some(l) if u32::from(l) < task.min_player_level => {
                future = true;
                unlock = Some(task.min_player_level);
            }
            Some(_) => {}
            None => conditional = true,
        }
    }

    for req in &task.trader_requirements {
        match trader_requirement(req, p) {
            Req::Met => {}
            Req::Unmet => future = true,
            Req::Unknown => conditional = true,
        }
    }

    for req in &task.other_requirements {
        if !chain_met(req, cat, p) {
            conditional = true;
        }
    }

    if task.lightkeeper_required {
        conditional = true;
    }

    let gate = if future {
        Gate::Future
    } else if conditional {
        Gate::Conditional
    } else {
        Gate::Applicable
    };
    (gate, unlock)
}

/// Remaining count for one objective and whether it requires found-in-raid.
fn remaining_for_objective(item_id: &str, o: &Objective, p: &Progress, show_non_fir: bool) -> (u32, bool) {
    if o.optional {
        return (0, false);
    }
    let progress = p.objectives.get(&o.id);
    match o.kind.as_str() {
        "giveItem" | "findItem" | "plantItem" => {
            if !o.items.iter().any(|i| i == item_id) {
                return (0, false);
            }
            let fir = o.kind != "plantItem" && o.found_in_raid;
            if !show_non_fir && !fir {
                return (0, fir);
            }
            let done = progress.map_or(0, |e| if e.complete { o.count } else { e.count });
            (o.count.saturating_sub(done), fir)
        }
        "mark" | "buildWeapon" => {
            if o.marker_item.as_deref() != Some(item_id) && o.build_item.as_deref() != Some(item_id) {
                return (0, false);
            }
            if !show_non_fir {
                return (0, false);
            }
            (o.count.max(1).saturating_sub(progress.map_or(0, |_| 1)), false)
        }
        _ => (0, false),
    }
}

/// (total, fir) remaining for a task. A giveItem paired with an equal-count, equal-FIR findItem counts once.
fn objective_need(item_id: &str, objectives: &[Objective], p: &Progress, show_non_fir: bool) -> (u32, u32) {
    let wants = |o: &Objective, kind: &str| !o.optional && o.kind == kind && o.items.iter().any(|i| i == item_id);
    let mut paired_find: Vec<usize> = Vec::new();
    let mut pair_of: Vec<(usize, usize)> = Vec::new();
    for (gi, give) in objectives.iter().enumerate().filter(|(_, o)| wants(o, "giveItem")) {
        let find = objectives.iter().enumerate().position(|(fi, f)| {
            wants(f, "findItem") && f.count == give.count && f.found_in_raid == give.found_in_raid && !paired_find.contains(&fi)
        });
        if let Some(fi) = find {
            paired_find.push(fi);
            pair_of.push((gi, fi));
        }
    }
    let (mut fir, mut non_fir) = (0, 0);
    for (i, o) in objectives.iter().enumerate() {
        if paired_find.contains(&i) {
            continue;
        }
        let (mut needed, requires_fir) = remaining_for_objective(item_id, o, p, show_non_fir);
        if let Some(&(_, fi)) = pair_of.iter().find(|(g, _)| *g == i) {
            needed = needed.min(remaining_for_objective(item_id, &objectives[fi], p, show_non_fir).0);
        }
        if requires_fir {
            fir += needed;
        } else {
            non_fir += needed;
        }
    }
    (fir + non_fir, fir)
}

pub fn quest_needs(cat: &Catalog, item_id: &str, p: &Progress, show_non_fir: bool) -> QuestNeeds {
    let mut n = QuestNeeds::default();
    for task in cat.tasks.values() {
        if EXCLUDED_TASKS.contains(&task.id.as_str()) {
            continue;
        }
        let (mut gate, unlock) = classify_gate(task, cat, p);
        // Recorded progress proves the unlock gates were already crossed.
        if gate != Gate::NotApplicable && task_shows_progress(task, p) {
            gate = Gate::Active;
        }
        if gate == Gate::NotApplicable {
            continue;
        }
        let (count, fir) = objective_need(item_id, &task.objectives, p, show_non_fir);
        if count == 0 {
            continue;
        }
        if gate == Gate::Applicable {
            gate = Gate::Available;
        }
        if task.kappa_required {
            n.kappa += count;
        }
        match gate {
            Gate::Active | Gate::Available => {
                n.current += count;
                n.current_fir += fir;
            }
            Gate::Future => {
                n.future += count;
                if let Some(l) = unlock {
                    n.unlock_level = Some(n.unlock_level.map_or(l, |u| u.min(l)));
                }
            }
            Gate::Conditional => n.conditional += count,
            Gate::NotApplicable | Gate::Applicable => {}
        }
        n.tasks.push(TaskNeed {
            task_id: task.id.clone(),
            name: task.name.clone(),
            gate,
            count,
            fir,
            missing_prereqs: missing_prereqs(task, cat, p),
            min_level: task.min_player_level,
            kappa: task.kappa_required,
        });
    }
    let rank = |t: &TaskNeed| match t.gate {
        Gate::Active => 0,
        Gate::Available => 1,
        _ => 2,
    };
    n.tasks.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            if rank(a) < 2 {
                b.count.cmp(&a.count)
            } else {
                a.missing_prereqs.cmp(&b.missing_prereqs).then(a.min_level.cmp(&b.min_level))
            }
        })
        .then(a.name.cmp(&b.name))
    });
    n
}

/// Distinct unfinished tasks anywhere in `task`'s prerequisite chain.
fn missing_prereqs(task: &Task, cat: &Catalog, p: &Progress) -> u32 {
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<&str> = task.task_requirements.iter().map(|r| r.task.as_str()).collect();
    while let Some(id) = stack.pop() {
        let Some(t) = cat.tasks.get(id) else { continue };
        if task_complete(t, p) || !seen.insert(id) {
            continue;
        }
        stack.extend(t.task_requirements.iter().map(|r| r.task.as_str()));
    }
    seen.len() as u32
}

/// Highest built level of a station.
fn built(cat: &Catalog, p: &Progress, station: &str) -> u8 {
    cat.stations
        .iter()
        .find(|s| s.id == station)
        .map_or(0, |s| s.levels.iter().filter(|l| p.hideout_modules.contains(&l.id)).map(|l| l.level).max().unwrap_or(0))
}

/// Adds every unbuilt (station, level) needed to reach `level` of `station`, prerequisites included.
fn unbuilt_path(cat: &Catalog, p: &Progress, station: &str, level: u8, out: &mut std::collections::HashSet<(String, u8)>) {
    let Some(s) = cat.stations.iter().find(|s| s.id == station) else { return };
    for l in s.levels.iter().filter(|l| l.level > built(cat, p, station) && l.level <= level) {
        if !out.insert((station.to_owned(), l.level)) {
            continue;
        }
        for (rs, rl) in &l.station_requirements {
            unbuilt_path(cat, p, rs, *rl, out);
        }
    }
}

/// Unbuilt station levels needing the item, one entry per level: buildable first, then fewest steps away.
pub fn hideout_needs(cat: &Catalog, item_id: &str, p: &Progress) -> Vec<HideoutNeed> {
    let mut out = Vec::new();
    for station in &cat.stations {
        for level in &station.levels {
            if p.hideout_modules.contains(&level.id) {
                continue;
            }
            let (mut remaining, mut fir) = (0, false);
            for req in level.item_requirements.iter().filter(|r| r.item == item_id) {
                let done = p.hideout_parts.get(&req.id).map_or(0, |e| if e.complete { req.count } else { e.count });
                let left = req.count.saturating_sub(done);
                if left > 0 {
                    remaining += left;
                    fir |= req.fir;
                }
            }
            if remaining > 0 {
                let mut path = std::collections::HashSet::new();
                unbuilt_path(cat, p, &station.id, level.level, &mut path);
                let traders_unmet =
                    level.trader_requirements.iter().filter(|r| !matches!(trader_requirement(r, p), Req::Met)).count() as u32;
                let distance = path.len().saturating_sub(1) as u32 + traders_unmet;
                out.push(HideoutNeed {
                    station: station.name.clone(),
                    level: level.level,
                    remaining,
                    fir,
                    buildable: distance == 0,
                    distance,
                });
            }
        }
    }
    out.sort_by(|a, b| a.distance.cmp(&b.distance).then(a.level.cmp(&b.level)).then(a.station.cmp(&b.station)));
    out
}

/// (barters, crafts) that take the item as an input.
pub fn recipes_using(cat: &Catalog, item_id: &str) -> (usize, usize) {
    let uses = |r: &&crate::data::Recipe| r.inputs.iter().any(|(i, _)| i == item_id);
    (cat.barters.iter().filter(uses).count(), cat.crafts.iter().filter(uses).count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{HideoutRequirement, Station, StationLevel};
    use crate::progress::{ObjectiveProgress, TaskProgress};

    const ITEM: &str = "item";

    fn obj(id: &str, kind: &str, count: u32, fir: bool) -> Objective {
        Objective { id: id.into(), kind: kind.into(), count, found_in_raid: fir, items: vec![ITEM.into()], ..Default::default() }
    }

    fn task(id: &str, objectives: Vec<Objective>) -> Task {
        Task { id: id.into(), name: id.into(), faction_name: "Any".into(), objectives, ..Default::default() }
    }

    fn catalog(tasks: Vec<Task>) -> Catalog {
        let mut c = Catalog::default();
        for t in tasks {
            c.tasks.insert(t.id.clone(), t);
        }
        c
    }

    fn known() -> Progress {
        Progress { level: Some(10), faction: Some("USEC".into()), ..Default::default() }
    }

    #[test]
    fn fir_give_and_find_pair_counted_once() {
        let c = catalog(vec![task("t", vec![obj("f", "findItem", 3, true), obj("g", "giveItem", 3, true)])]);
        let n = quest_needs(&c, ITEM, &known(), true);
        assert_eq!((n.current, n.current_fir), (3, 3));
    }

    #[test]
    fn pair_uses_the_further_along_half() {
        let c = catalog(vec![task("t", vec![obj("f", "findItem", 3, true), obj("g", "giveItem", 3, true)])]);
        let mut p = known();
        p.objectives.insert("f".into(), ObjectiveProgress { complete: false, count: 2, invalid: false });
        let n = quest_needs(&c, ITEM, &p, true);
        assert_eq!(n.current, 1);
        assert_eq!(n.tasks[0].gate, Gate::Active);
    }

    #[test]
    fn completed_task_contributes_nothing() {
        let c = catalog(vec![task("t", vec![obj("g", "giveItem", 2, false)])]);
        let mut p = known();
        p.tasks.insert("t".into(), TaskProgress { complete: true, ..Default::default() });
        assert_eq!(quest_needs(&c, ITEM, &p, true), QuestNeeds::default());
    }

    #[test]
    fn wrong_faction_contributes_nothing_unknown_faction_is_conditional() {
        let mut t = task("t", vec![obj("g", "giveItem", 2, false)]);
        t.faction_name = "BEAR".into();
        let c = catalog(vec![t]);
        assert_eq!(quest_needs(&c, ITEM, &known(), true), QuestNeeds::default());
        let mut p = known();
        p.faction = None;
        assert_eq!(quest_needs(&c, ITEM, &p, true).conditional, 2);
    }

    #[test]
    fn level_locked_task_is_future_with_unlock_level() {
        let mut t = task("t", vec![obj("g", "giveItem", 4, false)]);
        t.min_player_level = 20;
        let mut t2 = task("t2", vec![obj("g2", "giveItem", 1, false)]);
        t2.min_player_level = 15;
        let n = quest_needs(&catalog(vec![t, t2]), ITEM, &known(), true);
        assert_eq!((n.current, n.future, n.unlock_level), (0, 5, Some(15)));
    }

    #[test]
    fn server_invalid_task_contributes_nothing() {
        let c = catalog(vec![task("t", vec![obj("g", "giveItem", 2, true)])]);
        let mut p = known();
        p.tasks.insert("t".into(), TaskProgress { invalid: true, ..Default::default() });
        assert_eq!(quest_needs(&c, ITEM, &p, true), QuestNeeds::default());
    }

    #[test]
    fn non_fir_excluded_when_hidden() {
        let c = catalog(vec![task("t", vec![obj("g", "giveItem", 2, false), obj("p", "plantItem", 1, false)])]);
        assert_eq!(quest_needs(&c, ITEM, &known(), true).current, 3);
        assert_eq!(quest_needs(&c, ITEM, &known(), false).current, 0);
    }

    #[test]
    fn unmet_prerequisite_is_future_and_failed_mismatch_is_dead() {
        let mut t = task("t", vec![obj("g", "giveItem", 1, false)]);
        t.task_requirements = vec![TaskRequirement { task: "pre".into(), status: vec!["complete".into()] }];
        let c = catalog(vec![t, task("pre", vec![])]);
        assert_eq!(quest_needs(&c, ITEM, &known(), true).future, 1);
        let mut p = known();
        p.tasks.insert("pre".into(), TaskProgress { complete: false, failed: true, invalid: false });
        assert_eq!(quest_needs(&c, ITEM, &p, true).future + quest_needs(&c, ITEM, &p, true).current, 0);
    }

    #[test]
    fn trader_level_short_locks_unreported_passes() {
        let mut t = task("t", vec![obj("g", "giveItem", 1, false)]);
        t.trader_requirements =
            vec![TraderRequirement { trader: "tr".into(), requirement_type: "level".into(), compare_method: ">=".into(), value: 3.0 }];
        let c = catalog(vec![t]);
        assert_eq!(quest_needs(&c, ITEM, &known(), true).current, 1);
        let mut p = known();
        p.traders.insert("tr".into(), crate::progress::TraderStanding { level: Some(2), reputation: None });
        assert_eq!(quest_needs(&c, ITEM, &p, true).future, 1);
    }

    #[test]
    fn kappa_counts_every_open_gate() {
        let mut t = task("t", vec![obj("g", "giveItem", 2, true)]);
        t.kappa_required = true;
        t.min_player_level = 50;
        let n = quest_needs(&catalog(vec![t]), ITEM, &known(), true);
        assert_eq!((n.kappa, n.current), (2, 0));
    }

    #[test]
    fn built_hideout_level_needs_nothing() {
        let mut c = Catalog::default();
        let req = |id: &str, count| HideoutRequirement { id: id.into(), item: ITEM.into(), count, fir: true };
        c.stations.push(Station {
            id: "s".into(),
            name: "Medstation".into(),
            levels: vec![
                StationLevel { id: "s-1".into(), level: 1, item_requirements: vec![req("r1", 2)], ..Default::default() },
                StationLevel { id: "s-2".into(), level: 2, item_requirements: vec![req("r2", 5)], ..Default::default() },
            ],
        });
        let mut p = known();
        p.hideout_modules.insert("s-1".into());
        p.hideout_parts.insert("r2".into(), crate::progress::PartProgress { complete: false, count: 1 });
        assert_eq!(
            hideout_needs(&c, ITEM, &p),
            vec![HideoutNeed { station: "Medstation".into(), level: 2, remaining: 4, fir: true, buildable: true, distance: 0 }]
        );
    }

    #[test]
    fn hideout_buildable_first_then_fewest_steps() {
        let mut c = Catalog::default();
        let req = |id: &str| HideoutRequirement { id: id.into(), item: ITEM.into(), count: 1, fir: false };
        let lvl = |id: &str, level, r: &str, needs: Vec<(String, u8)>| StationLevel {
            id: id.into(),
            level,
            item_requirements: vec![req(r)],
            station_requirements: needs,
            ..Default::default()
        };
        c.stations.push(Station {
            id: "a".into(),
            name: "A".into(),
            levels: vec![lvl("a-1", 1, "ra1", vec![]), lvl("a-2", 2, "ra2", vec![]), lvl("a-3", 3, "ra3", vec![])],
        });
        c.stations.push(Station { id: "b".into(), name: "B".into(), levels: vec![lvl("b-1", 1, "rb1", vec![("a".into(), 2)])] });
        let mut p = known();
        p.hideout_modules.insert("a-1".into());
        let n = hideout_needs(&c, ITEM, &p);
        let got: Vec<(&str, u8, bool, u32)> = n.iter().map(|h| (h.station.as_str(), h.level, h.buildable, h.distance)).collect();
        // B1 needs A2 first (1 step); A3 needs A2 (1 step); tie → lower level first.
        assert_eq!(got, vec![("A", 2, true, 0), ("B", 1, false, 1), ("A", 3, false, 1)]);
    }

    #[test]
    fn future_tasks_in_unlock_order() {
        let mut near = task("near", vec![obj("g1", "giveItem", 1, false)]);
        near.min_player_level = 30;
        let mut far = task("far", vec![obj("g2", "giveItem", 1, false)]);
        far.task_requirements = vec![TaskRequirement { task: "p1".into(), status: vec!["complete".into()] }];
        let mut p1 = task("p1", vec![]);
        p1.task_requirements = vec![TaskRequirement { task: "p0".into(), status: vec!["complete".into()] }];
        let now = task("now", vec![obj("g3", "giveItem", 1, false)]);
        let c = catalog(vec![near, far, p1, task("p0", vec![]), now]);
        let n = quest_needs(&c, ITEM, &known(), true);
        let order: Vec<&str> = n.tasks.iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(order, vec!["now", "near", "far"]);
        assert_eq!(n.tasks[2].missing_prereqs, 2);
    }
}
