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
    pub lightkeeper: bool,
    /// Share of the task's other required objectives already done; `None` when there are none.
    pub other_done: Option<f32>,
    /// Tasks that (transitively) require this one.
    pub unlocks: u32,
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
    /// The `StationLevel.id`.
    pub level_id: String,
    pub remaining: u32,
    pub fir: bool,
    /// Next level of its station with every prerequisite met.
    pub buildable: bool,
    /// Unbuilt station levels (own and prerequisite) plus unmet trader gates before this one.
    pub distance: u32,
    /// Share of the level's other item requirements already handed in; `None` when there are none.
    pub other_done: Option<f32>,
    /// Station levels that (transitively) require this one.
    pub unlocks: u32,
}

/// done / total, `None` when total is 0.
fn share(done: u32, total: u32) -> Option<f32> {
    (total > 0).then(|| done as f32 / total as f32)
}

/// Progress on the task's required objectives that do not involve `item_id`.
fn other_objectives_done(task: &Task, item_id: &str, p: &Progress) -> Option<f32> {
    let involves = |o: &Objective| {
        o.items.iter().any(|i| i == item_id) || o.marker_item.as_deref() == Some(item_id) || o.build_item.as_deref() == Some(item_id)
    };
    let (mut done, mut total) = (0, 0);
    for o in task.objectives.iter().filter(|o| !o.optional && !involves(o)) {
        let need = o.count.max(1);
        let e = p.objectives.get(&o.id).filter(|e| !e.invalid);
        done += e.map_or(0, |e| if e.complete { need } else { e.count.min(need) });
        total += need;
    }
    share(done, total)
}

fn task_entry<'a>(p: &'a Progress, id: &str) -> Option<&'a crate::progress::TaskProgress> {
    p.tasks.get(id)
}

/// Started-but-unfinished tasks show up only as recorded objective progress.
fn task_shows_progress(task: &Task, p: &Progress) -> bool {
    let done = task_entry(p, &task.id).is_some_and(|e| e.complete || e.failed || e.invalid);
    // Item counts prove nothing: Arrow Up, Chad Fill and the site's own +/- add them to locked tasks too
    // (collecting ahead), which made one press flip a locked task to "now".
    !done
        && task
            .objectives
            .iter()
            .filter(|o| !matches!(o.kind.as_str(), "giveItem" | "findItem" | "plantItem"))
            .any(|o| p.objectives.get(&o.id).is_some_and(|e| !e.invalid))
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

/// Why a task with no unfinished prerequisites and a reached level is still not available.
pub fn other_blocker(task: &Task, cat: &Catalog, p: &Progress) -> &'static str {
    if task.trader_requirements.iter().any(|r| !matches!(trader_requirement(r, p), Req::Met)) {
        return "trader level";
    }
    if let Some(o) = task.other_requirements.iter().find(|r| !chain_met(r, cat, p)) {
        return if o.kind.eq_ignore_ascii_case("dialogue") { "trader dialogue" } else { "story step" };
    }
    if task.lightkeeper_required {
        return "Lightkeeper";
    }
    if !task.faction_name.is_empty() && !task.faction_name.eq_ignore_ascii_case("Any") && p.faction.is_none() {
        return "set faction";
    }
    "locked"
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

/// Pairs each `giveItem` of the item with an equal-count, equal-FIR `findItem`: (indices of the paired
/// finds, (give, find) index pairs).
fn pairs(item_id: &str, objectives: &[Objective]) -> (Vec<usize>, Vec<(usize, usize)>) {
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
    (paired_find, pair_of)
}

/// (total, fir) remaining for a task. A giveItem paired with an equal-count, equal-FIR findItem counts once.
fn objective_need(item_id: &str, objectives: &[Objective], p: &Progress, show_non_fir: bool) -> (u32, u32) {
    let (paired_find, pair_of) = pairs(item_id, objectives);
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
        // Hidden tasks are skipped like TarkovTracker's own needed items do; gates and unlock chains stay as they are.
        if EXCLUDED_TASKS.contains(&task.id.as_str()) || p.hidden_tasks.contains(&task.id) {
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
            lightkeeper: task.lightkeeper_required,
            other_done: other_objectives_done(task, item_id, p),
            unlocks: cat.task_unlocks.get(&task.id).copied().unwrap_or(0),
        });
    }
    let rank = |t: &TaskNeed| match t.gate {
        Gate::Active => 0,
        Gate::Available => 1,
        _ => 2,
    };
    n.tasks.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Task,
    Hideout,
}

/// One count an arrow key changes: a quest objective (with its paired find) or a hideout part.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub kind: TargetKind,
    /// Objective id, or hideout part (item requirement) id.
    pub id: String,
    /// Task name, or "<station> <level>" (e.g. "Security 2").
    pub label: String,
    pub required: u32,
    /// Recorded count; complete counts as at least `required`.
    pub count: u32,
    /// Still needed, with a paired find folded in (the smaller of the two).
    pub remaining: u32,
    pub fir: bool,
    /// The paired `findItem`, written alongside (quests only).
    pub find_id: Option<String>,
    pub find_count: u32,
}

fn recorded(o: &Objective, p: &Progress) -> u32 {
    p.objectives.get(&o.id).filter(|e| !e.invalid).map_or(0, |e| if e.complete { o.count.max(e.count) } else { e.count })
}

