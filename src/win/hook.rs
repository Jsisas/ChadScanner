//! `WH_MOUSE_LL` hook: emits a scan request when the configured hotkey is released.
//! `WH_KEYBOARD_LL` hook: emits a progress step when Arrow Up / Down is released.
//! Both also watch for EFT's show-exits key (recorded, see `exfil_opened_age`). Neither swallows input.
use crate::config::Hotkey;
use crate::eftkeys::{self, Key};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_DOWN, VK_MENU, VK_SHIFT, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    WM_XBUTTONUP,
};

#[derive(Debug, Clone, Copy)]
pub struct ScanRequest {
    pub x: i32,
    pub y: i32,
}

/// A quest-progress key press: Arrow Up raises, Arrow Down lowers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
}

static TX: OnceLock<Sender<ScanRequest>> = OnceLock::new();
static STEP_TX: OnceLock<Sender<Step>> = OnceLock::new();
static HOTKEY: AtomicU8 = AtomicU8::new(0);
static STEP_KEYS: AtomicBool = AtomicBool::new(false);
static EXFIL_KEY: OnceLock<eftkeys::ExfilKey> = OnceLock::new();
static BASE: OnceLock<Instant> = OnceLock::new();
/// Milliseconds since `BASE` plus one; 0 = never.
static LAST_PRESS: AtomicU64 = AtomicU64::new(0);
static OPENED: AtomicU64 = AtomicU64::new(0);
/// The keyboard trigger is held: Windows auto-repeat must not look like a double-tap.
static TRIGGER_DOWN: AtomicBool = AtomicBool::new(false);
/// Slack on top of EFT's double-click timeout for the gap between hook and game timestamps.
const DOUBLE_SLACK_MS: u64 = 100;

pub fn set_hotkey(h: Hotkey) {
    HOTKEY.store(Hotkey::ALL.iter().position(|x| *x == h).unwrap_or(0) as u8, Ordering::Relaxed);
}

pub fn set_progress_keys(on: bool) {
    STEP_KEYS.store(on, Ordering::Relaxed);
}

fn hotkey() -> Hotkey {
    Hotkey::ALL[HOTKEY.load(Ordering::Relaxed) as usize % Hotkey::ALL.len()]
}

fn down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

fn now_ms() -> u64 {
    BASE.get().map_or(0, |b| b.elapsed().as_millis() as u64) + 1
}

/// The player's show-exits key as shown in hints, e.g. "double-tap O".
pub fn exfil_label() -> &'static str {
    EXFIL_KEY.get().map_or("double-tap O", |k| k.label.as_str())
}

/// Time since the player last opened EFT's exit list, if ever.
pub fn exfil_opened_age() -> Option<Duration> {
    let at = OPENED.load(Ordering::Relaxed);
    (at != 0).then(|| Duration::from_millis(now_ms().saturating_sub(at)))
}

fn is_trigger(vk: u32) -> bool {
    EXFIL_KEY.get().is_some_and(|k| k.bindings.iter().any(|b| b.key == Key::Vk(vk as u16)))
}

/// A press of `key`: counts as opening the list when a binding matches, and for a double-tap binding the second press in time.
fn exfil_press(key: Key) {
    let Some(cfg) = EXFIL_KEY.get() else { return };
    if !cfg.bindings.iter().any(|b| b.key == key && b.held.iter().all(|vk| down(*vk))) {
        return;
    }
    let now = now_ms();
    let Some(gap) = cfg.double else {
        OPENED.store(now, Ordering::Relaxed);
        return;
    };
    let last = LAST_PRESS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) <= gap.as_millis() as u64 + DOUBLE_SLACK_MS {
        OPENED.store(now, Ordering::Relaxed);
        LAST_PRESS.store(0, Ordering::Relaxed);
    } else {
        LAST_PRESS.store(now, Ordering::Relaxed);
    }
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let msg = wparam.0 as u32;
        let xbutton = (ms.mouseData >> 16) as u16;
        match msg {
            WM_MBUTTONDOWN => exfil_press(Key::Mouse(2)),
            WM_XBUTTONDOWN => match (ms.mouseData >> 16) as u16 {
                1 => exfil_press(Key::Mouse(3)),
                2 => exfil_press(Key::Mouse(4)),
                _ => {}
            },
            _ => {}
        }
        let fire = match hotkey() {
            Hotkey::ShiftLeft => msg == WM_LBUTTONUP && down(VK_SHIFT.0),
            Hotkey::AltLeft => msg == WM_LBUTTONUP && down(VK_MENU.0),
            Hotkey::Middle => msg == WM_MBUTTONUP,
            Hotkey::Mouse4 => msg == WM_XBUTTONUP && xbutton == 1,
            Hotkey::Mouse5 => msg == WM_XBUTTONUP && xbutton == 2,
        };
        if fire {
            if let Some(tx) = TX.get() {
                let _ = tx.send(ScanRequest { x: ms.pt.x, y: ms.pt.y });
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Key-up, not key-down: auto-repeat would flood a held key with steps.
unsafe extern "system" fn key_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        let vk = (*(lparam.0 as *const KBDLLHOOKSTRUCT)).vkCode;
        if is_trigger(vk) {
            match msg {
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    if !TRIGGER_DOWN.swap(true, Ordering::Relaxed) {
                        exfil_press(Key::Vk(vk as u16));
                    }
                }
                WM_KEYUP | WM_SYSKEYUP => TRIGGER_DOWN.store(false, Ordering::Relaxed),
                _ => {}
            }
        }
        if msg == WM_KEYUP && STEP_KEYS.load(Ordering::Relaxed) {
            let step = if vk == VK_UP.0 as u32 {
                Some(Step::Up)
            } else if vk == VK_DOWN.0 as u32 {
                Some(Step::Down)
            } else {
                None
            };
            if let (Some(step), Some(tx)) = (step, STEP_TX.get()) {
                if !down(VK_SHIFT.0) && !down(VK_CONTROL.0) && !down(VK_MENU.0) {
                    let _ = tx.send(step);
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Installs both hooks on one thread with a message loop (required for LL hooks).
pub fn start(tx: Sender<ScanRequest>, step_tx: Sender<Step>, hotkey: Hotkey, progress_keys: bool, exfil_key: eftkeys::ExfilKey) {
    set_hotkey(hotkey);
    set_progress_keys(progress_keys);
    let _ = BASE.set(Instant::now());
    let _ = EXFIL_KEY.set(exfil_key);
    let _ = TX.set(tx);
    let _ = STEP_TX.set(step_tx);
    std::thread::Builder::new()
        .name("input-hook".into())
        .spawn(|| unsafe {
            let module = GetModuleHandleW(None).unwrap_or_default();
            match SetWindowsHookExW(WH_MOUSE_LL, Some(proc), module, 0) {
                Ok(_) => log::info!("mouse hook installed"),
                Err(e) => return log::error!("SetWindowsHookExW failed: {e}"),
            }
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), module, 0) {
                Ok(_) => log::info!("keyboard hook installed"),
                Err(e) => log::error!("keyboard hook failed: {e}"),
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        })
        .expect("spawn hook thread");
}
