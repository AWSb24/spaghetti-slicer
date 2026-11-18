//! Several objects on one build plate: how they are put on the bed, the
//! checks that they fit, and arranging them side by side.

use slicer_core::polygon::{self, Contour, Point};
use slicer_core::{Diagnostic, Diagnostics, Mesh};
use slicer_profiles::Settings;

use crate::FdmError;

/// Area in mm² by which the outlines of two objects must overlap to be
/// reported. Objects that only touch are fine.
const MIN_OVERLAP: f64 = 0.01;

/// Distance in millimetres by which an object may reach over the edge of the
/// bed before it counts as outside, to absorb rounding.
const EDGE_TOLERANCE: f64 = 1e-6;

/// Default distance between arranged objects, in millimetres.
pub const ARRANGE_GAP: f64 = 5.0;

/// How [`crate::generate_plate`] puts the objects on the bed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// The objects keep their positions relative to each other and are
    /// centred on the bed as a group, which is put down on the bed. What
    /// [`crate::generate`] does with its single mesh.
    #[default]
    Centre,
    /// Every object keeps its X and Y coordinates, which are bed coordinates,
    /// and is put down on the bed on its own. It must lie inside the bed.
    AsGiven,
}

/// The objects as one mesh, each put down on the bed on its own for
/// [`Placement::AsGiven`].
pub(crate) fn merge(objects: &[Mesh], placement: Placement) -> Mesh {
    let mut mesh = Mesh::default();
    for object in objects {
        let start = mesh.vertices.len();
        mesh.merge(object);
        if let (Placement::AsGiven, Some((min, _))) = (placement, object.bounds()) {
            for vertex in &mut mesh.vertices[start..] {
                vertex[2] -= min[2];
            }
        }
    }
    mesh
}

/// Fails if an object placed as given reaches over the edge of the bed, and
/// warns about objects whose outlines seen from above overlap.
pub(crate) fn check(
    objects: &[Mesh],
    placement: Placement,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
) -> Result<(), FdmError> {
    let bed = [settings.bed_size_x, settings.bed_size_y];
    if placement == Placement::AsGiven {
        for (object, mesh) in objects.iter().enumerate() {
            let Some((min, max)) = mesh.bounds() else {
                continue;
            };
            if (0..2).any(|a| min[a] < -EDGE_TOLERANCE || max[a] > bed[a] + EDGE_TOLERANCE) {
                return Err(FdmError::OutsideBed {
                    object,
                    footprint: [min[0], min[1], max[0], max[1]],
                    bed,
                });
            }
        }
    }
    if objects.len() < 2 {
        return Ok(());
    }
    // ponytail: convex outlines, so an object standing in the hole of a ring
    // is reported too. Comparing the first layers is the upgrade.
    let outlines: Vec<_> = objects.iter().map(|o| vec![vec![outline(o)]]).collect();
    for i in 0..outlines.len() {
        for j in i + 1..outlines.len() {
            let area = polygon::area(&polygon::intersection(&outlines[i], &outlines[j]));
            if area > MIN_OVERLAP {
                diagnostics.push(
                    Diagnostic::warning(
                        "slicing.objects_overlap",
                        format!(
                            "objects {} and {} overlap seen from above ({area:.1} mm² of their convex outlines); overlapping objects are printed as one",
                            i + 1,
                            j + 1
                        ),
                    )
                    .with("objects", format!("{} {}", i + 1, j + 1))
                    .with("area", format!("{area:.3}")),
                );
            }
        }
    }
    Ok(())
}

/// Convex hull of a mesh seen from above.
pub(crate) fn outline(mesh: &Mesh) -> Contour {
    let points: Vec<Point> = mesh
        .triangles
        .iter()
        .flatten()
        .map(|&i| mesh.vertices[i as usize])
        .map(|v| [v[0], v[1]])
        .collect();
    polygon::convex_hull(&points)
}

/// Places objects whose bounding boxes seen from above have the given sizes
/// (X, Y in millimetres) side by side on the bed, and returns the smallest X
/// and Y each one gets. They lie `gap` millimetres apart, at least
/// `extruder_clearance_radius` with `sequential_print`, plus room for the
/// brim of both.
///
/// Shelf packing ("next fit, decreasing height"; Coffman, Garey, Johnson and
/// Tarjan, SIAM J. Comput. 9(4), 1980): deepest first, rows filled from the
/// front left, a new row behind the deepest object of the last one. The
/// whole arrangement is centred on the bed. Objects that do not fit end up
/// beyond the back edge, where [`crate::generate_plate`] reports them. The
/// result depends only on the sizes and their order.
// ponytail: bounding boxes, so L-shapes and round parts waste room, and
// nothing is rotated. Packing the outlines (bottom-left fill with no-fit
// polygons) is the upgrade.
pub fn arrange(sizes: &[[f64; 2]], settings: &Settings, gap: f64) -> Vec<Point> {
    let bed = [settings.bed_size_x, settings.bed_size_y];
    let gap = if settings.sequential_print {
        gap.max(settings.extruder_clearance_radius)
    } else {
        gap
    } + 2.0 * settings.brim_width;
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    // Deepest first, then widest; the sort is stable, so equal objects keep
    // their order.
    order.sort_by(|&a, &b| {
        sizes[b][1]
            .total_cmp(&sizes[a][1])
            .then(sizes[b][0].total_cmp(&sizes[a][0]))
    });
    let mut corners = vec![[0.0, 0.0]; sizes.len()];
    let (mut x, mut y, mut row_depth, mut width) = (0.0, 0.0, 0.0_f64, 0.0_f64);
    for index in order {
        let [object_width, object_depth] = sizes[index];
        if x > 0.0 && x + object_width > bed[0] {
            (x, y, row_depth) = (0.0, y + row_depth + gap, 0.0);
        }
        corners[index] = [x, y];
        width = width.max(x + object_width);
        row_depth = row_depth.max(object_depth);
        x += object_width + gap;
    }
    let shift = [
        ((bed[0] - width) / 2.0).max(0.0),
        ((bed[1] - y - row_depth) / 2.0).max(0.0),
    ];
    for corner in &mut corners {
        corner[0] += shift[0];
        corner[1] += shift[1];
    }
    corners
}

