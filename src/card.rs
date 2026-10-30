//! The floating card: content from catalog + progress, rendered into a premultiplied pixmap.
use crate::data::{Catalog, Item};
use crate::acquire::{self, Availability, Buy};
use crate::needs;
use crate::progress::Progress;
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use tiny_skia::{Color, Paint, Pixmap, PremultipliedColorU8, Rect, Transform};

const WIDTH: f32 = 360.0;
const PAD: f32 = 10.0;
const BORDER: f32 = 2.0;
const GAP: f32 = 4.0;
const TITLE: f32 = 26.0;
const VALUE: f32 = 22.0;
const DETAIL: f32 = 17.0;

pub const TEXT: Rgb = Rgb(0xF2, 0xF2, 0xF2);
pub const SECONDARY: Rgb = Rgb(0xB8, 0xB8, 0xB8);
pub const AMBER: Rgb = Rgb(0xFF, 0xB0, 0x20);
pub const PURPLE: Rgb = Rgb(0xC0, 0x80, 0xFF);
/// Needed, but nothing can be done with it yet.
pub const LATER: Rgb = Rgb(0xC8, 0x90, 0x40);
pub const GREEN: Rgb = Rgb(0x40, 0xD0, 0x60);
pub const GREY: Rgb = Rgb(0x80, 0x80, 0x80);
pub const RED: Rgb = Rgb(0xFF, 0x50, 0x50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub size: f32,
    pub color: Rgb,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub border: Rgb,
    pub lines: Vec<Line>,
}

fn line(text: impl Into<String>, size: f32, color: Rgb) -> Line {
    Line { text: text.into(), size, color }
}

/// Thousands separated by a thin space.
pub fn rub(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('\u{2009}');
        }
        out.push(c);
    }
    if n < 0 {
        out.insert(0, '-');
    }
    out
}

/// Title + body lines, for toasts and rejected scans.
pub fn message(title: &str, body: &str, border: Rgb) -> Card {
    let mut lines = vec![line(title, VALUE, TEXT)];
    if !body.is_empty() {
        lines.push(line(body, DETAIL, SECONDARY));
    }
    Card { border, lines }
}

/// What the card shows; from Settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardOptions {
    pub show_non_fir: bool,
    pub max_rows: u32,
    pub quests: bool,
    pub hideout: bool,
    pub barters: bool,
    pub crafts: bool,
    pub buy_later_max: i64,
}

impl CardOptions {
    pub fn from_config(c: &crate::config::Config) -> CardOptions {
        CardOptions {
            show_non_fir: c.show_non_fir,
            max_rows: c.max_rows,
            quests: c.show_quests,
            hideout: c.show_hideout,
            barters: c.show_barters,
            crafts: c.show_crafts,
            buy_later_max: c.buy_later_max,
        }
    }
}

/// One need row; ordered by priority: active task, available task, buildable hideout,
/// other tasks in unlock order, other hideout levels in unlock order.
struct Row {
    rank: u8,
    keep: bool,
    count: u32,
    text: String,
    color: Rgb,
}

fn availability_line(cat: &Catalog, a: &Availability, p: &Progress) -> Option<String> {
    let trader = |id: &str| cat.traders.get(id).cloned().unwrap_or_else(|| id.to_owned());
    Some(match a {
        // The flea line below already shows it.
        Availability::Now(Buy::Flea(_)) => return None,
        Availability::Now(Buy::Trader { trader: t, level, price }) => format!("Buy: {} LL{level} {} ₽", trader(t), rub(*price)),
        Availability::Now(Buy::Barter { trader: t, level, cost }) => format!("Barter: {} LL{level} ≈{} ₽", trader(t), rub(*cost)),
        Availability::FleaAt(l) => match p.level {
            Some(you) => format!("Not buyable until flea (Lv{l}, you {you}) – collect"),
            None => format!("Not buyable until flea (Lv{l}) – collect"),
        },
        Availability::Collect => "Can't be bought now – collect".into(),
        Availability::Unknown => return None,
    })
}

