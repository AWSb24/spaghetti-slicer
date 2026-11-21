//! Support blockers and enforcers: volumes of an object that are not
//! printed and say where supports go, whatever the settings say.

use slicer_core::slice::slice_mesh;
use slicer_core::{Mesh, Shapes, parallel_map, polygon};

use crate::DEGENERATE_HEIGHT;
use crate::plate::Placement;

/// Largest number of support blockers and enforcers of one object.
pub const MAX_SUPPORT_VOLUMES: usize = 64;

/// What a [`SupportVolume`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportVolumeKind {
    /// Overhangs inside the volume, or above it, get no support, so no
    /// support passes through it.
    Blocker,
    /// Overhangs inside the volume get support, even where the object has
    /// `supports` off and even bridges that `support_bridge_length` would
    /// leave to carry themselves.
    Enforcer,
}

/// A support blocker or enforcer of an object: a mesh that is not printed.
/// It reaches only the overhangs of its own object; where a blocker and an
/// enforcer overlap, the blocker counts. Branches of tree supports keep
/// out of every blocker, except straight below the overhangs of other
/// objects that get supports.
// ponytail: an overhang counts as inside a volume where the volume covers
// it on the layer of the overhang (and, for blockers, on any layer below
// it), not by the volume of the support under it; a column that stands on
// the model above a blocker is left out too. Which object an overhang
// belongs to is told by the outlines of the other objects near the volume
// (cut on their own, 0.05 mm wider): where objects overlap, the overlap
// counts as theirs. Columns under other objects' overhangs pass straight
// through a blocker.
#[derive(Debug, Clone, PartialEq)]
pub struct SupportVolume {
    /// Blocker or enforcer.
    pub kind: SupportVolumeKind,
    /// The volume, in the same coordinates as the mesh of its object; it
    /// is moved with the object when that is put on the bed.
    pub mesh: Mesh,
}

impl SupportVolume {
    /// A support blocker of the volume `mesh`.
    pub fn blocker(mesh: Mesh) -> Self {
        Self {
            kind: SupportVolumeKind::Blocker,
            mesh,
        }
    }

    /// A support enforcer of the volume `mesh`.
    pub fn enforcer(mesh: Mesh) -> Self {
        Self {
            kind: SupportVolumeKind::Enforcer,
            mesh,
        }
    }
}

/// The support blockers and enforcers of all objects as the layers see
/// them.
pub(crate) struct SupportMasks {
    /// Where overhangs get no support on every layer: inside a blocker on
    /// that layer or on any layer below it, without the other objects
    /// there. Empty without blockers.
    pub(crate) blocked: Vec<Shapes>,
    /// Where overhangs get support on every layer: inside an enforcer,
    /// without the other objects there. Empty without enforcers.
    pub(crate) enforced: Vec<Shapes>,
    /// The blockers themselves on every layer, of all objects, which tree
    /// branches keep out of. Empty without blockers.
    pub(crate) blockers: Vec<Shapes>,
}

impl SupportMasks {
    /// The blocked area of layer `index`, if any.
    pub(crate) fn blocked(&self, index: usize) -> Option<&Shapes> {
        self.blocked.get(index).filter(|area| !area.is_empty())
    }

    /// The enforced area of layer `index`, if any.
    pub(crate) fn enforced(&self, index: usize) -> Option<&Shapes> {
        self.enforced.get(index).filter(|area| !area.is_empty())
    }

    /// The blockers on every layer, if there are any.
    pub(crate) fn blockers(&self) -> Option<&[Shapes]> {
        (!self.blockers.is_empty()).then_some(self.blockers.as_slice())
    }
}

/// Whether any of `volumes` (those of every object) is of `kind`.
pub(crate) fn any_of(volumes: &[Vec<SupportVolume>], kind: SupportVolumeKind) -> bool {
    volumes.iter().flatten().any(|volume| volume.kind == kind)
}

/// How much wider the other objects near a volume are taken out of it, so
/// that their own overhangs surely lie outside it.
const OTHER_OBJECT_MARGIN: f64 = 0.05;

/// Blocked and enforced areas of some objects on every layer, empty until
/// a volume of that kind is added.
#[derive(Default)]
struct Areas {
    blocked: Vec<Shapes>,
    enforced: Vec<Shapes>,
}

impl Areas {
    /// Adds the cut volume `cut` of `kind`.
    fn add(&mut self, kind: SupportVolumeKind, cut: &[Shapes], layers: usize) {
        let target = match kind {
            SupportVolumeKind::Blocker => &mut self.blocked,
            SupportVolumeKind::Enforcer => &mut self.enforced,
        };
        if target.is_empty() {
            *target = vec![Vec::new(); layers];
        }
        let merged = parallel_map(layers, |index| match cut.get(index) {
            Some(area) if !area.is_empty() => polygon::union(&target[index], area),
            _ => target[index].clone(),
        });
        *target = merged;
    }
}

/// Lets every blocked area reach every overhang above it.
fn reach_up(blocked: &mut [Shapes]) {
    for index in 1..blocked.len() {
        if !blocked[index - 1].is_empty() {
            blocked[index] = polygon::union(&blocked[index], &blocked[index - 1]);
        }
    }
}

