//! The floating card: content from catalog + progress, rendered into a premultiplied pixmap.
use crate::data::{Catalog, Item};
use crate::config::Goal;
use crate::needs;
use crate::progress::Progress;
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use tiny_skia::{Color, Paint, Pixmap, PremultipliedColorU8, Rect, Transform};

const WIDTH: f32 = 380.0;
const PAD: f32 = 12.0;
const BORDER: f32 = 2.0;
const GAP: f32 = 5.0;
const TITLE: f32 = 26.0;
const HEADLINE: f32 = 20.0;
const ROW: f32 = 18.0;
const LABEL: f32 = 12.0;
const SMALL: f32 = 14.0;
const FOOT: f32 = 16.0;

pub const TEXT: Rgb = Rgb(0xF2, 0xF2, 0xF2);
pub const SECONDARY: Rgb = Rgb(0xB8, 0xB8, 0xB8);
pub const MUTED: Rgb = Rgb(0x8C, 0x8C, 0x8C);
pub const AMBER: Rgb = Rgb(0xFF, 0xB0, 0x20);
pub const PURPLE: Rgb = Rgb(0xC0, 0x80, 0xFF);
/// Needed, but nothing can be done with it yet.
pub const LATER: Rgb = Rgb(0xC8, 0x90, 0x40);
pub const GREEN: Rgb = Rgb(0x40, 0xD0, 0x60);
pub const GREY: Rgb = Rgb(0x80, 0x80, 0x80);
pub const RED: Rgb = Rgb(0xFF, 0x50, 0x50);
const RULE: Rgb = Rgb(0x3A, 0x3A, 0x3A);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Style {
    Plain,
    /// Tinted band behind the line (the verdict).
    Band,
    /// Small uppercase group heading.
    Label,
    /// Thin separator; text is ignored.
    Rule,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub size: f32,
    pub color: Rgb,
    pub bold: bool,
    pub style: Style,
    /// Right-aligned text on the same line.
    pub right: Option<(String, Rgb)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub border: Rgb,
    pub lines: Vec<Line>,
}

fn line(text: impl Into<String>, size: f32, color: Rgb) -> Line {
    Line { text: text.into(), size, color, bold: true, style: Style::Plain, right: None }
}

impl Line {
    fn right(mut self, text: impl Into<String>, color: Rgb) -> Line {
        let t = text.into();
        if !t.is_empty() {
            self.right = Some((t, color));
        }
        self
    }
    fn style(mut self, style: Style) -> Line {
        self.style = style;
        self
    }
    fn regular(mut self) -> Line {
        self.bold = false;
        self
    }
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
    let mut lines = vec![line(title, HEADLINE, TEXT)];
    if !body.is_empty() {
        lines.push(line(body, SMALL, SECONDARY).regular());
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
    pub smart: bool,
    pub goal: Goal,
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
            smart: c.smart_ordering,
            goal: c.goal,
        }
    }
}

/// One need row with its ordering keys.
struct Row {
    /// Index into the group headings.
    group: usize,
    /// Smart: buildable hideout before the rest.
    rank: u8,
    count: u32,
    /// Share of the requirement's other parts already done (1.0 = this item is the last piece).
    done: f32,
    levels_to_go: u32,
    unlocks: u32,
    name: String,
    tag: String,
    tag_color: Rgb,
    color: Rgb,
}

fn in_goal(t: &needs::TaskNeed, goal: Goal) -> bool {
    match goal {
        Goal::None => true,
        Goal::Kappa => t.kappa,
        Goal::Lightkeeper => t.lightkeeper,
    }
}

fn tags(parts: &[(bool, &str)]) -> String {
    parts.iter().filter(|(on, _)| *on).map(|(_, t)| *t).collect::<Vec<_>>().join(" · ")
}

