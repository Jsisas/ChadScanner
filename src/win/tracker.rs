//! The TarkovTracker window: a plain top-level Win32 window hosting a WebView2 (via wry) on the UI
//! thread, whose message loop pumps WebView2. The page is driven by `crate::tracker`'s script.
use crate::config::{write_atomic, Paths};
use crate::needs::Target;
use crate::tracker;
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::num::NonZeroIsize;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromRect, MONITOR_DEFAULTTONULL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetWindowRect, IsIconic, IsWindowVisible, LoadCursorW, LoadIconW,
    PostMessageW, RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, CW_USEDEFAULT, IDC_ARROW,
    SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE, WM_APP, WM_CLOSE, WM_DPICHANGED,
    WM_SIZE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

/// Posted to the window instead of destroying it when X is pressed; the UI loop hides it.
pub const WM_TRACKER_CLOSE: u32 = WM_APP + 1;
/// Posted on every size change; the UI loop re-fits both webviews (they are plain children, not auto-resized).
pub const WM_TRACKER_RESIZE: u32 = WM_APP + 2;

/// What the window shows: the TarkovTracker page or the raid map.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Tracker,
    Map,
}

/// The window's client area as a webview rectangle, in physical pixels.
fn client_rect(hwnd: HWND) -> wry::Rect {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    wry::Rect {
        position: wry::dpi::PhysicalPosition::new(0, 0).into(),
        size: wry::dpi::PhysicalSize::new((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32).into(),
    }
}

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
        WM_SIZE => {
            let _ = PostMessageW(h, WM_TRACKER_RESIZE, WPARAM(0), LPARAM(0));
            DefWindowProcW(h, m, w, l)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

pub struct Tracker {
    hwnd: HWND,
    webview: wry::WebView,
    /// The raid map page; a sibling child of `webview`, shown instead of it while `view` is `Map`.
    map: wry::WebView,
    _context: wry::WebContext,
    messages: Rc<RefCell<Vec<String>>>,
    /// Id and name of the last scanned item.
    last: Option<(String, String)>,
    paths: Paths,
    /// Mirrors "the window is shown"; read by the key worker thread.
    open: Arc<AtomicBool>,
    /// ChadScanner's game mode key; the site is switched to it before every action, since a freshly loaded page
    /// misses the push made while its store was not ready.
    mode: String,
    in_raid: bool,
    view: View,
    /// Set once `map.html`'s script has run; scripts for the map wait in `map_pending` until then.
    map_ready: Cell<bool>,
    map_pending: RefCell<Vec<String>>,
}

impl Tracker {
    pub fn create(paths: &Paths, open: Arc<AtomicBool>, mode: &str) -> anyhow::Result<Tracker> {
        let hwnd = unsafe { create_window(paths)? };
        let mut context = wry::WebContext::new(Some(paths.webview()));
        let messages = Rc::new(RefCell::new(Vec::new()));
        let sink = messages.clone();
        // Both are children, not `build()`: that subclasses the parent for auto-resize under one fixed ID,
        // which two webviews on one window would fight over. `fit` resizes them instead.
        let bounds = client_rect(hwnd);
        let webview = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_url(tracker::START_URL)
            .with_initialization_script_for_main_only(tracker::script(), true)
            .with_ipc_handler(move |req| sink.borrow_mut().push(req.into_body()))
            // The OAuth login (Supabase) opens a popup that postMessages its opener; wry denies popups by default.
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Allow)
            .with_bounds(bounds)
            .build_as_child(&Handle(hwnd))?;
        // `with_html` pages have an opaque origin: no localStorage, so the page keeps its layer options in memory.
        let map_sink = messages.clone();
        let map = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_html(crate::map::page())
            .with_visible(false)
            .with_ipc_handler(move |req| map_sink.borrow_mut().push(req.into_body()))
            .with_bounds(bounds)
            .build_as_child(&Handle(hwnd))?;
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            open.store(true, Ordering::Relaxed);
        }
        Ok(Tracker {
            hwnd,
            webview,
            map,
            _context: context,
            messages,
            last: None,
            paths: paths.clone(),
            open,
            mode: mode.to_owned(),
            in_raid: false,
            view: View::Tracker,
            map_ready: Cell::new(false),
            map_pending: RefCell::new(Vec::new()),
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
        unsafe {
            let _ = ShowWindow(self.hwnd, if IsIconic(self.hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.apply_view();
        self.open.store(true, Ordering::Relaxed);
        if self.view == View::Tracker {
            if let Some((id, name)) = &self.last {
                self.eval(&tracker::mode_script(&self.mode));
                self.eval(&tracker::open_script(id, name));
            }
        }
    }

    pub fn hide(&mut self) {
        self.open.store(false, Ordering::Relaxed);
        self.save_rect();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.apply_view();
    }

    /// Shows only the webview of the current view, and none while the window is hidden.
    fn apply_view(&self) {
        let shown = self.visible();
        for (name, wv, v) in [("tracker", &self.webview, View::Tracker), ("map", &self.map, View::Map)] {
            if let Err(e) = wv.set_visible(shown && self.view == v) {
                log::warn!("tracker: switching {name} webview: {e}");
            }
        }
    }

    fn set_view(&mut self, v: View) {
        if self.view != v {
            self.view = v;
            self.apply_view();
        }
    }

    /// Re-fits both webviews to the client area.
    pub fn fit(&self) {
        let bounds = client_rect(self.hwnd);
        for wv in [&self.webview, &self.map] {
            if let Err(e) = wv.set_bounds(bounds) {
                log::warn!("tracker: resizing webview: {e}");
            }
        }
    }

    /// A raid started: show its map. The window is shown without taking focus, so EFT keeps it.
    pub fn raid_map(&mut self, json: &str) {
        self.in_raid = true;
        self.eval_map(&format!("window.__map&&__map.show({json})"));
        self.set_view(View::Map);
        if !self.visible() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.open.store(true, Ordering::Relaxed);
            self.apply_view();
        }
    }

    /// The extracts the player's own panel lists; the map narrows to them.
    pub fn raid_extracts(&self, names: &[String]) {
        let json = serde_json::to_string(names).expect("strings serialise");
        self.eval_map(&format!("window.__map&&__map.only({json})"));
    }

    /// The raid is over: back to TarkovTracker.
    pub fn raid_end(&mut self) {
        self.in_raid = false;
        self.set_view(View::Tracker);
    }

    /// The scan card closed: back to the map when in raid.
    pub fn card_closed(&mut self) {
        if self.in_raid {
            self.set_view(View::Map);
        }
    }

    /// A scan was accepted: remember the item; open it when the window is shown. In raid the item page replaces the map.
    pub fn item(&mut self, id: &str, name: &str) {
        self.last = Some((id.to_owned(), name.to_owned()));
        if self.in_raid {
            self.set_view(View::Tracker);
        }
        if !self.visible() {
            return;
        }
        self.eval(&tracker::mode_script(&self.mode));
        self.eval(&tracker::open_script(id, name));
    }

    /// ChadScanner's game mode changed: switch the site to `m`.
    pub fn mode(&mut self, m: &str) {
        self.mode = m.to_owned();
        self.eval(&tracker::mode_script(m));
    }

    /// One Arrow key press for item `id`; the script writes the chosen count into the site's progress store.
    pub fn step(&self, id: &str, name: &str, up: bool, targets: &[Target]) {
        self.eval(&tracker::mode_script(&self.mode));
        self.eval(&tracker::step_script(id, name, up, targets));
    }

    /// Chad Fill for item `id`; the script sets each of `targets` to its `counts` entry in the site's progress store.
    pub fn fill(&self, id: &str, name: &str, targets: &[Target], counts: &[u32], fir_left: u32, non_fir_left: u32) {
        self.eval(&tracker::mode_script(&self.mode));
        self.eval(&tracker::fill_script(id, name, targets, counts, fir_left, non_fir_left));
    }

    /// Messages the page posted since the last call (the map page's ready signal is handled here).
    pub fn drain(&self) -> Vec<String> {
        let mut all = std::mem::take(&mut *self.messages.borrow_mut());
        if all.iter().any(|m| m == crate::map::READY) {
            all.retain(|m| m != crate::map::READY);
            self.map_ready.set(true);
            for js in self.map_pending.take() {
                self.eval_map_now(&js);
            }
        }
        all
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

    fn eval_map(&self, js: &str) {
        if self.map_ready.get() {
            self.eval_map_now(js);
        } else {
            self.map_pending.borrow_mut().push(js.to_owned());
        }
    }

    fn eval_map_now(&self, js: &str) {
        if let Err(e) = self.map.evaluate_script(js) {
            log::warn!("tracker: map evaluate_script: {e}");
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
