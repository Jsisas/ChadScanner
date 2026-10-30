//! Shared app state and the worker threads: scan, EFT log tracking, config watch.
use crate::card::{self, Rgb};
use crate::config::{Config, Mode, Paths, Source};
use crate::data::Store;
use crate::eftlog::{self, Event, LogState, Tailer};
use crate::progress::{LocalSource, Notify, ProgressSource, TaskState, TrackerSource};
use crate::win::{self, hook::ScanRequest, ui};
use ab_glyph::FontArc;
use chadvision::engine::Engine;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime};

/// Top candidate must reach this score…
const ACCEPT: f32 = 0.55;
/// …and beat the runner-up by this much.
const MARGIN: f32 = 0.02;
const REJECT_SECONDS: f32 = 1.5;

pub struct App {
    pub paths: Paths,
    pub cfg: RwLock<Config>,
    store: RwLock<Arc<Store>>,
    progress: RwLock<Arc<dyn ProgressSource>>,
    /// Set when the source is local, for reloads after Settings edits.
    local: RwLock<Option<Arc<LocalSource>>>,
    pub engine: OnceLock<Engine>,
    ui: ui::Ui,
    font: FontArc,
    paused: AtomicBool,
    settings: Mutex<Option<Child>>,
    /// Serialises mode switches.
    switch: Mutex<()>,
}

impl App {
    pub fn new(paths: Paths, cfg: Config, ui: ui::Ui, font: FontArc) -> Arc<App> {
        let mode = cfg.mode;
        let app = Arc::new_cyclic(|weak: &std::sync::Weak<App>| {
            let w = weak.clone();
            let store = Store::start(mode, paths.cache(), move || {
                if let Some(a) = w.upgrade() {
                    a.toast("No item data yet – retrying", "json.tarkov.dev unreachable", card::AMBER);
                }
            });
            // Replaced by `rebuild_progress` below, which needs the finished `App` for its toasts.
            let initial: Arc<dyn ProgressSource> = Arc::new(LocalSource::open(paths.progress(), mode));
            App {
                paths,
                cfg: RwLock::new(cfg),
                store: RwLock::new(store),
                progress: RwLock::new(initial),
                local: RwLock::new(None),
                engine: OnceLock::new(),
                ui,
                font,
                paused: AtomicBool::new(false),
                settings: Mutex::new(None),
                switch: Mutex::new(()),
            }
        });
        app.rebuild_progress();
        app
    }

    fn notifier(self: &Arc<Self>) -> Notify {
        let w = Arc::downgrade(self);
        Arc::new(move |title: &str, body: &str| {
            if let Some(a) = w.upgrade() {
                a.toast(title, body, card::RED);
            }
        })
    }

    /// Progress source for the configured mode and source.
    fn rebuild_progress(self: &Arc<Self>) {
        let cfg = self.cfg.read().unwrap().clone();
        let (src, local): (Arc<dyn ProgressSource>, Option<Arc<LocalSource>>) = match (cfg.source, cfg.token(cfg.mode)) {
            (Source::Tracker, Some(token)) => {
                match TrackerSource::start(cfg.mode, token, self.paths.pending(), self.notifier()) {
                    Ok(t) => (t, None),
                    Err(e) => {
                        log::error!("TarkovTracker setup failed: {e:#}; using local progress");
                        let l = Arc::new(LocalSource::open(self.paths.progress(), cfg.mode));
                        (l.clone(), Some(l))
                    }
                }
            }
            (Source::Tracker, None) => {
                log::warn!("no TarkovTracker token for {}; using local progress", cfg.mode.label());
                self.toast("No TarkovTracker token", &format!("{} uses local progress", cfg.mode.label()), card::AMBER);
                let l = Arc::new(LocalSource::open(self.paths.progress(), cfg.mode));
                (l.clone(), Some(l))
            }
            (Source::Local, _) => {
                let l = Arc::new(LocalSource::open(self.paths.progress(), cfg.mode));
                (l.clone(), Some(l))
            }
        };
        *self.progress.write().unwrap() = src;
        *self.local.write().unwrap() = local;
        log::info!("progress: {:?} {}", cfg.source, cfg.mode.label());
    }

    fn progress(&self) -> Arc<dyn ProgressSource> {
        self.progress.read().unwrap().clone()
    }