/// Objectives of `task` that take `item_id`, in task order, full ones included.
pub fn task_targets(task: &Task, item_id: &str, p: &Progress) -> Vec<Target> {
    let (paired_find, pair_of) = pairs(item_id, &task.objectives);
    let mut out = Vec::new();
    for (i, o) in task.objectives.iter().enumerate() {
        let takes = !o.optional
            && !o.id.is_empty()
            && matches!(o.kind.as_str(), "giveItem" | "findItem" | "plantItem")
            && o.items.iter().any(|x| x == item_id);
        if paired_find.contains(&i) || !takes {
            continue;
        }
        let (mut remaining, fir) = remaining_for_objective(item_id, o, p, true);
        let find = pair_of.iter().find(|(g, _)| *g == i).map(|&(_, fi)| &task.objectives[fi]);
        if let Some(f) = find {
            remaining = remaining.min(remaining_for_objective(item_id, f, p, true).0);
        }
        out.push(Target {
            kind: TargetKind::Task,
            id: o.id.clone(),
            label: task.name.clone(),
            required: o.count,
            count: recorded(o, p),
            remaining,
            fir,
            find_id: find.filter(|f| !f.id.is_empty()).map(|f| f.id.clone()),
            find_count: find.map_or(0, |f| recorded(f, p)),
        });
    }
    out
}

/// Parts of station level `level_id` that take `item_id`; empty for an unknown level.
pub fn hideout_targets(cat: &Catalog, level_id: &str, item_id: &str, p: &Progress) -> Vec<Target> {
    let Some((station, level)) = cat.stations.iter().find_map(|s| s.levels.iter().find(|l| l.id == level_id).map(|l| (s, l))) else {
        return Vec::new();
    };
    level
        .item_requirements
        .iter()
        .filter(|r| r.item == item_id)
        .map(|req| {
            let part = p.hideout_parts.get(&req.id);
            let complete = part.is_some_and(|e| e.complete);
            let recorded = part.map_or(0, |e| e.count);
            let done = if complete { req.count } else { recorded };
            Target {
                kind: TargetKind::Hideout,
                id: req.id.clone(),
                label: format!("{} {}", station.name, level.level),
                required: req.count,
                count: if complete { recorded.max(req.count) } else { recorded },
                remaining: req.count.saturating_sub(done),
                fir: req.fir,
                find_id: None,
                find_count: 0,
            }
        })
        .collect()
}

/// Targets Arrow Up may fill, in order. With `overfill_fir` off, an item with any FIR target fills only FIR ones.
pub fn fillable(targets: &[Target], overfill_fir: bool) -> impl Iterator<Item = &Target> {
    let fir_only = !overfill_fir && targets.iter().any(|t| t.fir);
    targets.iter().filter(move |t| t.fir || !fir_only)
}

/// Arrow Up's target: the first fillable one with room.
pub fn next_fill(targets: &[Target], overfill_fir: bool) -> Option<&Target> {
    fillable(targets, overfill_fir).find(|t| t.remaining > 0)
}

/// Arrow Down's target: `preferred` (the last raised) while it has a count, else the last with a count.
pub fn next_unfill<'a>(targets: &'a [Target], preferred: Option<&str>) -> Option<&'a Target> {
    preferred.and_then(|id| targets.iter().find(|t| t.id == id && t.count > 0)).or_else(|| targets.iter().rev().find(|t| t.count > 0))
}

/// One need row of TarkovTracker's item popup: its task (`Task`) or station level (`Hideout`) id, whether it
/// needs found-in-raid items, and the count it shows (at most its requirement, as the Collected totals count it).
#[derive(Debug, Clone, PartialEq)]
pub struct PopupRow {
    pub kind: TargetKind,
    pub group: String,
    pub fir: bool,
    pub count: u32,
}

/// What the item popup showed when Chad Fill was clicked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Popup {
    /// The Collected FIR / Non-FIR totals.
    pub fir: u32,
    pub non_fir: u32,
    /// Need rows in the popup's order.
    pub rows: Vec<PopupRow>,
}

/// `p` as if none of `item_id` were handed in. Chad Fill picks and orders its targets from this, so its own
/// writes never change what the next fill does.
pub fn without_item(cat: &Catalog, item_id: &str, p: &Progress) -> Progress {
    let mut out = p.clone();
    for o in cat.tasks.values().flat_map(|t| &t.objectives) {
        if matches!(o.kind.as_str(), "giveItem" | "findItem" | "plantItem") && o.items.iter().any(|x| x == item_id) {
            out.objectives.remove(&o.id);
        }
    }
    for r in cat.stations.iter().flat_map(|s| &s.levels).flat_map(|l| &l.item_requirements) {
        if r.item == item_id {
            out.hideout_parts.remove(&r.id);
        }
    }
    out
}

/// Splits the popup's collected totals over `targets` (fill order) from scratch: each target's new count, plus
/// what is left over. Pass 1: FIR items go to FIR targets and non-FIR items to non-FIR targets, in order.
/// Pass 2 (only with `overfill_fir`): leftover FIR items go to non-FIR targets still short, in order.
/// Non-FIR items never fill FIR targets. Returns `(counts, fir_left, non_fir_left)`; `counts[i]` is at most
/// `targets[i].required`.
pub fn distribute(targets: &[Target], fir: u32, non_fir: u32, overfill_fir: bool) -> (Vec<u32>, u32, u32) {
    let (mut fir, mut non_fir) = (fir, non_fir);
    let mut counts = vec![0u32; targets.len()];
    for (t, c) in targets.iter().zip(counts.iter_mut()) {
        let pool = if t.fir { &mut fir } else { &mut non_fir };
        *c = (*pool).min(t.required);
        *pool -= *c;
    }
    if overfill_fir {
        for (t, c) in targets.iter().zip(counts.iter_mut()).filter(|(t, _)| !t.fir) {
            let take = fir.min(t.required - *c);
            *c += take;
            fir -= take;
        }
    }
    (counts, fir, non_fir)
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
                let (mut done, mut total) = (0, 0);
                for r in level.item_requirements.iter().filter(|r| r.item != item_id) {
                    done += p.hideout_parts.get(&r.id).map_or(0, |e| if e.complete { r.count } else { e.count.min(r.count) });
                    total += r.count;
                }
                out.push(HideoutNeed {
                    station: station.name.clone(),
                    level: level.level,
                    level_id: level.id.clone(),
                    remaining,
                    fir,
                    buildable: distance == 0,
                    distance,
                    other_done: share(done, total),
                    unlocks: cat.level_unlocks.get(&level.id).copied().unwrap_or(0),
                });
            }
        }
    }
    out.sort_by(|a, b| a.distance.cmp(&b.distance).then(a.level.cmp(&b.level)).then(a.station.cmp(&b.station)));
    out
}

