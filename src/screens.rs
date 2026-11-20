//! Reading EFT's Traders and Hideout menus from screenshots: which tab is open, each unlocked trader's loyalty
//! level (roman-numeral badge, crown = 4), the player's level and the level of the hideout station whose detail panel is open.
//!
//! EFT draws its menu in a centred 16:9 box, so every position here is in 1080p reference units inside that box
//! and `Frame` maps it to monitor pixels. Region captures are cropped with `Frame::rect`; the parsers take the
//! capture plus `scale` (= monitor height / 1080 for 16:9 and wider).

use crate::exfil::is_text;
use image::{GrayImage, Rgb, RgbImage};

/// A rectangle in 1080p reference units.
#[derive(Clone, Copy)]
pub struct Ref {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Bottom menu bar; the active tab is a light box at rows 1051-1077.
pub const NAV: Ref = Ref { x0: 0.0, y0: 1048.0, x1: 1920.0, y1: 1080.0 };
/// Trader portrait row.
pub const TRADERS: Ref = Ref { x0: 0.0, y0: 55.0, x1: 1140.0, y1: 205.0 };
/// The player's level badge left of the portrait, top right of a trader's screen.
pub const PLAYER: Ref = Ref { x0: 1735.0, y0: 48.0, x1: 1783.0, y1: 94.0 };
/// Where a hideout station's detail panel can be (its top moves with the panel's height).
pub const DETAIL: Ref = Ref { x0: 990.0, y0: 80.0, x1: 1910.0, y1: 800.0 };

/// Minimum `lexicon::score` for OCR text to count as a known word.
pub const MATCH: f32 = 0.8;

/// Where EFT's 16:9 menu box sits on a monitor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub ox: f32,
    pub oy: f32,
    pub scale: f32,
}

impl Frame {
    pub fn for_monitor(w: u32, h: u32) -> Frame {
        let (w, h) = (w as f32, h as f32);
        if w * 9.0 >= h * 16.0 {
            let scale = h / 1080.0;
            Frame { ox: (w - 1920.0 * scale) / 2.0, oy: 0.0, scale }
        } else {
            // Taller than 16:9 (unverified: no sample); the box is letterboxed top and bottom.
            let scale = w / 1920.0;
            Frame { ox: 0.0, oy: (h - 1080.0 * scale) / 2.0, scale }
        }
    }

