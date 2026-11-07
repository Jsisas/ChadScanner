//! Reads the extraction list EFT shows at spawn ("EXIT01 Trailer Park", …) from a screenshot of
//! the top-right of the screen: finds the panel by its green header, then the bright text rows below it, OCRs each one, keeps the EXIT lines.
use image::RgbImage;
use regex::Regex;
use std::sync::LazyLock;

/// Text is bright and nearly grey; the coloured header and TRANSIT labels fall out.
const MIN_LUMA: u32 = 150;
const MAX_SATURATION: i32 = 40;
/// A row belongs to a text line when it holds at least this many text pixels.
const MIN_ROW_PIXELS: u32 = 2;

static LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*EX(?:[I1l]T|F[I1l]L)\s*0?\d{1,2}\s+(?P<name>.+?)\s*$").unwrap());
static TIMER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+\d{1,2}:\d{2}(:\d{2})?$").unwrap());

pub(crate) fn is_text(p: &image::Rgb<u8>) -> bool {
    let [r, g, b] = [p[0] as u32, p[1] as u32, p[2] as u32];
    let luma = (299 * r + 587 * g + 114 * b) / 1000;
    let sat = r.max(g).max(b) as i32 - r.min(g).min(b) as i32;
    luma >= MIN_LUMA && sat <= MAX_SATURATION
}

/// Header pixels are green: `g` well above both `r` and `b` (the fixture's is `(108,159,5)`).
pub(crate) fn is_header(p: &image::Rgb<u8>) -> bool {
    let [r, g, b] = [p[0] as i32, p[1] as i32, p[2] as i32];
    g >= 120 && g - b >= 80 && g - r >= 20
}

/// The green "Find an extraction point" header atop the panel: `(left, right, bottom)` in `img`, right/bottom exclusive.
pub fn header(img: &RgbImage, scale: f32) -> Option<(u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let (min_px, min_run) = ((150.0 * scale) as u32, (30.0 * scale) as u32);
    let is_row = |y: u32| (0..w).filter(|&x| is_header(img.get_pixel(x, y))).count() as u32 >= min_px;
    let mut y = 0;
    while y < h {
        if !is_row(y) {
            y += 1;
            continue;
        }
        let start = y;
        while y < h && is_row(y) {
            y += 1;
        }
        let len = y - start;
        if len < min_run {
            continue;
        }
        let wide = |x: u32| (start..y).filter(|&r| is_header(img.get_pixel(x, r))).count() as u32 >= len / 2;
        let left = (0..w).find(|&x| wide(x))?;
        let right = (0..w).rev().find(|&x| wide(x))? + 1;
        return Some((left, right, y));
    }
    None
}

