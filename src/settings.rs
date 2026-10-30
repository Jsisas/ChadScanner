//! Settings window (`chadscanner.exe --settings`), a separate process started from the tray.
//! Every change is saved to `config.json` at once; the running overlay picks it up within a second.
use crate::config::{Config, Goal, Hotkey, Mode, Paths, Source};
use crate::data::{self, Catalog};
use crate::progress::{LocalSource, ProgressSource, TrackerSource};
use eframe::egui::{self, RichText};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Card,
    Progress,
    Hideout,
    Scanning,
    Logs,
    About,
}

impl Page {
    const ALL: [Page; 6] = [Page::Card, Page::Progress, Page::Hideout, Page::Scanning, Page::Logs, Page::About];

    fn label(self) -> &'static str {
        match self {
            Page::Card => "Card",
            Page::Progress => "Progress",
            Page::Hideout => "Hideout",
            Page::Scanning => "Scanning",
            Page::Logs => "EFT logs",
            Page::About => "About",
        }
    }
}

struct SettingsApp {
    paths: Paths,
    page: Page,
    cfg: Config,
    /// What is on disk, to save only real changes (token encryption differs on every call).
    saved: Config,
    tokens: [String; 3],
    saved_tokens: [String; 3],
    token_status: [String; 3],
    local: LocalSource,
    catalog: Option<Catalog>,
    level: u8,
    faction: String,
    status: String,
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().weak());
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong().size(15.0));
        ui.add_space(4.0);
        body(ui);
    });
}

impl SettingsApp {
    fn new(paths: Paths) -> SettingsApp {
        let cfg = Config::load(&paths.config());
        let tokens = Mode::ALL.map(|m| cfg.token(m).unwrap_or_default());
        let mut app = SettingsApp {
            local: LocalSource::open(paths.progress(), cfg.mode),
            catalog: data::load_cached(&paths.cache(), cfg.mode),
            paths,
            page: Page::Card,
            saved: cfg.clone(),
            cfg,
            saved_tokens: tokens.clone(),
            tokens,
            token_status: Default::default(),
            level: 1,
            faction: String::new(),
            status: String::new(),
        };
        app.load_profile();
        app
    }

    fn load_profile(&mut self) {
        let p = self.local.snapshot();
        self.level = p.level.unwrap_or(1);
        self.faction = p.faction.clone().unwrap_or_default();
    }

    fn switch_mode(&mut self, mode: Mode) {
        self.cfg.mode = mode;
        self.local = LocalSource::open(self.paths.progress(), mode);
        self.catalog = data::load_cached(&self.paths.cache(), mode);
        self.load_profile();
    }

    fn mode_index(&self) -> usize {
        Mode::ALL.iter().position(|m| *m == self.cfg.mode).unwrap_or(0)
    }

    /// Writes config.json when anything changed since the last save.
    fn autosave(&mut self) {
        if self.cfg == self.saved && self.tokens == self.saved_tokens {
            return;
        }
        if self.tokens != self.saved_tokens {
            for (i, m) in Mode::ALL.into_iter().enumerate() {
                if let Err(e) = self.cfg.set_token(m, &self.tokens[i]) {
                    self.status = format!("Token encryption failed: {e:#}");
                    return;
                }
            }
        }
        self.status = match self.cfg.save(&self.paths.config()) {
            Ok(()) => {
                self.saved = self.cfg.clone();
                self.saved_tokens = self.tokens.clone();
                "Saved – the overlay uses it right away".into()
            }
            Err(e) => format!("Save failed: {e:#}"),
        };
    }

    fn tracker(&self) -> Option<Arc<TrackerSource>> {
        let token = self.tokens[self.mode_index()].trim().to_owned();
        if token.is_empty() {
            return None;
        }
        TrackerSource::start(self.cfg.mode, token, self.paths.pending(), Arc::new(|_: &str, _: &str| {})).ok()
    }

    // ---------------------------------------------------------------- pages