/// `a` and `b` together, layer by layer; one of them alone where the other
/// is empty.
fn merge(a: Vec<Shapes>, b: Vec<Shapes>) -> Vec<Shapes> {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    parallel_map(a.len(), |index| {
        let (x, y) = (&a[index], &b[index]);
        if y.is_empty() {
            x.clone()
        } else if x.is_empty() {
            y.clone()
        } else {
            polygon::union(x, y)
        }
    })
}

/// Cuts the support blockers and enforcers `volumes[i]` of every object
/// `objects[i]` at `cut_heights`, after they were put on the bed with
/// their objects as for the whole plate and moved by `shift`, as the
/// modifiers are. Other objects near a volume are taken out of it on every
/// layer, so that it reaches only the overhangs of its own object. `None`
/// without any.
pub(crate) fn masks(
    volumes: &[Vec<SupportVolume>],
    objects: &[Mesh],
    placement: Placement,
    shift: Option<[f64; 3]>,
    cut_heights: &[f64],
) -> Option<SupportMasks> {
    if volumes.iter().all(Vec::is_empty) {
        return None;
    }
    let layers = cut_heights.len();
    // Repaired and put on the bed with its object, as the plate is. An
    // object placed as given stands on the bed on its own; its volumes go
    // down with it.
    let lift = |object: &Mesh| match (placement, object.bounds()) {
        (Placement::AsGiven, Some((min, _))) => min[2],
        _ => 0.0,
    };
    let place = |mut mesh: Mesh, lift: f64| {
        mesh.repair(DEGENERATE_HEIGHT);
        let [x, y, z] = shift.unwrap_or([0.0; 3]);
        mesh.translate([x, y, z - lift]);
        mesh
    };
    let outline = |mesh: &Mesh| mesh.bounds().map(|(lo, hi)| [lo[0], lo[1], hi[0], hi[1]]);
    let overlap =
        |a: [f64; 4], b: [f64; 4]| a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
    // Where the objects are on the bed, seen from above.
    let [dx, dy, _] = shift.unwrap_or([0.0; 3]);
    let spots: Vec<Option<[f64; 4]>> = objects
        .iter()
        .map(|object| outline(object).map(|o| [o[0] + dx, o[1] + dy, o[2] + dx, o[3] + dy]))
        .collect();
    // Objects without other objects near their volumes go together.
    let mut common = Areas::default();
    let mut own: Vec<(Areas, Vec<usize>)> = Vec::new();
    for (number, (object, list)) in objects.iter().zip(volumes).enumerate() {
        let object_lift = lift(object);
        let meshes: Vec<(SupportVolumeKind, Mesh)> = list
            .iter()
            .take(MAX_SUPPORT_VOLUMES)
            .map(|volume| (volume.kind, place(volume.mesh.clone(), object_lift)))
            .collect();
        if meshes.is_empty() {
            continue;
        }
        let boxes: Vec<[f64; 4]> = meshes.iter().filter_map(|(_, m)| outline(m)).collect();
        let near: Vec<usize> = (0..objects.len())
            .filter(|&other| {
                other != number
                    && spots[other].is_some_and(|o| boxes.iter().any(|&v| overlap(o, v)))
            })
            .collect();
        let mut separate = Areas::default();
        let areas = if near.is_empty() {
            &mut common
        } else {
            &mut separate
        };
        for (kind, mesh) in &meshes {
            areas.add(*kind, &slice_mesh(mesh, cut_heights), layers);
        }
        if !near.is_empty() {
            own.push((separate, near));
        }
    }
    // The blockers as they are, for tree branches.
    let mut blockers = common.blocked.clone();
    for (areas, _) in &own {
        blockers = merge(blockers, areas.blocked.clone());
    }
    reach_up(&mut common.blocked);
    let Areas {
        mut blocked,
        mut enforced,
    } = common;
    let mut sliced: Vec<Option<Vec<Shapes>>> = vec![None; objects.len()];
    for (mut areas, near) in own {
        reach_up(&mut areas.blocked);
        for &other in &near {
            let cut = sliced[other].get_or_insert_with(|| {
                let mesh = &objects[other];
                let cut = slice_mesh(&place(mesh.clone(), lift(mesh)), cut_heights);
                parallel_map(layers, |index| match cut.get(index) {
                    Some(area) if !area.is_empty() => polygon::offset(area, OTHER_OBJECT_MARGIN),
                    _ => Vec::new(),
                })
            });
            for area in [&mut areas.blocked, &mut areas.enforced] {
                if area.is_empty() {
                    continue;
                }
                let without = parallel_map(layers, |index| match cut.get(index) {
                    Some(other) if !other.is_empty() && !area[index].is_empty() => {
                        polygon::difference(&area[index], other)
                    }
                    _ => area[index].clone(),
                });
                *area = without;
            }
        }
        blocked = merge(blocked, areas.blocked);
        enforced = merge(enforced, areas.enforced);
    }
    Some(SupportMasks {
        blocked,
        enforced,
        blockers,
    })
}
