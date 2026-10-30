//! `%APPDATA%\ChadOverlay\config.json` and the app's file locations.
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
    /// Non-FIR needs for items obtainable now for at most this much (₽ each) are "buy later". 0 = off.
    pub buy_later_max: i64,
    pub log_tracking: bool,
    pub logs_override: String,
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
            buy_later_max: 20_000,
            log_tracking: true,
            logs_override: String::new(),
        }
    }
}

impl Config {
    /// Missing file → defaults; an unreadable file is logged and replaced by defaults.
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
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
    pub data: PathBuf,
    pub appdata: PathBuf,
}

impl Paths {
    pub fn detect() -> anyhow::Result<Paths> {
        let exe_dir = std::env::current_exe()?.parent().map(Path::to_path_buf).unwrap_or_default();
        let appdata = directories::BaseDirs::new()
            .map(|d| d.config_dir().join("ChadOverlay"))
            .ok_or_else(|| anyhow::anyhow!("no APPDATA directory"))?;
        std::fs::create_dir_all(appdata.join("cache"))?;
        Ok(Paths { data: exe_dir.join("data"), appdata })
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
    pub fn cache(&self) -> PathBuf {
        self.appdata.join("cache")
    }
}
