//! Objects on the plate with settings of their own: which settings an
//! object may change, and the region of every layer that each group of
//! objects with the same settings takes.

use slicer_core::slice::slice_mesh;
use slicer_core::{Mesh, Shapes, parallel_map, polygon};
use slicer_profiles::Settings;

use crate::plate::{self, Placement};
use crate::{DEGENERATE_HEIGHT, MAX_HOLE_EDGES};

/// Process settings that an object on the plate may set for itself,
/// overriding the plate's: walls, top and bottom shells, sparse infill and
/// whether it gets supports. Everything else, among it the layer heights,
/// line width, skirt, brim, the shape of supports, the seam, speeds and
/// travel, stays the same for the whole plate.
pub const OBJECT_SETTINGS: &[&str] = &[
    "wall_loops",
    "wall_order",
    "top_layers",
    "bottom_layers",
    "infill_density",
    "infill_pattern",
    "infill_angle",
    "infill_connect",
    "infill_overlap",
    "supports",
];

/// The settings an object prints with: `plate` with the values of
/// [`OBJECT_SETTINGS`] taken from `own`.
pub fn object_settings(plate: &Settings, own: &Settings) -> Settings {
    Settings {
        wall_loops: own.wall_loops,
        wall_order: own.wall_order,
        top_layers: own.top_layers,
        bottom_layers: own.bottom_layers,
        infill_density: own.infill_density,
        infill_pattern: own.infill_pattern,
        infill_angle: own.infill_angle,
        infill_connect: own.infill_connect,
        infill_overlap: own.infill_overlap,
        supports: own.supports,
        ..plate.clone()
    }
}

/// The different settings among `count` objects, the first of them those
/// of the first object, and for every object the index of its settings.
/// Object `i` has `own[i]` if given, otherwise the plate's.
pub(crate) fn groups(
    count: usize,
    own: &[Option<Settings>],
    plate: &Settings,
) -> (Vec<Settings>, Vec<usize>) {
    let mut groups: Vec<Settings> = Vec::new();
    let mut membership = Vec::with_capacity(count);
    for index in 0..count {
        let settings = match own.get(index) {
            Some(Some(own)) => object_settings(plate, own),
            _ => plate.clone(),
        };
        let group = match groups.iter().position(|g| *g == settings) {
            Some(group) => group,
            None => {
                groups.push(settings);
                groups.len() - 1
            }
        };
        membership.push(group);
    }
    if groups.is_empty() {
        groups.push(plate.clone());
    }
    (groups, membership)
}

/// For every group of objects, the part of every layer it takes, cut at
/// `cut_heights` after the objects were put on the bed as for the whole
/// plate and moved by `shift`. Where objects of different groups overlap,
/// the later group takes the overlap.
// ponytail: every group is repaired and cut once more on its own, so the
// work of preparing and cutting grows with the number of groups. Keeping
// track of which object every triangle comes from through the repair is
// the upgrade.
pub(crate) fn regions(
    objects: &[Mesh],
    placement: Placement,
    membership: &[usize],
    groups: usize,
    shift: Option<[f64; 3]>,
    cut_heights: &[f64],
) -> Vec<Vec<Shapes>> {
    let own: Vec<Vec<Shapes>> = (0..groups)
        .map(|group| {
            let members: Vec<Mesh> = objects
                .iter()
                .zip(membership)
                .filter(|&(_, &m)| m == group)
                .map(|(object, _)| object.clone())
                .collect();
            let mut mesh = plate::merge(&members, placement);
            mesh.orient();
            mesh.remove_degenerate(DEGENERATE_HEIGHT);
            mesh.close_holes(MAX_HOLE_EDGES);
            if let Some(shift) = shift {
                mesh.translate(shift);
            }
            slice_mesh(&mesh, cut_heights)
        })
        .collect();
    (0..groups)
        .map(|group| {
            parallel_map(cut_heights.len(), |index| {
                own[group + 1..]
                    .iter()
                    .fold(own[group][index].clone(), |region, later| {
                        polygon::difference(&region, &later[index])
                    })
            })
        })
        .collect()
}

/// On every layer, everything that groups whose supports are `on` take.
pub(crate) fn with_supports(regions: &[Vec<Shapes>], groups: &[Settings], on: bool) -> Vec<Shapes> {
    let layers = regions.first().map_or(0, Vec::len);
    parallel_map(layers, |index| {
        regions
            .iter()
            .zip(groups)
            .filter(|(_, settings)| settings.supports == on)
            .fold(Vec::new(), |area, (region, _)| {
                polygon::union(&area, &region[index])
            })
    })
}

