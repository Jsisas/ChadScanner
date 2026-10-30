//! The floating card: content from catalog + progress, rendered into a premultiplied pixmap.
use crate::data::{Catalog, Item};
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

pub fn item_card(cat: &Catalog, item: &Item, score: f32, p: &Progress, show_non_fir: bool) -> Card {
    let q = needs::quest_needs(cat, &item.id, p, show_non_fir);
    let mut hideout = needs::hideout_needs(cat, &item.id, p);
    // Nearest upgrades first.
    hideout.sort_by(|a, b| a.level.cmp(&b.level).then(b.remaining.cmp(&a.remaining)));
    let hideout_total: u32 = hideout.iter().map(|h| h.remaining).sum();
    let (barters, crafts) = needs::recipes_using(cat, &item.id);
    let best = item.best_trader();

    let title = if score < 0.7 { format!("{} ?", item.short_name) } else { item.short_name.clone() };
    let (verdict, color) = if q.current > 0 {
        let fir = if q.current_fir > 0 { " (FIR)" } else { "" };
        (format!("KEEP – QUEST {}×{fir}", q.current), AMBER)
    } else if hideout_total > 0 {
        (format!("KEEP – HIDEOUT {hideout_total}×"), AMBER)
    } else if q.kappa > 0 {
        (format!("KEEP – KAPPA {}×", q.kappa), PURPLE)
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
    if let (Some(avg), Some(per)) = (item.avg24h, item.price_per_slot()) {
        lines.push(line(format!("Flea {} ₽  ·  {} ₽/slot", rub(avg), rub(per)), VALUE, TEXT));
    }
    if let Some(b) = best {
        let name = cat.traders.get(&b.trader).cloned().unwrap_or_else(|| b.trader.clone());
        lines.push(line(format!("{name} {} ₽", rub(b.price_rub)), VALUE, TEXT));
    }
    for t in q.tasks.iter().take(3) {
        let color = if t.is_current() { TEXT } else { SECONDARY };
        lines.push(line(format!("{}× {}", t.count, t.name), DETAIL, color));
    }
    if let Some(l) = more(q.tasks.iter().skip(3).map(|t| t.count), "tasks") {
        lines.push(l);
    }
    for h in hideout.iter().take(3) {
        lines.push(line(format!("{}× {} Lv{}", h.remaining, h.station, h.level), DETAIL, TEXT));
    }
    if let Some(l) = more(hideout.iter().skip(3).map(|h| h.remaining), "upgrades") {
        lines.push(l);
    }
    if barters + crafts > 0 {
        lines.push(line(format!("Barters: {barters} · Crafts: {crafts}"), DETAIL, SECONDARY));
    }
    Card { border: color, lines }
}

/// `+n× in k more <what>` for list entries that did not fit, so the lines add up to the verdict total.
fn more(rest: impl Iterator<Item = u32>, what: &str) -> Option<Line> {
    let (n, k) = rest.fold((0, 0), |(n, k), c| (n + c, k + 1));
    (k > 0).then(|| line(format!("+{n}× in {k} more {what}"), DETAIL, SECONDARY))
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

    #[test]
    fn verdict_precedence_quest_over_price() {
        let mut cat = Catalog::default();
        let item = Item { id: "i".into(), short_name: "PS".into(), width: 1, height: 1, avg24h: Some(50_000), ..Default::default() };
        cat.items.insert("i".into(), item.clone());
        assert_eq!(item_card(&cat, &item, 0.9, &Progress::default(), true).lines[1].text, "VALUABLE");
        let o = crate::data::Objective { id: "o".into(), kind: "giveItem".into(), count: 2, found_in_raid: true, items: vec!["i".into()], ..Default::default() };
        cat.tasks.insert("t".into(), crate::data::Task { id: "t".into(), name: "Task".into(), objectives: vec![o], ..Default::default() });
        let card = item_card(&cat, &item, 0.6, &Progress::default(), true);
        assert_eq!(card.lines[0].text, "PS ?");
        assert_eq!(card.lines[1].text, "KEEP – QUEST 2× (FIR)");
        assert_eq!(card.border, AMBER);
    }
}
