//! The window's own line icons, drawn as vector strokes so that they stay
//! sharp at any size. Drawn for this project in the thin outline style of
//! desktop slicers; no other program's images are used.

use std::f32::consts::PI;

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Vec2};

use crate::theme;

/// An icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// The Prepare tab: a cube.
    Prepare,
    /// The Preview tab: stacked layers.
    Preview,
    /// The printer section.
    Printer,
    /// The filament section: a spool.
    Filament,
    /// The process section: sliders.
    Process,
    /// The object list: two boxes.
    Objects,
    /// Add models: a cube with a plus.
    Add,
    /// Arrange models: four squares.
    Arrange,
    /// Move to the middle of the bed: cross hairs.
    Center,
    /// Delete: a bin.
    Delete,
    /// Turn left by 90 degrees.
    TurnLeft,
    /// Turn right by 90 degrees.
    TurnRight,
    /// Look at the whole bed again.
    Fit,
    /// Undo the last change: a hooked arrow to the left.
    Undo,
    /// Redo an undone change.
    Redo,
    /// Copy the selected model: two boxes and a plus.
    Duplicate,
    /// A shown line type.
    Eye,
    /// A hidden line type.
    EyeClosed,
    /// Opens a list.
    Chevron,
    /// Lay the model on a face: a box resting on the bed, arrow down.
    LayOnFace,
    /// Turn the model for printing: a box in a circling arrow.
    AutoOrient,
    /// Mirror the model: two triangles either side of an axis.
    Mirror,
}