pub fn item_card(cat: &Catalog, item: &Item, score: f32, p: &Progress, o: &CardOptions) -> Card {
    let q = if o.quests { needs::quest_needs(cat, &item.id, p, o.show_non_fir) } else { Default::default() };
    let hideout = if o.hideout { needs::hideout_needs(cat, &item.id, p) } else { Vec::new() };
    let avail = acquire::availability(cat, item, p);
    // Non-FIR needs of an item that is cheap to obtain now need not take stash space.
    let buyable = acquire::buy_later(&avail, o.buy_later_max);
    let task_keep = |t: &needs::TaskNeed| if buyable { t.fir } else { t.count };
    let hideout_keep = |h: &needs::HideoutNeed| if buyable && !h.fir { 0 } else { h.remaining };

    let quest_total: u32 = q.tasks.iter().map(|t| t.count).sum();
    let hideout_total: u32 = hideout.iter().map(|h| h.remaining).sum();
    let total = quest_total + hideout_total;
    let keep_current: u32 = q.tasks.iter().filter(|t| t.is_current()).map(task_keep).sum();
    let current_fir = q.tasks.iter().any(|t| t.is_current() && t.fir > 0);
    let keep_buildable: u32 = hideout.iter().filter(|h| h.buildable).map(hideout_keep).sum();
    let keep_kappa: u32 = q.tasks.iter().filter(|t| t.kappa).map(task_keep).sum();
    let keep_total: u32 = q.tasks.iter().map(task_keep).sum::<u32>() + hideout.iter().map(hideout_keep).sum::<u32>();
    let (barters, crafts) = needs::recipes_using(cat, &item.id);
    let best = item.best_trader();

    let title = if score < 0.7 { format!("{} ?", item.short_name) } else { item.short_name.clone() };
    let (verdict, color) = if keep_current > 0 {
        let fir = if current_fir { " (FIR)" } else { "" };
        (format!("KEEP – QUEST {keep_current}×{fir}"), AMBER)
    } else if keep_buildable > 0 {
        (format!("KEEP – HIDEOUT {keep_buildable}×"), AMBER)
    } else if keep_kappa > 0 {
        (format!("KEEP – KAPPA {keep_kappa}×"), PURPLE)
    } else if keep_total > 0 {
        (format!("KEEP – LATER {keep_total}×"), LATER)
    } else if total > 0 {
        (format!("BUY LATER {total}×"), TEXT)
    } else {
        // Items without a flea price are judged by their best trader offer.
        let slots = (item.width * item.height).max(1);
        let per_slot = item.price_per_slot().or(best.map(|b| b.price_rub / slots)).unwrap_or(0);
        if per_slot >= 30_000 {
            ("VALUABLE".into(), GREEN)
        } else if per_slot >= 10_000 {
            ("OK".into(), TEXT)
        } else {
            ("LOW".into(), GREY)
        }
    };

    let mut lines = vec![line(title, TITLE, TEXT), line(verdict, VALUE, color)];
    if total > 0 {
        let parts: Vec<String> = [(o.quests, quest_total, "quests"), (o.hideout, hideout_total, "hideout")]
            .iter()
            .filter(|(on, n, _)| *on && *n > 0)
            .map(|(_, n, what)| format!("{what} {n}"))
            .collect();
        lines.push(line(format!("Total {total}×  ({})", parts.join(" · ")), VALUE, TEXT));
        if let Some(a) = availability_line(cat, &avail, p) {
            lines.push(line(a, DETAIL, SECONDARY));
        }
    }
    if let (Some(avg), Some(per)) = (item.avg24h, item.price_per_slot()) {
        lines.push(line(format!("Flea {} ₽  ·  {} ₽/slot", rub(avg), rub(per)), VALUE, TEXT));
    }
    if let Some(b) = best {
        let name = cat.traders.get(&b.trader).cloned().unwrap_or_else(|| b.trader.clone());
        lines.push(line(format!("{name} {} ₽", rub(b.price_rub)), VALUE, TEXT));
    }

    // Priority: active task, available task, buildable hideout, other tasks, other hideout;
    // within each, needs to keep before needs that can be bought later.
    let buy_tag = |keep: u32| if keep == 0 { " · buy" } else { "" };
    let mut rows: Vec<Row> = q
        .tasks
        .iter()
        .map(|t| {
            let keep = task_keep(t);
            let lvl = match p.level {
                Some(l) if !t.is_current() && u32::from(l) < t.min_level => format!(" (Lv{})", t.min_level),
                _ => String::new(),
            };
            Row {
                rank: match t.gate {
                    needs::Gate::Active => 0,
                    needs::Gate::Available => 1,
                    _ => 3,
                },
                keep: keep > 0,
                count: t.count,
                text: format!("{}× {}{lvl}{}", t.count, t.name, buy_tag(keep)),
                color: if t.is_current() && keep > 0 { TEXT } else { SECONDARY },
            }
        })
        .chain(hideout.iter().map(|h| {
            let keep = hideout_keep(h);
            Row {
                rank: if h.buildable { 2 } else { 4 },
                keep: keep > 0,
                count: h.remaining,
                text: format!("{}× {} Lv{}{}", h.remaining, h.station, h.level, buy_tag(keep)),
                color: if h.buildable && keep > 0 { TEXT } else { SECONDARY },
            }
        }))
        .collect();
    rows.sort_by_key(|r| (r.rank, !r.keep));
    let shown = o.max_rows as usize;
    for r in rows.iter().take(shown) {
        lines.push(line(r.text.clone(), DETAIL, r.color));
    }
    if let Some(l) = more(rows.iter().skip(shown).map(|r| r.count)) {
        lines.push(l);
    }

    let mut recipes = Vec::new();
    if o.barters && barters > 0 {
        recipes.push(format!("Barters: {barters}"));
    }
    if o.crafts && crafts > 0 {
        recipes.push(format!("Crafts: {crafts}"));
    }
    if !recipes.is_empty() {
        lines.push(line(recipes.join(" · "), DETAIL, SECONDARY));
    }
    Card { border: color, lines }
}