    /// Applies a changed config; mode/source/token changes reload catalog and progress.
    fn apply_config(self: &Arc<Self>, new: Config) {
        let _g = self.switch.lock().unwrap();
        let old = self.cfg.read().unwrap().clone();
        if old == new {
            return;
        }
        *self.cfg.write().unwrap() = new.clone();
        win::hook::set_hotkey(new.hotkey);
        if old.mode != new.mode {
            let w = Arc::downgrade(self);
            *self.store.write().unwrap() = Store::start(new.mode, self.paths.cache(), move || {
                if let Some(a) = w.upgrade() {
                    a.toast("No item data yet – retrying", "json.tarkov.dev unreachable", card::AMBER);
                }
            });
        }
        if old.mode != new.mode || old.source != new.source || old.tokens != new.tokens {
            self.rebuild_progress();
        }
        log::info!("config applied");
    }

    fn set_mode(self: &Arc<Self>, mode: Mode, quiet: bool) {
        let mut cfg = self.cfg.read().unwrap().clone();
        if cfg.mode == mode {
            return;
        }
        cfg.mode = mode;
        if let Err(e) = cfg.save(&self.paths.config()) {
            log::warn!("saving config: {e:#}");
        }
        self.apply_config(cfg);
        log::info!("game mode → {}", mode.label());
        if !quiet {
            self.toast(&format!("Game mode → {}", mode.label()), "", card::TEXT);
        }
    }

    pub fn toast(&self, title: &str, body: &str, border: Rgb) {
        let scale = self.cfg.read().unwrap().ui_scale;
        let pix = card::render(&card::message(title, body, border), &self.font, scale);
        self.ui.send(ui::Cmd::Toast(pix));
    }

    /// `seconds`: `None` = no timeout.
    fn show_card(&self, c: &card::Card, at: (i32, i32), seconds: Option<f32>) {
        let (scale, close_on_leave) = {
            let c = self.cfg.read().unwrap();
            (c.ui_scale, c.close_on_leave)
        };
        self.ui.send(ui::Cmd::Card { pix: card::render(c, &self.font, scale), at, seconds, close_on_leave });
    }

    // ------------------------------------------------------------ scanning

    pub fn scan_worker(self: Arc<Self>, rx: Receiver<ScanRequest>) {
        for req in rx {
            if self.paused.load(Ordering::Relaxed) || !win::eft_in_foreground() {
                continue;
            }
            self.ui.send(ui::Cmd::HideCard);
            let started = std::time::Instant::now();
            if let Err(e) = self.scan(req) {
                log::warn!("scan failed: {e:#}");
            }
            log::info!("scan at ({},{}) took {} ms", req.x, req.y, started.elapsed().as_millis());
        }
    }

    fn scan(&self, req: ScanRequest) -> anyhow::Result<()> {
        let at = (req.x, req.y);
        let Some(engine) = self.engine.get() else {
            self.show_card(&card::message("…", "recognition still loading", card::GREY), at, Some(REJECT_SECONDS));
            return Ok(());
        };
        let Some(cat) = self.store.read().unwrap().get() else {
            self.show_card(&card::message("?", "no item data yet", card::GREY), at, Some(REJECT_SECONDS));
            return Ok(());
        };
        let shot = win::capture::around(req.x, req.y)?;
        let cands = engine.identify_at(&shot.image, shot.mouse, shot.scale, 5);
        log::info!("candidates: {:?}", cands.iter().take(3).collect::<Vec<_>>());
        let top = cands.first().map_or(0.0, |c| c.1);
        let second = cands.get(1).map_or(0.0, |c| c.1);
        let item = cands.first().filter(|_| top >= ACCEPT && top - second >= MARGIN).and_then(|(id, _)| cat.items.get(id));
        let Some(item) = item else {
            self.show_card(&card::message("?", "no confident match", card::GREY), at, Some(REJECT_SECONDS));
            return Ok(());
        };
        let (opts, seconds) = {
            let c = self.cfg.read().unwrap();
            (card::CardOptions::from_config(&c), c.tooltip_seconds)
        };
        let src = self.progress();
        // Picks up counts just marked on tarkovtracker.org (cheap: ETag, rate-limited).
        src.refresh_if_stale();
        let c = card::item_card(&cat, item, top, &src.snapshot(), &opts);
        self.show_card(&c, at, Some(seconds).filter(|s| *s > 0.0));
        Ok(())
    }

    // ------------------------------------------------------------ EFT logs

    fn task_name(&self, id: &str) -> String {
        self.store.read().unwrap().get().and_then(|c| c.tasks.get(id).map(|t| t.name.clone())).unwrap_or_else(|| id.to_owned())
    }

