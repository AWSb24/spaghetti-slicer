//! Colours, spacing and text sizes of the window, modelled on the dark
//! theme of common desktop slicers: grey panels, a teal accent, a mid-grey
//! 3D canvas. The values were measured from screenshots; nothing is taken
//! from another program's files.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Vec2};

/// Accent: selected checkboxes, underlines, primary buttons.
pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0x96, 0x88);
/// Darker accent: the selected main tab, pressed buttons.
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x00, 0x67, 0x5b);
/// Panels and input fields.
pub const PANEL: Color32 = Color32::from_rgb(0x2d, 0x2d, 0x31);
/// Section headers in the side panel.
pub const HEADER: Color32 = Color32::from_rgb(0x36, 0x36, 0x3b);
/// Background of the 3D view.
pub const CANVAS: Color32 = Color32::from_rgb(0x54, 0x54, 0x5a);
/// The bed in the 3D view.
pub const BED: Color32 = Color32::from_rgb(0x35, 0x35, 0x3b);
/// Grid lines on the bed.
pub const BED_GRID: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x51);
/// Border of the bed.
pub const BED_BORDER: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x92);
/// Floating toolbars over the 3D view.
pub const TOOLBAR: Color32 = Color32::from_rgb(0x39, 0x3c, 0x42);
/// Popups, the legend and notifications.
pub const POPUP: Color32 = Color32::from_rgb(0x26, 0x26, 0x27);
/// Buttons and other widgets at rest.
pub const WIDGET: Color32 = Color32::from_rgb(0x3e, 0x3e, 0x44);
/// Widgets under the pointer.
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x51);
/// Outlines of fields and separators.
pub const LINE: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x50);
/// Normal text.
pub const TEXT: Color32 = Color32::from_rgb(0xe2, 0xe2, 0xe4);
/// Secondary text: units, hints.
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x8e, 0x8e, 0x94);
/// The selected model in the 3D view.
pub const MODEL_SELECTED: [u8; 3] = [0x5e, 0xd1, 0xc4];
/// Models in the 3D view.
pub const MODEL: [u8; 3] = [0xd8, 0xd8, 0xdc];
/// A model that does not fit the printer.
pub const MODEL_OUTSIDE: [u8; 3] = [0xe6, 0x4b, 0x4b];
/// A modifier volume in the 3D view.
pub const VOLUME_MODIFIER: [u8; 3] = [0xf0, 0xa0, 0x30];
/// A negative volume in the 3D view.
pub const VOLUME_NEGATIVE: [u8; 3] = [0xe6, 0x4b, 0x4b];
/// A support blocker in the 3D view.
pub const VOLUME_BLOCKER: [u8; 3] = [0x4a, 0x90, 0xe0];
/// A support enforcer in the 3D view.
pub const VOLUME_ENFORCER: [u8; 3] = [0x5e, 0xd1, 0x84];
/// The volume the object tree edits.
pub const VOLUME_SELECTED: [u8; 3] = [0xff, 0xff, 0xff];

/// Applies the theme; called once at start.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = POPUP;
    visuals.window_stroke = Stroke::new(1.0, LINE);
    visuals.extreme_bg_color = PANEL;
    visuals.text_edit_bg_color = Some(PANEL);
    visuals.faint_bg_color = HEADER;
    visuals.code_bg_color = POPUP;
    visuals.hyperlink_color = ACCENT;
    visuals.override_text_color = None;
    visuals.selection.bg_fill = ACCENT_DARK;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.slider_trailing_fill = true;
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = CornerRadius::same(4);
    let radius = CornerRadius::same(3);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL;
    widgets.noninteractive.weak_bg_fill = PANEL;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    for (state, fill, stroke) in [
        (&mut widgets.inactive, WIDGET, LINE),
        (&mut widgets.hovered, WIDGET_HOVER, ACCENT),
        (&mut widgets.active, ACCENT_DARK, ACCENT),
        (&mut widgets.open, WIDGET_HOVER, ACCENT),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, TEXT);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    widgets.inactive.bg_stroke = Stroke::NONE;
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.spacing.item_spacing = Vec2::new(6.0, 5.0);
        style.spacing.button_padding = Vec2::new(8.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, FontId::proportional(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
    });
}