/// The regions to print with their extruder and settings: the regions of
/// the extruders (`extruders`, empty if one extruder prints everything)
/// split by the regions of the groups of objects (`objects`, empty if all
/// objects have the same settings). Without either, empty.
pub(crate) fn split<'a>(
    extruders: Vec<(u32, Vec<Shapes>)>,
    objects: Vec<Vec<Shapes>>,
    groups: &'a [Settings],
    extruder: u32,
    settings: &'a Settings,
) -> Vec<(u32, &'a Settings, Vec<Shapes>)> {
    if objects.is_empty() {
        return extruders
            .into_iter()
            .map(|(e, region)| (e, settings, region))
            .collect();
    }
    if extruders.is_empty() {
        return objects
            .into_iter()
            .zip(groups)
            .map(|(region, settings)| (extruder, settings, region))
            .collect();
    }
    let mut regions = Vec::new();
    for (e, region) in &extruders {
        for (group, settings) in objects.iter().zip(groups) {
            let part = parallel_map(region.len(), |index| {
                polygon::intersection(&region[index], &group[index])
            });
            regions.push((*e, settings, part));
        }
    }
    regions
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::AtomicBool;

    use slicer_core::Diagnostics;
    use slicer_profiles::{InfillPattern, WallOrder};

    use super::*;
    use crate::{Layer, PathKind, Progress, Toolpaths, generate_plate_with_object_settings};

    /// Slices `objects` where they are, object `i` with `own[i]`.
    fn slice(
        objects: &[Mesh],
        own: &[Option<Settings>],
        settings: &Settings,
    ) -> (Toolpaths, Diagnostics) {
        let mut diagnostics = Diagnostics::new();
        let toolpaths = generate_plate_with_object_settings(
            objects,
            own,
            Placement::AsGiven,
            settings,
            &mut diagnostics,
            &AtomicBool::new(false),
            &Progress::new(),
        )
        .unwrap();
        (toolpaths, diagnostics)
    }

    /// A 20 mm cube with its smallest corner at `x`, 50 mm.
    fn cube(x: f64) -> Mesh {
        let mut mesh = Mesh::cuboid(20.0, 20.0, 20.0);
        mesh.translate([x, 50.0, 0.0]);
        mesh
    }

    /// A column with a plate on top that reaches out on both sides, its
    /// smallest corner at `x`, 50 mm.
    fn tee(x: f64) -> Mesh {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 12.0);
        mesh.translate([x + 10.0, 50.0, 0.0]);
        let mut plate = Mesh::cuboid(30.0, 10.0, 2.0);
        plate.translate([x, 50.0, 10.0]);
        mesh.merge(&plate);
        mesh
    }

    /// The kinds of the paths of `layer` that lie left of X = 100 (the
    /// first object) or right of it (the second).
    fn kinds(layer: &Layer, left: bool) -> Vec<PathKind> {
        let mut kinds: Vec<PathKind> = layer
            .paths
            .iter()
            .filter(|p| (p.points[0][0] < 100.0) == left)
            .map(|p| p.kind)
            .collect();
        kinds.sort_by_key(|k| *k as u8);
        kinds.dedup();
        kinds
    }

    #[test]
    fn objects_print_with_their_own_walls_and_infill() {
        let settings = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let objects = [cube(20.0), cube(140.0)];
        let solid = Settings {
            infill_density: 100.0,
            wall_loops: 3,
            ..Settings::default()
        };
        let empty = Settings {
            infill_density: 0.0,
            ..Settings::default()
        };
        let (plain, _) = slice(&objects, &[], &settings);
        let (toolpaths, diagnostics) = slice(&objects, &[Some(solid), Some(empty)], &settings);
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.codes());
        assert_eq!(toolpaths.layers.len(), plain.layers.len());
        use PathKind::*;
        let middle = &toolpaths.layers[50];
        assert_eq!(
            kinds(&plain.layers[50], true),
            [OuterWall, InnerWall, SparseInfill]
        );
        assert_eq!(kinds(middle, true), [OuterWall, InnerWall, SolidInfill]);
        assert_eq!(kinds(middle, false), [OuterWall, InnerWall]);
        let loops = |kind: PathKind, left: bool| {
            middle
                .paths
                .iter()
                .filter(|p| p.kind == kind && (p.points[0][0] < 100.0) == left)
                .count()
        };
        assert_eq!((loops(InnerWall, true), loops(InnerWall, false)), (2, 1));
        // Top and bottom stay solid on the empty one.
        assert_eq!(
            kinds(&toolpaths.layers[1], false),
            [OuterWall, InnerWall, SolidInfill]
        );
    }

    #[test]
    fn objects_without_own_values_print_as_before() {
        let settings = Settings::default();
        let objects = [cube(20.0), tee(140.0)];
        let (plain, warnings) = slice(&objects, &[], &settings);
        // Own settings that equal the plate's, or differ only in keys an
        // object cannot change, are no settings of its own.
        let same = Settings {
            layer_height: 0.3,
            ..settings.clone()
        };
        let (own, own_warnings) = slice(&objects, &[Some(same), Some(settings.clone())], &settings);
        assert_eq!(own, plain);
        assert_eq!(own_warnings.codes(), warnings.codes());
        // Equal own settings on all objects are the plate's.
        let dense = Settings {
            infill_density: 40.0,
            ..settings.clone()
        };
        let (both, _) = slice(
            &objects,
            &[Some(dense.clone()), Some(dense.clone())],
            &settings,
        );
        assert_eq!(both, slice(&objects, &[], &dense).0);
    }

    #[test]
    fn supports_go_only_under_objects_that_ask_for_them() {
        let objects = [tee(20.0), tee(140.0)];
        let with = Settings {
            supports: true,
            ..Settings::default()
        };
        let without = Settings::default();
        let supported = |toolpaths: &Toolpaths, left: bool| {
            toolpaths
                .layers
                .iter()
                .any(|layer| kinds(layer, left).contains(&PathKind::Support))
        };
        // The plate has none, the first object asks for them; then the
        // other way round.
        for (plate, own) in [(&without, &with), (&with, &without)] {
            let (toolpaths, warnings) = slice(&objects, &[Some(own.clone()), None], plate);
            assert_eq!(supported(&toolpaths, true), own.supports);
            assert_eq!(supported(&toolpaths, false), plate.supports);
            assert_eq!(warnings.codes(), ["slicing.unsupported_overhangs"]);
        }
    }

    #[test]
    fn own_settings_and_several_extruders() {
        let settings = Settings {
            skirt_loops: 0,
            extruder_count: 2,
            ..Settings::default()
        };
        let mut second = cube(140.0);
        second.extruders = vec![1; second.triangles.len()];
        let objects = [cube(20.0), second];
        let solid = Settings {
            infill_density: 100.0,
            ..Settings::default()
        };
        let (toolpaths, _) = slice(&objects, &[Some(solid)], &settings);
        let middle = &toolpaths.layers[50];
        use PathKind::*;
        assert_eq!(kinds(middle, true), [OuterWall, InnerWall, SolidInfill]);
        assert_eq!(
            kinds(middle, false),
            [OuterWall, InnerWall, SparseInfill, PrimeTower]
        );
        for path in middle.paths.iter().filter(|p| p.kind != PrimeTower) {
            let left = path.points[0][0] < 100.0;
            assert_eq!(path.extruder, if left { 0 } else { 1 });
        }
    }

    #[test]
    fn objects_take_only_their_own_keys() {
        let plate = Settings::default();
        let own = Settings {
            wall_loops: 4,
            wall_order: WallOrder::OuterInner,
            top_layers: 2,
            bottom_layers: 7,
            infill_density: 100.0,
            infill_pattern: InfillPattern::Gyroid,
            infill_angle: 10.0,
            infill_connect: false,
            infill_overlap: 30.0,
            supports: true,
            layer_height: 0.1,
            brim_width: 5.0,
            ..Settings::default()
        };
        let mixed = object_settings(&plate, &own).values();
        let (plate, own) = (plate.values(), own.values());
        for (key, value) in &mixed {
            if OBJECT_SETTINGS.contains(&key.as_str()) {
                assert_ne!(own[key], plate[key], "{key} unchanged in the test");
                assert_eq!(*value, own[key], "{key}");
            } else {
                assert_eq!(*value, plate[key], "{key}");
            }
        }
        for key in OBJECT_SETTINGS {
            assert!(mixed.contains_key(*key), "{key}");
        }
    }

    #[test]
    fn objects_with_equal_settings_share_a_group() {
        let plate = Settings::default();
        let dense = Settings {
            infill_density: 100.0,
            ..Settings::default()
        };
        // Only the keys of an object count: another layer height is the
        // plate's settings again.
        let thin = Settings {
            layer_height: 0.1,
            ..Settings::default()
        };
        let own = [None, Some(dense.clone()), Some(thin), Some(dense)];
        let (groups, membership) = groups(5, &own, &plate);
        assert_eq!(membership, [0, 1, 0, 1, 0]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], plate);
        assert_eq!(groups[1].infill_density, 100.0);
        assert_eq!(super::groups(0, &[], &plate).0, [plate]);
    }
}
