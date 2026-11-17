//! The setup assistant: on the first start, and from the File menu, it asks
//! which printers (with which nozzles) and which filaments the user has.
//! Those built-in profiles are then listed in the pickers of the side panel.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Vec2};
use slicer_profiles::Flavor;
use slicer_profiles::catalog::{MATERIALS, PRINTERS, Printer, vendors};
use slicer_ui::profiles::Setup;

use crate::icons::{self, Icon};
use crate::theme;

/// The pages of the assistant.
const PAGES: [&str; 4] = ["Welcome", "Printers", "Filaments", "Finish"];

/// Width of one printer card.
const CARD_WIDTH: f32 = 196.0;

/// How the assistant was closed.
pub enum WizardEnd {
    /// Use these printers and materials.
    Finish(Setup),
    /// Closed without changing anything.
    Cancel,
}

/// The open assistant.
pub struct Wizard {
    page: usize,
    vendor: &'static str,
    search: String,
    printers: Vec<String>,
    materials: Vec<String>,
}

impl Wizard {
    /// Opens the assistant with what was chosen before; PLA and PETG are
    /// chosen if no material was.
    pub fn new(setup: &Setup) -> Self {
        let materials = if setup.materials.is_empty() {
            vec!["generic-pla".to_owned(), "generic-petg".to_owned()]
        } else {
            setup.materials.clone()
        };
        Self {
            page: 0,
            vendor: PRINTERS[0].vendor,
            search: String::new(),
            printers: setup.printers.clone(),
            materials,
        }
    }