pub fn item_card(cat: &Catalog, item: &Item, score: f32, p: &Progress, o: &CardOptions) -> Card {
    let q = if o.quests { needs::quest_needs(cat, &item.id, p, o.show_non_fir) } else { Default::default() };
    let hideout = if o.hideout { needs::hideout_needs(cat, &item.id, p) } else { Vec::new() };
    let quest_total: u32 = q.tasks.iter().map(|t| t.count).sum();
    let hideout_total: u32 = hideout.iter().map(|h| h.remaining).sum();
    let total = quest_total + hideout_total;
    // KEEP counts only quests inside the goal.
    let goal_tasks = || q.tasks.iter().filter(|t| in_goal(t, o.goal));
    let current: u32 = goal_tasks().filter(|t| t.is_current()).map(|t| t.count).sum();
    let current_fir = goal_tasks().any(|t| t.is_current() && t.fir > 0);
    let buildable: u32 = hideout.iter().filter(|h| h.buildable).map(|h| h.remaining).sum();
    let kappa: u32 = q.tasks.iter().filter(|t| t.kappa).map(|t| t.count).sum();
    let keep_later: u32 = goal_tasks().map(|t| t.count).sum::<u32>() + hideout_total;
    let (barters, crafts) = needs::recipes_using(cat, &item.id);
    let best = item.best_trader();
    let slots = (item.width * item.height).max(1);

    let (verdict, why, color) = if current > 0 {
        let fir = if current_fir { "  FIR" } else { "" };
        (format!("KEEP {current}×{fir}"), "for a task you have now".to_owned(), AMBER)
    } else if buildable > 0 {
        (format!("KEEP {buildable}×"), "for a buildable upgrade".to_owned(), AMBER)
    } else if o.goal == Goal::None && kappa > 0 {
        (format!("KEEP {kappa}×"), "for Kappa".to_owned(), PURPLE)
    } else if keep_later > 0 {
        (format!("KEEP {keep_later}×"), "for later".to_owned(), LATER)
    } else if total > 0 {
        ("NOT NEEDED".to_owned(), format!("{total}× outside {}", o.goal.label()), GREY)
    } else {
        // Items without a flea price are judged by their best trader offer.
        let per_slot = item.price_per_slot().or(best.map(|b| b.price_rub / slots)).unwrap_or(0);
        let why = if per_slot > 0 { format!("{} ₽/slot", rub(per_slot)) } else { String::new() };
        if per_slot >= 30_000 {
            ("VALUABLE".into(), why, GREEN)
        } else if per_slot >= 10_000 {
            ("OK".into(), why, TEXT)
        } else {
            ("LOW".into(), why, GREY)
        }
    };

    let title = if score < 0.7 { format!("{} ?", item.short_name) } else { item.short_name.clone() };
    let needed = if total > 0 { format!("{total}× needed") } else { String::new() };
    let mut lines = vec![
        line(title, TITLE, TEXT).right(needed, MUTED),
        line(verdict, HEADLINE, color).style(Style::Band).right(why, color),
    ];

    // Groups: smart = NOW / HIDEOUT / LATER, plain = QUESTS / HIDEOUT; quests outside the goal last.
    let other = format!("NOT {}", o.goal.label().to_uppercase());
    let groups: [&str; 4] = if o.smart { ["NOW", "HIDEOUT", "LATER", &other] } else { ["QUESTS", "HIDEOUT", "", &other] };
    let mut rows: Vec<Row> = q
        .tasks
        .iter()
        .map(|t| {
            let levels_to_go = p.level.map_or(0, |l| t.min_level.saturating_sub(u32::from(l)));
            let goal = in_goal(t, o.goal);
            let group = if !goal {
                3
            } else if !o.smart || t.is_current() {
                0
            } else {
                2
            };
            let lvl = format!("Lv {}", t.min_level);
            Row {
                group,
                rank: 0,
                count: t.count,
                done: t.other_done.unwrap_or(0.0),
                levels_to_go,
                unlocks: t.unlocks,
                name: format!("{}×  {}", t.count, t.name),
                tag: tags(&[
                    (t.fir > 0, "FIR"),
                    (o.smart && levels_to_go > 0, &lvl),
                    (o.smart && t.other_done == Some(1.0), "last item"),
                ]),
                tag_color: if t.fir > 0 { AMBER } else { SECONDARY },
                color: if !goal {
                    MUTED
                } else if t.is_current() {
                    TEXT
                } else {
                    SECONDARY
                },
            }
        })
        .chain(hideout.iter().map(|h| Row {
            group: 1,
            rank: if o.smart && h.buildable { 0 } else { 1 },
            count: h.remaining,
            done: h.other_done.unwrap_or(0.0),
            levels_to_go: h.distance,
            unlocks: h.unlocks,
            name: format!("{}×  {} {}", h.remaining, h.station, h.level),
            tag: tags(&[(o.smart && h.buildable, "buildable"), (o.smart && h.other_done == Some(1.0), "last item")]),
            tag_color: GREEN,
            color: if h.buildable { TEXT } else { SECONDARY },
        }))
        .collect();
    if o.smart {
        rows.sort_by(|a, b| {
            a.group
                .cmp(&b.group)
                .then(a.rank.cmp(&b.rank))
                .then(b.done.total_cmp(&a.done))
                .then(a.levels_to_go.cmp(&b.levels_to_go))
                .then(b.unlocks.cmp(&a.unlocks))
        });
    } else {
        rows.sort_by(|a, b| a.group.cmp(&b.group).then(b.count.cmp(&a.count)));
    }
    let shown = o.max_rows as usize;
    let mut last_group = None;
    for r in rows.iter().take(shown) {
        if last_group != Some(r.group) {
            lines.push(line(groups[r.group], LABEL, MUTED).style(Style::Label));
            last_group = Some(r.group);
        }
        lines.push(line(r.name.clone(), ROW, r.color).right(r.tag.clone(), r.tag_color));
    }
    if let Some(l) = more(rows.iter().skip(shown).map(|r| r.count)) {
        lines.push(l);
    }

    let flea = item.avg24h.map(|avg| {
        let per = if slots > 1 { format!("  ·  {} ₽/slot", rub(avg / slots)) } else { String::new() };
        format!("Flea {} ₽{per}", rub(avg))
    });
    let trader = best.map(|b| {
        let name = cat.traders.get(&b.trader).cloned().unwrap_or_else(|| b.trader.clone());
        format!("{name} {} ₽", rub(b.price_rub))
    });
    let mut recipes = Vec::new();
    if o.barters && barters > 0 {
        recipes.push(format!("Barters {barters}"));
    }
    if o.crafts && crafts > 0 {
        recipes.push(format!("Crafts {crafts}"));
    }
    if flea.is_some() || trader.is_some() || !recipes.is_empty() {
        lines.push(line("", 1.0, RULE).style(Style::Rule));
    }
    match (flea, trader) {
        (Some(f), t) => lines.push(line(f, FOOT, TEXT).regular().right(t.unwrap_or_default(), TEXT)),
        (None, Some(t)) => lines.push(line(t, FOOT, TEXT).regular()),
        (None, None) => {}
    }
    if !recipes.is_empty() {
        lines.push(line(recipes.join(" · "), SMALL, MUTED).regular());
    }
    Card { border: color, lines }
}