/// `+n× in k more` for rows that did not fit, so the rows add up to the total.
fn more(rest: impl Iterator<Item = u32>) -> Option<Line> {
    let (n, k) = rest.fold((0, 0), |(n, k), c| (n + c, k + 1));
    (k > 0).then(|| line(format!("+{n}× in {k} more"), DETAIL, SECONDARY))
}

/// Replaces characters the font lacks with ASCII stand-ins.
fn displayable(font: &FontArc, s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ' ' || font.glyph_id(c).0 != 0 {
            out.push(c);
            continue;
        }
        out.push_str(match c {
            '₽' => "RUB",
            '\u{2009}' => " ",
            '–' => "-",
            '·' => "|",
            '×' => "x",
            '…' => "...",
            _ => "?",
        });
    }
    out
}

fn text_width(font: &FontArc, size: f32, s: &str) -> f32 {
    let f = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, id);
        }
        w += f.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Cuts `s` with an ellipsis to fit `max` px.
fn fit(font: &FontArc, size: f32, s: String, max: f32) -> String {
    if text_width(font, size, &s) <= max {
        return s;
    }
    let ell = displayable(font, "…");
    let mut chars: Vec<char> = s.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let t: String = chars.iter().collect::<String>().trim_end().to_owned() + &ell;
        if text_width(font, size, &t) <= max {
            return t;
        }
    }
    ell
}