/// Draws `icon` into `rect` in `color`.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let size = rect.width().min(rect.height());
    let origin = rect.center() - Vec2::splat(size / 2.0);
    // Coordinates from 0 to 1 across the icon.
    let at = |x: f32, y: f32| origin + Vec2::new(x * size, y * size);
    let stroke = Stroke::new((size / 14.0).max(1.2), color);
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let closed = |points: &[(f32, f32)]| {
        painter.add(Shape::closed_line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x: f32, y: f32, r: f32| {
        painter.circle_stroke(at(x, y), r * size, stroke);
    };
    let arc = |x: f32, y: f32, r: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=16)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 16.0;
                at(x + r * a.cos(), y + r * a.sin())
            })
            .collect()
    };
    match icon {
        Icon::Prepare | Icon::Add | Icon::Fit => {
            // An upright cube seen from above the front corner.
            closed(&[
                (0.5, 0.1),
                (0.85, 0.28),
                (0.85, 0.72),
                (0.5, 0.9),
                (0.15, 0.72),
                (0.15, 0.28),
            ]);
            line(&[(0.15, 0.28), (0.5, 0.46), (0.85, 0.28)]);
            line(&[(0.5, 0.46), (0.5, 0.9)]);
            if icon == Icon::Add {
                painter.circle_filled(at(0.8, 0.8), 0.2 * size, theme::ACCENT);
                let plus = Stroke::new(stroke.width, Color32::WHITE);
                painter.line_segment([at(0.8, 0.7), at(0.8, 0.9)], plus);
                painter.line_segment([at(0.7, 0.8), at(0.9, 0.8)], plus);
            }
        }
        Icon::Preview => {
            for dy in [0.0, 0.2, 0.4] {
                closed(&[
                    (0.5, 0.15 + dy),
                    (0.9, 0.32 + dy),
                    (0.5, 0.49 + dy),
                    (0.1, 0.32 + dy),
                ]);
            }
        }
        Icon::Printer => {
            closed(&[(0.15, 0.1), (0.85, 0.1), (0.85, 0.9), (0.15, 0.9)]);
            closed(&[(0.28, 0.25), (0.72, 0.25), (0.72, 0.62), (0.28, 0.62)]);
            line(&[(0.15, 0.75), (0.85, 0.75)]);
        }
        Icon::Filament => {
            circle(0.5, 0.5, 0.38);
            circle(0.5, 0.5, 0.12);
            line(&[(0.88, 0.5), (0.98, 0.9)]);
        }
        Icon::Process => {
            for (y, knob) in [(0.25, 0.35), (0.5, 0.68), (0.75, 0.45)] {
                line(&[(0.1, y), (0.9, y)]);
                painter.circle_filled(at(knob, y), 0.09 * size, color);
            }
        }
        Icon::Objects => {
            closed(&[(0.1, 0.35), (0.6, 0.35), (0.6, 0.9), (0.1, 0.9)]);
            line(&[
                (0.4, 0.35),
                (0.4, 0.1),
                (0.9, 0.1),
                (0.9, 0.65),
                (0.6, 0.65),
            ]);
        }
        Icon::Arrange => {
            for (x, y) in [(0.1, 0.1), (0.55, 0.1), (0.1, 0.55), (0.55, 0.55)] {
                closed(&[(x, y), (x + 0.35, y), (x + 0.35, y + 0.35), (x, y + 0.35)]);
            }
        }
        Icon::Center => {
            circle(0.5, 0.5, 0.3);
            line(&[(0.5, 0.05), (0.5, 0.35)]);
            line(&[(0.5, 0.65), (0.5, 0.95)]);
            line(&[(0.05, 0.5), (0.35, 0.5)]);
            line(&[(0.65, 0.5), (0.95, 0.5)]);
        }
        Icon::Delete => {
            line(&[(0.1, 0.22), (0.9, 0.22)]);
            line(&[(0.38, 0.22), (0.38, 0.1), (0.62, 0.1), (0.62, 0.22)]);
            closed(&[(0.2, 0.22), (0.8, 0.22), (0.72, 0.92), (0.28, 0.92)]);
            line(&[(0.42, 0.38), (0.42, 0.78)]);
            line(&[(0.58, 0.38), (0.58, 0.78)]);
        }
        Icon::TurnLeft | Icon::TurnRight => {
            let right = icon == Icon::TurnRight;
            let points = arc(0.5, 0.55, 0.33, -PI * 0.95, PI * 0.45);
            let points: Vec<Pos2> = if right {
                points
                    .into_iter()
                    .map(|p| Pos2::new(2.0 * rect.center().x - p.x, p.y))
                    .collect()
            } else {
                points
            };
            if let Some(&tip) = points.first() {
                let dir = if right { 1.0 } else { -1.0 };
                let a = Vec2::new(dir * 0.0, -0.22 * size);
                let b = Vec2::new(dir * 0.2 * size, -0.02 * size);
                painter.add(Shape::line(vec![tip + a, tip, tip + b], stroke));
            }
            painter.add(Shape::line(points, stroke));
        }
        Icon::Undo | Icon::Redo => {
            // A U-turn: from the arrow head along the top, round the right
            // end and back along the bottom; mirrored for redo.
            let flip = |x: f32| if icon == Icon::Redo { 1.0 - x } else { x };
            let mut points: Vec<Pos2> = vec![at(flip(0.15), 0.35), at(flip(0.6), 0.35)];
            points.extend(
                arc(0.6, 0.575, 0.225, -PI / 2.0, PI / 2.0)
                    .into_iter()
                    .map(|p| {
                        Pos2::new(
                            if icon == Icon::Redo {
                                2.0 * rect.center().x - p.x
                            } else {
                                p.x
                            },
                            p.y,
                        )
                    }),
            );
            points.push(at(flip(0.3), 0.8));
            painter.add(Shape::line(points, stroke));
            line(&[(flip(0.32), 0.18), (flip(0.15), 0.35), (flip(0.32), 0.52)]);
        }
        Icon::Duplicate => {
            closed(&[(0.1, 0.32), (0.58, 0.32), (0.58, 0.9), (0.1, 0.9)]);
            line(&[
                (0.32, 0.32),
                (0.32, 0.1),
                (0.9, 0.1),
                (0.9, 0.68),
                (0.58, 0.68),
            ]);
            line(&[(0.34, 0.48), (0.34, 0.74)]);
            line(&[(0.21, 0.61), (0.47, 0.61)]);
        }
        Icon::Eye | Icon::EyeClosed => {
            let upper = arc(0.5, 0.95, 0.6, -PI * 0.78, -PI * 0.22);
            let lower = arc(0.5, 0.05, 0.6, PI * 0.22, PI * 0.78);
            painter.add(Shape::line(upper, stroke));
            painter.add(Shape::line(lower, stroke));
            if icon == Icon::Eye {
                painter.circle_filled(at(0.5, 0.5), 0.14 * size, color);
            } else {
                line(&[(0.15, 0.85), (0.85, 0.15)]);
            }
        }
        Icon::Chevron => {
            line(&[(0.25, 0.4), (0.5, 0.65), (0.75, 0.4)]);
        }
        Icon::LayOnFace => {
            line(&[(0.05, 0.92), (0.95, 0.92)]);
            closed(&[(0.25, 0.42), (0.75, 0.42), (0.75, 0.84), (0.25, 0.84)]);
            painter.rect_filled(
                Rect::from_min_max(at(0.25, 0.78), at(0.75, 0.86)),
                0.0,
                theme::ACCENT,
            );
            line(&[(0.5, 0.04), (0.5, 0.3)]);
            line(&[(0.38, 0.19), (0.5, 0.31), (0.62, 0.19)]);
        }
        Icon::AutoOrient => {
            closed(&[(0.35, 0.35), (0.65, 0.35), (0.65, 0.65), (0.35, 0.65)]);
            let points = arc(0.5, 0.5, 0.4, -PI * 0.35, PI * 1.35);
            if let Some(&tip) = points.last() {
                let a = Vec2::new(0.16 * size, -0.02 * size);
                let b = Vec2::new(0.02 * size, 0.16 * size);
                painter.add(Shape::line(vec![tip + a, tip, tip - b], stroke));
            }
            painter.add(Shape::line(points, stroke));
        }
        Icon::Mirror => {
            closed(&[(0.08, 0.82), (0.4, 0.82), (0.4, 0.22)]);
            closed(&[(0.92, 0.82), (0.6, 0.82), (0.6, 0.22)]);
            for (y0, y1) in [(0.08, 0.22), (0.34, 0.48), (0.6, 0.74), (0.86, 0.95)] {
                line(&[(0.5, y0), (0.5, y1)]);
            }
        }
    }
}

/// A square button showing `icon`, `size` pixels wide, with `tip` on hover;
/// `selected` draws it highlighted.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, tip: &str, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let enabled = ui.is_enabled();
    if response.hovered() && enabled || selected {
        let fill = if selected {
            theme::ACCENT_DARK
        } else {
            theme::WIDGET_HOVER
        };
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let color = if enabled {
        theme::TEXT
    } else {
        theme::TEXT_WEAK.gamma_multiply(0.6)
    };
    paint(ui.painter(), rect.shrink(size * 0.2), icon, color);
    response.on_hover_text(tip)
}