    /// `(x, y, w, h)` in monitor pixels, relative to the monitor's top-left.
    pub fn rect(&self, r: Ref) -> (i32, i32, i32, i32) {
        let x0 = (self.ox + r.x0 * self.scale).floor() as i32;
        let y0 = (self.oy + r.y0 * self.scale).floor() as i32;
        let x1 = (self.ox + r.x1 * self.scale).ceil() as i32;
        let y1 = (self.oy + r.y1 * self.scale).ceil() as i32;
        (x0, y0, x1 - x0, y1 - y0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Traders,
    Hideout,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraderRead {
    pub name: String,
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StationRead {
    pub name: String,
    pub level: u8,
}

fn luma(p: &Rgb<u8>) -> u32 {
    (299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000
}

fn is_light(p: &Rgb<u8>) -> bool {
    let sat = p[0].max(p[1]).max(p[2]) as i32 - p[0].min(p[1]).min(p[2]) as i32;
    luma(p) >= 120 && sat <= 35
}

/// Luma of `img[x0..x1, y0..y1]`, clamped to the image; `None` when nothing is left.
fn luma_crop(img: &RgbImage, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<GrayImage> {
    let (w, h) = img.dimensions();
    let (x0, y0) = (x0.max(0) as u32, y0.max(0) as u32);
    let (x1, y1) = ((x1.max(0) as u32).min(w), (y1.max(0) as u32).min(h));
    (x1 > x0 && y1 > y0).then(|| GrayImage::from_fn(x1 - x0, y1 - y0, |x, y| image::Luma([luma(img.get_pixel(x0 + x, y0 + y)) as u8])))
}

fn rgb_crop(img: &RgbImage, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<RgbImage> {
    let (w, h) = img.dimensions();
    let (x0, y0) = (x0.max(0) as u32, y0.max(0) as u32);
    let (x1, y1) = ((x1.max(0) as u32).min(w), (y1.max(0) as u32).min(h));
    (x1 > x0 && y1 > y0).then(|| image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image())
}

fn median(v: &[u8]) -> u8 {
    let mut s = v.to_vec();
    s.sort_unstable();
    s.get(s.len() / 2).copied().unwrap_or(0)
}

/// Pixel x-range `(start, end)` (end exclusive) of the active, light-boxed tab in a `NAV` capture.
pub fn active_tab(nav: &RgbImage, scale: f32) -> Option<(u32, u32)> {
    let (w, h) = nav.dimensions();
    let y = ((6.0 * scale).round() as u32).min(h.checked_sub(1)?);
    let min_run = (60.0 * scale).round() as u32;
    let mut x = 0;
    while x < w {
        if !is_light(nav.get_pixel(x, y)) {
            x += 1;
            continue;
        }
        let start = x;
        while x < w && is_light(nav.get_pixel(x, y)) {
            x += 1;
        }
        if x - start >= min_run {
            return Some((start, x));
        }
    }
    None
}

/// The label of the tab spanning `x0..x1`, ready for OCR.
pub fn tab_crop(nav: &RgbImage, scale: f32, (x0, x1): (u32, u32)) -> Option<RgbImage> {
    let px = |v: f32| (v * scale).round() as i32;
    rgb_crop(nav, x0 as i32, px(3.0), x1 as i32, px(29.0))
}

/// The screen named by a tab label's OCR text.
pub fn screen_of(text: &str) -> Option<Screen> {
    let t = chadvision::lexicon::score(text, "TRADERS");
    let h = chadvision::lexicon::score(text, "HIDEOUT");
    match (t >= MATCH, h >= MATCH) {
        (true, false) => Some(Screen::Traders),
        (false, true) => Some(Screen::Hideout),
        (true, true) => Some(if t >= h { Screen::Traders } else { Screen::Hideout }),
        _ => None,
    }
}

/// Loyalty level from a trader badge's luma square: roman numeral strokes I/II/III, or the crown = 4.
fn badge_level(g: &GrayImage) -> Option<u8> {
    let (w, h) = g.dimensions();
    if w < 8 || h < 8 {
        return None;
    }
    let med = median(g.as_raw()) as f32;
    if med < 150.0 {
        return None;
    }
    let (x0, x1) = (2, w - 2);
    let dark = |x: u32, y: u32| (g.get_pixel(x, y)[0] as f32) < 0.6 * med;
    let frac = g.pixels().filter(|p| (p[0] as f32) < 0.6 * med).count() as f32 / (w * h) as f32;
    let (y0, y1) = ((0.4 * h as f32) as u32, (0.8 * h as f32) as u32);
    let wide_run = (y0..y1).any(|y| {
        let (mut best, mut run) = (0, 0);
        for x in x0..x1 {
            run = if dark(x, y) { run + 1 } else { 0 };
            best = best.max(run);
        }
        best as f32 >= 0.5 * (w - 4) as f32
    });
    if frac >= 0.19 && wide_run {
        return Some(4);
    }
    let (y0, y1) = ((0.25 * h as f32) as u32, (0.65 * h as f32) as u32);
    let col_dark = |x: u32| {
        let sum: u32 = (y0..y1).map(|y| g.get_pixel(x, y)[0] as u32).sum();
        (sum as f32 / (y1 - y0) as f32) < 0.75 * med
    };
    let (mut strokes, mut inside) = (0, false);
    for x in x0..x1 {
        let d = col_dark(x);
        if d && !inside {
            strokes += 1;
        }
        inside = d;
    }
    (1..=3).contains(&strokes).then_some(strokes as u8)
}

/// Player level from a `PLAYER` capture: a light badge (else `Err`, no OCR) holding a 1-79 number.
pub fn player_level(img: &RgbImage, ocr: impl Fn(&RgbImage) -> String) -> Result<u8, String> {
    let g = luma_crop(img, 0, 0, img.width() as i32, img.height() as i32).ok_or("empty capture")?;
    let m = median(g.as_raw());
    if m < 150 {
        return Err(format!("median {m} < 150"));
    }
    // The badge border and bevel throw the line recogniser off (empty or "D"); read only the digits' face.
    const INSET: u32 = 12;
    let face = image::imageops::crop_imm(img, INSET, INSET, img.width() - 2 * INSET, img.height() - 2 * INSET).to_image();
    let t = ocr(&face);
    let digits: String = t.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 2 {
        return Err(format!("OCR {t:?}"));
    }
    let l: u8 = digits.parse().map_err(|_| format!("OCR {t:?}"))?;
    if !(1..=79).contains(&l) {
        return Err(format!("level {l} out of range"));
    }
    Ok(l)
}

/// Loyalty levels of the unlocked traders in a `TRADERS` capture, left to right.
pub fn traders(img: &RgbImage, scale: f32, ocr: impl Fn(&RgbImage) -> String) -> Vec<TraderRead> {
    let px = |v: f32| (v * scale).round() as i32;
    let mut out = Vec::new();
    for i in 0..9 {
        let sx = 12.0 + 125.0 * i as f32;
        // A locked portrait shows only a centred padlock; an unlocked one shows the standing icon and number.
        let Some(strip) = luma_crop(img, px(sx + 5.0), px(121.0), px(sx + 115.0), px(145.0)) else { continue };
        let med = median(strip.as_raw()) as i32;
        let Some(core) = luma_crop(img, px(sx + 20.0), px(121.0), px(sx + 48.0), px(145.0)) else { continue };
        let ink = core.pixels().filter(|p| (p[0] as i32 - med).abs() > 60).count() as f32;
        if ink < 25.0 * scale * scale {
            continue;
        }
        let Some(badge) = luma_crop(img, px(sx + 3.0), px(5.0), px(sx + 24.0), px(25.0)) else { continue };
        let Some(level) = badge_level(&badge) else { continue };
        let Some(name) = rgb_crop(img, px(sx + 4.0), px(91.0), px(sx + 118.0), px(113.0)) else { continue };
        let name = ocr(&name).trim().to_owned();
        if !name.is_empty() {
            out.push(TraderRead { name, level });
        }
    }
    out
}

/// Runs `(start, end)` (end exclusive) of consecutive `true`s.
fn runs(flags: impl Iterator<Item = bool>) -> Vec<(u32, u32)> {
    let (mut out, mut start) = (Vec::new(), None);
    let mut n = 0;
    for (i, f) in flags.enumerate() {
        n = i as u32 + 1;
        match (f, start) {
            (true, None) => start = Some(i as u32),
            (false, Some(s)) => {
                out.push((s, i as u32));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, n));
    }
    out
}

/// The station named in an open detail panel (`DETAIL` capture) and its level: `Locked` = 0, otherwise the
/// two-digit number on the icon. The panel's top moves with its height, so its red close button anchors it.
pub fn detail(img: &RgbImage, scale: f32, ocr: impl Fn(&RgbImage) -> String) -> Option<StationRead> {
    let px = |v: f32| (v * scale).round() as i32;
    let (w, h) = img.dimensions();
    let is_red = |p: &Rgb<u8>| p[0] >= 55 && p[0] as i32 - p[1] as i32 >= 40 && p[0] as i32 - p[2] as i32 >= 40;
    let (cx0, cx1) = (px(865.0) as u32, (px(910.0) as u32).min(w));
    let yc = (0..h).find(|&y| (cx0..cx1).filter(|&x| is_red(img.get_pixel(x, y))).count() >= px(12.0) as usize)?;

    let (tx0, tx1) = (px(60.0) as u32, (px(710.0) as u32).min(w));
    let rows = (yc..(yc + px(90.0) as u32).min(h))
        .map(|y| (tx0..tx1).filter(|&x| is_text(img.get_pixel(x, y))).count() >= 2);
    let lines: Vec<(u32, u32)> = runs(rows)
        .into_iter()
        .filter(|(a, b)| (px(8.0) as u32..=px(30.0) as u32).contains(&(b - a)))
        .map(|(a, b)| (a + yc, b + yc))
        .take(2)
        .collect();
    let read = |&(a, b): &(u32, u32)| {
        let cols: Vec<u32> = (tx0..tx1).filter(|&x| (a..b).any(|y| is_text(img.get_pixel(x, y)))).collect();
        let (c0, c1) = (*cols.first()?, *cols.last()? + 1);
        rgb_crop(img, c0 as i32 - px(4.0), a as i32 - px(3.0), c1 as i32 + px(4.0), b as i32 + px(3.0)).map(|c| ocr(&c))
    };
    let name = read(lines.first()?)?.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let level = if lines.get(1).and_then(read).is_some_and(|t| chadvision::lexicon::score(&t, "Locked") >= MATCH) {
        0
    } else {
        let d = rgb_crop(img, px(18.0), yc as i32 + px(66.0), px(54.0), yc as i32 + px(88.0))?;
        let digits = ocr(&d);
        let digits = digits.trim();
        match digits.parse::<u8>() {
            Ok(n) if digits.len() == 2 && (1..=9).contains(&n) => n,
            _ => return None,
        }
    };
    Some(StationRead { name, level })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALE: f32 = 1439.0 / 1080.0;

    fn fixture(name: &str) -> RgbImage {
        image::open(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap().to_rgb8()
    }

    fn engine() -> Option<chadvision::engine::Engine> {
        let data = std::path::Path::new("../ChadVision/data");
        if !data.join("models/ppocrv5_latin_rec.onnx").exists() {
            eprintln!("skipped: no ../ChadVision/data");
            return None;
        }
        Some(chadvision::engine::Engine::open(data).unwrap())
    }

    #[test]
    fn frame_centres_the_16_9_box() {
        let f = Frame::for_monitor(3440, 1440);
        assert_eq!(f.ox, 440.0);
        assert_eq!(f.oy, 0.0);
        assert_eq!(f.scale, 1440.0 / 1080.0);
        assert_eq!(f.rect(NAV), (440, 1397, 2560, 43));
        let tall = Frame::for_monitor(1920, 1200);
        assert_eq!((tall.ox, tall.oy, tall.scale), (0.0, 60.0, 1.0));
    }

    #[test]
    fn finds_the_active_tab() {
        let t = active_tab(&fixture("screens-traders-nav-1440p.png"), SCALE).unwrap();
        let h = active_tab(&fixture("screens-hideout-nav-1440p.png"), SCALE).unwrap();
        eprintln!("tabs: {t:?} {h:?}");
        assert!(t.0.abs_diff(1238) <= 8, "{t:?}");
        assert!(t.1.abs_diff(1392) <= 8, "{t:?}");
        assert!(h.0.abs_diff(237) <= 8, "{h:?}");
        assert!(h.1.abs_diff(362) <= 8, "{h:?}");
    }

    #[test]
    fn reads_trader_badges() {
        let levels = |f: &str| traders(&fixture(f), SCALE, |_| "x".into()).iter().map(|t| t.level).collect::<Vec<_>>();
        assert_eq!(levels("screens-traders-locked-1440p.png"), [1, 1, 1]);
        assert_eq!(levels("screens-traders-ll2-1440p.png"), [2, 2, 1, 2, 2, 2, 2, 2, 1]);
    }

    #[test]
    fn crown_is_level_four() {
        let crown = fixture("screens-badge-crown.png");
        let g = luma_crop(&crown, 0, 0, crown.width() as i32, crown.height() as i32).unwrap();
        assert_eq!(badge_level(&g), Some(4));
    }

    #[test]
    fn names_the_open_screen() {
        let Some(engine) = engine() else { return };
        for (f, want) in [("screens-traders-nav-1440p.png", Screen::Traders), ("screens-hideout-nav-1440p.png", Screen::Hideout)] {
            let nav = fixture(f);
            let tab = tab_crop(&nav, SCALE, active_tab(&nav, SCALE).unwrap()).unwrap();
            assert_eq!(screen_of(&engine.read_line(&tab)), Some(want));
        }
    }

    #[test]
    fn no_player_level_without_a_badge() {
        assert!(player_level(&RgbImage::new(64, 61), |_| panic!("OCR on a dark capture")).is_err());
    }

    #[test]
    fn player_level_ignores_ocr_noise() {
        let img = RgbImage::from_pixel(65, 63, image::Rgb([200, 200, 200]));
        assert_eq!(player_level(&img, |_| "4.".into()), Ok(4));
        assert_eq!(player_level(&img, |_| "I 26 |".into()), Ok(26));
        assert!(player_level(&img, |_| "abc".into()).is_err());
    }

    #[test]
    fn reads_the_player_level() {
        let Some(engine) = engine() else { return };
        for (f, want) in [("screens-player-26-1440p.png", 26), ("screens-player-4-1440p.png", 4), ("screens-player-live-1440p.png", 7)] {
            assert_eq!(player_level(&fixture(f), |i| engine.read_line(i)), Ok(want), "{f}");
        }
    }

    #[test]
    fn reads_trader_names() {
        let Some(engine) = engine() else { return };
        let reads = traders(&fixture("screens-traders-ll2-1440p.png"), SCALE, |l| engine.read_line(l));
        let want = ["Prapor", "Therapist", "Fence", "Skier", "Peacekeeper", "Mechanic", "Ragman", "Jaeger", "Ref"];
        assert_eq!(reads.len(), want.len(), "{reads:?}");
        for (r, w) in reads.iter().zip(want) {
            assert!(chadvision::lexicon::score(&r.name, w) >= MATCH, "{} vs {w}", r.name);
        }
    }

    const DETAIL_SCALE: f32 = 1440.0 / 1080.0;

    #[test]
    fn no_detail_panel_without_a_close_button() {
        let blank = RgbImage::new(1226, 960);
        assert_eq!(detail(&blank, DETAIL_SCALE, |_| panic!("OCR on a panel-less capture")), None);
    }

    #[test]
    fn reads_the_open_detail_panel() {
        let Some(engine) = engine() else { return };
        for (f, want) in [
            ("screens-detail-vents-1440p.png", Some(("Vents", 2))),
            ("screens-detail-gym-1440p.png", Some(("Gym", 0))),
            ("screens-detail-stash-1440p.png", Some(("Stash", 4))),
            ("screens-detail-intel-1440p.png", Some(("Intelligence Center", 1))),
            ("screens-detail-cultist-1440p.png", None),
        ] {
            let got = detail(&fixture(f), DETAIL_SCALE, |l| engine.read_line(l));
            match (got, want) {
                (Some(r), Some((n, l))) => {
                    assert!(chadvision::lexicon::score(&r.name, n) >= MATCH, "{f}: {}", r.name);
                    assert_eq!(r.level, l, "{f}");
                }
                (got, want) => assert_eq!(got.is_some(), want.is_some(), "{f}: {got:?}"),
            }
        }
    }

    fn gray(w: u32, h: u32, v: u8) -> GrayImage {
        GrayImage::from_pixel(w, h, image::Luma([v]))
    }

    fn fill_gray(g: &mut GrayImage, x0: u32, x1: u32, y0: u32, y1: u32, v: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                g.put_pixel(x, y, image::Luma([v]));
            }
        }
    }

    fn fill(img: &mut RgbImage, x0: u32, x1: u32, y0: u32, y1: u32, c: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Rgb(c));
            }
        }
    }

    #[test]
    fn crops_clamp_to_the_image_and_report_empty() {
        let img = RgbImage::from_pixel(10, 8, Rgb([10, 20, 30]));
        assert_eq!(luma_crop(&img, -5, -5, 4, 3).unwrap().dimensions(), (4, 3));
        assert_eq!(rgb_crop(&img, 6, 5, 50, 50).unwrap().dimensions(), (4, 3));
        for (x0, y0, x1, y1) in [(10, 0, 20, 8), (0, 8, 10, 20), (-9, 0, -1, 8), (3, 3, 3, 6), (3, 3, 6, 3)] {
            assert!(luma_crop(&img, x0, y0, x1, y1).is_none(), "luma {x0},{y0},{x1},{y1}");
            assert!(rgb_crop(&img, x0, y0, x1, y1).is_none(), "rgb {x0},{y0},{x1},{y1}");
        }
    }

    #[test]
    fn median_of_nothing_is_zero() {
        assert_eq!(median(&[]), 0);
        assert_eq!(median(&[9, 1, 5]), 5);
    }

    #[test]
    fn runs_close_at_the_end_of_input() {
        assert_eq!(runs(Vec::<bool>::new().into_iter()), vec![]);
        assert_eq!(runs([false, true, true, false, true].into_iter()), vec![(1, 3), (4, 5)]);
        assert_eq!(runs([true, true].into_iter()), vec![(0, 2)]);
    }

    #[test]
    fn active_tab_skips_short_light_runs() {
        let light = [200, 200, 200];
        let mut nav = RgbImage::new(300, 40);
        assert_eq!(active_tab(&nav, 1.0), None);
        fill(&mut nav, 10, 30, 6, 7, light); // 20 px: too short for a tab
        assert_eq!(active_tab(&nav, 1.0), None);
        fill(&mut nav, 100, 200, 6, 7, light);
        assert_eq!(active_tab(&nav, 1.0), Some((100, 200)));
        // A tab touching the right edge ends at the image width.
        let mut edge = RgbImage::new(300, 40);
        fill(&mut edge, 240, 300, 6, 7, light);
        assert_eq!(active_tab(&edge, 1.0), Some((240, 300)));
        // A short run at the edge alone is no tab.
        let mut short = RgbImage::new(300, 40);
        fill(&mut short, 250, 300, 6, 7, light);
        assert_eq!(active_tab(&short, 1.0), None);
        assert_eq!(active_tab(&RgbImage::new(300, 0), 1.0), None);
    }

    #[test]
    fn tab_crop_needs_rows_inside_the_capture() {
        assert!(tab_crop(&RgbImage::new(10, 2), 1.0, (0, 5)).is_none());
        assert_eq!(tab_crop(&RgbImage::new(100, 40), 1.0, (10, 50)).unwrap().dimensions(), (40, 26));
    }

    #[test]
    fn screen_of_matches_labels_and_rejects_noise() {
        assert_eq!(screen_of("TRADERS"), Some(Screen::Traders));
        assert_eq!(screen_of("Traders"), Some(Screen::Traders));
        assert_eq!(screen_of("HIDEOUT"), Some(Screen::Hideout));
        assert_eq!(screen_of("H1DEOUT"), Some(Screen::Hideout));
        assert_eq!(screen_of("CHARACTER"), None);
        assert_eq!(screen_of(""), None);
    }

    #[test]
    fn badge_level_counts_numeral_strokes() {
        // Too small to hold a badge.
        assert_eq!(badge_level(&gray(7, 20, 200)), None);
        assert_eq!(badge_level(&gray(20, 7, 200)), None);
        // Dark badge face (median below 150).
        assert_eq!(badge_level(&gray(21, 20, 100)), None);
        // Blank light badge: no strokes.
        assert_eq!(badge_level(&gray(21, 20, 200)), None);
        for (cols, want) in [(vec![9], 1), (vec![6, 11], 2), (vec![4, 9, 14], 3)] {
            let mut g = gray(21, 20, 200);
            for c in &cols {
                fill_gray(&mut g, *c, c + 2, 0, 20, 20);
            }
            assert_eq!(badge_level(&g), Some(want), "{cols:?}");
        }
        // Four strokes is not a numeral.
        let mut g = gray(21, 20, 200);
        for c in [3, 7, 11, 15] {
            fill_gray(&mut g, c, c + 2, 0, 20, 20);
        }
        assert_eq!(badge_level(&g), None);
        // A broad dark band (the crown's base) is level 4.
        let mut crown = gray(21, 20, 200);
        fill_gray(&mut crown, 0, 21, 8, 16, 20);
        assert_eq!(badge_level(&crown), Some(4));
        // A thin dark line is wide but too little ink to be a crown, and no vertical stroke either.
        let mut line = gray(21, 20, 200);
        fill_gray(&mut line, 2, 19, 10, 11, 20);
        assert_eq!(badge_level(&line), None);
    }

    #[test]
    fn player_level_rejects_bad_captures_and_numbers() {
        let img = RgbImage::from_pixel(65, 63, Rgb([200, 200, 200]));
        assert_eq!(player_level(&RgbImage::new(0, 0), |_| panic!("OCR on empty")), Err("empty capture".into()));
        assert!(player_level(&img, |_| String::new()).is_err());
        assert!(player_level(&img, |_| "123".into()).unwrap_err().starts_with("OCR"));
        assert_eq!(player_level(&img, |_| "00".into()), Err("level 0 out of range".into()));
        assert_eq!(player_level(&img, |_| "80".into()), Err("level 80 out of range".into()));
        assert_eq!(player_level(&img, |_| "79".into()), Ok(79));
        // OCR sees only the digit face, inset from the badge border.
        assert_eq!(player_level(&img, |face| format!("{}x{}", face.width(), face.height()).replace('x', "")), Err("OCR \"4139\"".into()));
    }

    #[test]
    fn traders_skips_unreadable_portraits() {
        // Capture too small to hold any portrait.
        assert!(traders(&RgbImage::new(10, 10), 1.0, |_| panic!("OCR on a tiny capture")).is_empty());
        // Wide enough for the portrait strip but not its icon core.
        assert!(traders(&RgbImage::new(25, 150), 1.0, |_| panic!("OCR with no core")).is_empty());
    }

    /// A first-portrait capture: light background, inked icon core, and a badge with `strokes` dark columns.
    fn portrait(bg: u8, strokes: &[u32]) -> RgbImage {
        let mut img = RgbImage::from_pixel(140, 150, Rgb([bg, bg, bg]));
        fill(&mut img, 35, 45, 125, 135, [0, 0, 0]);
        for x in strokes {
            fill(&mut img, 15 + x, 17 + x, 5, 25, [10, 10, 10]);
        }
        img
    }

    #[test]
    fn traders_reads_level_and_name_and_drops_blank_names() {
        let img = portrait(200, &[9]);
        assert_eq!(traders(&img, 1.0, |_| " Prapor ".into()), vec![TraderRead { name: "Prapor".into(), level: 1 }]);
        assert!(traders(&img, 1.0, |_| "  ".into()).is_empty());
        // Dark badge background: level unreadable, no OCR.
        assert!(traders(&portrait(100, &[9]), 1.0, |_| panic!("OCR with unreadable badge")).is_empty());
        // No inked core: a locked portrait.
        let locked = RgbImage::from_pixel(140, 150, Rgb([200, 200, 200]));
        assert!(traders(&locked, 1.0, |_| panic!("OCR on a locked portrait")).is_empty());
    }

    const RED: [u8; 3] = [200, 30, 30];

    /// A 1x-scale detail capture: red close button from row 10, then optional white text lines.
    fn panel(h: u32, lines: &[(u32, u32)]) -> RgbImage {
        let mut img = RgbImage::new(920, h);
        fill(&mut img, 870, 890, 10, 30, RED);
        for &(y0, y1) in lines {
            fill(&mut img, 100, 200, y0, y1, [230, 230, 230]);
        }
        img
    }

    fn scripted(answers: &[&str]) -> impl Fn(&RgbImage) -> String {
        let q = std::cell::RefCell::new(answers.iter().map(|s| s.to_string()).collect::<std::collections::VecDeque<_>>());
        move |_| q.borrow_mut().pop_front().expect("unexpected extra OCR call")
    }

    #[test]
    fn detail_close_button_must_be_red_enough() {
        let mut img = RgbImage::new(920, 60);
        fill(&mut img, 870, 890, 10, 20, [60, 60, 60]); // reddish only in the r >= 55 test, not r - g
        fill(&mut img, 870, 890, 20, 30, [100, 50, 90]); // r - g ok, r - b too small
        fill(&mut img, 870, 890, 30, 40, [40, 0, 0]); // too dark
        fill(&mut img, 870, 880, 40, 50, RED); // only 10 px wide
        assert_eq!(detail(&img, 1.0, |_| panic!("OCR without a close button")), None);
    }

    #[test]
    fn detail_reads_name_and_level() {
        let img = panel(200, &[(30, 42), (50, 62)]);
        let got = detail(&img, 1.0, scripted(&["Vents", "Requirements", "02"])).unwrap();
        assert_eq!(got, StationRead { name: "Vents".into(), level: 2 });
        // A single text line still reads its icon number.
        let one = panel(200, &[(30, 42)]);
        assert_eq!(detail(&one, 1.0, scripted(&["Gym", "05"])).unwrap().level, 5);
    }

    #[test]
    fn detail_locked_station_is_level_zero_without_reading_the_icon() {
        let img = panel(200, &[(30, 42), (50, 62)]);
        let got = detail(&img, 1.0, scripted(&["Gym", "Locked"])).unwrap();
        assert_eq!(got, StationRead { name: "Gym".into(), level: 0 });
    }

    #[test]
    fn detail_rejects_unreadable_panels() {
        let two = panel(200, &[(30, 42), (50, 62)]);
        // Blank name.
        assert_eq!(detail(&two, 1.0, scripted(&["  "])), None);
        // Icon digits that are not a two-digit level 1-9.
        for digits in ["2", "10", "00", "ab", ""] {
            assert_eq!(detail(&two, 1.0, scripted(&["Gym", "Requirements", digits])), None, "{digits:?}");
        }
        // No title text under the button.
        assert_eq!(detail(&panel(200, &[]), 1.0, |_| panic!("OCR without text")), None);
        // Text lines of implausible height (3 px, 40 px) are not title lines.
        assert_eq!(detail(&panel(200, &[(30, 33), (50, 90)]), 1.0, |_| panic!("OCR on noise")), None);
        // Capture ends above the icon number's rows.
        assert_eq!(detail(&panel(50, &[(30, 42)]), 1.0, scripted(&["Gym"])), None);
    }
}
