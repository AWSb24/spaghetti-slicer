//! Printing the objects on a plate one after the other, every one with all
//! its layers, instead of together layer by layer (`sequential_print`).

use std::sync::atomic::AtomicBool;

use slicer_core::{Diagnostics, Mesh, polygon};
use slicer_profiles::Settings;

use crate::plate::{self, Placement};
use crate::{FdmError, NO_OBJECT, OwnSettings, Progress, Toolpaths};

/// Area in mm² by which the outlines of two objects, grown by half the
/// clearance each, must overlap for the objects to count as too close.
const MIN_OVERLAP: f64 = 0.01;

/// Generates every object on its own, in the order of [`order`], and puts
/// their layers one after the other. Every object stands on the bed by
/// itself. The skirt goes round the first object only and there is no prime
/// tower; every object keeps its brim.
// ponytail: the prime tower is left out, since every object would start a
// tower of its own in the same place. Diagnostics come once per object, and
// the progress starts over with every object.
pub(crate) fn generate(
    objects: &[Mesh],
    own: OwnSettings,
    placement: Placement,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
    stop: &AtomicBool,
    progress: &Progress,
) -> Result<Toolpaths, FdmError> {
    plate::check(objects, placement, settings, diagnostics)?;
    let mut placed = objects.to_vec();
    // The modifiers, negative volumes, support blockers and enforcers go
    // along with their objects.
    let mut modifiers = own.modifiers.to_vec();
    let mut negatives = own.negatives.to_vec();
    let mut support_volumes = own.support_volumes.to_vec();
    if placement == Placement::Centre {
        // The objects keep their places relative to each other, centred on
        // the bed as a group.
        let merged = plate::merge(objects, placement);
        let (min, max) = merged.bounds().ok_or(FdmError::EmptyModel)?;
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let volume = [
            settings.bed_size_x,
            settings.bed_size_y,
            settings.max_print_height,
        ];
        if (0..3).any(|a| size[a] > volume[a] + 1e-6) {
            return Err(FdmError::TooLarge {
                model: size,
                volume,
            });
        }
        let shift = [
            volume[0] / 2.0 - (min[0] + max[0]) / 2.0,
            volume[1] / 2.0 - (min[1] + max[1]) / 2.0,
            0.0,
        ];
        for mesh in &mut placed {
            mesh.translate(shift);
        }
        for modifier in modifiers.iter_mut().flatten() {
            modifier.mesh.translate(shift);
        }
        for volume in negatives.iter_mut().flatten() {
            volume.translate(shift);
        }
        for volume in support_volumes.iter_mut().flatten() {
            volume.mesh.translate(shift);
        }
    }
    let order = order(&placed, settings)?;
    let mut layers = Vec::new();
    for (place, &index) in order.iter().enumerate() {
        let alone = Settings {
            sequential_print: false,
            skirt_loops: if place == 0 { settings.skirt_loops } else { 0 },
            prime_tower_width: 0.0,
            ..settings.clone()
        };
        let settings_of_one = [own.objects.get(index).cloned().flatten()];
        let ranges_of_one = [own.ranges.get(index).cloned().unwrap_or_default()];
        let modifiers_of_one = [modifiers.get(index).cloned().unwrap_or_default()];
        let negatives_of_one = [negatives.get(index).cloned().unwrap_or_default()];
        let volumes_of_one = [support_volumes.get(index).cloned().unwrap_or_default()];
        let one = OwnSettings {
            objects: &settings_of_one,
            ranges: &ranges_of_one,
            modifiers: &modifiers_of_one,
            negatives: &negatives_of_one,
            support_volumes: &volumes_of_one,
        };
        let toolpaths = crate::generate_plate_with_own_settings(
            std::slice::from_ref(&placed[index]),
            one,
            Placement::AsGiven,
            &alone,
            diagnostics,
            stop,
            progress,
        )?;
        for mut layer in toolpaths.layers {
            for path in &mut layer.paths {
                if path.object != NO_OBJECT {
                    path.object = index as u32;
                }
            }
            layers.push(layer);
        }
    }
    Ok(Toolpaths {
        layers,
        object_settings: crate::objects::speed_table(objects.len(), own.objects, settings),
    })
}