/// (barters, crafts) that take the item as an input.
pub fn recipes_using(cat: &Catalog, item_id: &str) -> (usize, usize) {
    let uses = |r: &&crate::data::Recipe| r.required_items.iter().any(|i| i == item_id);
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
        assert!(n.tasks[0].is_current());
    }

    #[test]
    fn item_counts_on_a_locked_task_do_not_unlock_it() {
        // `t` needs `pre`, which is not done; collecting ahead puts a count on t's hand-in.
        let mut t = task("t", vec![obj("g", "giveItem", 3, false), obj("v", "visit", 1, false)]);
        t.task_requirements = vec![crate::data::TaskRequirement { task: "pre".into(), status: vec!["complete".into()] }];
        let c = catalog(vec![task("pre", vec![obj("k", "shoot", 5, false)]), t]);
        let mut p = known();
        p.objectives.insert("g".into(), ObjectiveProgress { complete: false, count: 1, invalid: false });
        let n = quest_needs(&c, ITEM, &p, true);
        let need = n.tasks.iter().find(|x| x.task_id == "t").unwrap();
        assert_eq!(need.gate, Gate::Future);
        assert_eq!(need.missing_prereqs, 1);
        // Progress on a non-item objective still proves the task was started.
        p.objectives.insert("v".into(), ObjectiveProgress { complete: true, count: 1, invalid: false });
        let n = quest_needs(&c, ITEM, &p, true);
        assert_eq!(n.tasks.iter().find(|x| x.task_id == "t").unwrap().gate, Gate::Active);
    }

    #[test]
    fn task_hidden_on_tarkovtracker_contributes_nothing() {
        let c = catalog(vec![task("t", vec![obj("g", "giveItem", 2, false)])]);
        assert_eq!(quest_needs(&c, ITEM, &known(), true).current, 2);
        let mut p = known();
        p.hidden_tasks.insert("t".into());
        assert_eq!(quest_needs(&c, ITEM, &p, true), QuestNeeds::default());
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
        // The player's own faction (any case) keeps the task open.
        p.faction = Some("bear".into());
        assert_eq!(quest_needs(&c, ITEM, &p, true).current, 2);
    }

    #[test]
    fn dialogue_gated_find_give_pair_counts_once_as_conditional() {
        let mut t = task("placeholder_pay_the_fare", vec![obj("find", "findItem", 1, true), obj("give", "giveItem", 1, true)]);
        t.other_requirements = vec![crate::data::OtherRequirement { kind: "dialogue".into(), ..Default::default() }];
        let n = quest_needs(&catalog(vec![t]), ITEM, &known(), true);
        assert_eq!((n.current, n.conditional), (0, 1));
        assert_eq!(n.tasks.len(), 1);
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
        // A FIR hand-in still counts when non-FIR is hidden.
        let fir = catalog(vec![task("t", vec![obj("g", "giveItem", 2, true)])]);
        assert_eq!(quest_needs(&fir, ITEM, &known(), false).current_fir, 2);
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
            vec![HideoutNeed {
                station: "Medstation".into(),
                level: 2,
                level_id: "s-2".into(),
                remaining: 4,
                fir: true,
                buildable: true,
                distance: 0,
                other_done: None,
                unlocks: 0,
            }]
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

    #[test]
    fn fill_skips_full_targets_and_fir_only_when_overfill_off() {
        let t = task("t", vec![obj("f", "giveItem", 2, true), obj("n", "giveItem", 3, false)]);
        let mut p = known();
        p.objectives.insert("f".into(), ObjectiveProgress { complete: false, count: 2, invalid: false });
        let targets = task_targets(&t, ITEM, &p);
        assert_eq!(next_fill(&targets, true).map(|t| t.id.as_str()), Some("n"));
        assert_eq!(next_fill(&targets, false), None);
        // Order is the caller's: the open FIR target comes first here only because it is first.
        let open = task_targets(&t, ITEM, &known());
        assert_eq!(next_fill(&open, false).map(|t| t.id.as_str()), Some("f"));
    }

    #[test]
    fn non_fir_only_item_fills_regardless_of_setting() {
        let t = task("t", vec![obj("n", "giveItem", 3, false)]);
        let targets = task_targets(&t, ITEM, &known());
        assert_eq!(next_fill(&targets, false).map(|t| t.id.as_str()), Some("n"));
    }

    #[test]
    fn paired_find_is_one_target_with_find_id() {
        let t = task("t", vec![obj("f", "findItem", 3, true), obj("g", "giveItem", 3, true)]);
        let targets = task_targets(&t, ITEM, &known());
        assert_eq!(targets.len(), 1);
        assert_eq!((targets[0].id.as_str(), targets[0].find_id.as_deref()), ("g", Some("f")));
        assert_eq!((targets[0].remaining, targets[0].required, targets[0].fir), (3, 3, true));
    }

    #[test]
    fn unfill_prefers_last_increase_else_last_filled() {
        let t = task("t", vec![obj("f", "giveItem", 2, true), obj("n", "giveItem", 3, false)]);
        let mut p = known();
        p.objectives.insert("f".into(), ObjectiveProgress { complete: true, count: 2, invalid: false });
        p.objectives.insert("n".into(), ObjectiveProgress { complete: false, count: 1, invalid: false });
        let targets = task_targets(&t, ITEM, &p);
        let id = |s: Option<&Target>| s.map(|t| t.id.clone());
        assert_eq!(id(next_unfill(&targets, None)), Some("n".into()));
        assert_eq!(id(next_unfill(&targets, Some("f"))), Some("f".into()));
        // A preferred target with nothing recorded falls back to the last filled one.
        p.objectives.remove("n");
        let targets = task_targets(&t, ITEM, &p);
        assert_eq!(id(next_unfill(&targets, Some("n"))), Some("f".into()));
        assert_eq!(id(next_unfill(&task_targets(&t, ITEM, &known()), None)), None);
    }

    #[test]
    fn hideout_targets_treat_complete_part_as_full_and_unknown_level_as_empty() {
        let mut c = Catalog::default();
        let req = HideoutRequirement { id: "r".into(), item: ITEM.into(), count: 3, fir: true };
        c.stations.push(Station {
            id: "s".into(),
            name: "Lavatory".into(),
            levels: vec![StationLevel { id: "s-2".into(), level: 2, item_requirements: vec![req], ..Default::default() }],
        });
        let mut p = known();
        p.hideout_parts.insert("r".into(), crate::progress::PartProgress { complete: true, count: 0 });
        let t = hideout_targets(&c, "s-2", ITEM, &p);
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].count, t[0].remaining, t[0].required, t[0].fir), (3, 0, 3, true));
        assert_eq!((t[0].kind, t[0].label.as_str()), (TargetKind::Hideout, "Lavatory 2"));
        assert!(hideout_targets(&c, "nope", ITEM, &p).is_empty());
    }

    #[test]
    fn distribute_keeps_fir_items_for_fir_needs_in_any_order() {
        let t = |id: &str, required: u32, fir: bool| Target {
            kind: TargetKind::Hideout,
            id: id.into(),
            label: id.into(),
            required,
            count: 0,
            remaining: required,
            fir,
            find_id: None,
            find_count: 0,
        };
        // Card order puts the non-FIR need first; FIR items must not be spent on it before the FIR need.
        let order = [t("sec2", 1, false), t("wc3", 2, true)];
        assert_eq!(distribute(&order, 2, 1, false), (vec![1, 2], 0, 0));
        assert_eq!(distribute(&order, 3, 0, true), (vec![1, 2], 0, 0));
        assert_eq!(distribute(&order, 3, 0, false), (vec![0, 2], 1, 0));
        assert_eq!(distribute(&order, 0, 5, true), (vec![1, 0], 0, 4));
        assert_eq!(distribute(&order, 0, 0, true), (vec![0, 0], 0, 0));
    }

    fn prog(complete: bool, count: u32) -> ObjectiveProgress {
        ObjectiveProgress { complete, count, invalid: false }
    }

    fn req(task: &str, status: &[&str]) -> TaskRequirement {
        TaskRequirement { task: task.into(), status: status.iter().map(|s| s.to_string()).collect() }
    }

    fn trader_req(kind: &str, method: &str, value: f64) -> TraderRequirement {
        TraderRequirement { trader: "Tr".into(), requirement_type: kind.into(), compare_method: method.into(), value }
    }

    fn standing(level: Option<u32>, reputation: Option<f64>) -> crate::progress::TraderStanding {
        crate::progress::TraderStanding { level, reputation }
    }

    #[test]
    fn other_objectives_done_ignores_item_optional_and_invalid_objectives() {
        let mut marker = obj("m", "mark", 1, false);
        marker.items.clear();
        marker.marker_item = Some(ITEM.into());
        let mut build = obj("b", "buildWeapon", 1, false);
        build.items.clear();
        build.build_item = Some(ITEM.into());
        let mut optional = obj("o", "visit", 1, false);
        optional.optional = true;
        let mut visit = obj("v", "visit", 0, false);
        visit.items.clear();
        let mut shoot = obj("s", "shoot", 4, false);
        shoot.items.clear();
        let mut extract = obj("e", "extract", 2, false);
        extract.items.clear();
        let t = task("t", vec![obj("g", "giveItem", 1, false), marker, build, optional, visit, shoot, extract]);
        assert_eq!(other_objectives_done(&t, ITEM, &known()), Some(0.0));
        let mut p = known();
        p.objectives.insert("v".into(), prog(true, 0));
        p.objectives.insert("s".into(), prog(false, 9));
        p.objectives.insert("e".into(), ObjectiveProgress { complete: true, count: 2, invalid: true });
        // visit counts 1/1 (zero-count objective needs 1), shoot is clamped to 4/4, invalid extract is 0/2.
        assert_eq!(other_objectives_done(&t, ITEM, &p), Some(5.0 / 7.0));
        let only_item = task("t", vec![obj("g", "giveItem", 1, false)]);
        assert_eq!(other_objectives_done(&only_item, ITEM, &p), None);
    }

    #[test]
    fn quest_need_reports_other_objective_progress_and_unlock_count() {
        let mut t = task(
            "t",
            vec![obj("g", "giveItem", 1, false), Objective { id: "k".into(), kind: "shoot".into(), count: 2, ..Default::default() }],
        );
        t.lightkeeper_required = true;
        let mut c = catalog(vec![t]);
        c.task_unlocks.insert("t".into(), 7);
        let mut p = known();
        p.objectives.insert("k".into(), prog(false, 1));
        let n = quest_needs(&c, ITEM, &p, true);
        // A recorded non-item objective proves the task was started, so it is active despite the Lightkeeper gate.
        let need = &n.tasks[0];
        assert_eq!((need.gate, need.other_done, need.unlocks, need.lightkeeper), (Gate::Active, Some(0.5), 7, true));
    }

    #[test]
    fn task_failed_or_invalid_progress_is_not_complete_and_all_objectives_done_is() {
        let t = task("t", vec![obj("a", "giveItem", 1, false), obj("b", "visit", 1, false)]);
        let mut p = known();
        assert!(!task_complete(&t, &p));
        p.objectives.insert("a".into(), prog(true, 1));
        assert!(!task_complete(&t, &p));
        p.objectives.insert("b".into(), prog(true, 1));
        assert!(task_complete(&t, &p));
        p.tasks.insert("t".into(), TaskProgress { failed: true, ..Default::default() });
        assert!(!task_complete(&t, &p));
        // An invalid tracker entry is ignored, so the objectives decide.
        p.tasks.insert("t".into(), TaskProgress { failed: true, invalid: true, ..Default::default() });
        assert!(task_complete(&t, &p));
        p.tasks.insert("t".into(), TaskProgress { complete: true, ..Default::default() });
        p.objectives.clear();
        assert!(task_complete(&t, &p));
        // A tracker entry that is neither failed nor complete falls through to objectives.
        p.tasks.insert("t".into(), TaskProgress::default());
        assert!(!task_complete(&t, &p));
        assert!(!task_complete(&task("empty", vec![]), &p));
        // Optional objectives never count; a task of only optional objectives is never complete.
        let mut opt = obj("o", "visit", 1, false);
        opt.optional = true;
        assert!(!task_complete(&task("only-opt", vec![opt]), &p));
        // An invalid objective entry never counts as complete.
        p.objectives.insert("a".into(), ObjectiveProgress { complete: true, count: 1, invalid: true });
        p.objectives.insert("b".into(), prog(true, 1));
        assert!(!task_complete(&t, &p));
    }

    #[test]
    fn task_shows_progress_needs_a_non_item_objective_and_an_unfinished_task() {
        let t = task(
            "t",
            vec![
                obj("g", "giveItem", 1, false),
                obj("p", "plantItem", 1, false),
                obj("f", "findItem", 1, false),
                obj("v", "visit", 1, false),
            ],
        );
        let mut p = known();
        for id in ["g", "p", "f"] {
            p.objectives.insert(id.into(), prog(false, 1));
        }
        assert!(!task_shows_progress(&t, &p));
        p.objectives.insert("v".into(), ObjectiveProgress { complete: false, count: 1, invalid: true });
        assert!(!task_shows_progress(&t, &p));
        p.objectives.insert("v".into(), prog(false, 1));
        assert!(task_shows_progress(&t, &p));
        for done in [
            TaskProgress { complete: true, ..Default::default() },
            TaskProgress { failed: true, ..Default::default() },
            TaskProgress { invalid: true, ..Default::default() },
        ] {
            p.tasks.insert("t".into(), done);
            assert!(!task_shows_progress(&t, &p));
        }
        p.tasks.insert("t".into(), TaskProgress::default());
        assert!(task_shows_progress(&t, &p));
    }

    fn gate_of(requirements: Vec<TaskRequirement>, cat_extra: Vec<Task>, p: &Progress) -> (Gate, Option<u32>) {
        let mut t = task("t", vec![obj("g", "giveItem", 1, false)]);
        t.task_requirements = requirements;
        let mut all = cat_extra;
        all.push(t.clone());
        classify_gate(&t, &catalog(all), p)
    }

    #[test]
    fn prerequisite_end_states_decide_the_gate() {
        let done = |complete, failed| TaskProgress { complete, failed, invalid: false };
        let with = |id: &str, tp: TaskProgress| {
            let mut p = known();
            p.tasks.insert(id.into(), tp);
            p
        };
        // Completed prerequisite satisfies "complete" (case-insensitive) and passes unverifiable "active".
        assert_eq!(gate_of(vec![req("pre", &["Complete"])], vec![], &with("pre", done(true, false))).0, Gate::Applicable);
        assert_eq!(gate_of(vec![req("pre", &["active"])], vec![], &with("pre", done(true, false))).0, Gate::Applicable);
        // Completed when only "failed" was wanted kills the task.
        assert_eq!(gate_of(vec![req("pre", &["failed"])], vec![], &with("pre", done(true, false))).0, Gate::NotApplicable);
        // Failed prerequisite: ok when "failed" is accepted, dead otherwise.
        assert_eq!(gate_of(vec![req("pre", &["failed"])], vec![], &with("pre", done(false, true))).0, Gate::Applicable);
        assert_eq!(gate_of(vec![req("pre", &["complete"])], vec![], &with("pre", done(false, true))).0, Gate::NotApplicable);
        // Active-only prerequisite: satisfied only by recorded progress on the prerequisite.
        let pre = task("pre", vec![obj("pv", "visit", 1, false)]);
        let mut p = known();
        assert_eq!(gate_of(vec![req("pre", &["active"])], vec![pre.clone()], &p).0, Gate::Applicable);
        p.objectives.insert("pv".into(), prog(false, 1));
        assert_eq!(gate_of(vec![req("pre", &["active"])], vec![pre], &p).0, Gate::Applicable);
        // "active"+"complete" and an unknown prerequisite with active status are not locked by a known gate.
        assert_eq!(gate_of(vec![req("ghost", &["active"])], vec![], &known()).0, Gate::Applicable);
        assert_eq!(gate_of(vec![req("ghost", &["active", "complete"])], vec![], &known()).0, Gate::Future);
        assert_eq!(gate_of(vec![req("ghost", &["active", "failed"])], vec![], &known()).0, Gate::Future);
        assert_eq!(gate_of(vec![req("ghost", &["complete"])], vec![], &known()).0, Gate::Future);
    }

    #[test]
    fn level_gate_unknown_player_level_is_conditional_and_met_level_is_open() {
        let mut t = task("t", vec![obj("g", "giveItem", 1, false)]);
        t.min_player_level = 10;
        let c = catalog(vec![t.clone()]);
        assert_eq!(classify_gate(&t, &c, &known()), (Gate::Applicable, None));
        let mut p = known();
        p.level = None;
        assert_eq!(classify_gate(&t, &c, &p), (Gate::Conditional, None));
        p.level = Some(9);
        assert_eq!(classify_gate(&t, &c, &p), (Gate::Future, Some(10)));
    }

    #[test]
    fn trader_requirements_compare_level_or_reputation() {
        let gate = |r: TraderRequirement, p: &Progress| {
            let mut t = task("t", vec![obj("g", "giveItem", 1, false)]);
            t.trader_requirements = vec![r];
            classify_gate(&t, &catalog(vec![t.clone()]), p).0
        };
        let with = |s| {
            let mut p = known();
            p.traders.insert("tr".into(), s);
            p
        };
        // Unknown requirement type is conditional; unreported trader or missing reputation is met.
        assert_eq!(gate(trader_req("loyalty", ">=", 1.0), &known()), Gate::Conditional);
        assert_eq!(gate(trader_req("Level", ">=", 4.0), &known()), Gate::Applicable);
        assert_eq!(gate(trader_req("reputation", ">=", 0.5), &with(standing(Some(1), None))), Gate::Applicable);
        assert_eq!(gate(trader_req("level", ">=", 2.0), &with(standing(None, Some(0.1)))), Gate::Applicable);
        // Reputation comparisons, trader id matched case-insensitively.
        let rep = with(standing(Some(1), Some(0.5)));
        for (method, value, want) in [
            (">=", 0.5, Gate::Applicable),
            (">=", 0.6, Gate::Future),
            (">", 0.5, Gate::Future),
            (">", 0.4, Gate::Applicable),
            ("<=", 0.5, Gate::Applicable),
            ("<=", 0.4, Gate::Future),
            ("<", 0.6, Gate::Applicable),
            ("<", 0.5, Gate::Future),
            ("==", 0.5, Gate::Applicable),
            ("=", 0.5, Gate::Applicable),
            ("=", 0.4, Gate::Future),
            ("!=", 0.4, Gate::Future),
        ] {
            assert_eq!(gate(trader_req("reputation", method, value), &rep), want, "{method} {value}");
        }
    }

    #[test]
    fn story_chain_requirement_is_met_by_progress_on_a_later_stage() {
        let global = |method: &str, value: f64| OtherRequirement {
            kind: "globalVariable".into(),
            variable_id: "story".into(),
            compare_method: method.into(),
            value,
        };
        let mut gated = task("gated", vec![obj("g", "giveItem", 1, false)]);
        gated.other_requirements = vec![global(">=", 2.0)];
        let stage1 = task("s1", vec![obj("v1", "visit", 1, false)]);
        let stage2 = task("s2", vec![obj("v2", "visit", 1, false)]);
        let stage3 = task("s3", vec![obj("v3", "visit", 1, false)]);
        let mut c = catalog(vec![gated.clone(), stage1, stage2, stage3]);
        c.chains.insert("story".into(), vec![(1.0, "s1".into()), (2.0, "s2".into()), (3.0, "s3".into()), (9.0, "missing".into())]);
        let mut p = known();
        assert_eq!(classify_gate(&gated, &c, &p).0, Gate::Conditional);
        p.objectives.insert("v1".into(), prog(true, 1));
        assert_eq!(classify_gate(&gated, &c, &p).0, Gate::Conditional);
        // Merely started stage 2 proves the counter is at least 2.
        p.objectives.insert("v2".into(), prog(false, 1));
        assert_eq!(classify_gate(&gated, &c, &p).0, Gate::Applicable);
        // Unsupported comparison or kind can never be settled; a chain-less variable has nothing reached.
        for other in [
            global(">", 1.0),
            OtherRequirement { kind: "dialogue".into(), ..global(">=", 1.0) },
            OtherRequirement { variable_id: "none".into(), ..global(">=", 1.0) },
        ] {
            let mut t = gated.clone();
            t.other_requirements = vec![other];
            assert_eq!(classify_gate(&t, &c, &p).0, Gate::Conditional);
        }
    }

    #[test]
    fn other_blocker_names_the_first_unmet_reason() {
        let c = Catalog::default();
        let mut t = task("t", vec![]);
        assert_eq!(other_blocker(&t, &c, &known()), "locked");
        t.faction_name = "BEAR".into();
        let mut p = known();
        p.faction = None;
        assert_eq!(other_blocker(&t, &c, &p), "set faction");
        p.faction = Some("BEAR".into());
        assert_eq!(other_blocker(&t, &c, &p), "locked");
        t.faction_name = "Any".into();
        t.lightkeeper_required = true;
        assert_eq!(other_blocker(&t, &c, &known()), "Lightkeeper");
        t.other_requirements = vec![OtherRequirement { kind: "dialogue".into(), ..Default::default() }];
        assert_eq!(other_blocker(&t, &c, &known()), "trader dialogue");
        t.other_requirements = vec![OtherRequirement { kind: "globalVariable".into(), compare_method: ">".into(), ..Default::default() }];
        assert_eq!(other_blocker(&t, &c, &known()), "story step");
        t.trader_requirements = vec![trader_req("level", ">=", 3.0)];
        let mut low = known();
        low.traders.insert("tr".into(), standing(Some(1), None));
        assert_eq!(other_blocker(&t, &c, &low), "trader level");
        // Faction "Any" with no faction set is not a blocker.
        let mut any = task("a", vec![]);
        any.faction_name = String::new();
        assert_eq!(other_blocker(&any, &c, &p), "locked");
    }

    #[test]
    fn mark_and_build_objectives_count_only_with_non_fir_shown() {
        let mut mark = obj("m", "mark", 0, false);
        mark.items.clear();
        mark.marker_item = Some(ITEM.into());
        let mut build = obj("b", "buildWeapon", 2, false);
        build.items.clear();
        build.build_item = Some(ITEM.into());
        let mut other = obj("x", "mark", 1, false);
        other.items.clear();
        other.marker_item = Some("else".into());
        let mut optional = obj("o", "mark", 1, false);
        optional.optional = true;
        optional.marker_item = Some(ITEM.into());
        let c = catalog(vec![task("t", vec![mark, build, other, optional, obj("s", "shoot", 5, false)])]);
        let mut p = known();
        assert_eq!(quest_needs(&c, ITEM, &p, true).current, 3);
        assert_eq!(quest_needs(&c, ITEM, &p, false).current, 0);
        // Any recorded progress on a marker objective satisfies one of its count.
        p.objectives.insert("m".into(), prog(false, 0));
        assert_eq!(quest_needs(&c, ITEM, &p, true).current, 2);
    }

    #[test]
    fn fir_find_without_matching_give_is_not_paired_and_hand_in_uses_count_and_completion() {
        let c = catalog(vec![task(
            "t",
            vec![
                obj("f", "findItem", 3, true),
                obj("g", "giveItem", 2, true),
                obj("g2", "giveItem", 3, false),
                obj("f2", "findItem", 3, false),
            ],
        )]);
        let mut p = known();
        // find(3,FIR) and give(2,FIR) differ in count, so both count; give(3) pairs with find(3).
        assert_eq!(quest_needs(&c, ITEM, &p, true).tasks[0].count, 3 + 2 + 3);
        assert_eq!(quest_needs(&c, ITEM, &p, true).tasks[0].fir, 5);
        p.objectives.insert("g".into(), prog(true, 0));
        p.objectives.insert("g2".into(), prog(false, 1));
        assert_eq!(quest_needs(&c, ITEM, &p, true).tasks[0].count, 3 + 0 + 2);
    }

    #[test]
    fn quest_needs_orders_active_before_available_and_skips_excluded_tasks() {
        let mut active_small = task("a1", vec![obj("g1", "giveItem", 1, false), obj("v1", "visit", 1, false)]);
        active_small.name = "Active small".into();
        let mut active_big = task("a2", vec![obj("g2", "giveItem", 5, false), obj("v2", "visit", 1, false)]);
        active_big.name = "Active big".into();
        let mut avail_a = task("b1", vec![obj("g3", "giveItem", 2, false)]);
        avail_a.name = "Same B".into();
        let mut avail_b = task("b2", vec![obj("g4", "giveItem", 2, false)]);
        avail_b.name = "Same A".into();
        let mut avail_big = task("b3", vec![obj("g5", "giveItem", 9, false)]);
        avail_big.name = "Avail big".into();
        let mut future = task("f", vec![obj("g6", "giveItem", 1, false)]);
        future.min_player_level = 40;
        let mut lk = task("lk", vec![obj("g7", "giveItem", 1, false)]);
        lk.lightkeeper_required = true;
        lk.name = "A lightkeeper".into();
        let excluded = task(EXCLUDED_TASKS[0], vec![obj("g8", "giveItem", 100, false)]);
        let c = catalog(vec![active_small, active_big, avail_a, avail_b, avail_big, future, lk, excluded]);
        let mut p = known();
        p.objectives.insert("v1".into(), prog(true, 1));
        p.objectives.insert("v2".into(), prog(false, 1));
        let n = quest_needs(&c, ITEM, &p, true);
        let order: Vec<&str> = n.tasks.iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(order, vec!["a2", "a1", "b3", "b2", "b1", "lk", "f"]);
        assert_eq!((n.current, n.future, n.conditional), (1 + 5 + 2 + 2 + 9, 1, 1));
    }

    #[test]
    fn task_targets_skip_optional_unnamed_and_other_kind_objectives() {
        let mut optional = obj("o", "giveItem", 1, false);
        optional.optional = true;
        let unnamed = obj("", "giveItem", 1, false);
        let kill = obj("k", "shoot", 1, false);
        let mut elsewhere = obj("e", "giveItem", 1, false);
        elsewhere.items = vec!["else".into()];
        let find_unnamed = obj("", "findItem", 2, true);
        let give = obj("g", "giveItem", 2, true);
        let plant = obj("p", "plantItem", 1, true);
        let t = task("t", vec![optional, unnamed, kill, elsewhere, find_unnamed, give, plant]);
        let mut p = known();
        p.objectives.insert("p".into(), prog(true, 0));
        let targets = task_targets(&t, ITEM, &p);
        assert_eq!(targets.len(), 2);
        // Paired with a find that has no id: the pair is one target without a find id.
        assert_eq!((targets[0].id.as_str(), targets[0].find_id.clone(), targets[0].count, targets[0].remaining), ("g", None, 0, 2));
        // A plantItem is never FIR; a completed one reads as its required count.
        assert_eq!((targets[1].id.as_str(), targets[1].fir, targets[1].count, targets[1].remaining), ("p", false, 1, 0));
    }

    #[test]
    fn missing_prereqs_counts_each_unfinished_ancestor_once_and_skips_unknown_and_done() {
        let chain = |id: &str, pre: &[&str]| {
            let mut t = task(id, vec![]);
            t.task_requirements = pre.iter().map(|p| req(p, &["complete"])).collect();
            t
        };
        let root = chain("root", &["a", "b", "ghost"]);
        let c = catalog(vec![
            root.clone(),
            chain("a", &["shared", "done"]),
            chain("b", &["shared"]),
            chain("shared", &["a"]),
            chain("done", &["hidden"]),
            chain("hidden", &[]),
        ]);
        let mut p = known();
        p.tasks.insert("done".into(), TaskProgress { complete: true, ..Default::default() });
        // a, b, shared (a cycle back to a is not recounted); done and ghost are skipped.
        assert_eq!(missing_prereqs(&root, &c, &p), 3);
    }

    #[test]
    fn hideout_needs_skip_built_and_full_levels_and_report_trader_gates_and_other_progress() {
        let item_req = |id: &str, item: &str, count, fir| HideoutRequirement { id: id.into(), item: item.into(), count, fir };
        let mut c = Catalog::default();
        c.stations.push(Station {
            id: "s".into(),
            name: "Gym".into(),
            levels: vec![
                StationLevel { id: "s-1".into(), level: 1, item_requirements: vec![item_req("r1", ITEM, 2, false)], ..Default::default() },
                StationLevel {
                    id: "s-2".into(),
                    level: 2,
                    item_requirements: vec![
                        item_req("r2", ITEM, 3, false),
                        item_req("r2b", ITEM, 1, true),
                        item_req("o1", "other", 4, false),
                        item_req("o2", "other", 2, false),
                        item_req("o3", "other", 5, false),
                    ],
                    trader_requirements: vec![trader_req("level", ">=", 3.0), trader_req("level", ">=", 1.0)],
                    ..Default::default()
                },
                StationLevel {
                    id: "s-3".into(),
                    level: 3,
                    item_requirements: vec![item_req("r3", ITEM, 1, false)],
                    station_requirements: vec![("ghost".into(), 1)],
                    ..Default::default()
                },
            ],
        });
        c.level_unlocks.insert("s-2".into(), 4);
        let mut p = known();
        p.traders.insert("tr".into(), standing(Some(1), None));
        p.hideout_modules.insert("s-1".into());
        // r2 is half done, r2b fully handed in (complete), so only FIR-free remainder shows.
        p.hideout_parts.insert("r2".into(), crate::progress::PartProgress { complete: false, count: 2 });
        p.hideout_parts.insert("r2b".into(), crate::progress::PartProgress { complete: true, count: 0 });
        p.hideout_parts.insert("o1".into(), crate::progress::PartProgress { complete: true, count: 0 });
        p.hideout_parts.insert("o2".into(), crate::progress::PartProgress { complete: false, count: 9 });
        let n = hideout_needs(&c, ITEM, &p);
        assert_eq!(n.len(), 2);
        let s2 = &n[0];
        assert_eq!((s2.level, s2.remaining, s2.fir, s2.distance, s2.buildable, s2.unlocks), (2, 1, false, 1, false, 4));
        assert_eq!(s2.other_done, Some(6.0 / 11.0));
        // Level 3 has an unknown station prerequisite that adds no steps.
        assert_eq!((n[1].level, n[1].distance, n[1].buildable, n[1].unlocks, n[1].other_done), (3, 1, false, 0, None));
        // Fully handed-in levels do not need the item at all.
        p.hideout_parts.insert("r2".into(), crate::progress::PartProgress { complete: true, count: 0 });
        assert_eq!(hideout_needs(&c, ITEM, &p).len(), 1);
    }

    #[test]
    fn without_item_clears_only_that_items_progress() {
        let mut c = catalog(vec![task(
            "t",
            vec![
                obj("g", "giveItem", 1, false),
                obj("f", "findItem", 1, false),
                obj("pl", "plantItem", 1, false),
                obj("v", "visit", 1, false),
                {
                    let mut o = obj("x", "giveItem", 1, false);
                    o.items = vec!["else".into()];
                    o
                },
            ],
        )]);
        let item_req = |id: &str, item: &str| HideoutRequirement { id: id.into(), item: item.into(), count: 1, fir: false };
        c.stations.push(Station {
            id: "s".into(),
            name: "S".into(),
            levels: vec![StationLevel {
                id: "s-1".into(),
                level: 1,
                item_requirements: vec![item_req("h1", ITEM), item_req("h2", "else")],
                ..Default::default()
            }],
        });
        let mut p = known();
        for id in ["g", "f", "pl", "v", "x"] {
            p.objectives.insert(id.into(), prog(true, 1));
        }
        for id in ["h1", "h2"] {
            p.hideout_parts.insert(id.into(), crate::progress::PartProgress { complete: true, count: 1 });
        }
        let q = without_item(&c, ITEM, &p);
        let mut kept: Vec<&str> = q.objectives.keys().map(String::as_str).collect();
        kept.sort();
        assert_eq!(kept, vec!["v", "x"]);
        assert_eq!(q.hideout_parts.keys().collect::<Vec<_>>(), vec!["h2"]);
    }

    #[test]
    fn recipes_using_counts_barters_and_crafts_separately() {
        let recipe = |items: &[&str]| crate::data::Recipe { required_items: items.iter().map(|s| s.to_string()).collect() };
        let mut c = Catalog::default();
        c.barters = vec![recipe(&[ITEM, "x"]), recipe(&["x"]), recipe(&["y", ITEM])];
        c.crafts = vec![recipe(&[ITEM]), recipe(&[])];
        assert_eq!(recipes_using(&c, ITEM), (2, 1));
    }
}
