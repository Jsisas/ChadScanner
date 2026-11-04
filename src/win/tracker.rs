//! The TarkovTracker window: a plain top-level Win32 window hosting a WebView2 (via wry) on the UI
//! thread, whose message loop pumps WebView2. The page is driven by `crate::tracker`'s script.
use crate::config::{write_atomic, Paths};
use crate::needs::Target;
use crate::tracker;
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::num::NonZeroIsize;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromRect, MONITOR_DEFAULTTONULL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetWindowRect, IsIconic, IsWindowVisible, LoadCursorW, LoadIconW, PostMessageW,
    RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, CW_USEDEFAULT, IDC_ARROW, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE, WM_APP, WM_CLOSE, WM_DPICHANGED, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};

/// Posted to the window instead of destroying it when X is pressed; the UI loop hides it.
pub const WM_TRACKER_CLOSE: u32 = WM_APP + 1;

#[derive(Serialize, Deserialize, Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

struct Handle(HWND);

impl HasWindowHandle for Handle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let h = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: the window outlives the WebView (both are owned by `Tracker`).
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(h))) })
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_CLOSE => {
            let _ = PostMessageW(h, WM_TRACKER_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = &*(l.0 as *const RECT);
            let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

pub struct Tracker {
    hwnd: HWND,
    webview: wry::WebView,
    _context: wry::WebContext,
    messages: Rc<RefCell<Vec<String>>>,
    /// Id and name of the last scanned item.
    last: Option<(String, String)>,
    paths: Paths,
    /// Mirrors "the window is shown"; read by the key worker thread.
    open: Arc<AtomicBool>,
}

impl Tracker {
    pub fn create(paths: &Paths, open: Arc<AtomicBool>) -> anyhow::Result<Tracker> {
        let hwnd = unsafe { create_window(paths)? };
        let mut context = wry::WebContext::new(Some(paths.webview()));
        let messages = Rc::new(RefCell::new(Vec::new()));
        let sink = messages.clone();
        let webview = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_url(tracker::START_URL)
            .with_initialization_script_for_main_only(tracker::script(), true)
            .with_ipc_handler(move |req| sink.borrow_mut().push(req.into_body()))
            // The OAuth login (Supabase) opens a popup that postMessages its opener; wry denies popups by default.
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Allow)
            .build(&Handle(hwnd))?;
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            open.store(true, Ordering::Relaxed);
        }
        Ok(Tracker {
            hwnd,
            webview,
            _context: context,
            messages,
            last: None,
            paths: paths.clone(),
            open,
        })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// From the tray: shows the window with the last scanned item open.
    pub fn show(&self) {
        if let Err(e) = self.webview.set_visible(true) {
            log::warn!("tracker: showing webview: {e}");
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, if IsIconic(self.hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.open.store(true, Ordering::Relaxed);
        if let Some((id, name)) = &self.last {
            self.eval(&tracker::open_script(id, name));
        }
    }

    pub fn hide(&mut self) {
        self.open.store(false, Ordering::Relaxed);
        self.save_rect();
        if let Err(e) = self.webview.set_visible(false) {
            log::warn!("tracker: hiding webview: {e}");
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// A scan was accepted: remember the item; open it when the window is shown.
    pub fn item(&mut self, id: &str, name: &str) {
        self.last = Some((id.to_owned(), name.to_owned()));
        if !self.visible() {
            return;
        }
        self.eval(&tracker::open_script(id, name));
    }

    /// One Arrow key press for item `id`; the script writes the chosen count into the site's progress store.
    pub fn step(&self, id: &str, name: &str, up: bool, targets: &[Target]) {
        self.eval(&tracker::step_script(id, name, up, targets));
    }

    /// Chad Fill for item `id`; the script sets each of `targets` to its `counts` entry in the site's progress store.
    pub fn fill(&self, id: &str, name: &str, targets: &[Target], counts: &[u32], fir_left: u32, non_fir_left: u32) {
        self.eval(&tracker::fill_script(id, name, targets, counts, fir_left, non_fir_left));
    }

    /// Messages the page posted since the last call.
    pub fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.messages.borrow_mut())
    }

    pub fn save_rect(&self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                return;
            }
            let mut r = RECT::default();
            if GetWindowRect(self.hwnd, &mut r).is_err() {
                return;
            }
            let rect = Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top };
            let saved = serde_json::to_vec(&rect).map_err(anyhow::Error::from).and_then(|b| write_atomic(&self.paths.tracker_window(), &b));
            if let Err(e) = saved {
                log::warn!("saving tracker window position: {e:#}");
            }
        }
    }

    fn eval(&self, js: &str) {
        if let Err(e) = self.webview.evaluate_script(js) {
            log::warn!("tracker: evaluate_script: {e}");
        }
    }
}

/// The saved rect if it still lands on a monitor.
fn saved_rect(paths: &Paths) -> Option<Rect> {
    let r: Rect = serde_json::from_slice(&std::fs::read(paths.tracker_window()).ok()?).ok()?;
    let rc = RECT { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h };
    (r.w > 0 && r.h > 0 && !unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONULL) }.is_invalid()).then_some(r)
}

unsafe fn create_window(paths: &Paths) -> anyhow::Result<HWND> {
    let hinstance = GetModuleHandleW(None)?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance.into(),
        lpszClassName: w!("ChadScannerTracker"),
        // Resource 1 is the group icon embedded by build.rs.
        hIcon: LoadIconW(hinstance, PCWSTR(1 as _)).unwrap_or_default(),
        hCursor: LoadCursorW(None, IDC_ARROW)?,
        ..Default::default()
    };
    RegisterClassW(&class);
    let (x, y, w, h) = match saved_rect(paths) {
        Some(r) => (r.x, r.y, r.w, r.h),
        None => (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT),
    };
    Ok(CreateWindowExW(
        Default::default(),
        w!("ChadScannerTracker"),
        w!("ChadScanner · TarkovTracker"),
        WS_OVERLAPPEDWINDOW,
        x,
        y,
        w,
        h,
        None,
        None,
        hinstance,
        None,
    )?)
}
