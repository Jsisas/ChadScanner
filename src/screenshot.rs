//! EFT writes the player's world position and rotation into each screenshot's file name:
//! `2026-09-04[18-33]_7.86, 38.06, -27.57_-0.03307, -0.13322, 0.00384, -0.99053_21.87 (0).png`
//! (`date[time]_x, y, z_qx, qy, qz, qw_gameTime (n).png`), the same source TarkovMonitor uses.
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^\d{4}-\d{2}-\d{2}\[\d{2}-\d{2}\]_",
        r"(?P<x>-?\d+(?:\.\d+)?),\s*(?P<y>-?\d+(?:\.\d+)?),\s*(?P<z>-?\d+(?:\.\d+)?)_",
        r"(?P<qx>-?\d+(?:\.\d+)?),\s*(?P<qy>-?\d+(?:\.\d+)?),\s*(?P<qz>-?\d+(?:\.\d+)?),\s*(?P<qw>-?\d+(?:\.\d+)?)",
        r".*\.png$"
    ))
    .unwrap()
});

/// Where the player stood, in the map page's coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    /// `[z, x]`, the order `maps::xz` uses.
    pub pos: [f64; 2],
    /// Heading in degrees (TarkovMonitor's `QuarternionsToYaw`).
    pub yaw: f64,
}

/// The position in a screenshot file name; `None` for other PNGs and for menu screenshots (no coordinates).
pub fn parse(file_name: &str) -> Option<Position> {
    let c = NAME.captures(file_name)?;
    let n = |k: &str| c[k].parse::<f64>().ok();
    let (x, z) = (n("x")?, n("z")?);
    let (qx, qy, qz, qw) = (n("qx")?, n("qy")?, n("qz")?, n("qw")?);
    let yaw = (2.0 * (qw * qy + qx * qz)).atan2(1.0 - 2.0 * (qz * qz + qy * qy)).to_degrees();
    Some(Position { pos: [z, x], yaw })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_position_and_yaw() {
        let p = parse("2026-09-04[18-33]_7.86, 38.06, -27.57_-0.03307, -0.13322, 0.00384, -0.99053_21.87 (0).png").unwrap();
        assert_eq!(p.pos, [-27.57, 7.86]);
        assert!((p.yaw - 15.3).abs() < 0.1, "yaw {}", p.yaw);
    }

    #[test]
    fn menu_screenshot_has_no_position() {
        assert_eq!(parse("2026-09-04[18-33]_ (0).png"), None);
    }

    #[test]
    fn other_png_has_no_position() {
        assert_eq!(parse("notes.png"), None);
    }
}
