#![cfg_attr(windows, windows_subsystem = "windows")]
// Off Windows only the portable modules build (for tests); their consumers are Windows-only.
#![cfg_attr(not(windows), allow(dead_code))]
//! ChadOverlay: item info for Escape from Tarkov as a floating tooltip.

mod card;
mod config;
mod data;
mod eftlog;
mod needs;
mod progress;
mod secret;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod settings;
#[cfg(windows)]
mod win;

#[cfg(not(windows))]
fn main() {
    eprintln!("ChadOverlay runs on Windows only.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(e) = run() {
        log::error!("fatal: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn load_font() -> anyhow::Result<ab_glyph::FontArc> {
    for name in ["segoeuib.ttf", "arialbd.ttf"] {
        let path = std::path::Path::new(r"C:\Windows\Fonts").join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(ab_glyph::FontArc::try_from_vec(bytes)?);
        }
    }
    anyhow::bail!(r"neither C:\Windows\Fonts\segoeuib.ttf nor arialbd.ttf found")
}

#[cfg(windows)]
fn run() -> anyhow::Result<()> {
    use std::sync::Arc;
    use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let paths = config::Paths::detect()?;
    let settings_mode = std::env::args().any(|a| a == "--settings");
    let log_file = std::fs::OpenOptions::new().create(true).append(true).open(paths.log())?;
    simplelog::WriteLogger::init(log::LevelFilter::Info, simplelog::Config::default(), log_file)?;
    if settings_mode {
        return settings::run(paths);
    }
    if !single_instance() {
        log::warn!("ChadOverlay is already running");
        return Ok(());
    }
    log::info!("ChadOverlay {} starting; data {}", env!("CARGO_PKG_VERSION"), paths.data.display());

    let font = load_font()?;
    let cfg = config::Config::load(&paths.config());
    let (ui, ui_rx) = win::ui::Ui::new();
    let app = app::App::new(paths.clone(), cfg.clone(), ui.clone(), font);

    let loader = app.clone();
    std::thread::Builder::new().name("engine".into()).spawn(move || {
        let t = std::time::Instant::now();
        match chadvision::engine::Engine::open(&loader.paths.data) {
            Ok(e) => {
                let _ = loader.engine.set(e);
                log::info!("recognition ready in {} ms", t.elapsed().as_millis());
            }
            Err(e) => {
                log::error!("recognition failed to load: {e:#}");
                loader.toast("Recognition failed to load", "see log.txt", card::RED);
            }
        }
    })?;

    let (scan_tx, scan_rx) = std::sync::mpsc::channel();
    let worker = app.clone();
    std::thread::Builder::new().name("scan".into()).spawn(move || worker.scan_worker(scan_rx))?;
    let logs = app.clone();
    std::thread::Builder::new().name("eft-logs".into()).spawn(move || logs.log_worker())?;
    let watch = app.clone();
    std::thread::Builder::new().name("config".into()).spawn(move || watch.config_worker())?;
    win::hook::start(scan_tx, cfg.hotkey);

    let actions: Arc<app::App> = app;
    win::ui::run(&ui, ui_rx, &actions)?;
    actions.shutdown();
    log::info!("quit");
    Ok(())
}

/// One overlay per user session: a second one would double the hook and the tooltips.
#[cfg(windows)]
fn single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        // Leaked on purpose: held for the life of the process.
        let _ = CreateMutexW(None, true, w!("Local\\ChadOverlay"));
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}