    fn card_page(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        section(ui, "What the card shows", |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut c.show_quests, "Quests");
                ui.checkbox(&mut c.show_hideout, "Hideout upgrades");
                ui.checkbox(&mut c.show_barters, "Barters");
                ui.checkbox(&mut c.show_crafts, "Crafts");
            });
            ui.checkbox(&mut c.show_non_fir, "Include needs that don't have to be found in raid");
            hint(ui, "Off: only FIR needs are counted and listed.");
            ui.horizontal(|ui| {
                ui.label("Rows");
                ui.add(egui::Slider::new(&mut c.max_rows, 0..=15));
            });
            hint(ui, "Quest and hideout rows on the card; the rest is summed up as \"+n× in k more\".");
        });

        section(ui, "Order", |ui| {
            ui.radio_value(&mut c.smart_ordering, true, "Smart (recommended)");
            hint(ui, "Closest first: what you can do now, then the fewest quests, upgrades or levels away.");
            ui.radio_value(&mut c.smart_ordering, false, "Simple");
            hint(ui, "Biggest count first.");
        });

        section(ui, "Goal", |ui| {
            ui.horizontal(|ui| {
                for g in Goal::ALL {
                    ui.radio_value(&mut c.goal, g, g.label());
                }
            });
            hint(ui, "Kappa / Lightkeeper: other quests move to the bottom under \"NOT KAPPA\".");
        });

        section(ui, "Look and behaviour", |ui| {
            ui.horizontal(|ui| {
                ui.label("Size");
                ui.add(egui::Slider::new(&mut c.ui_scale, 0.75..=2.0).fixed_decimals(2).suffix("×"));
            });
            ui.checkbox(&mut c.close_on_leave, "Close when the mouse leaves the item");
            ui.horizontal(|ui| {
                ui.label("Close after");
                ui.add(egui::Slider::new(&mut c.tooltip_seconds, 0.0..=30.0).fixed_decimals(0).suffix(" s"));
            });
            hint(ui, "0 s = no timeout; the next scan replaces the card anyway.");
        });
    }

    fn progress_page(&mut self, ui: &mut egui::Ui) {
        section(ui, "Where your progress is kept", |ui| {
            ui.radio_value(&mut self.cfg.source, Source::Local, "On this PC");
            hint(ui, "A local file in TarkovTracker backup format. Finished tasks are filled in from EFT's logs; set level, faction and hideout here.");
            ui.radio_value(&mut self.cfg.source, Source::Tracker, "TarkovTracker.org");
            hint(ui, "Reads and writes your tarkovtracker.org progress with an API token. Items you mark there lower the counts on the card.");
        });

        if self.cfg.source == Source::Tracker {
            section(ui, "TarkovTracker tokens", |ui| {
                ui.horizontal(|ui| {
                    hint(ui, "Create one per game mode at");
                    ui.hyperlink_to(RichText::new("tarkovtracker.org > Settings > API").small(), "https://tarkovtracker.org/settings");
                    hint(ui, "(needs read + write).");
                });
                egui::Grid::new("tokens").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                    for (i, m) in Mode::ALL.into_iter().enumerate() {
                        let label = if m == self.cfg.mode { RichText::new(m.label()).strong() } else { RichText::new(m.label()) };
                        ui.label(label);
                        ui.add(egui::TextEdit::singleline(&mut self.tokens[i]).password(true).desired_width(240.0).hint_text("paste token"));
                        if ui.add_enabled(!self.tokens[i].trim().is_empty(), egui::Button::new("Test")).clicked() {
                            self.token_status[i] = match crate::progress::test_token(self.tokens[i].trim()) {
                                Ok((perms, mode)) => format!("OK: {mode}, {}", perms.join(" ")),
                                Err(e) => format!("Failed: {e}"),
                            };
                        }
                        ui.label(&self.token_status[i]);
                        ui.end_row();
                    }
                });
                hint(ui, "Only the token of the current game mode is used. Tokens are stored encrypted for your Windows user.");
            });
        }

        section(ui, "Player", |ui| {
            let local = self.cfg.source == Source::Local;
            ui.horizontal(|ui| {
                ui.label("Level");
                let changed = ui.add(egui::DragValue::new(&mut self.level).range(1..=79)).changed();
                if local && changed {
                    self.status = match self.local.set_level(self.level) {
                        Ok(()) => format!("Level {} saved", self.level),
                        Err(e) => format!("{e:#}"),
                    };
                }
                if !local && ui.button("Send to TarkovTracker").clicked() {
                    self.status = match self.tracker() {
                        Some(t) => t.set_level(self.level).map(|()| format!("Level {} sent", self.level)).unwrap_or_else(|e| format!("{e:#}")),
                        None => format!("No token for {}", self.cfg.mode.label()),
                    };
                }
            });
            ui.horizontal(|ui| {
                ui.label("Faction");
                if local {
                    for f in ["USEC", "BEAR"] {
                        if ui.radio(self.faction == f, f).clicked() && self.faction != f {
                            self.faction = f.into();
                            self.status = match self.local.set_faction(Some(f)) {
                                Ok(()) => format!("Faction {f} saved"),
                                Err(e) => format!("{e:#}"),
                            };
                        }
                    }
                } else {
                    hint(ui, "read from TarkovTracker");
                }
            });
            hint(ui, "Level and faction decide which quests you can take now.");
        });

        if self.cfg.source == Source::Local {
            section(ui, "Backup", |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Import TarkovTracker backup…").clicked() {
                        if let Some(file) = rfd::FileDialog::new().add_filter("TarkovTracker backup", &["json"]).pick_file() {
                            self.status = match self.local.import(&file) {
                                Ok(()) => {
                                    self.load_profile();
                                    format!("Imported {}", file.display())
                                }
                                Err(e) => format!("Import failed: {e:#}"),
                            };
                        }
                    }
                    if ui.button("Show progress file").clicked() {
                        let _ = std::process::Command::new("explorer.exe").arg("/select,").arg(self.paths.progress()).spawn();
                    }
                });
                hint(ui, "progress.json can be imported at tarkovtracker.org > Settings > Import.");
            });
        }
    }

    fn hideout_page(&mut self, ui: &mut egui::Ui) {
        if self.cfg.source != Source::Local {
            section(ui, "Hideout", |ui| {
                ui.label("Your hideout comes from TarkovTracker. Its API cannot change the hideout, so edit it on tarkovtracker.org.");
            });
            return;
        }
        let Some(cat) = &self.catalog else {
            section(ui, "Hideout", |ui| {
                ui.label("No item data yet. Start the overlay once with internet access.");
            });
            return;
        };
        let built = self.local.snapshot().hideout_modules.clone();
        let mut change = None;
        section(ui, "Built levels", |ui| {
            hint(ui, "EFT's logs don't record upgrades, so set them here. Built levels stop counting on the card.");
            ui.add_space(4.0);
            egui::Grid::new("hideout").striped(true).num_columns(3).spacing([16.0, 4.0]).show(ui, |ui| {
                for s in &cat.stations {
                    let max = s.levels.iter().map(|l| l.level).max().unwrap_or(0);
                    let mut level = s.levels.iter().filter(|l| built.contains(&l.id)).map(|l| l.level).max().unwrap_or(0);
                    ui.label(&s.name);
                    if ui.add(egui::Slider::new(&mut level, 0..=max).integer()).changed() {
                        change = Some((s.id.clone(), level));
                    }
                    ui.label(if level == max { RichText::new("max").weak() } else { RichText::new("") });
                    ui.end_row();
                }
            });
        });
        if let Some((id, level)) = change {
            if let Err(e) = self.local.set_hideout_level(&id, level) {
                self.status = format!("{e:#}");
            }
        }
    }

    fn scanning_page(&mut self, ui: &mut egui::Ui) {
        section(ui, "Scan button", |ui| {
            egui::ComboBox::from_id_salt("hotkey").selected_text(self.cfg.hotkey.label()).show_ui(ui, |ui| {
                for h in Hotkey::ALL {
                    ui.selectable_value(&mut self.cfg.hotkey, h, h.label());
                }
            });
            hint(ui, "Works only while Escape from Tarkov is the active window. The click still reaches the game.");
        });
        section(ui, "Game", |ui| {
            hint(ui, "Run EFT in Borderless mode; the card cannot be drawn over exclusive fullscreen. Pause scanning from the tray icon.");
        });
    }

    fn logs_page(&mut self, ui: &mut egui::Ui) {
        section(ui, "Quest tracking from EFT logs", |ui| {
            ui.checkbox(&mut self.cfg.log_tracking, "Mark tasks completed / failed and switch game mode automatically");
            hint(ui, "Tasks finished while the overlay was closed are caught up at the next start.");
        });
        section(ui, "Logs folder", |ui| {
            match crate::eftlog::default_logs_dir(&self.cfg.logs_override) {
                Some(dir) => ui.label(format!("Using {}", dir.display())),
                None => ui.colored_label(egui::Color32::LIGHT_RED, "Not found – choose the Logs folder in your EFT install."),
            };
            ui.horizontal(|ui| {
                if ui.button("Choose folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.cfg.logs_override = dir.display().to_string();
                    }
                }
                if !self.cfg.logs_override.is_empty() && ui.button("Detect automatically").clicked() {
                    self.cfg.logs_override.clear();
                }
            });
        });
    }

    fn about_page(&mut self, ui: &mut egui::Ui) {
        section(ui, "ChadScanner", |ui| {
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            hint(ui, "Item data: json.tarkov.dev · Recognition: ChadVision");
            ui.horizontal(|ui| {
                if ui.button("Open log file").clicked() {
                    let _ = std::process::Command::new("notepad.exe").arg(self.paths.log()).spawn();
                }
                if ui.button("Open settings folder").clicked() {
                    let _ = std::process::Command::new("explorer.exe").arg(&self.paths.appdata).spawn();
                }
            });
        });
        section(ui, "Debug report", |ui| {
            ui.label("Press Ctrl + Shift + F12 right after a scan that went wrong.");
            hint(ui, "Saves the scan's screenshot, the candidates and why it was accepted or rejected, the screen now and the log tail.");
            if ui.button("Open reports folder").clicked() {
                let _ = std::fs::create_dir_all(self.paths.reports());
                let _ = std::process::Command::new("explorer.exe").arg(self.paths.reports()).spawn();
            }
        });
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("mode").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Game mode").strong());
                for m in Mode::ALL {
                    if ui.selectable_label(self.cfg.mode == m, m.label()).clicked() && self.cfg.mode != m {
                        self.switch_mode(m);
                    }
                }
                hint(ui, "switches by itself when EFT starts a session");
            });
            ui.add_space(6.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(&self.status).weak());
            ui.add_space(4.0);
        });
        egui::Panel::left("nav").resizable(false).exact_size(120.0).show(ui, |ui| {
            ui.add_space(8.0);
            for p in Page::ALL {
                let text = RichText::new(p.label()).size(15.0);
                if ui.add_sized([108.0, 28.0], egui::Button::selectable(self.page == p, text)).clicked() {
                    self.page = p;
                }
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                Page::Card => self.card_page(ui),
                Page::Progress => self.progress_page(ui),
                Page::Hideout => self.hideout_page(ui),
                Page::Scanning => self.scanning_page(ui),
                Page::Logs => self.logs_page(ui),
                Page::About => self.about_page(ui),
            });
        });
        self.autosave();
    }
}

/// The exe icon (`assets/icon.svg`) for the title bar and taskbar.
fn window_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/icon-64.png")).expect("bundled icon").to_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData { rgba: img.into_raw(), width, height }
}

pub fn run(paths: Paths) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ChadScanner Settings")
            .with_inner_size([720.0, 640.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native("ChadScanner Settings", options, Box::new(|_cc| Ok(Box::new(SettingsApp::new(paths)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}