/// Text-line boxes `(x, y, w, h)` in `img`, top to bottom. `scale` = screen height / 1080.
pub fn bands(img: &RgbImage, scale: f32) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let (px0, px1, py0) = header(img, scale).unwrap_or((0, w, 0));
    let (min_h, max_h) = ((10.0 * scale) as u32, (30.0 * scale).ceil() as u32);
    let (pad_x, pad_y) = ((4.0 * scale).round() as u32, (3.0 * scale).round() as u32);
    let row = |y: u32| -> (u32, u32, u32) {
        let (mut n, mut lo, mut hi) = (0, u32::MAX, 0);
        for x in px0..px1 {
            if is_text(img.get_pixel(x, y)) {
                n += 1;
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        (n, lo, hi)
    };
    let mut out = Vec::new();
    let mut open: Option<(u32, u32, u32)> = None; // (first row, min x, max x)
    for y in py0..=h {
        let (n, lo, hi) = if y < h { row(y) } else { (0, 0, 0) };
        match (&mut open, n >= MIN_ROW_PIXELS) {
            (Some(b), true) => {
                b.1 = b.1.min(lo);
                b.2 = b.2.max(hi);
            }
            (None, true) => open = Some((y, lo, hi)),
            (Some(_), false) => {
                let (y0, lo, hi) = open.take().unwrap();
                let height = y - y0;
                if (min_h..=max_h).contains(&height) {
                    let x0 = lo.saturating_sub(pad_x);
                    let x1 = (hi + 1 + pad_x).min(w);
                    let top = y0.saturating_sub(pad_y);
                    let bottom = (y + pad_y).min(h);
                    out.push((x0, top, x1 - x0, bottom - top));
                }
            }
            (None, false) => {}
        }
    }
    out
}

/// The extract name in an OCR'd panel line, if it is an `EXITnn Name` row.
pub fn parse_line(text: &str) -> Option<String> {
    let name = &LINE.captures(text)?["name"];
    let name = TIMER.replace(name, "");
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Raw OCR text of every text band in `img` (the panel region, `scale` = screen height / 1080), top to bottom.
pub fn read_lines(img: &RgbImage, scale: f32, ocr: impl Fn(&RgbImage) -> String) -> Vec<String> {
    bands(img, scale)
        .into_iter()
        .map(|(x, y, w, h)| ocr(&image::imageops::crop_imm(img, x, y, w, h).to_image()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(img: &RgbImage, scale: f32, ocr: impl Fn(&RgbImage) -> String) -> Vec<String> {
        read_lines(img, scale, ocr).iter().filter_map(|l| parse_line(l)).collect()
    }

    const SCALE: f32 = 1439.0 / 1080.0;

    fn fixture() -> RgbImage {
        image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/exfil-customs-1440p.png")).unwrap().to_rgb8()
    }

    #[test]
    fn parses_exit_rows_only() {
        assert_eq!(parse_line("EXIT03 ZB-1011").as_deref(), Some("ZB-1011"));
        assert_eq!(parse_line("EXIT05 Boiler Room Basement (Co-op)").as_deref(), Some("Boiler Room Basement (Co-op)"));
        assert_eq!(parse_line("EX1T01 Trailer Park").as_deref(), Some("Trailer Park"));
        assert_eq!(parse_line("EXIT02 Old Gas Station 0:34:55").as_deref(), Some("Old Gas Station"));
        assert_eq!(parse_line("EXIT10 Crossroads").as_deref(), Some("Crossroads"));
        assert_eq!(parse_line("EXFIL01 Dorms V-Ex").as_deref(), Some("Dorms V-Ex"));
        assert_eq!(parse_line("EXFIL05 Railroad Passage (Flare)").as_deref(), Some("Railroad Passage (Flare)"));
        assert_eq!(parse_line("Transit to Reserve"), None);
        assert_eq!(parse_line("TRANSIT01 Transit to Reserve 0:00:55"), None);
        assert_eq!(parse_line("0:34:55"), None);
        assert_eq!(parse_line("EXIT01 "), None);
    }

    fn assert_exit_rows(found: &[(u32, u32, u32, u32)]) {
        assert!(found.len() >= 5, "{found:?}");
        let exits: Vec<_> = found.iter().filter(|b| b.0 <= 300).collect();
        let want = [104, 173, 242, 312, 381];
        assert_eq!(exits.len(), 5, "{found:?}");
        for (b, y) in exits.iter().zip(want) {
            // Bands are padded by round(3·scale) = 4 px above the text.
            let text_top = b.1 as i32 + (3.0 * SCALE).round() as i32;
            assert!((text_top - y).abs() <= 4, "band {b:?} vs y {y}");
        }
    }

    #[test]
    fn finds_the_five_exit_rows() {
        assert_exit_rows(&bands(&fixture(), SCALE));
    }

    #[test]
    fn finds_the_header() {
        assert_eq!(header(&fixture(), SCALE), Some((267, 839, 79)));
    }

    #[test]
    fn ignores_bright_scenery_beside_the_panel() {
        let mut img = fixture();
        let sky = image::Rgb([205, 210, 215]);
        for (x, y, p) in img.enumerate_pixels_mut() {
            if x < 250 || (x >= 850 && y < 300) {
                *p = sky;
            }
        }
        assert_exit_rows(&bands(&img, SCALE));
    }

    /// OCR end to end; needs ChadVision's model files next to this repo.
    #[test]
    fn reads_the_customs_panel() {
        let data = std::path::Path::new("../ChadVision/data");
        if !data.join("models/ppocrv5_latin_rec.onnx").exists() {
            return eprintln!("skipped: no ../ChadVision/data");
        }
        let engine = chadvision::engine::Engine::open(data).unwrap();
        let names = read(&fixture(), SCALE, |l| engine.read_line(l));
        eprintln!("ocr: {names:?}");
        let want = ["Trailer Park", "Old Gas Station", "ZB-1011", "Administration Gate", "Boiler Room Basement (Co-op)"];
        let real = ["/tmp/maps.json", "/tmp/maps_en.json", "/tmp/tdmaps.json"].map(|p| std::fs::read(p).ok());
        if let [Some(m), Some(en), Some(td)] = real {
            let maps = crate::maps::build(&m, &en, &td).unwrap();
            let view = maps.for_scene("maps/customs_preset.bundle").unwrap();
            let mut got = crate::maps::match_extracts(view, &names, None);
            got.sort();
            let mut want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
            want.sort();
            assert_eq!(got, want);
            assert_eq!(view.side_of(&got), Some(crate::maps::Side::Scav));
        } else {
            assert_eq!(names.len(), want.len(), "{names:?}");
            for (n, w) in names.iter().zip(want) {
                assert!(chadvision::lexicon::score(n, w) >= 0.75, "{n} vs {w}");
            }
        }
    }
}
