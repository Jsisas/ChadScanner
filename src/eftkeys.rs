//! EFT's "show exits" key (`WatchTimerAndExits`), read from its `Control.ini`.
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// A key or mouse button. Mouse: 2 middle, 3 X1, 4 X2 (Unity `Mouse2`..`Mouse4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Vk(u16),
    Mouse(u8),
}

/// `key` triggers it while every `held` virtual key is down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub key: Key,
    pub held: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExfilKey {
    pub bindings: Vec<Binding>,
    /// Max gap between the two presses when EFT wants a double-tap.
    pub double: Option<Duration>,
    pub label: String,
}

const DEFAULT_DOUBLE: Duration = Duration::from_millis(300);

impl Default for ExfilKey {
    fn default() -> Self {
        ExfilKey {
            bindings: vec![Binding { key: Key::Vk(0x4F), held: Vec::new() }],
            double: Some(DEFAULT_DOUBLE),
            label: "double-tap O".into(),
        }
    }
}

pub fn control_ini() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(base.config_dir().join("Battlestate Games").join("Escape from Tarkov").join("Settings").join("Control.ini"))
}

/// The player's binding, or double-tap O when the file is missing or unusable.
pub fn load() -> ExfilKey {
    let read = control_ini().ok_or_else(|| "no config folder".to_string()).and_then(|p| {
        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display())).and_then(|s| parse(&s))
    });
    read.unwrap_or_else(|e| {
        log::warn!("exfil key: {e}; using double-tap O");
        ExfilKey::default()
    })
}

pub fn parse(json: &str) -> Result<ExfilKey, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("Control.ini is not JSON: {e}"))?;
    let entry = root["keyBindings"]
        .as_array()
        .and_then(|a| a.iter().find(|b| b["keyName"] == "WatchTimerAndExits"))
        .ok_or("no WatchTimerAndExits binding")?;
    let press = entry["pressType"].as_str().unwrap_or("");
    let mut bindings = Vec::new();
    let mut label = None;
    for v in entry["variants"].as_array().into_iter().flatten() {
        if v["isAxis"].as_bool() == Some(true) {
            continue;
        }
        let names: Vec<&str> = v["keyCode"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let Some((trigger, held_names)) = names.split_first() else { continue };
        let Some(key) = key(trigger) else { continue };
        let held: Option<Vec<u16>> = held_names
            .iter()
            .map(|n| match crate::eftkeys::key(n) {
                Some(Key::Vk(vk)) => Some(vk),
                _ => None,
            })
            .collect();
        let Some(held) = held else { continue };
        if label.is_none() {
            let verb = match press {
                "DoubleClick" => "double-tap",
                "Continuous" => "hold",
                _ => "press",
            };
            let mut keys: Vec<&str> = held_names.iter().map(|n| n.strip_prefix("Alpha").unwrap_or(n)).collect();
            keys.push(trigger.strip_prefix("Alpha").unwrap_or(trigger));
            label = Some(format!("{verb} {}", keys.join("+")));
        }
        bindings.push(Binding { key, held });
    }
    let label = label.ok_or("WatchTimerAndExits has no usable key")?;
    let double = (press == "DoubleClick").then(|| {
        root["DoubleClickTimeout"]
            .as_f64()
            .filter(|t| *t > 0.0)
            .map_or(DEFAULT_DOUBLE, Duration::from_secs_f64)
    });
    Ok(ExfilKey { bindings, double, label })
}

