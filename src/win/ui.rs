//! UI thread: the click-through tooltip card, toasts and the tray icon.
//!
//! Every frame is pushed whole with `UpdateLayeredWindow` (position, size and pixels in one
//! call), so there is no retained surface that could show a stale or half-resized frame.
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_F12};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetTimer, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, MSG, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WM_HOTKEY, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

pub enum Cmd {
    /// Card next to screen point `at`; hidden after `seconds` (if set), when the cursor leaves it
    /// (if `close_on_leave`), or by the next scan.
    Card { pix: Pixmap, at: (i32, i32), seconds: Option<f32>, close_on_leave: bool },
    HideCard,
    /// Top-centre of the primary monitor for 4 s.
    Toast(Pixmap),
}

#[derive(Clone)]
pub struct Ui {
    tx: Sender<Cmd>,
    hwnd: Arc<AtomicIsize>,
}

impl Ui {
    pub fn new() -> (Ui, Receiver<Cmd>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Ui { tx, hwnd: Arc::new(AtomicIsize::new(0)) }, rx)
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
        let h = self.hwnd.load(Ordering::Acquire);
        if h != 0 {
            unsafe {
                let _ = PostMessageW(HWND(h as _), WM_APP, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Tray menu actions handled by the app.
pub trait TrayActions {
    fn open_settings(&self);
    fn set_paused(&self, paused: bool);
    /// Ctrl+Shift+F12.
    fn debug_report(&self);
}

const TOAST_SECONDS: f32 = 4.0;
const DEBUG_HOTKEY: i32 = 1;
/// Cursor travel that dismisses the card.
const LEAVE_PX: i32 = 40;

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

struct Layered {
    hwnd: HWND,
    visible: bool,
}

impl Layered {
    unsafe fn create() -> anyhow::Result<Layered> {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("ChadScannerCard"),
            w!("ChadScanner"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            GetModuleHandleW(None)?,
            None,
        )?;
        Ok(Layered { hwnd, visible: false })
    }

    /// Pushes a premultiplied RGBA pixmap at screen (`x`, `y`) in one `UpdateLayeredWindow` call.
    unsafe fn push(&self, pix: &Pixmap, x: i32, y: i32) -> anyhow::Result<()> {
        let (w, h) = (pix.width() as i32, pix.height() as i32);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = (|| -> anyhow::Result<()> {
            let dib = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
            for (d, s) in dst.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
                d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
            }
            let old = SelectObject(mem, dib);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let r = UpdateLayeredWindow(
                self.hwnd,
                screen,
                Some(&POINT { x, y }),
                Some(&SIZE { cx: w, cy: h }),
                mem,
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(mem, old);
            let _ = DeleteObject(dib);
            Ok(r?)
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }

    unsafe fn show(&mut self, pix: &Pixmap, x: i32, y: i32) {
        if let Err(e) = self.push(pix, x, y) {
            return log::warn!("UpdateLayeredWindow failed: {e:#}");
        }
        if !self.visible {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            self.visible = true;
        }
        // EFT may have raised itself since; stay above it without taking focus.
        let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }

    unsafe fn hide(&mut self) {
        if !self.visible {
            return;
        }
        // Clear first, so the next show can never flash this frame.
        if let Some(clear) = Pixmap::new(1, 1) {
            let _ = self.push(&clear, -32000, -32000);
        }
        let _ = ShowWindow(self.hwnd, SW_HIDE);
        self.visible = false;
    }
}

/// Top-left for a card of `w`×`h` at cursor + (24, 24), flipped left/up to stay on the monitor.
fn place(at: (i32, i32), w: i32, h: i32) -> (i32, i32) {
    let (l, t, r, b) = super::monitor_rect(at.0, at.1);
    let mut x = at.0 + 24;
    let mut y = at.1 + 24;
    if x + w > r {
        x = at.0 - 24 - w;
    }
    if y + h > b {
        y = at.1 - 24 - h;
    }
    (x.max(l), y.max(t))
}

/// The exe icon (`assets/icon.svg`).
fn tray_icon() -> anyhow::Result<tray_icon::Icon> {
    let img = image::load_from_memory(include_bytes!("../../assets/icon-32.png"))?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok(tray_icon::Icon::from_rgba(img.into_raw(), w, h)?)
}

/// Runs the UI message loop on the calling thread until Quit.
pub fn run(ui: &Ui, rx: Receiver<Cmd>, actions: &dyn TrayActions) -> anyhow::Result<()> {
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None)?.into(),
            lpszClassName: w!("ChadScannerCard"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let mut card = Layered::create()?;
        let mut toast = Layered::create()?;
        ui.hwnd.store(card.hwnd.0 as isize, Ordering::Release);
        SetTimer(card.hwnd, 1, 50, None);
        // Thread hotkey (no window): WM_HOTKEY arrives with a null hwnd.
        if let Err(e) = RegisterHotKey(None, DEBUG_HOTKEY, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, VK_F12.0 as u32) {
            log::warn!("Ctrl+Shift+F12 unavailable (taken by another program?): {e}");
        }

        let settings = MenuItem::new("Settings", true, None);
        let pause = CheckMenuItem::new("Pause scanning", true, false, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&settings, &pause, &PredefinedMenuItem::separator(), &quit])?;
        let _tray = tray_icon::TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ChadScanner")
            .with_icon(tray_icon()?)
            .build()?;

        // (deadline, scan point, close on leave)
        let mut card_until: Option<(Option<Instant>, (i32, i32), bool)> = None;
        let mut toast_until: Option<Instant> = None;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY && msg.wParam.0 == DEBUG_HOTKEY as usize {
                actions.debug_report();
                continue;
            }
            if msg.hwnd == card.hwnd && msg.message == WM_APP {
                while let Ok(c) = rx.try_recv() {
                    match c {
                        Cmd::Card { pix, at, seconds, close_on_leave } => {
                            let (x, y) = place(at, pix.width() as i32, pix.height() as i32);
                            card.show(&pix, x, y);
                            let deadline = seconds.map(|s| Instant::now() + Duration::from_secs_f32(s));
                            card_until = Some((deadline, at, close_on_leave));
                        }
                        Cmd::HideCard => {
                            card.hide();
                            card_until = None;
                        }
                        Cmd::Toast(pix) => {
                            let (l, t, r, _) = super::monitor_rect(0, 0);
                            let x = (l + r - pix.width() as i32) / 2;
                            toast.show(&pix, x, t + 40);
                            toast_until = Some(Instant::now() + Duration::from_secs_f32(TOAST_SECONDS));
                        }
                    }
                }
                continue;
            }
            if msg.hwnd == card.hwnd && msg.message == WM_TIMER {
                let now = Instant::now();
                if let Some((until, at, on_leave)) = card_until {
                    let (cx, cy) = super::cursor_pos();
                    let moved = on_leave && (cx - at.0).pow(2) + (cy - at.1).pow(2) > LEAVE_PX * LEAVE_PX;
                    if until.is_some_and(|t| now >= t) || moved {
                        card.hide();
                        card_until = None;
                    }
                }
                if toast_until.is_some_and(|t| now >= t) {
                    toast.hide();
                    toast_until = None;
                }
                while let Ok(e) = MenuEvent::receiver().try_recv() {
                    if e.id == *settings.id() {
                        actions.open_settings();
                    } else if e.id == *pause.id() {
                        actions.set_paused(pause.is_checked());
                    } else if e.id == *quit.id() {
                        PostQuitMessage(0);
                    }
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
