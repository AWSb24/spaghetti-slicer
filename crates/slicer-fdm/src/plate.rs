//! Several objects on one build plate: how they are put on the bed, the
//! checks that they fit, and arranging them side by side.

use slicer_core::polygon::{self, Contour, Point, Shapes};
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
    // ponytail: convex outlines by default, so an object standing in the
    // hole of a ring is reported too; with `arrange_nonconvex` the real
    // outlines of the pairs whose convex outlines overlap.
    let outlines: Vec<_> = objects.iter().map(|o| vec![vec![footprint(o)]]).collect();
    let real = settings.arrange_nonconvex;
    let mut top_views: Vec<Option<Shapes>> = vec![None; objects.len()];
    let kind = if real { "outlines" } else { "convex outlines" };
    for i in 0..outlines.len() {
        for j in i + 1..outlines.len() {
            let mut area = polygon::area(&polygon::intersection(&outlines[i], &outlines[j]));
            if real && area > MIN_OVERLAP {
                for k in [i, j] {
                    if top_views[k].is_none() {
                        top_views[k] = Some(top_view(&objects[k]));
                    }
                }
                if let (Some(a), Some(b)) = (&top_views[i], &top_views[j]) {
                    area = polygon::area(&polygon::intersection(a, b));
                }
            }
            if area > MIN_OVERLAP {
                diagnostics.push(
                    Diagnostic::warning(
                        "slicing.objects_overlap",
                        format!(
                            "objects {} and {} overlap seen from above ({area:.1} mm² of their {kind}); overlapping objects are printed as one",
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

/// Convex hull of a mesh seen from above: the outline that arranging
/// packs and the overlap warning compares (both without
/// `arrange_nonconvex`, which uses [`top_view`]) and that
/// `sequential_print` keeps apart (always: the print head is round).
pub fn footprint(mesh: &Mesh) -> Contour {
    let points: Vec<Point> = mesh
        .triangles
        .iter()
        .flatten()
        .map(|&i| mesh.vertices[i as usize])
        .map(|v| [v[0], v[1]])
        .collect();
    polygon::convex_hull(&points)
}

/// The area a mesh covers seen from above, exactly: the union of its
/// triangles projected onto XY, with holes where nothing lies above or
/// below. The outline that `arrange_nonconvex` packs and its overlap
/// warning compares.
pub fn top_view(mesh: &Mesh) -> Shapes {
    let loops: Vec<Contour> = mesh
        .triangles
        .iter()
        .filter_map(|triangle| {
            let [a, b, c] = triangle.map(|i| {
                let v = mesh.vertices[i as usize];
                [v[0], v[1]]
            });
            let twice = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            // Standing triangles cover nothing; the others counterclockwise.
            if twice.abs() < 1e-12 {
                None
            } else if twice > 0.0 {
                Some(vec![a, b, c])
            } else {
                Some(vec![a, c, b])
            }
        })
        .collect();
    polygon::from_oriented_loops(&loops)
}

/// The distance between arranged objects asked for as `gap`: at least
/// `extruder_clearance_radius` with `sequential_print`, plus room for the
/// brim of both.
pub(crate) fn spacing(settings: &Settings, gap: f64) -> f64 {
    let gap = if settings.sequential_print {
        gap.max(settings.extruder_clearance_radius)
    } else {
        gap
    };
    gap + 2.0 * settings.brim_width
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
// nothing is rotated; [`crate::arrange_outlines`] packs outlines, with
// `arrange_outlines` or `arrange_rotation`. This stays the default so
// that existing arrangements keep their places.
pub fn arrange(sizes: &[[f64; 2]], settings: &Settings, gap: f64) -> Vec<Point> {
    let bed = [settings.bed_size_x, settings.bed_size_y];
    let gap = spacing(settings, gap);
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

/// Moves `objects` side by side on the bed, for [`Placement::AsGiven`]:
/// where [`arrange`] puts their bounding boxes seen from above, or, with
/// `arrange_outlines` or `arrange_rotation`, where
/// [`crate::arrange_outlines`] puts their outlines, turned about Z as it
/// says, and with `arrange_nonconvex` where [`crate::arrange_footprints`]
/// puts their real outlines ([`top_view`]).
pub fn arrange_objects(objects: &mut [Mesh], settings: &Settings, gap: f64) {
    let spots = arrangement(objects, settings, gap);
    for (mesh, spot) in objects.iter_mut().zip(spots) {
        if let Some(spot) = spot {
            spot.apply(mesh);
        }
    }
}

/// Where [`arrange_objects`] moves every one of `objects`; `None` for an
/// object without vertices, which stays where it is.
pub(crate) fn arrangement(
    objects: &[Mesh],
    settings: &Settings,
    gap: f64,
) -> Vec<Option<crate::Spot>> {
    if settings.arrange_nonconvex {
        let footprints: Vec<Shapes> = objects.iter().map(top_view).collect();
        return crate::arrange_footprints(&footprints, settings, gap)
            .into_iter()
            .map(Some)
            .collect();
    }
    if crate::arranges_outlines(settings) {
        let outlines: Vec<Contour> = objects.iter().map(footprint).collect();
        return crate::arrange_outlines(&outlines, settings, gap)
            .into_iter()
            .map(Some)
            .collect();
    }
    let bounds: Vec<_> = objects.iter().map(Mesh::bounds).collect();
    let sizes: Vec<[f64; 2]> = bounds
        .iter()
        .map(|b| b.map_or([0.0, 0.0], |(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]]))
        .collect();
    let corners = arrange(&sizes, settings, gap);
    bounds
        .iter()
        .zip(corners)
        .map(|(bounds, corner)| {
            bounds.map(|(lo, _)| crate::Spot::shifted([corner[0] - lo[0], corner[1] - lo[1]]))
        })
        .collect()
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

    #[test]
    fn real_outlines_are_compared_with_arrange_nonconvex() {
        // A cube in the corner of an L, clear of it but inside its convex
        // outline.
        let ell = slicer_core::primitives::extrude(
            &[
                [0.0, 0.0],
                [60.0, 0.0],
                [60.0, 10.0],
                [10.0, 10.0],
                [10.0, 60.0],
                [0.0, 60.0],
            ],
            [5.0, 5.0],
            1.0,
            1,
            0.0,
        );
        let mut cube = Mesh::cuboid(10.0, 10.0, 1.0);
        cube.translate([20.0, 20.0, 0.0]);
        let objects = [ell.clone(), cube];
        let convex = Settings::default();
        let real = Settings {
            arrange_nonconvex: true,
            ..Settings::default()
        };
        let mut diagnostics = Diagnostics::new();
        check(&objects, Placement::AsGiven, &convex, &mut diagnostics).unwrap();
        assert_eq!(diagnostics.codes(), ["slicing.objects_overlap"]);
        assert!(
            diagnostics.as_slice()[0]
                .message
                .contains("convex outlines")
        );
        let mut diagnostics = Diagnostics::new();
        check(&objects, Placement::AsGiven, &real, &mut diagnostics).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        // Where they really overlap, it is still reported.
        let mut onto = Mesh::cuboid(10.0, 10.0, 1.0);
        onto.translate([5.0, 5.0, 0.0]);
        check(&[ell, onto], Placement::AsGiven, &real, &mut diagnostics).unwrap();
        assert_eq!(diagnostics.codes(), ["slicing.objects_overlap"]);
        assert!(
            diagnostics.as_slice()[0]
                .message
                .contains("75.0 mm² of their outlines")
        );
        // The real outline of a ring has its hole.
        let ring = top_view(&slicer_core::primitives::tube(20.0, 10.0, 5.0, 64));
        let area = polygon::area(&ring);
        assert!(
            (area - 3.0 * std::f64::consts::PI * 100.0).abs() < 15.0,
            "{area}"
        );
    }
}