/// Moves `objects` side by side on the bed where [`arrange`] puts their
/// bounding boxes seen from above, for [`Placement::AsGiven`].
pub fn arrange_objects(objects: &mut [Mesh], settings: &Settings, gap: f64) {
    let bounds: Vec<_> = objects.iter().map(Mesh::bounds).collect();
    let sizes: Vec<[f64; 2]> = bounds
        .iter()
        .map(|b| b.map_or([0.0, 0.0], |(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]]))
        .collect();
    let corners = arrange(&sizes, settings, gap);
    for ((mesh, bounds), corner) in objects.iter_mut().zip(&bounds).zip(corners) {
        if let Some((lo, _)) = bounds {
            mesh.translate([corner[0] - lo[0], corner[1] - lo[1], 0.0]);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn arranged_boxes_do_not_overlap_and_stay_on_the_bed() {
        let settings = Settings {
            brim_width: 2.0,
            ..Settings::default()
        };
        let sizes = [
            [60.0, 20.0],
            [30.0, 50.0],
            [100.0, 40.0],
            [30.0, 50.0],
            [80.0, 10.0],
            [5.0, 5.0],
        ];
        let corners = arrange(&sizes, &settings, ARRANGE_GAP);
        // Deterministic: the same input gives the same output.
        assert_eq!(corners, arrange(&sizes, &settings, ARRANGE_GAP));
        let boxes: Vec<[f64; 4]> = corners
            .iter()
            .zip(&sizes)
            .map(|(c, s)| [c[0], c[1], c[0] + s[0], c[1] + s[1]])
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            assert!(a[0] >= 0.0 && a[1] >= 0.0 && a[2] <= 220.0 && a[3] <= 220.0);
            for b in &boxes[i + 1..] {
                // At least the gap and both brims apart in X or in Y.
                let apart = b[0] - a[2] >= 9.0 - 1e-9
                    || a[0] - b[2] >= 9.0 - 1e-9
                    || b[1] - a[3] >= 9.0 - 1e-9
                    || a[1] - b[3] >= 9.0 - 1e-9;
                assert!(apart, "{a:?} {b:?}");
            }
        }
        // The arrangement is centred: equal margins left and right of the
        // widest row, front and back.
        let lo = boxes
            .iter()
            .fold([f64::MAX; 2], |m, b| [m[0].min(b[0]), m[1].min(b[1])]);
        let hi = boxes
            .iter()
            .fold([0.0_f64; 2], |m, b| [m[0].max(b[2]), m[1].max(b[3])]);
        assert!((lo[0] - (220.0 - hi[0])).abs() < 1e-9);
        assert!((lo[1] - (220.0 - hi[1])).abs() < 1e-9);
    }

    #[test]
    fn objects_that_do_not_fit_go_beyond_the_back() {
        let settings = Settings::default();
        let corners = arrange(&[[200.0, 150.0], [200.0, 150.0]], &settings, ARRANGE_GAP);
        assert_eq!(corners, [[10.0, 0.0], [10.0, 155.0]]);
        let mut second = Mesh::cuboid(200.0, 150.0, 1.0);
        second.translate([corners[1][0], corners[1][1], 0.0]);
        let error = check(
            &[Mesh::cuboid(1.0, 1.0, 1.0), second],
            Placement::AsGiven,
            &settings,
            &mut Diagnostics::new(),
        )
        .unwrap_err();
        assert!(matches!(error, FdmError::OutsideBed { object: 1, .. }));
        assert!(error.to_string().starts_with("object 2 "), "{error}");
    }

    #[test]
    fn overlapping_outlines_are_reported() {
        let settings = Settings::default();
        let a = Mesh::cuboid(10.0, 10.0, 1.0);
        let mut touching = a.clone();
        touching.translate([10.0, 0.0, 5.0]);
        let mut overlapping = a.clone();
        overlapping.translate([5.0, 5.0, 0.0]);
        let mut diagnostics = Diagnostics::new();
        check(
            &[a.clone(), touching],
            Placement::AsGiven,
            &settings,
            &mut diagnostics,
        )
        .unwrap();
        assert!(diagnostics.is_empty());
        check(
            &[a, overlapping],
            Placement::Centre,
            &settings,
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(diagnostics.codes(), ["slicing.objects_overlap"]);
        assert!(diagnostics.as_slice()[0].message.contains("25.0 mm²"));
    }
}
