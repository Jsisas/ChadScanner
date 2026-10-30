//! Settings window (`chadoverlay.exe --settings`), a separate process started from the tray.
//! Saving writes `config.json`; the running overlay picks it up within a second.
use crate::config::{Config, Hotkey, Mode, Paths, Source};
use crate::data::{self, Catalog};
use crate::progress::{LocalSource, ProgressSource, TrackerSource};
use eframe::egui;
use std::sync::Arc;

struct SettingsApp {
    paths: Paths,
    cfg: Config,
    tokens: [String; 3],
    token_status: [String; 3],
    local: LocalSource,
    catalog: Option<Catalog>,
    level: u8,
    faction: String,
    status: String,
}

impl SettingsApp {
    fn new(paths: Paths) -> SettingsApp {
        let cfg = Config::load(&paths.config());
        let tokens = Mode::ALL.map(|m| cfg.token(m).unwrap_or_default());
        let mut app = SettingsApp {
            local: LocalSource::open(paths.progress(), cfg.mode),
            catalog: data::load_cached(&paths.cache(), cfg.mode),
            paths,
            cfg,
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

    fn save(&mut self) {
        for (i, m) in Mode::ALL.into_iter().enumerate() {
            if let Err(e) = self.cfg.set_token(m, &self.tokens[i]) {
                self.status = format!("Token encryption failed: {e:#}");
                return;
            }
        }
        self.status = match self.cfg.save(&self.paths.config()) {
            Ok(()) => "Saved".into(),
            Err(e) => format!("Save failed: {e:#}"),
        };
    }

    fn tracker(&self) -> Option<Arc<TrackerSource>> {
        let token = self.tokens[Mode::ALL.iter().position(|m| *m == self.cfg.mode)?].trim().to_owned();
        if token.is_empty() {
            return None;
        }
        TrackerSource::start(self.cfg.mode, token, self.paths.pending(), Arc::new(|_: &str, _: &str| {})).ok()
    }

    fn profile(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Player level");
            ui.add(egui::DragValue::new(&mut self.level).range(1..=79));
            if ui.button("Set").clicked() {
                let r = match self.cfg.source {
                    Source::Local => self.local.set_level(self.level),
                    Source::Tracker => match self.tracker() {
                        Some(t) => t.set_level(self.level),
                        None => Err(anyhow::anyhow!("no token for {}", self.cfg.mode.label())),
                    },
                };
                self.status = r.map(|()| format!("Level {} saved", self.level)).unwrap_or_else(|e| format!("{e:#}"));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Faction");
            let enabled = self.cfg.source == Source::Local;
            ui.add_enabled_ui(enabled, |ui| {
                for f in ["USEC", "BEAR"] {
                    if ui.selectable_label(self.faction == f, f).clicked() {
                        self.faction = f.into();
                        self.status = match self.local.set_faction(Some(f)) {
                            Ok(()) => format!("Faction {f} saved"),
                            Err(e) => format!("{e:#}"),
                        };
                    }
                }
            });
            if !enabled {
                ui.label("(from TarkovTracker)");
            }
        });
    }

    fn hideout(&mut self, ui: &mut egui::Ui) {
        if self.cfg.source != Source::Local {
            ui.label("TarkovTracker's API has no hideout write; edit the hideout on tarkovtracker.org.");
            return;
        }
        let Some(cat) = &self.catalog else {
            ui.label("No item data cached yet; start the overlay once with internet access.");
            return;
        };
        let built = self.local.snapshot().hideout_modules.clone();
        let mut change = None;
        egui::Grid::new("hideout").striped(true).show(ui, |ui| {
            for s in &cat.stations {
                let max = s.levels.iter().map(|l| l.level).max().unwrap_or(0);
                let mut level = s.levels.iter().filter(|l| built.contains(&l.id)).map(|l| l.level).max().unwrap_or(0);
                ui.label(&s.name);
                if ui.add(egui::DragValue::new(&mut level).range(0..=max)).changed() {
                    change = Some((s.id.clone(), level));
                }
                ui.label(format!("/ {max}"));
                ui.end_row();
            }
        });
        if let Some((id, level)) = change {
            if let Err(e) = self.local.set_hideout_level(&id, level) {
                self.status = format!("{e:#}");
            }
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.save();
                }
                ui.label(&self.status);
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Game");
                ui.horizontal(|ui| {
                    ui.label("Game mode");
                    for m in Mode::ALL {
                        if ui.selectable_label(self.cfg.mode == m, m.label()).clicked() && self.cfg.mode != m {
                            self.switch_mode(m);
                        }
                    }
                });

                ui.separator();
                ui.heading("Progress");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.cfg.source, Source::Local, "Local file");
                    ui.radio_value(&mut self.cfg.source, Source::Tracker, "TarkovTracker");
                });
                for (i, m) in Mode::ALL.into_iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} token", m.label()));
                        ui.add(egui::TextEdit::singleline(&mut self.tokens[i]).password(true).desired_width(260.0));
                        if ui.button("Test").clicked() {
                            self.token_status[i] = match crate::progress::test_token(self.tokens[i].trim()) {
                                Ok((perms, mode)) => format!("OK: {} · {mode}", perms.join(",")),
                                Err(e) => format!("Failed: {e}"),
                            };
                        }
                        ui.label(&self.token_status[i]);
                    });
                }
                self.profile(ui);
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
                });

                ui.separator();
                ui.heading("Scanning");
                egui::ComboBox::from_label("Scan hotkey").selected_text(self.cfg.hotkey.label()).show_ui(ui, |ui| {
                    for h in Hotkey::ALL {
                        ui.selectable_value(&mut self.cfg.hotkey, h, h.label());
                    }
                });
                ui.checkbox(&mut self.cfg.close_on_leave, "Close tooltip when the mouse leaves the item");
                ui.add(egui::Slider::new(&mut self.cfg.tooltip_seconds, 0.0..=30.0).text("Tooltip timeout, s (0 = none)"));
                ui.add(egui::Slider::new(&mut self.cfg.ui_scale, 0.75..=2.0).text("UI scale"));
                ui.checkbox(&mut self.cfg.show_non_fir, "Show non-FIR needs");
                ui.horizontal(|ui| {
                    ui.label("Show on card:");
                    ui.checkbox(&mut self.cfg.show_quests, "Quests");
                    ui.checkbox(&mut self.cfg.show_hideout, "Hideout");
                    ui.checkbox(&mut self.cfg.show_barters, "Barters");
                    ui.checkbox(&mut self.cfg.show_crafts, "Crafts");
                });
                ui.add(egui::Slider::new(&mut self.cfg.max_rows, 0..=15).text("Quest/hideout rows"));
                ui.checkbox(&mut self.cfg.smart_ordering, "Smart ordering")
                    .on_hover_text("Active and available tasks, then buildable hideout, then the rest; closest to done and fewest levels away first. Off: quests then hideout, biggest count first.");
                egui::ComboBox::from_label("Goal").selected_text(self.cfg.goal.label()).show_ui(ui, |ui| {
                    for g in crate::config::Goal::ALL {
                        ui.selectable_value(&mut self.cfg.goal, g, g.label());
                    }
                });

                ui.separator();
                ui.heading("EFT logs");
                ui.checkbox(&mut self.cfg.log_tracking, "Track quests from EFT logs");
                ui.horizontal(|ui| {
                    ui.label("Logs folder override");
                    ui.text_edit_singleline(&mut self.cfg.logs_override);
                });

                ui.separator();
                ui.collapsing("Hideout", |ui| self.hideout(ui));

                ui.separator();
                if ui.button("Open log file").clicked() {
                    let _ = std::process::Command::new("notepad.exe").arg(self.paths.log()).spawn();
                }
            });
        });
    }
}

pub fn run(paths: Paths) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("ChadOverlay Settings").with_inner_size([620.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native("ChadOverlay Settings", options, Box::new(|_cc| Ok(Box::new(SettingsApp::new(paths)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}