    fn apply_event(self: &Arc<Self>, e: &Event, quiet: bool) {
        let prefix = if quiet { "catch-up: " } else { "" };
        match e {
            Event::SessionMode(m) => self.set_mode(*m, quiet),
            Event::TaskStarted(id) => {
                log::info!("{prefix}task {id} started");
                if !quiet {
                    self.toast("Task started", &self.task_name(id), card::TEXT);
                }
            }
            Event::TaskFinished(id) => {
                let src = self.progress();
                if src.snapshot().tasks.get(id).is_some_and(|t| t.complete && !t.failed) {
                    return log::info!("{prefix}task {id} already complete");
                }
                if src.writes_objectives() {
                    if let Some(task) = self.store.read().unwrap().get().and_then(|c| c.tasks.get(id).cloned()) {
                        for o in task.objectives.iter().filter(|o| !o.optional && !o.id.is_empty()) {
                            if let Err(err) = src.set_objective(&o.id, true, o.count) {
                                log::warn!("objective {}: {err:#}", o.id);
                            }
                        }
                    }
                }
                match src.set_task(id, TaskState::Completed) {
                    Ok(()) => log::info!("{prefix}task {id} completed"),
                    Err(err) => log::warn!("{prefix}task {id} completed, but recording failed: {err:#}"),
                }
                if !quiet {
                    self.toast("Task completed", &self.task_name(id), card::GREEN);
                }
            }
            Event::TaskFailed(id) => {
                let src = self.progress();
                if src.snapshot().tasks.get(id).is_some_and(|t| t.failed) {
                    return;
                }
                match src.set_task(id, TaskState::Failed) {
                    Ok(()) => log::info!("{prefix}task {id} failed"),
                    Err(err) => log::warn!("{prefix}task {id} failed, but recording failed: {err:#}"),
                }
                if !quiet {
                    self.toast("Task failed", &self.task_name(id), card::RED);
                }
            }
            Event::FleaSold { item, count } => {
                if quiet {
                    return;
                }
                let name = item
                    .as_ref()
                    .and_then(|i| self.store.read().unwrap().get().and_then(|c| c.items.get(i).map(|x| x.short_name.clone())));
                match name {
                    Some(n) => self.toast("Flea sale", &format!("{}{n}", count.map(|c| format!("{c}× ")).unwrap_or_default()), card::GREEN),
                    None => self.toast("Flea item sold", "", card::GREEN),
                }
            }
        }
    }

    pub fn log_worker(self: Arc<Self>) {
        use notify::Watcher;
        // Waits until tracking is on and the folder exists (Settings may change either).
        let mut warned = false;
        let dir = loop {
            let (enabled, override_dir) = {
                let c = self.cfg.read().unwrap();
                (c.log_tracking, c.logs_override.clone())
            };
            if enabled {
                match eftlog::default_logs_dir(&override_dir) {
                    Some(d) => break d,
                    None if !warned => {
                        warned = true;
                        log::warn!("EFT logs folder not found; set it in Settings");
                    }
                    None => {}
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        };
        log::info!("tracking EFT logs in {}", dir.display());
        let state_path = self.paths.logstate();
        let (mut tailer, events) = Tailer::start(dir.clone(), LogState::load(&state_path));
        for e in &events {
            self.apply_event(e, true);
        }
        tailer.state.save(&state_path);

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |_| {
            let _ = tx.send(());
        })
        .ok();
        if let Some(w) = watcher.as_mut() {
            if let Err(e) = w.watch(&dir, notify::RecursiveMode::Recursive) {
                log::warn!("log watcher: {e}; polling only");
            }
        }
        loop {
            // Change notification or the 2 s poll fallback, whichever comes first.
            let _ = rx.recv_timeout(Duration::from_secs(2));
            while rx.try_recv().is_ok() {}
            if !self.cfg.read().unwrap().log_tracking {
                continue;
            }
            let before = tailer.state.clone();
            for e in tailer.poll() {
                self.apply_event(&e, false);
            }
            if tailer.state != before {
                tailer.state.save(&state_path);
            }
        }
    }

    // ------------------------------------------------------------ config watch

    pub fn config_worker(self: Arc<Self>) {
        let path = self.paths.config();
        let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let mut seen: Option<SystemTime> = mtime(&path);
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let m = mtime(&path);
            if m != seen {
                seen = m;
                self.apply_config(Config::load(&path));
            }
            if let Some(l) = self.local.read().unwrap().clone() {
                l.reload_if_changed();
            }
        }
    }
}

impl ui::TrayActions for Arc<App> {
    fn open_settings(&self) {
        let mut child = self.settings.lock().unwrap();
        if child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))) {
            return;
        }
        match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn()) {
            Ok(c) => *child = Some(c),
            Err(e) => log::error!("opening Settings: {e}"),
        }
    }

    fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
        log::info!("scanning {}", if paused { "paused" } else { "resumed" });
    }
}
