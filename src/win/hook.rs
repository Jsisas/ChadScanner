//! `WH_MOUSE_LL` hook: emits a scan request when the configured hotkey is released.
//! `WH_KEYBOARD_LL` hook: emits a progress step when Arrow Up / Down is released. Neither swallows input.
use crate::config::Hotkey;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_DOWN, VK_MENU, VK_SHIFT, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_KEYUP, WM_LBUTTONUP, WM_MBUTTONUP, WM_XBUTTONUP,
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

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let msg = wparam.0 as u32;
        let xbutton = (ms.mouseData >> 16) as u16;
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
    if code >= 0 && wparam.0 as u32 == WM_KEYUP && STEP_KEYS.load(Ordering::Relaxed) {
        let vk = (*(lparam.0 as *const KBDLLHOOKSTRUCT)).vkCode;
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
    CallNextHookEx(None, code, wparam, lparam)
}

/// Installs both hooks on one thread with a message loop (required for LL hooks).
pub fn start(tx: Sender<ScanRequest>, step_tx: Sender<Step>, hotkey: Hotkey, progress_keys: bool) {
    set_hotkey(hotkey);
    set_progress_keys(progress_keys);
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
