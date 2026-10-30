//! `WH_MOUSE_LL` hook: emits a scan request when the configured hotkey is released. Never swallows input.
use crate::config::Hotkey;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONUP, WM_MBUTTONUP,
    WM_XBUTTONUP,
};

#[derive(Debug, Clone, Copy)]
pub struct ScanRequest {
    pub x: i32,
    pub y: i32,
}

static TX: OnceLock<Sender<ScanRequest>> = OnceLock::new();
static HOTKEY: AtomicU8 = AtomicU8::new(0);

pub fn set_hotkey(h: Hotkey) {
    HOTKEY.store(Hotkey::ALL.iter().position(|x| *x == h).unwrap_or(0) as u8, Ordering::Relaxed);
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

/// Installs the hook on its own thread with a message loop (required for LL hooks).
pub fn start(tx: Sender<ScanRequest>, hotkey: Hotkey) {
    set_hotkey(hotkey);
    let _ = TX.set(tx);
    std::thread::Builder::new()
        .name("input-hook".into())
        .spawn(|| unsafe {
            let module = GetModuleHandleW(None).unwrap_or_default();
            match SetWindowsHookExW(WH_MOUSE_LL, Some(proc), module, 0) {
                Ok(_) => log::info!("mouse hook installed"),
                Err(e) => return log::error!("SetWindowsHookExW failed: {e}"),
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        })
        .expect("spawn hook thread");
}
