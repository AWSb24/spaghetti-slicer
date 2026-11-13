//! SVG rendering of single layers for visual inspection.

use std::fmt::Write;

use crate::{Layer, PathKind};

fn color(kind: PathKind) -> &'static str {
    match kind {
        PathKind::Skirt | PathKind::Brim => "#8a8a8a",
        PathKind::Support => "#c9a227",
        PathKind::OuterWall => "#e8590c",
        PathKind::InnerWall => "#f2b01e",
        PathKind::SolidInfill => "#7b2cbf",
        PathKind::SparseInfill => "#2a9d8f",
    }
}

/// Renders a layer as an SVG document. Lines are drawn at their real width,
/// coloured by path kind, with Y pointing up as on the printer.
pub fn layer_svg(layer: &Layer, line_width: f64) -> String {
    let points = || layer.paths.iter().flat_map(|p| &p.points);
    let bound = |axis: usize, start: f64, pick: fn(f64, f64) -> f64| {
        points().map(|p| p[axis]).fold(start, pick)
    };
    let margin = 2.0 * line_width;
    let (min_x, max_x) = (
        bound(0, f64::INFINITY, f64::min),
        bound(0, f64::NEG_INFINITY, f64::max),
    );
    let (min_y, max_y) = (
        bound(1, f64::INFINITY, f64::min),
        bound(1, f64::NEG_INFINITY, f64::max),
    );
    let (width, height) = if min_x.is_finite() {
        (max_x - min_x + 2.0 * margin, max_y - min_y + 2.0 * margin)
    } else {
        (1.0, 1.0)
    };
    let (left, top) = if min_x.is_finite() {
        (min_x - margin, max_y + margin)
    } else {
        (0.0, 0.0)
    };

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.3} {height:.3}\" \
         width=\"{:.0}\" height=\"{:.0}\">",
        width * 20.0,
        height * 20.0
    );
    let _ = writeln!(
        svg,
        "<rect width=\"100%\" height=\"100%\" fill=\"#1b1b1f\"/>"
    );
    let _ = writeln!(
        svg,
        "<g fill=\"none\" stroke-width=\"{line_width}\" stroke-linecap=\"round\" \
         stroke-linejoin=\"round\" stroke-opacity=\"0.85\">"
    );
    for path in &layer.paths {
        let tag = if path.closed { "polygon" } else { "polyline" };
        let _ = write!(svg, "<{tag} stroke=\"{}\" points=\"", color(path.kind));
        for p in &path.points {
            let _ = write!(svg, "{:.3},{:.3} ", p[0] - left, top - p[1]);
        }
        let _ = writeln!(svg, "\"/>");
    }
    let _ = writeln!(svg, "</g>\n</svg>");
    svg
}