/// `+n× in k more` for rows that did not fit, so the rows add up to the total.
fn more(rest: impl Iterator<Item = u32>) -> Option<Line> {
    let (n, k) = rest.fold((0, 0), |(n, k), c| (n + c, k + 1));
    (k > 0).then(|| line(format!("+{n}× in {k} more"), SMALL, MUTED).regular())
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

/// Bold for headings and rows, regular for prices and small print.
#[derive(Clone)]
pub struct Fonts {
    pub bold: FontArc,
    pub regular: FontArc,
}

/// Full card at `scale`: fixed width, height from the lines. Premultiplied RGBA.
pub fn render(card: &Card, fonts: &Fonts, scale: f32) -> Pixmap {
    let s = scale.clamp(0.75, 2.0);
    let width = (WIDTH * s).round();
    let left = (PAD + BORDER) * s;
    let inner = width - 2.0 * left;
    let band_pad = 5.0 * s;

    struct Laid {
        text: String,
        right: Option<(String, Rgb, f32)>,
        size: f32,
        font: FontArc,
        color: Rgb,
        style: Style,
        top: f32,
        height: f32,
    }
    let mut laid = Vec::new();
    let mut y = left;
    for l in &card.lines {
        let font = if l.bold { fonts.bold.clone() } else { fonts.regular.clone() };
        let size = l.size * s;
        let (top_extra, height) = match l.style {
            Style::Rule => (2.0 * s, 1.0 * s + 4.0 * s),
            Style::Label => (4.0 * s, font.as_scaled(PxScale::from(size)).height()),
            Style::Band => (2.0 * s, font.as_scaled(PxScale::from(size)).height() + 2.0 * band_pad),
            Style::Plain => (0.0, font.as_scaled(PxScale::from(size)).height()),
        };
        // Right text is drawn in regular weight at most at the line's size.
        let right = l.right.as_ref().map(|(t, c)| {
            let rsize = size.min(SMALL * s + 2.0 * s);
            let t = displayable(&fonts.regular, t);
            let w = text_width(&fonts.regular, rsize, &t);
            (t, *c, rsize, w)
        });
        let avail = inner - right.as_ref().map_or(0.0, |r| r.3 + 12.0 * s) - if l.style == Style::Band { 2.0 * band_pad } else { 0.0 };
        let text = if l.style == Style::Rule { String::new() } else { fit(&font, size, displayable(&font, &l.text), avail) };
        y += top_extra;
        laid.push(Laid { text, right: right.map(|(t, c, rs, _)| (t, c, rs)), size, font, color: l.color, style: l.style, top: y, height });
        y += height + GAP * s;
    }
    let height = (y - GAP * s + left).ceil();

    let mut pix = Pixmap::new(width as u32, height.max(1.0) as u32).expect("card size");
    let mut paint = Paint::default();
    let border = card.border;
    paint.set_color(Color::from_rgba8(border.0, border.1, border.2, 255));
    pix.fill_rect(Rect::from_xywh(0.0, 0.0, width, height).unwrap(), &paint, Transform::identity(), None);
    paint.set_color(Color::from_rgba8(0x10, 0x10, 0x10, 235));
    let b = BORDER * s;
    // Clear the inside first so the background alpha is not composited over the border colour.
    pix.fill_rect(Rect::from_xywh(b, b, width - 2.0 * b, height - 2.0 * b).unwrap(), &Paint { blend_mode: tiny_skia::BlendMode::Source, ..paint.clone() }, Transform::identity(), None);

    for l in &laid {
        match l.style {
            Style::Rule => {
                paint.set_color(Color::from_rgba8(RULE.0, RULE.1, RULE.2, 255));
                let r = Rect::from_xywh(left, l.top + 2.0 * s, inner, 1.0 * s).unwrap();
                pix.fill_rect(r, &paint, Transform::identity(), None);
                continue;
            }
            Style::Band => {
                paint.set_color(Color::from_rgba8(l.color.0, l.color.1, l.color.2, 46));
                let r = Rect::from_xywh(left - 4.0 * s, l.top, inner + 8.0 * s, l.height).unwrap();
                pix.fill_rect(r, &paint, Transform::identity(), None);
            }
            Style::Label | Style::Plain => {}
        }
        let pad = if l.style == Style::Band { band_pad } else { 0.0 };
        let ascent = l.font.as_scaled(PxScale::from(l.size)).ascent();
        let baseline = l.top + pad + ascent;
        draw_text(&mut pix, &l.font, l.size, &l.text, left + pad, baseline, l.color);
        if let Some((t, c, rs)) = &l.right {
            let w = text_width(&fonts.regular, *rs, t);
            draw_text(&mut pix, &fonts.regular, *rs, t, left + inner - pad - w, baseline, *c);
        }
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
        CardOptions { show_non_fir: true, max_rows: 10, quests: true, hideout: true, barters: true, crafts: true, smart: true, goal: Goal::None };

    /// Lines as `text | right`; separators omitted.
    fn show(c: &Card) -> Vec<String> {
        c.lines
            .iter()
            .filter(|l| l.style != Style::Rule)
            .map(|l| match &l.right {
                Some((r, _)) => format!("{} | {r}", l.text),
                None => l.text.clone(),
            })
            .collect()
    }

    fn ordering_catalog() -> (Catalog, Item) {
        use crate::data::{HideoutRequirement, Objective, Station, StationLevel, Task};
        let mut cat = Catalog::default();
        let item = Item { id: "i".into(), short_name: "Shus".into(), width: 1, height: 1, ..Default::default() };
        let give = |id: &str, n, fir| Objective {
            id: id.into(),
            kind: "giveItem".into(),
            count: n,
            found_in_raid: fir,
            items: vec!["i".into()],
            ..Default::default()
        };
        let mut kappa = Task { id: "k".into(), name: "Kappa task".into(), objectives: vec![give("ok", 1, false)], ..Default::default() };
        kappa.kappa_required = true;
        kappa.min_player_level = 30;
        cat.tasks.insert("k".into(), kappa);
        cat.tasks.insert("s".into(), Task { id: "s".into(), name: "Side task".into(), objectives: vec![give("os", 4, true)], ..Default::default() });
        let req = |id: &str, item: &str, count| HideoutRequirement { id: id.into(), item: item.into(), count, fir: false };
        let lvl = |id: &str, level, reqs| StationLevel { id: id.into(), level, item_requirements: reqs, ..Default::default() };
        cat.stations.push(Station {
            id: "a".into(),
            name: "Stash".into(),
            levels: vec![lvl("a-1", 1, vec![]), lvl("a-2", 2, vec![req("a2", "i", 8), req("a2x", "x", 5)])],
        });
        cat.stations.push(Station {
            id: "b".into(),
            name: "Rack".into(),
            levels: vec![lvl("b-1", 1, vec![req("b1", "i", 2)]), lvl("b-2", 2, vec![req("b2", "i", 5), req("b2x", "x", 5)])],
        });
        (cat, item)
    }

    #[test]
    fn price_verdict_without_needs() {
        let cat = Catalog::default();
        let item = Item { id: "i".into(), short_name: "PS".into(), width: 1, height: 1, avg24h: Some(50_000), ..Default::default() };
        let c = item_card(&cat, &item, 0.6, &Progress::default(), &OPTS);
        assert_eq!(show(&c)[..2], ["PS ?", "VALUABLE | 50\u{2009}000 ₽/slot"]);
        assert_eq!(c.border, GREEN);
    }

    #[test]
    fn smart_groups_now_hideout_later_with_reason_tags() {
        let (cat, item) = ordering_catalog();
        let mut p = Progress { level: Some(16), ..Default::default() };
        // Rack 2's other part is handed in → Shus is its last missing piece.
        p.hideout_parts.insert("b2x".into(), crate::progress::PartProgress { complete: true, count: 5 });
        let c = item_card(&cat, &item, 0.9, &p, &OPTS);
        assert_eq!(
            show(&c),
            [
                "Shus | 20× needed",
                "KEEP 4×  FIR | for a task you have now",
                "NOW",
                "4×  Side task | FIR",
                "HIDEOUT",
                "2×  Rack 1 | buildable",
                "5×  Rack 2 | last item",
                "8×  Stash 2",
                "LATER",
                "1×  Kappa task | Lv 30",
            ]
        );
        assert_eq!(c.border, AMBER);
    }

    #[test]
    fn plain_ordering_and_row_limit_add_up() {
        let (cat, item) = ordering_catalog();
        let p = Progress { level: Some(16), ..Default::default() };
        let o = CardOptions { smart: false, max_rows: 3, ..OPTS };
        assert_eq!(
            show(&item_card(&cat, &item, 0.9, &p, &o))[2..],
            ["QUESTS", "4×  Side task | FIR", "1×  Kappa task", "HIDEOUT", "8×  Stash 2", "+7× in 2 more"]
        );
    }

    #[test]
    fn marked_counts_lower_the_total() {
        let (cat, item) = ordering_catalog();
        let mut p = Progress { level: Some(16), ..Default::default() };
        p.hideout_parts.insert("a2".into(), crate::progress::PartProgress { complete: false, count: 1 });
        assert_eq!(show(&item_card(&cat, &item, 0.9, &p, &OPTS))[0], "Shus | 19× needed");
        let no_hideout = CardOptions { hideout: false, ..OPTS };
        assert_eq!(show(&item_card(&cat, &item, 0.9, &p, &no_hideout))[0], "Shus | 5× needed");
    }

    #[test]
    fn goal_drops_other_quests_from_keep_and_sinks_them() {
        let (cat, item) = ordering_catalog();
        let p = Progress { level: Some(40), ..Default::default() };
        let o = CardOptions { goal: Goal::Kappa, hideout: false, ..OPTS };
        assert_eq!(
            show(&item_card(&cat, &item, 0.9, &p, &o))[1..],
            ["KEEP 1× | for a task you have now", "NOW", "1×  Kappa task", "NOT KAPPA", "4×  Side task | FIR"]
        );
    }
}