    /// Draws the assistant over the window. Returns how it was closed,
    /// once it is.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<WizardEnd> {
        let mut end = None;
        let screen = ctx.content_rect();
        let size = Vec2::new(
            (screen.width() - 40.0).clamp(320.0, 900.0),
            (screen.height() - 40.0).clamp(300.0, 620.0),
        );
        let modal = egui::Modal::new(egui::Id::unique("setup_assistant")).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            self.steps(ui);
            ui.separator();
            let footer = 40.0;
            let body = ui.available_height() - footer;
            ui.allocate_ui(Vec2::new(size.x, body), |ui| {
                ui.set_min_height(body);
                match self.page {
                    0 => welcome(ui),
                    1 => self.printer_page(ui),
                    2 => self.material_page(ui),
                    _ => self.finish_page(ui),
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button("Skip")
                    .on_hover_text("Close without changes")
                    .clicked()
                {
                    end = Some(WizardEnd::Cancel);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let last = self.page == PAGES.len() - 1;
                    let label = if last { "Finish" } else { "Next" };
                    let next = egui::Button::new(RichText::new(label).color(Color32::WHITE))
                        .fill(theme::ACCENT)
                        .min_size(Vec2::new(90.0, 28.0));
                    if ui.add(next).clicked() {
                        if last {
                            end = Some(WizardEnd::Finish(Setup {
                                done: true,
                                printers: self.printers.clone(),
                                materials: self.materials.clone(),
                            }));
                        } else {
                            self.page += 1;
                        }
                    }
                    if self.page > 0
                        && ui
                            .add(egui::Button::new("Back").min_size(Vec2::new(90.0, 28.0)))
                            .clicked()
                    {
                        self.page -= 1;
                    }
                });
            });
        });
        if modal.should_close() && end.is_none() {
            end = Some(WizardEnd::Cancel);
        }
        end
    }

    /// The numbered steps across the top, the current one highlighted.
    fn steps(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Setup assistant").size(18.0).strong());
            ui.add_space(24.0);
            for (index, name) in PAGES.iter().enumerate() {
                let color = if index == self.page {
                    theme::ACCENT
                } else {
                    theme::TEXT_WEAK
                };
                let text = RichText::new(format!("{} {name}", index + 1)).color(color);
                if ui
                    .add(egui::Label::new(text).sense(Sense::click()))
                    .clicked()
                {
                    self.page = index;
                }
                ui.add_space(12.0);
            }
        });
    }

    /// Makers on the left; the models of the chosen one, or those matching
    /// the search, as cards with one box per nozzle.
    fn printer_page(&mut self, ui: &mut egui::Ui) {
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(170.0);
                ui.set_min_height(height);
                egui::ScrollArea::vertical()
                    .id_salt("vendors")
                    .show(ui, |ui| {
                        for vendor in vendors() {
                            let chosen = PRINTERS
                                .iter()
                                .filter(|p| p.vendor == vendor)
                                .flat_map(|p| p.nozzles.iter().map(|&n| p.id(n)))
                                .filter(|id| self.printers.contains(id))
                                .count();
                            let text = if chosen > 0 {
                                format!("{vendor}  ({chosen})")
                            } else {
                                vendor.to_owned()
                            };
                            let open = self.vendor == vendor && self.search.is_empty();
                            if ui.selectable_label(open, text).clicked() {
                                self.vendor = vendor;
                                self.search.clear();
                            }
                        }
                    });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Search all printers")
                            .desired_width(220.0),
                    );
                    let shown = self.shown();
                    let first: Vec<String> = shown.iter().map(|p| p.id(p.nozzles[0])).collect();
                    if ui.button("Select all").clicked() {
                        for id in first {
                            if !self.printers.contains(&id) {
                                self.printers.push(id);
                            }
                        }
                    }
                    if ui.button("Select none").clicked() {
                        self.printers.retain(|id| {
                            !shown
                                .iter()
                                .any(|p| p.nozzles.iter().any(|&n| p.id(n) == *id))
                        });
                    }
                    ui.label(
                        RichText::new(format!("{} chosen", self.printers.len()))
                            .color(theme::TEXT_WEAK),
                    );
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("models")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let columns = ((ui.available_width() / (CARD_WIDTH + 8.0)) as usize).max(1);
                        let shown = self.shown();
                        for row in shown.chunks(columns) {
                            ui.horizontal_top(|ui| {
                                for printer in row {
                                    self.card(ui, printer);
                                }
                            });
                            ui.add_space(6.0);
                        }
                        if shown.is_empty() {
                            ui.weak("No printer matches the search.");
                        }
                    });
            });
        });
    }

    /// The printers of the open maker, or those whose name contains the
    /// search text.
    fn shown(&self) -> Vec<&'static Printer> {
        let search = self.search.trim().to_lowercase();
        PRINTERS
            .iter()
            .filter(|p| {
                if search.is_empty() {
                    p.vendor == self.vendor
                } else {
                    p.full_name().to_lowercase().contains(&search)
                }
            })
            .collect()
    }

    /// One printer: picture, name, build volume, firmware and a box per
    /// nozzle.
    fn card(&mut self, ui: &mut egui::Ui, printer: &Printer) {
        let any = printer
            .nozzles
            .iter()
            .any(|&n| self.printers.contains(&printer.id(n)));
        // Darker than the boxes inside, which use the widget colour.
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(
                1.0,
                if any { theme::ACCENT } else { theme::LINE },
            ))
            .corner_radius(4)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_width(CARD_WIDTH - 18.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                        icons::paint(
                            ui.painter(),
                            rect,
                            Icon::Printer,
                            if any { theme::ACCENT } else { theme::TEXT_WEAK },
                        );
                        ui.vertical(|ui| {
                            ui.label(RichText::new(printer.model).strong());
                            if !self.search.is_empty() {
                                ui.label(RichText::new(printer.vendor).small());
                            }
                        });
                    });
                    let firmware = match printer.firmware {
                        Flavor::Marlin => "Marlin",
                        Flavor::Klipper => "Klipper",
                    };
                    ui.label(
                        RichText::new(format!(
                            "{} × {} × {} mm · {firmware}",
                            printer.bed[0], printer.bed[1], printer.height
                        ))
                        .small()
                        .color(theme::TEXT_WEAK),
                    );
                    let mut nozzles = printer.nozzles.to_vec();
                    nozzles.sort_by(f64::total_cmp);
                    ui.horizontal_wrapped(|ui| {
                        for nozzle in nozzles {
                            let id = printer.id(nozzle);
                            let mut on = self.printers.contains(&id);
                            if ui.checkbox(&mut on, format!("{nozzle} mm")).changed() {
                                if on {
                                    self.printers.push(id);
                                } else {
                                    self.printers.retain(|other| *other != id);
                                }
                            }
                        }
                    });
                });
            });
    }

    /// A box per material with its starting temperatures.
    fn material_page(&mut self, ui: &mut egui::Ui) {
        ui.label("Which filaments do you print? Each is a starting point; adjust it to the maker's data sheet.");
        ui.horizontal(|ui| {
            if ui.button("Select all").clicked() {
                self.materials = MATERIALS.iter().map(|m| m.id()).collect();
            }
            if ui.button("Select none").clicked() {
                self.materials.clear();
            }
        });
        ui.add_space(6.0);
        egui::Grid::new("materials")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                for material in MATERIALS {
                    let id = material.id();
                    let mut on = self.materials.contains(&id);
                    if ui.checkbox(&mut on, material.display_name()).changed() {
                        if on {
                            self.materials.push(id);
                        } else {
                            self.materials.retain(|other| *other != id);
                        }
                    }
                    ui.label(
                        RichText::new(format!(
                            "Nozzle {} °C · Bed {} °C · Fan {} %",
                            material.nozzle[0], material.bed, material.fan
                        ))
                        .color(theme::TEXT_WEAK),
                    );
                    ui.end_row();
                }
            });
    }

    /// What was chosen, before the assistant closes.
    fn finish_page(&self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("Ready").size(16.0).strong());
        ui.add_space(6.0);
        let names = |ids: &[String], name: &dyn Fn(&str) -> Option<String>| -> String {
            let names: Vec<String> = ids.iter().filter_map(|id| name(id)).collect();
            if names.is_empty() {
                "none".to_owned()
            } else {
                names.join(", ")
            }
        };
        let printers = names(&self.printers, &|id| {
            slicer_profiles::catalog::printer_by_id(id).map(|(p, n)| p.name(n))
        });
        let materials = names(&self.materials, &|id| {
            slicer_profiles::catalog::material_by_id(id).map(|m| m.display_name())
        });
        ui.label(format!("Printers: {printers}"));
        ui.add_space(4.0);
        ui.label(format!("Filaments: {materials}"));
        ui.add_space(12.0);
        if self.printers.is_empty() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "No printer chosen. Go back to choose one, or use your own settings files.",
            );
        }
        ui.label(
            RichText::new(
                "The first printer is selected now, with the first filament and the standard \
                 process. The built-in printer profiles use general starting values and have \
                 not yet run a print; watch the first one. You can open this assistant again \
                 from the File menu.",
            )
            .color(theme::TEXT_WEAK),
        );
    }
}

/// The first page.
fn welcome(ui: &mut egui::Ui) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(72.0), Sense::hover());
        icons::paint(ui.painter(), rect, Icon::Printer, theme::ACCENT);
        ui.add_space(12.0);
        ui.label(
            RichText::new("Welcome to Spaghetti Slicer")
                .size(22.0)
                .strong(),
        );
        ui.add_space(8.0);
        ui.label(format!(
            "Choose your printers and filaments in the next steps. {} printer models of {} makers \
             are built in, and {} filament types.",
            PRINTERS.len(),
            vendors().len(),
            MATERIALS.len()
        ));
        ui.label("Your own settings files and imported Orca Slicer profiles work as before.");
    });
}