/// Unity `KeyCode` name to a Windows virtual key or mouse button.
fn key(name: &str) -> Option<Key> {
    let num = |prefix: &str, max: u16| -> Option<u16> {
        name.strip_prefix(prefix)?.parse::<u16>().ok().filter(|n| *n <= max)
    };
    if name.len() == 1 && name.as_bytes()[0].is_ascii_uppercase() {
        return Some(Key::Vk(name.as_bytes()[0] as u16));
    }
    if let Some(n) = num("Alpha", 9) {
        return Some(Key::Vk(0x30 + n));
    }
    if let Some(n) = num("Keypad", 9) {
        return Some(Key::Vk(0x60 + n));
    }
    if let Some(n) = num("F", 15).filter(|n| *n >= 1) {
        return Some(Key::Vk(0x70 + n - 1));
    }
    let vk = match name {
        "Mouse2" => return Some(Key::Mouse(2)),
        "Mouse3" => return Some(Key::Mouse(3)),
        "Mouse4" => return Some(Key::Mouse(4)),
        "Space" => 0x20,
        "Tab" => 0x09,
        "Return" => 0x0D,
        "Backspace" => 0x08,
        "CapsLock" => 0x14,
        "BackQuote" => 0xC0,
        "Minus" => 0xBD,
        "Equals" => 0xBB,
        "LeftBracket" => 0xDB,
        "RightBracket" => 0xDD,
        "Semicolon" => 0xBA,
        "Quote" => 0xDE,
        "Comma" => 0xBC,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Backslash" => 0xDC,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "SysReq" | "Print" => 0x2C,
        "UpArrow" => 0x26,
        "DownArrow" => 0x28,
        "LeftArrow" => 0x25,
        "RightArrow" => 0x27,
        "LeftShift" => 0xA0,
        "RightShift" => 0xA1,
        "LeftControl" => 0xA2,
        "RightControl" => 0xA3,
        "LeftAlt" => 0xA4,
        "RightAlt" => 0xA5,
        _ => return None,
    };
    Some(Key::Vk(vk))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ini(entry: &str) -> String {
        format!(r#"{{"DoubleClickTimeout":0.3,"keyBindings":[{{"keyName":"Other","variants":[{{"keyCode":["P"]}}],"pressType":"Press"}},{entry}]}}"#)
    }

    #[test]
    fn real_entry_is_double_tap_o() {
        let k = parse(&ini(r#"{"keyName":"WatchTimerAndExits","variants":[{"keyCode":["O"]},{"keyCode":[]}],"pressType":"DoubleClick"}"#)).unwrap();
        assert_eq!(k.bindings, vec![Binding { key: Key::Vk(0x4F), held: vec![] }]);
        assert_eq!(k.double, Some(Duration::from_millis(300)));
        assert_eq!(k.label, "double-tap O");
    }

    #[test]
    fn held_key_press() {
        let k = parse(&ini(r#"{"keyName":"WatchTimerAndExits","variants":[{"keyCode":["O","LeftAlt"]}],"pressType":"Press"}"#)).unwrap();
        assert_eq!(k.bindings, vec![Binding { key: Key::Vk(0x4F), held: vec![0xA4] }]);
        assert_eq!(k.double, None);
        assert_eq!(k.label, "press LeftAlt+O");
    }

    #[test]
    fn keyboard_and_mouse_variants() {
        let k = parse(&ini(r#"{"keyName":"WatchTimerAndExits","variants":[{"keyCode":["O"]},{"keyCode":["Mouse4"]}],"pressType":"DoubleClick"}"#)).unwrap();
        assert_eq!(k.bindings.len(), 2);
        assert_eq!(k.bindings[1].key, Key::Mouse(4));
    }

    #[test]
    fn key_names() {
        assert_eq!(key("Alpha5"), Some(Key::Vk(0x35)));
        assert_eq!(key("Alpha0"), Some(Key::Vk(0x30)));
        assert_eq!(key("F1"), Some(Key::Vk(0x70)));
        assert_eq!(key("F12"), Some(Key::Vk(0x7B)));
        assert_eq!(key("F16"), None);
        assert_eq!(key("F0"), None);
        assert_eq!(key("Keypad7"), Some(Key::Vk(0x67)));
    }

    #[test]
    fn unusable_inputs_error() {
        for entry in [
            r#"{"keyName":"WatchTimerAndExits","variants":[{"keyCode":["Mouse0"]}],"pressType":"Press"}"#,
            r#"{"keyName":"WatchTimerAndExits","variants":[{"keyCode":["Foo"]}],"pressType":"Press"}"#,
            r#"{"keyName":"WatchTimerAndExits","variants":[{"keyCode":["O","Mouse2"]}],"pressType":"Press"}"#,
        ] {
            assert!(parse(&ini(entry)).is_err(), "{entry}");
        }
        assert!(parse(&ini(r#"{"keyName":"Nope","variants":[],"pressType":"Press"}"#)).is_err());
        assert!(parse("not json").is_err());
    }
}
