//! `%APPDATA%\ChadScanner\config.json` and the app's file locations.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Pvp,
    #[default]
    Pve,
    Seasonal,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Pvp, Mode::Pve, Mode::Seasonal];

    /// Path segment on json.tarkov.dev.
    pub fn api(self) -> &'static str {
        match self {
            Mode::Pvp => "regular",
            Mode::Pve => "pve",
            Mode::Seasonal => "pvp-season",
        }
    }

    /// Section key in a TarkovTracker backup, and TarkovTracker's own mode name.
    pub fn key(self) -> &'static str {
        match self {
            Mode::Pvp => "pvp",
            Mode::Pve => "pve",
            Mode::Seasonal => "seasonal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Pvp => "PvP",
            Mode::Pve => "PvE",
            Mode::Seasonal => "Seasonal",
        }
    }

    /// The game's spelling in `Session mode: …`; `None` for anything unrecognised.
    pub fn from_session(raw: &str) -> Option<Mode> {
        match raw.to_ascii_lowercase().as_str() {
            "pve" => Some(Mode::Pve),
            "regular" | "pvp" => Some(Mode::Pvp),
            "pvpseason" | "seasonal" | "szn" => Some(Mode::Seasonal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Local,
    Tracker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Hotkey {
    #[default]
    ShiftLeft,
    AltLeft,
    Middle,
    Mouse4,
    Mouse5,
}

impl Hotkey {
    pub const ALL: [Hotkey; 5] = [Hotkey::ShiftLeft, Hotkey::AltLeft, Hotkey::Middle, Hotkey::Mouse4, Hotkey::Mouse5];

    pub fn label(self) -> &'static str {
        match self {
            Hotkey::ShiftLeft => "Shift + Left click",
            Hotkey::AltLeft => "Alt + Left click",
            Hotkey::Middle => "Middle click",
            Hotkey::Mouse4 => "Mouse 4",
            Hotkey::Mouse5 => "Mouse 5",
        }
    }
}

/// Quests outside the goal sink to the bottom of the card and do not count toward KEEP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Goal {
    #[default]
    None,
    Kappa,
    Lightkeeper,
}

impl Goal {
    pub const ALL: [Goal; 3] = [Goal::None, Goal::Kappa, Goal::Lightkeeper];

    pub fn label(self) -> &'static str {
        match self {
            Goal::None => "None",
            Goal::Kappa => "Kappa",
            Goal::Lightkeeper => "Lightkeeper",
        }
    }
}

/// Per-mode TarkovTracker tokens, DPAPI-encrypted (user scope) and hex-encoded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Tokens {
    pub pvp: String,
    pub pve: String,
    pub seasonal: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    pub source: Source,
    pub tokens: Tokens,
    pub hotkey: Hotkey,
    /// 0 = no timeout.
    pub tooltip_seconds: f32,
    /// Hide the card once the cursor moves away from the scanned item.
    pub close_on_leave: bool,
    pub ui_scale: f32,
    pub show_non_fir: bool,
    /// Quest/hideout rows on the card before the "+n× more" line.
    pub max_rows: u32,
    pub show_quests: bool,
    pub show_hideout: bool,
    pub show_barters: bool,
    pub show_crafts: bool,
    /// Priority ordering of need rows (see `card`); off = quests then hideout, biggest count first.
    pub smart_ordering: bool,
    pub goal: Goal,
    pub log_tracking: bool,
    pub logs_override: String,
    /// Arrow Up / Down change quest progress for the last scanned item while EFT is focused.
    pub progress_keys: bool,
    /// Arrow Up may fill non-FIR needs; off = only FIR ones when the item has any.
    pub overfill_fir: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mode: Mode::Pve,
            source: Source::Local,
            tokens: Tokens::default(),
            hotkey: Hotkey::ShiftLeft,
            tooltip_seconds: 0.0,
            close_on_leave: true,
            ui_scale: 1.0,
            show_non_fir: true,
            max_rows: 5,
            show_quests: true,
            show_hideout: true,
            show_barters: true,
            show_crafts: true,
            smart_ordering: true,
            goal: Goal::None,
            log_tracking: true,
            logs_override: String::new(),
            overfill_fir: true,
            progress_keys: true,
        }
    }
}