fn draw_text(pix: &mut Pixmap, font: &FontArc, size: f32, s: &str, x: f32, baseline: f32, c: Rgb) {
    let f = font.as_scaled(PxScale::from(size));
    let (w, h) = (pix.width() as i32, pix.height() as i32);
    let data = pix.pixels_mut();
    let mut caret = x;
    let mut prev = None;
    for ch in s.chars() {
        let id = f.glyph_id(ch);
        if let Some(p) = prev {
            caret += f.kern(p, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(caret, baseline));
        caret += f.h_advance(id);
        prev = Some(id);
        let Some(outline) = font.outline_glyph(glyph) else { continue };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, cov| {
            let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
            if px < 0 || py < 0 || px >= w || py >= h || cov <= 0.0 {
                return;
            }
            let a = cov.min(1.0);
            let dst = &mut data[(py * w + px) as usize];
            // Source-over in premultiplied space.
            let blend = |s: u8, d: u8| (s as f32 * a + d as f32 * (1.0 - a)).round() as u8;
            let out_a = blend(255, dst.alpha());
            let r = blend(c.0, dst.red()).min(out_a);
            let g = blend(c.1, dst.green()).min(out_a);
            let bl = blend(c.2, dst.blue()).min(out_a);
            *dst = PremultipliedColorU8::from_rgba(r, g, bl, out_a).expect("premultiplied");
        });
    }
}

