//! Negative volumes of objects: meshes whose volume is cut out of their
//! object on every layer right after slicing, before regions, walls, top
//! and bottom surfaces, supports and modifiers are worked out. Everything
//! after slicing sees the object as if it had been modelled with the hole,
//! so walls go round the cut, surfaces form at it and an opening facing
//! down gets supports.

use slicer_core::slice::slice_mesh;
use slicer_core::{Mesh, Shapes, parallel_map, polygon};

use crate::DEGENERATE_HEIGHT;
use crate::plate::Placement;

/// Largest number of negative volumes of one object.
pub const MAX_NEGATIVES: usize = 64;

/// What the negative volumes of the objects (`negatives[i]` those of
/// object `i`, in the coordinates of its mesh) cut out of the plate on
/// every layer, cut at `cut_heights` after they were put on the bed with
/// their objects as for the whole plate and moved by `shift`. Empty if no
/// object has a negative volume, so that such plates print exactly as
/// without them. A negative volume reaches only its own object: where
/// another object overlaps it, that object keeps its part, unless a
/// negative volume of its own takes it as well.
// ponytail: only objects whose outline seen from above overlaps the
// outline of a negative volume of another object are cut once more on
// their own to keep their part; on a plate whose objects stand apart, as
// they should, nothing is cut twice.
pub(crate) fn cuts(
    objects: &[Mesh],
    negatives: &[Vec<Mesh>],
    placement: Placement,
    shift: Option<[f64; 3]>,
    cut_heights: &[f64],
) -> Vec<Shapes> {
    let with: Vec<usize> = (0..objects.len())
        .filter(|&i| negatives.get(i).is_some_and(|list| !list.is_empty()))
        .collect();
    if with.is_empty() {
        return Vec::new();
    }
    // Repaired, put on the bed with the object and cut, as the plate is.
    let cut = |mut mesh: Mesh, lift: f64| {
        mesh.repair(DEGENERATE_HEIGHT);
        let [x, y, z] = shift.unwrap_or([0.0; 3]);
        mesh.translate([x, y, z - lift]);
        slice_mesh(&mesh, cut_heights)
    };
    // An object placed as given stands on the bed on its own; its negative
    // volumes go down with it.
    let lift = |object: &Mesh| match (placement, object.bounds()) {
        (Placement::AsGiven, Some((min, _))) => min[2],
        _ => 0.0,
    };
    // The negative volumes of every object together, by object.
    let mut inside: Vec<Option<Vec<Shapes>>> = vec![None; objects.len()];
    for &object in &with {
        let (Some(mesh), Some(list)) = (objects.get(object), negatives.get(object)) else {
            continue;
        };
        let lift = lift(mesh);
        let volumes: Vec<Vec<Shapes>> = list
            .iter()
            .take(MAX_NEGATIVES)
            .map(|volume| cut(volume.clone(), lift))
            .collect();
        inside[object] = Some(parallel_map(cut_heights.len(), |index| {
            volumes
                .iter()
                .fold(Vec::new(), |area, layers| match layers.get(index) {
                    Some(layer) if !layer.is_empty() => polygon::union(&area, layer),
                    _ => area,
                })
        }));
    }
    // Other objects reaching into a negative volume keep what lies outside
    // their own negative volumes.
    let outline = |mesh: &Mesh| mesh.bounds().map(|(lo, hi)| [lo[0], lo[1], hi[0], hi[1]]);
    let overlap =
        |a: [f64; 4], b: [f64; 4]| a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
    let mut kept: Vec<Vec<Shapes>> = Vec::new();
    for (index, mesh) in objects.iter().enumerate() {
        let Some(own) = outline(mesh) else {
            continue;
        };
        let near = with.iter().any(|&other| {
            other != index
                && negatives.get(other).is_some_and(|list| {
                    list.iter()
                        .take(MAX_NEGATIVES)
                        .filter_map(outline)
                        .any(|volume| overlap(own, volume))
                })
        });
        if !near {
            continue;
        }
        let whole = cut(mesh.clone(), lift(mesh));
        let own_inside = inside.get(index).and_then(Option::as_ref);
        kept.push(match own_inside {
            Some(own_inside) => parallel_map(cut_heights.len(), |layer| {
                match (whole.get(layer), own_inside.get(layer)) {
                    (Some(area), Some(cut)) if !cut.is_empty() => polygon::difference(area, cut),
                    (Some(area), _) => area.clone(),
                    _ => Vec::new(),
                }
            }),
            None => whole,
        });
    }
    parallel_map(cut_heights.len(), |layer| {
        let mut area: Shapes = Vec::new();
        for layers in inside.iter().flatten() {
            if let Some(part) = layers.get(layer).filter(|part| !part.is_empty()) {
                area = polygon::union(&area, part);
            }
        }
        for layers in &kept {
            if area.is_empty() {
                break;
            }
            if let Some(part) = layers.get(layer).filter(|part| !part.is_empty()) {
                area = polygon::difference(&area, part);
            }
        }
        area
    })
}