impl Config {
    /// Missing file → defaults; an unreadable file is logged and replaced by defaults.
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<Config>(&text)
                .map(|mut c| {
                    c.tooltip_seconds = if c.tooltip_seconds.is_finite() { c.tooltip_seconds.clamp(0.0, 30.0) } else { 0.0 };
                    c
                })
                .unwrap_or_else(|e| {
                    log::warn!("config.json unreadable ({e}); using defaults");
                    Config::default()
                }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    fn token_field(&mut self, mode: Mode) -> &mut String {
        match mode {
            Mode::Pvp => &mut self.tokens.pvp,
            Mode::Pve => &mut self.tokens.pve,
            Mode::Seasonal => &mut self.tokens.seasonal,
        }
    }

    /// Plain token for `mode`; `None` when unset or not decryptable by this Windows user.
    pub fn token(&self, mode: Mode) -> Option<String> {
        let hex = match mode {
            Mode::Pvp => &self.tokens.pvp,
            Mode::Pve => &self.tokens.pve,
            Mode::Seasonal => &self.tokens.seasonal,
        };
        if hex.is_empty() {
            return None;
        }
        let plain = crate::secret::unprotect(&from_hex(hex)?)?;
        String::from_utf8(plain).ok().filter(|t| !t.is_empty())
    }

    pub fn set_token(&mut self, mode: Mode, token: &str) -> anyhow::Result<()> {
        let hex = if token.is_empty() { String::new() } else { to_hex(&crate::secret::protect(token.trim().as_bytes())?) };
        *self.token_field(mode) = hex;
        Ok(())
    }
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Temp file + rename, so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Where everything lives. Data (models, index, refs) ships next to the exe.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The exe's folder; updates rewrite it.
    pub install: PathBuf,
    pub data: PathBuf,
    pub appdata: PathBuf,
}

impl Paths {
    pub fn detect() -> anyhow::Result<Paths> {
        let exe_dir = std::env::current_exe()?.parent().map(Path::to_path_buf).unwrap_or_default();
        let base = directories::BaseDirs::new().ok_or_else(|| anyhow::anyhow!("no APPDATA directory"))?;
        let mut appdata = base.config_dir().join("ChadScanner");
        // Settings, progress and reports from before the rename (the app was called ChadOverlay).
        // If the move fails (e.g. the old app still runs), keep using the old folder.
        let old = base.config_dir().join("ChadOverlay");
        if !appdata.exists() && old.is_dir() && std::fs::rename(&old, &appdata).is_err() {
            appdata = old;
        }
        std::fs::create_dir_all(appdata.join("cache"))?;
        Ok(Paths { data: exe_dir.join("data"), install: exe_dir, appdata })
    }
    pub fn config(&self) -> PathBuf {
        self.appdata.join("config.json")
    }
    pub fn progress(&self) -> PathBuf {
        self.appdata.join("progress.json")
    }
    pub fn pending(&self) -> PathBuf {
        self.appdata.join("pending.json")
    }
    pub fn log(&self) -> PathBuf {
        self.appdata.join("log.txt")
    }
    pub fn logstate(&self) -> PathBuf {
        self.appdata.join("logstate.json")
    }
    /// Last panel capture when no extract was read in a raid's first 30 s (overwritten each time).
    pub fn exfil_miss(&self) -> PathBuf {
        self.appdata.join("exfil-miss.png")
    }
    /// Last player-level badge capture that did not read (overwritten each time).
    pub fn player_miss(&self) -> PathBuf {
        self.appdata.join("player-miss.png")
    }
    pub fn reports(&self) -> PathBuf {
        self.appdata.join("reports")
    }
    /// Saved TarkovTracker window position (its own file: Settings rewrites `config.json` whole).
    pub fn tracker_window(&self) -> PathBuf {
        self.appdata.join("tracker-window.json")
    }
    /// WebView2 profile of the TarkovTracker window (keeps the login).
    pub fn webview(&self) -> PathBuf {
        self.appdata.join("webview")
    }
    pub fn cache(&self) -> PathBuf {
        self.appdata.join("cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        assert_eq!(from_hex("0a"), Some(vec![10]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex(""), Some(vec![]));
        assert_eq!(from_hex(&to_hex(&[0, 255, 16])), Some(vec![0, 255, 16]));
    }

    #[test]
    fn load_clamps_tooltip_seconds() {
        let d = std::env::temp_dir().join(format!("chadscanner-test-{}-cfg", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config.json");
        for (json, want) in [(r#"{"tooltip_seconds":1e30}"#, 30.0), (r#"{"tooltip_seconds":-5}"#, 0.0), (r#"{"tooltip_seconds":7}"#, 7.0)] {
            std::fs::write(&p, json).unwrap();
            assert_eq!(Config::load(&p).tooltip_seconds, want, "{json}");
        }
    }
}
