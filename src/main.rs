#![cfg_attr(windows, windows_subsystem = "windows")]
// Off Windows only the portable modules build (for tests); their consumers are Windows-only.
#![cfg_attr(not(windows), allow(dead_code))]
//! ChadScanner: item info for Escape from Tarkov as a floating tooltip.

mod card;
mod config;
mod data;
mod eftlog;
mod needs;
mod progress;
mod secret;
mod tracker;
mod update;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod settings;
#[cfg(windows)]
mod win;

#[cfg(not(windows))]
fn main() {
    eprintln!("ChadScanner runs on Windows only.");
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
fn load_fonts() -> anyhow::Result<card::Fonts> {
    let load = |names: &[&str]| {
        names.iter().find_map(|n| std::fs::read(std::path::Path::new(r"C:\Windows\Fonts").join(n)).ok()).map(ab_glyph::FontArc::try_from_vec)
    };
    let bold = load(&["segoeuib.ttf", "arialbd.ttf"])
        .ok_or_else(|| anyhow::anyhow!(r"neither C:\Windows\Fonts\segoeuib.ttf nor arialbd.ttf found"))??;
    // Regular weight is optional; bold stands in.
    let regular = load(&["segoeui.ttf", "arial.ttf"]).and_then(Result::ok).unwrap_or_else(|| bold.clone());
    Ok(card::Fonts { bold, regular })
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
    let after_update = std::env::args().any(|a| a == "--after-update");
    // After an update the old process may still be exiting; wait for its mutex.
    let wait = if after_update { std::time::Duration::from_secs(15) } else { std::time::Duration::ZERO };
    if !single_instance(wait) {
        log::warn!("ChadScanner is already running");
        return Ok(());
    }
    log::info!("ChadScanner {} starting; data {}", env!("CARGO_PKG_VERSION"), paths.data.display());
    let finished = update::finish(&paths.install);

    let font = load_fonts()?;
    let cfg = config::Config::load(&paths.config());
    let (ui, ui_rx) = win::ui::Ui::new();
    let app = app::App::new(paths.clone(), cfg.clone(), ui.clone(), font);
    match finished {
        Ok(true) => app.toast(&format!("Updated to v{}", env!("CARGO_PKG_VERSION")), "", card::GREEN),
        Ok(false) => {}
        Err(e) => {
            log::error!("finishing update: {e:#}");
            app.toast("Update incomplete", "see log.txt", card::RED);
        }
    }

    let loader = app.clone();
    std::thread::Builder::new().name("engine".into()).spawn(move || {
        if !loader.paths.data.join("refs").join("catalog.json").exists() {
            loader.toast("Downloading item data", "one-time, about 180 MB", card::AMBER);
            if let Err(e) = update::agent().and_then(|a| update::install_data(&a, &loader.paths.install)) {
                log::error!("item data download: {e:#}");
                loader.toast("Item data download failed", "see log.txt – restart to retry", card::RED);
                return;
            }
        }
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
    let (step_tx, step_rx) = std::sync::mpsc::channel();
    let steps = app.clone();
    std::thread::Builder::new().name("steps".into()).spawn(move || steps.step_worker(step_rx))?;
    let upd = app.clone();
    std::thread::Builder::new().name("update".into()).spawn(move || upd.update_worker())?;
    win::hook::start(scan_tx, step_tx, cfg.hotkey, cfg.progress_keys);

    let actions: Arc<app::App> = app;
    win::ui::run(&ui, ui_rx, &actions, &paths)?;
    actions.shutdown();
    log::info!("quit");
    Ok(())
}

/// One overlay per user session: a second one would double the hook and the tooltips.
/// Retries for `wait` (the previous process may still be exiting).
#[cfg(windows)]
fn single_instance(wait: std::time::Duration) -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let start = std::time::Instant::now();
    loop {
        unsafe {
            let handle = CreateMutexW(None, true, w!("Local\\ChadScanner"));
            if GetLastError() != ERROR_ALREADY_EXISTS {
                // Leaked on purpose: held for the life of the process.
                return true;
            }
            if let Ok(h) = handle {
                let _ = CloseHandle(h);
            }
        }
        if start.elapsed() >= wait {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