/// Full card at `scale`: fixed width, height from the lines. Premultiplied RGBA.
pub fn render(card: &Card, font: &FontArc, scale: f32) -> Pixmap {
    let s = scale.clamp(0.75, 2.0);
    let width = (WIDTH * s).round();
    let inner = width - 2.0 * (PAD + BORDER) * s;
    let lines: Vec<(String, f32, Rgb)> = card
        .lines
        .iter()
        .map(|l| {
            let size = l.size * s;
            (fit(font, size, displayable(font, &l.text), inner), size, l.color)
        })
        .collect();
    let heights: Vec<f32> = lines.iter().map(|(_, size, _)| font.as_scaled(PxScale::from(*size)).height()).collect();
    let height = (2.0 * (PAD + BORDER) * s + heights.iter().sum::<f32>() + GAP * s * (lines.len().saturating_sub(1)) as f32).ceil();

    let mut pix = Pixmap::new(width as u32, height.max(1.0) as u32).expect("card size");
    let mut paint = Paint::default();
    let border = card.border;
    paint.set_color(Color::from_rgba8(border.0, border.1, border.2, 255));
    pix.fill_rect(Rect::from_xywh(0.0, 0.0, width, height).unwrap(), &paint, Transform::identity(), None);
    paint.set_color(Color::from_rgba8(0x10, 0x10, 0x10, 235));
    let b = BORDER * s;
    // Clear the inside first so the background alpha is not composited over the border colour.
    pix.fill_rect(Rect::from_xywh(b, b, width - 2.0 * b, height - 2.0 * b).unwrap(), &Paint { blend_mode: tiny_skia::BlendMode::Source, ..paint }, Transform::identity(), None);

    let mut y = (PAD + BORDER) * s;
    for ((text, size, color), h) in lines.iter().zip(&heights) {
        let ascent = font.as_scaled(PxScale::from(*size)).ascent();
        draw_text(&mut pix, font, *size, text, (PAD + BORDER) * s, y + ascent, *color);
        y += h + GAP * s;
    }
    pix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rub_groups_thousands() {
        assert_eq!(rub(1234567), "1\u{2009}234\u{2009}567");
        assert_eq!(rub(999), "999");
        assert_eq!(rub(-1000), "-1\u{2009}000");
    }

    const OPTS: CardOptions =
        CardOptions { show_non_fir: true, max_rows: 1, quests: true, hideout: true, barters: true, crafts: true, buy_later_max: 20_000 };

    #[test]
    fn verdict_precedence_quest_over_price() {
        let mut cat = Catalog::default();
        let item = Item { id: "i".into(), short_name: "PS".into(), width: 1, height: 1, avg24h: Some(50_000), ..Default::default() };
        cat.items.insert("i".into(), item.clone());
        assert_eq!(item_card(&cat, &item, 0.9, &Progress::default(), &OPTS).lines[1].text, "VALUABLE");
        let o = crate::data::Objective { id: "o".into(), kind: "giveItem".into(), count: 2, found_in_raid: true, items: vec!["i".into()], ..Default::default() };
        cat.tasks.insert("t".into(), crate::data::Task { id: "t".into(), name: "Task".into(), objectives: vec![o], ..Default::default() });
        let card = item_card(&cat, &item, 0.6, &Progress::default(), &OPTS);
        assert_eq!(card.lines[0].text, "PS ?");
        assert_eq!(card.lines[1].text, "KEEP – QUEST 2× (FIR)");
        assert_eq!(card.border, AMBER);
    }

    #[test]
    fn rows_by_priority_total_follows_marked_counts_and_toggles() {
        use crate::data::{HideoutRequirement, Objective, Station, StationLevel, Task};
        let mut cat = Catalog::default();
        let item = Item { id: "i".into(), short_name: "Shus".into(), width: 1, height: 1, ..Default::default() };
        let o = Objective { id: "o".into(), kind: "giveItem".into(), count: 2, items: vec!["i".into()], ..Default::default() };
        cat.tasks.insert("t".into(), Task { id: "t".into(), name: "Task".into(), objectives: vec![o], ..Default::default() });
        let req = |id: &str, count| HideoutRequirement { id: id.into(), item: "i".into(), count, fir: false };
        cat.stations.push(Station {
            id: "s".into(),
            name: "Stash".into(),
            levels: vec![
                StationLevel { id: "s-1".into(), level: 1, item_requirements: vec![req("r1", 3)], ..Default::default() },
                StationLevel { id: "s-2".into(), level: 2, item_requirements: vec![req("r2", 5)], ..Default::default() },
            ],
        });
        let opts = CardOptions { max_rows: 2, ..OPTS };
        let text = |p: &Progress, o: &CardOptions| -> Vec<String> {
            item_card(&cat, &item, 0.9, p, o).lines.into_iter().map(|l| l.text).collect()
        };
        let mut p = Progress::default();
        assert_eq!(text(&p, &opts)[2..], ["Total 10×  (quests 2 · hideout 8)", "2× Task", "3× Stash Lv1", "+5× in 1 more"]);
        // One Shus marked on TarkovTracker's needed-items page = hideout part count 1.
        p.hideout_parts.insert("r1".into(), crate::progress::PartProgress { complete: false, count: 1 });
        assert_eq!(text(&p, &opts)[2], "Total 9×  (quests 2 · hideout 7)");
        let no_hideout = CardOptions { hideout: false, ..opts };
        assert_eq!(text(&p, &no_hideout)[2..], ["Total 2×  (quests 2)", "2× Task"]);
    }

    #[test]
    fn cheap_non_fir_need_is_buy_later_fir_or_locked_flea_is_keep() {
        use crate::data::{Objective, Task};
        let mut cat = Catalog { flea_min_level: 15, ..Default::default() };
        let item = Item { id: "i".into(), short_name: "Bolts".into(), width: 1, height: 1, avg24h: Some(5_000), ..Default::default() };
        let quest = |fir| {
            let o = Objective { id: "o".into(), kind: "giveItem".into(), count: 2, found_in_raid: fir, items: vec!["i".into()], ..Default::default() };
            Task { id: "t".into(), name: "Task".into(), objectives: vec![o], ..Default::default() }
        };
        let p = |l| Progress { level: Some(l), ..Default::default() };
        let text = |cat: &Catalog, p: &Progress| -> Vec<String> {
            item_card(cat, &item, 0.9, p, &OPTS).lines.into_iter().map(|l| l.text).collect()
        };
        cat.tasks.insert("t".into(), quest(false));
        let t = text(&cat, &p(20));
        assert_eq!(t[1], "BUY LATER 2×");
        assert!(t.contains(&"2× Task · buy".to_owned()));
        // Flea still locked: nothing to buy it with yet, so keep the found ones.
        assert_eq!(text(&cat, &p(10))[1], "KEEP – QUEST 2×");
        assert!(text(&cat, &p(10)).iter().any(|l| l.starts_with("Not buyable until flea (Lv15, you 10)")));
        cat.tasks.insert("t".into(), quest(true));
        assert_eq!(text(&cat, &p(20))[1], "KEEP – QUEST 2× (FIR)");
    }
}