/// The order to print the objects in: lowest first, so that the tallest
/// comes last; objects of equal height keep their order. Fails if two
/// objects seen from above stand closer together than
/// `extruder_clearance_radius`, or if an object besides the last one is
/// taller than `gantry_height`.
// ponytail: convex outlines without the brim, and a single number for the
// print head. Shapes of the print head per side and a brim-aware distance
// are the upgrade.
fn order(objects: &[Mesh], settings: &Settings) -> Result<Vec<usize>, FdmError> {
    let reach = settings.extruder_clearance_radius / 2.0;
    let grown: Vec<_> = objects
        .iter()
        .map(|o| polygon::offset(&vec![vec![plate::footprint(o)]], reach))
        .collect();
    for i in 0..grown.len() {
        for j in i + 1..grown.len() {
            if polygon::area(&polygon::intersection(&grown[i], &grown[j])) > MIN_OVERLAP {
                return Err(FdmError::ObjectsTooClose {
                    objects: [i, j],
                    clearance: settings.extruder_clearance_radius,
                });
            }
        }
    }
    let heights: Vec<f64> = objects
        .iter()
        .map(|o| o.bounds().map_or(0.0, |(lo, hi)| hi[2] - lo[2]))
        .collect();
    let mut order: Vec<usize> = (0..objects.len()).collect();
    order.sort_by(|&a, &b| heights[a].total_cmp(&heights[b]));
    if let [.., object, _] = order[..]
        && heights[object] > settings.gantry_height + 1e-6
    {
        return Err(FdmError::ObjectTooTall {
            object,
            height: heights[object],
            gantry: settings.gantry_height,
        });
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use slicer_core::Diagnose;

    use super::*;
    use crate::{PathKind, generate_plate};

    fn cube(x: f64, height: f64) -> Mesh {
        let mut mesh = Mesh::cuboid(10.0, 10.0, height);
        mesh.translate([x, 0.0, 0.0]);
        mesh
    }

    fn plate(objects: &[Mesh], settings: &Settings) -> Result<Toolpaths, FdmError> {
        let stop = AtomicBool::new(false);
        generate_plate(
            objects,
            Placement::Centre,
            settings,
            &mut Diagnostics::new(),
            &stop,
        )
    }

    #[test]
    fn paths_know_their_object() {
        let objects = [cube(0.0, 1.0), cube(20.0, 1.0)];
        let mut settings = Settings::default();
        let together = plate(&objects, &settings).unwrap();
        let all = |t: &Toolpaths| -> Vec<u32> {
            t.layers
                .iter()
                .flat_map(|l| &l.paths)
                .map(|p| p.object)
                .collect()
        };
        assert!(all(&together).iter().all(|&o| o == NO_OBJECT));
        settings.label_objects = true;
        let labelled = plate(&objects, &settings).unwrap();
        for layer in &labelled.layers {
            for path in &layer.paths {
                let expected = match path.kind {
                    PathKind::Skirt => NO_OBJECT,
                    // The first cube lies left of the second.
                    _ if path.points[0][0] < 110.0 => 0,
                    _ => 1,
                };
                assert_eq!(path.object, expected, "{:?}", path.kind);
            }
        }
        // Only the objects differ.
        let mut cleared = labelled.clone();
        for path in cleared.layers.iter_mut().flat_map(|l| &mut l.paths) {
            path.object = NO_OBJECT;
        }
        assert_eq!(cleared, together);
        // One object on its own knows it without being asked.
        let single = plate(&objects[..1], &Settings::default()).unwrap();
        assert!(single.layers[0].paths.iter().any(|p| p.object == 0));
    }

    #[test]
    fn objects_print_one_after_the_other_lowest_first() {
        let objects = [cube(0.0, 2.0), cube(50.0, 1.0)];
        let settings = Settings {
            sequential_print: true,
            ..Settings::default()
        };
        let toolpaths = plate(&objects, &settings).unwrap();
        let z: Vec<f64> = toolpaths.layers.iter().map(|l| l.z).collect();
        assert_eq!(z.len(), 15);
        // The lower second cube first, then the first one.
        for (k, &z) in z.iter().enumerate() {
            let expected = if k < 5 { k + 1 } else { k - 4 };
            assert!((z - 0.2 * expected as f64).abs() < 1e-9, "{k}: {z}");
        }
        let objects_of = |layers: &[crate::Layer]| -> Vec<u32> {
            let mut seen: Vec<u32> = layers
                .iter()
                .flat_map(|l| &l.paths)
                .map(|p| p.object)
                .filter(|&o| o != NO_OBJECT)
                .collect();
            seen.dedup();
            seen
        };
        assert_eq!(objects_of(&toolpaths.layers[..5]), [1]);
        assert_eq!(objects_of(&toolpaths.layers[5..]), [0]);
        // The skirt goes round the first object printed only.
        let skirts = |layer: &crate::Layer| {
            layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Skirt)
                .count()
        };
        assert!(skirts(&toolpaths.layers[0]) > 0);
        assert_eq!(skirts(&toolpaths.layers[5]), 0);

        // Settings of its own stay with the object.
        let own = [
            Some(Settings {
                wall_loops: 4,
                ..settings.clone()
            }),
            None,
        ];
        let walls = crate::generate_plate_with_object_settings(
            &objects,
            &own,
            Placement::Centre,
            &settings,
            &mut Diagnostics::new(),
            &AtomicBool::new(false),
            &Progress::new(),
        )
        .unwrap();
        let inner = |layer: &crate::Layer| {
            layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::InnerWall)
                .count()
        };
        assert_eq!(inner(&walls.layers[2]), inner(&toolpaths.layers[2]));
        assert_eq!(inner(&walls.layers[7]), 3 * inner(&toolpaths.layers[7]));
    }

    #[test]
    fn objects_too_close_or_too_tall_are_refused() {
        let settings = Settings {
            sequential_print: true,
            ..Settings::default()
        };
        let close = plate(&[cube(0.0, 1.0), cube(30.0, 1.0)], &settings).unwrap_err();
        assert!(matches!(
            close,
            FdmError::ObjectsTooClose {
                objects: [0, 1],
                ..
            }
        ));
        assert_eq!(close.code(), "slicing.objects_too_close");
        // 35 mm apart is far enough.
        assert!(plate(&[cube(0.0, 1.0), cube(45.0, 1.0)], &settings).is_ok());

        // One tall object can go last, two cannot.
        let tall = [cube(0.0, 25.0), cube(50.0, 1.0)];
        assert_eq!(order(&tall, &settings).unwrap(), [1, 0]);
        let error = plate(&[cube(0.0, 25.0), cube(50.0, 22.0)], &settings).unwrap_err();
        assert_eq!(error.code(), "slicing.object_too_tall");
        assert!(
            error.to_string().starts_with("object 2 (22.0 mm)"),
            "{error}"
        );
    }
}