/// `layers` with `cuts` (see [`cuts`]) taken out of every layer; unchanged
/// without cuts, and every layer without a cut stays as it is.
pub(crate) fn apply(layers: Vec<Shapes>, cuts: &[Shapes]) -> Vec<Shapes> {
    if cuts.is_empty() {
        return layers;
    }
    parallel_map(layers.len(), |index| match cuts.get(index) {
        Some(cut) if !cut.is_empty() && !layers[index].is_empty() => {
            polygon::difference(&layers[index], cut)
        }
        _ => layers[index].clone(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::AtomicBool;

    use slicer_core::Diagnostics;
    use slicer_profiles::Settings;

    use super::*;
    use crate::{
        FdmError, OwnSettings, PathKind, Progress, Toolpaths, generate_plate_with_own_settings,
    };

    /// Slices `objects` placed as `placement`, object `i` with the negative
    /// volumes `negatives[i]`.
    fn slice(
        objects: &[Mesh],
        negatives: &[Vec<Mesh>],
        placement: Placement,
        settings: &Settings,
    ) -> Result<Toolpaths, FdmError> {
        let own = OwnSettings {
            negatives,
            ..OwnSettings::default()
        };
        generate_plate_with_own_settings(
            objects,
            own,
            placement,
            settings,
            &mut Diagnostics::new(),
            &AtomicBool::new(false),
            &Progress::new(),
        )
    }

    /// An upright cylinder of radius `r` around (`x`, `y`) from `z` up by
    /// `height`, with 64 sides.
    fn cylinder(r: f64, [x, y, z]: [f64; 3], height: f64) -> Mesh {
        let sides = 64;
        let at = |i: usize| {
            let angle = std::f64::consts::TAU * i as f64 / sides as f64;
            [(x + r * angle.cos()) as f32, (y + r * angle.sin()) as f32]
        };
        let (bottom, top) = (z as f32, (z + height) as f32);
        let (cx, cy) = (x as f32, y as f32);
        let mut soup = Vec::new();
        for i in 0..sides {
            let ([ax, ay], [bx, by]) = (at(i), at(i + 1));
            soup.push([[cx, cy, bottom], [bx, by, bottom], [ax, ay, bottom]]);
            soup.push([[cx, cy, top], [ax, ay, top], [bx, by, top]]);
            soup.push([[ax, ay, bottom], [bx, by, bottom], [bx, by, top]]);
            soup.push([[ax, ay, bottom], [bx, by, top], [ax, ay, top]]);
        }
        Mesh::from_triangle_soup(&soup)
    }

    /// A cuboid of `size` with its smallest corner at `corner`.
    fn block(size: [f64; 3], corner: [f64; 3]) -> Mesh {
        let mut mesh = Mesh::cuboid(size[0], size[1], size[2]);
        mesh.translate(corner);
        mesh
    }

    /// Roughly the volume extruded: the length of every path times its
    /// width and the layer height, for the paths of object `object` (all
    /// paths with `None`).
    fn extruded(toolpaths: &Toolpaths, settings: &Settings, object: Option<u32>) -> f64 {
        let mut volume = 0.0;
        for layer in &toolpaths.layers {
            for path in &layer.paths {
                if object.is_some_and(|object| path.object != object) {
                    continue;
                }
                let mut points = path.points.clone();
                if path.closed
                    && let Some(&first) = points.first()
                {
                    points.push(first);
                }
                let length: f64 = points
                    .windows(2)
                    .map(|p| ((p[1][0] - p[0][0]).powi(2) + (p[1][1] - p[0][1]).powi(2)).sqrt())
                    .sum();
                volume += length * path.width.unwrap_or(settings.line_width) * layer.height;
            }
        }
        volume
    }

    /// The outer walls of the layer closest to `z`.
    fn outer_walls(toolpaths: &Toolpaths, z: f64) -> Vec<&crate::Path> {
        let layer = toolpaths
            .layers
            .iter()
            .min_by(|a, b| (a.z - z).abs().total_cmp(&(b.z - z).abs()))
            .unwrap();
        layer
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::OuterWall)
            .collect()
    }

    #[test]
    fn a_negative_cylinder_leaves_a_hole_with_walls() {
        // Solid, so that what is printed follows the volume.
        let settings = Settings {
            infill_density: 100.0,
            skirt_loops: 0,
            ..Settings::default()
        };
        let cube = block([20.0, 20.0, 20.0], [0.0; 3]);
        // A 10 mm cylinder through the middle of the cube, reaching out at
        // top and bottom.
        let hole = cylinder(5.0, [10.0, 10.0, -2.0], 24.0);
        let objects = std::slice::from_ref(&cube);
        let plain = slice(objects, &[], Placement::Centre, &settings).unwrap();
        let cut = slice(objects, &[vec![hole]], Placement::Centre, &settings).unwrap();
        assert_eq!(plain.layers.len(), cut.layers.len());
        // In the middle the hole gets an outer wall of its own, close
        // round the cylinder (the cube is centred on the bed).
        assert_eq!(outer_walls(&plain, 10.0).len(), 1);
        let walls = outer_walls(&cut, 10.0);
        assert_eq!(walls.len(), 2);
        let centre = [settings.bed_size_x / 2.0, settings.bed_size_y / 2.0];
        let round = walls.iter().any(|wall| {
            wall.points.iter().all(|p| {
                let r = ((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2)).sqrt();
                (r - 5.0 - settings.line_width / 2.0).abs() < 0.1
            })
        });
        assert!(round, "no wall round the hole");
        // Less is printed, as much less as the hole takes of the cube:
        // 78.5 of 400 mm² on every layer.
        let (before, after) = (
            extruded(&plain, &settings, None),
            extruded(&cut, &settings, None),
        );
        let share = after / before;
        assert!((share - 0.804).abs() < 0.03, "{after} of {before}");
    }

    #[test]
    fn a_cube_cut_out_of_a_cube_leaves_a_hole_too() {
        let settings = Settings::default();
        let cube = block([20.0, 20.0, 20.0], [0.0; 3]);
        let hole = block([10.0, 10.0, 30.0], [5.0, 5.0, -5.0]);
        let cut = slice(&[cube], &[vec![hole]], Placement::Centre, &settings).unwrap();
        assert_eq!(outer_walls(&cut, 10.0).len(), 2);
    }

    #[test]
    fn a_pocket_open_below_gets_supports() {
        let settings = Settings {
            supports: true,
            ..Settings::default()
        };
        let cube = block([30.0, 30.0, 20.0], [0.0; 3]);
        // A pocket 16 mm wide and 10 mm high from below: wider than a
        // bridge may span.
        let pocket = block([16.0, 16.0, 11.0], [7.0, 7.0, -1.0]);
        let count = |toolpaths: &Toolpaths, kinds: &[PathKind]| {
            toolpaths
                .layers
                .iter()
                .flat_map(|layer| &layer.paths)
                .filter(|p| kinds.contains(&p.kind))
                .count()
        };
        let support = [PathKind::Support, PathKind::SupportInterface];
        let objects = std::slice::from_ref(&cube);
        let plain = slice(objects, &[], Placement::Centre, &settings).unwrap();
        assert_eq!(count(&plain, &support), 0);
        let cut = slice(
            objects,
            &[vec![pocket.clone()]],
            Placement::Centre,
            &settings,
        )
        .unwrap();
        assert!(count(&cut, &support) > 0);
        // Without supports the pocket's roof is bridged.
        let off = Settings {
            supports: false,
            ..settings
        };
        let bridged = slice(objects, &[vec![pocket]], Placement::Centre, &off).unwrap();
        assert_eq!(count(&bridged, &support), 0);
        assert!(count(&bridged, &[PathKind::Bridge]) > 0);
    }

    #[test]
    fn objects_with_settings_of_their_own_are_cut_too() {
        let settings = Settings::default();
        let objects = [
            block([20.0, 20.0, 20.0], [60.0, 60.0, 0.0]),
            block([20.0, 20.0, 20.0], [100.0, 60.0, 0.0]),
        ];
        let own = [
            Some(Settings {
                infill_density: 40.0,
                ..settings.clone()
            }),
            None,
        ];
        let negatives = [vec![cylinder(5.0, [70.0, 70.0, -2.0], 24.0)], vec![]];
        let toolpaths = generate_plate_with_own_settings(
            &objects,
            OwnSettings {
                objects: &own,
                negatives: &negatives,
                ..OwnSettings::default()
            },
            Placement::AsGiven,
            &settings,
            &mut Diagnostics::new(),
            &AtomicBool::new(false),
            &Progress::new(),
        )
        .unwrap();
        // The hole in the first cube and both cubes' outlines.
        assert_eq!(outer_walls(&toolpaths, 10.0).len(), 3);
    }

    #[test]
    fn plates_without_negatives_print_as_before() {
        let settings = Settings::default();
        let objects = [
            block([20.0, 20.0, 20.0], [60.0, 60.0, 0.0]),
            cylinder(8.0, [140.0, 140.0, 0.0], 15.0),
        ];
        let plain = slice(&objects, &[], Placement::AsGiven, &settings).unwrap();
        let empty = slice(&objects, &[vec![], vec![]], Placement::AsGiven, &settings).unwrap();
        assert_eq!(plain, empty);
    }

    #[test]
    fn a_negative_reaches_only_its_own_object() {
        let settings = Settings {
            label_objects: true,
            infill_density: 100.0,
            ..Settings::default()
        };
        let objects = [
            block([20.0, 20.0, 20.0], [60.0, 60.0, 0.0]),
            block([20.0, 20.0, 20.0], [85.0, 60.0, 0.0]),
        ];
        // A slab through the right half of the first cube that reaches
        // well into the second one.
        let slab = block([30.0, 6.0, 30.0], [70.0, 67.0, -5.0]);
        let plain = slice(&objects, &[], Placement::AsGiven, &settings).unwrap();
        let cut = slice(&objects, &[vec![slab]], Placement::AsGiven, &settings).unwrap();
        let first = |t: &Toolpaths| extruded(t, &settings, Some(0));
        let second = |t: &Toolpaths| extruded(t, &settings, Some(1));
        assert!(first(&cut) < 0.97 * first(&plain));
        // The second cube keeps its walls exactly; its infill lines may be
        // joined in another order.
        let walls = |t: &Toolpaths| {
            let mut walls = t.clone();
            for layer in &mut walls.layers {
                layer
                    .paths
                    .retain(|p| matches!(p.kind, PathKind::OuterWall | PathKind::InnerWall));
            }
            extruded(&walls, &settings, Some(1))
        };
        assert_eq!(walls(&plain), walls(&cut));
        let (before, after) = (second(&plain), second(&cut));
        assert!(
            (after - before).abs() < 1e-3 * before,
            "{after} != {before}"
        );
    }

    #[test]
    fn layers_cut_away_whole_stay_empty() {
        let settings = Settings::default();
        let cube = block([20.0, 20.0, 20.0], [0.0; 3]);
        // The top 8 mm are cut away.
        let top = block([30.0, 30.0, 10.0], [-5.0, -5.0, 12.0]);
        let objects = std::slice::from_ref(&cube);
        let cut = slice(objects, &[vec![top]], Placement::Centre, &settings).unwrap();
        let printed = cut
            .layers
            .iter()
            .filter(|layer| !layer.paths.is_empty())
            .map(|layer| layer.z)
            .fold(0.0, f64::max);
        assert!((11.5..=12.1).contains(&printed), "{printed}");
        // The new top gets top surfaces.
        let below = cut
            .layers
            .iter()
            .min_by(|a, b| (a.z - 11.9).abs().total_cmp(&(b.z - 11.9).abs()))
            .unwrap();
        assert!(below.paths.iter().any(|p| p.kind == PathKind::SolidInfill));
        // Cut away entirely, there is nothing to print.
        let all = block([30.0, 30.0, 30.0], [-5.0, -5.0, -5.0]);
        let error = slice(objects, &[vec![all]], Placement::Centre, &settings).unwrap_err();
        assert!(matches!(error, FdmError::NothingToPrint), "{error:?}");
    }

    #[test]
    fn negatives_go_with_their_objects_printed_one_after_the_other() {
        let settings = Settings {
            sequential_print: true,
            ..Settings::default()
        };
        let objects = [
            block([20.0, 20.0, 20.0], [0.0, 0.0, 0.0]),
            block([20.0, 20.0, 20.0], [60.0, 0.0, 0.0]),
        ];
        let hole = cylinder(5.0, [70.0, 10.0, -2.0], 24.0);
        let negatives = [vec![], vec![hole]];
        let cut = slice(&objects, &negatives, Placement::Centre, &settings).unwrap();
        // Two objects of the same height, each with all its layers: the
        // first has one outer wall in the middle, the second two.
        let middles: Vec<usize> = cut
            .layers
            .iter()
            .filter(|layer| (layer.z - 10.0).abs() < 0.1)
            .map(|layer| {
                layer
                    .paths
                    .iter()
                    .filter(|p| p.kind == PathKind::OuterWall)
                    .count()
            })
            .collect();
        assert_eq!(middles, [1, 2]);
    }
}
