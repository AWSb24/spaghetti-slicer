//! Support blockers and enforcers: volumes of an object that are not
//! printed and say where supports go, whatever the settings say.

use slicer_core::slice::slice_mesh;
use slicer_core::{Mesh, Shapes, parallel_map, polygon};

use crate::plate::Placement;
use crate::{DEGENERATE_HEIGHT, MAX_HOLE_EDGES};

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
/// It reaches every overhang inside it, whatever object that belongs to;
/// where a blocker and an enforcer overlap, the blocker counts.
// ponytail: an overhang counts as inside a volume where the volume covers
// it on the layer of the overhang (and, for blockers, on any layer below
// it), not by the volume of the support under it. Branches of tree
// supports that lean sideways may still pass through a blocker, and a
// column that stands on the model above a blocker is left out too.
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
    /// that layer or on any layer below it. Empty without blockers.
    pub(crate) blocked: Vec<Shapes>,
    /// Where overhangs get support on every layer: inside an enforcer.
    /// Empty without enforcers.
    pub(crate) enforced: Vec<Shapes>,
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
}

/// Whether any of `volumes` (those of every object) is of `kind`.
pub(crate) fn any_of(volumes: &[Vec<SupportVolume>], kind: SupportVolumeKind) -> bool {
    volumes.iter().flatten().any(|volume| volume.kind == kind)
}

/// Cuts the support blockers and enforcers `volumes[i]` of every object
/// `objects[i]` at `cut_heights`, after they were put on the bed with
/// their objects as for the whole plate and moved by `shift`, as the
/// modifiers are. `None` without any.
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
    let mut blocked: Vec<Shapes> = Vec::new();
    let mut enforced: Vec<Shapes> = Vec::new();
    for (object, list) in objects.iter().zip(volumes) {
        // An object placed as given stands on the bed on its own; its
        // volumes go down with it.
        let lift = match (placement, object.bounds()) {
            (Placement::AsGiven, Some((min, _))) => min[2],
            _ => 0.0,
        };
        for volume in list.iter().take(MAX_SUPPORT_VOLUMES) {
            let mut mesh = volume.mesh.clone();
            mesh.orient();
            mesh.remove_degenerate(DEGENERATE_HEIGHT);
            mesh.close_holes(MAX_HOLE_EDGES);
            let [x, y, z] = shift.unwrap_or([0.0; 3]);
            mesh.translate([x, y, z - lift]);
            let cut = slice_mesh(&mesh, cut_heights);
            let target = match volume.kind {
                SupportVolumeKind::Blocker => &mut blocked,
                SupportVolumeKind::Enforcer => &mut enforced,
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
    // A blocker reaches every overhang above it.
    for index in 1..blocked.len() {
        if !blocked[index - 1].is_empty() {
            blocked[index] = polygon::union(&blocked[index], &blocked[index - 1]);
        }
    }
    Some(SupportMasks { blocked, enforced })
}
