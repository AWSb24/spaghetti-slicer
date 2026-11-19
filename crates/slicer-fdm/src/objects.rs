//! Objects on the plate with settings of their own: which settings an
//! object may change, and the region of every layer that each group of
//! objects with the same settings takes.

use slicer_core::slice::slice_mesh;
use slicer_core::{Diagnose, Diagnostic, Mesh, Shapes, parallel_map, polygon};
use slicer_profiles::{InfillPattern, Settings};

use crate::plate::{self, Placement};
use crate::{DEGENERATE_HEIGHT, MAX_HOLE_EDGES};

/// Process settings that an object on the plate may set for itself,
/// overriding the plate's: walls and their seam, top and bottom shells,
/// sparse infill, whether it gets supports, the speeds of its walls,
/// infill, bridges, overhangs and ironing, its fan, flow ratio, and the
/// retraction, wipe and Z hop after its lines. Everything else, among it
/// the layer heights, line width, skirt, brim, the shape and speed of
/// supports, the first layer speed, travel, the shortest travel that
/// retracts and the minimum layer time, stays the same for the whole
/// plate.
pub const OBJECT_SETTINGS: &[&str] = &[
    "wall_loops",
    "wall_order",
    "seam_position",
    "top_layers",
    "bottom_layers",
    "infill_density",
    "infill_pattern",
    "infill_angle",
    "infill_connect",
    "infill_overlap",
    "supports",
    "outer_wall_speed",
    "inner_wall_speed",
    "infill_speed",
    "bridge_speed",
    "overhang_speed",
    "overhang_speed_full",
    "ironing_speed",
    "fan_speed",
    "bridge_fan_speed",
    "overhang_fan_speed",
    "fan_off_layers",
    "fan_layer_time",
    "fan_max_speed",
    "flow_ratio",
    "retraction_length",
    "retraction_speed",
    "z_hop",
    "wipe_distance",
];

/// Settings of [`OBJECT_SETTINGS`] that an object may also set for a range
/// of its height (see [`HeightRange`]): walls and their seam, top and
/// bottom shells, sparse infill and whether it gets supports. The speeds
/// stay those of the whole object.
// ponytail: speeds per height range would need the G-code writer to look
// up the settings of a path by its layer as well as its object; the layer
// height per range would need layers of different heights side by side.
pub const RANGE_SETTINGS: &[&str] = &[
    "wall_loops",
    "wall_order",
    "seam_position",
    "top_layers",
    "bottom_layers",
    "infill_density",
    "infill_pattern",
    "infill_angle",
    "infill_connect",
    "infill_overlap",
    "supports",
];

/// Settings of an object for a range of its height: every layer whose
/// middle lies at least `from` and less than `to` millimetres above the bed
/// prints with the object's settings and `values` over them. Where ranges
/// overlap, the later one counts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeightRange {
    /// Height above the bed where the range starts, in millimetres.
    pub from: f64,
    /// Height above the bed where the range ends, in millimetres; a layer
    /// whose middle lies exactly there is above the range.
    pub to: f64,
    /// Settings by name with their values as text, in the form
    /// [`Settings::set`] takes; only those of [`RANGE_SETTINGS`] count.
    pub values: Vec<(String, String)>,
}

impl HeightRange {
    /// A range from `from` to `to` millimetres that changes nothing yet.
    pub fn new(from: f64, to: f64) -> Self {
        Self {
            from,
            to,
            values: Vec::new(),
        }
    }

    /// Whether a layer whose middle lies `z` millimetres above the bed is
    /// in the range.
    pub fn contains(&self, z: f64) -> bool {
        self.from <= z && z < self.to
    }

    /// Sets `key` to `value`, replacing an earlier value of it.
    fn put(&mut self, key: &str, value: String) {
        match self.values.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.values.push((key.to_string(), value)),
        }
    }
}

/// The settings the layers of `range` print with: `object`, the settings
/// of the object, with the values of `range` that are settings of
/// [`RANGE_SETTINGS`] over them.
pub fn range_settings(object: &Settings, range: &HeightRange) -> Settings {
    let mut settings = object.clone();
    let values = range
        .values
        .iter()
        .filter(|(key, _)| RANGE_SETTINGS.contains(&key.as_str()))
        .map(|(key, value)| (key.as_str(), value.as_str()));
    // The values were checked when the range was made; values that do not
    // fit those of the object leave the object's settings as they are.
    if settings.set_all(values).is_err() {
        return object.clone();
    }
    settings
}

/// Layers a settings file in TOML form over `range`, a height range of an
/// object whose settings are `object`, as the command line and the server
/// do. Fails, changing nothing, if the file names a setting outside
/// [`RANGE_SETTINGS`] (`settings.not_per_range`) or holds a value that is
/// not valid.
pub fn apply_range_toml(
    range: &mut HeightRange,
    object: &Settings,
    text: &str,
) -> Result<(), Diagnostic> {
    let keys = Settings::toml_keys(text).map_err(|e| e.diagnostic())?;
    for key in &keys {
        if !RANGE_SETTINGS.contains(&key.as_str()) {
            let allowed = RANGE_SETTINGS.join(", ");
            return Err(Diagnostic::error(
                "settings.not_per_range",
                format!("setting '{key}' cannot be set per height range (allowed: {allowed})"),
            )
            .with("key", key)
            .with("allowed", allowed));
        }
    }
    let mut trial = range_settings(object, range);
    trial.apply_toml(text).map_err(|e| e.diagnostic())?;
    let values = trial.values();
    for key in keys {
        if let Some(value) = values.get(&key) {
            range.put(&key, value.clone());
        }
    }
    Ok(())
}

/// Layers settings by the names of Orca Slicer and Bambu Studio, as their
/// project files store them for a height range of an object, over
/// `range`; `object` are the settings of the object. Only settings of
/// [`RANGE_SETTINGS`] are taken; the others are returned by their Orca
/// name with the reason, as by [`apply_object_orca`].
pub fn apply_range_orca(
    range: &mut HeightRange,
    object: &Settings,
    values: &[(String, String)],
) -> Vec<(String, String)> {
    let mut trial = range_settings(object, range);
    let pairs = values
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()));
    let import = slicer_profiles::import_orca_values(pairs, &mut trial.clone());
    let mut skipped = import.skipped;
    for (ours, value, orca) in import.taken {
        if !RANGE_SETTINGS.contains(&ours.as_str()) {
            skipped.push((orca, format!("'{ours}' cannot be set per height range")));
        } else if let Err(error) = trial.set(&ours, &value) {
            skipped.push((orca, error.to_string()));
        } else {
            range.put(&ours, value);
        }
    }
    skipped
}

/// Settings of [`RANGE_SETTINGS`] that a modifier (see [`Modifier`]) may
/// set for the part of its object inside it: the thickness of the top and
/// bottom shells, the sparse infill (density, pattern, angle, connecting
/// its lines) and whether it gets supports.
///
/// The object keeps one region on every layer, which gets the walls and in
/// which the top and bottom surfaces are found, exactly as without the
/// modifier. Only the fill area inside the walls is split: the sparse
/// infill inside the modifier gets its pattern, density and angle (at
/// 100 % it is solid infill), the shells inside it are as thick as its
/// `top_layers` and `bottom_layers` ask for, measured against the
/// surfaces of the object. The modifier's own top and bottom therefore
/// start or end no surface: a modifier with 100 % infill inside an object
/// with 15 % fills its part of each layer with solid infill, without
/// extra shells above or below it.
// ponytail: walls, their order and seam and the infill overlap stay those
// of the object, since walls inside an object along the borders of a
// modifier would need the region split at the walls. Lightning infill
// cannot be asked for, because it is planned for a whole region across its
// layers; a modifier of an object with lightning infill keeps that.
pub const MODIFIER_SETTINGS: &[&str] = &[
    "top_layers",
    "bottom_layers",
    "infill_density",
    "infill_pattern",
    "infill_angle",
    "infill_connect",
    "supports",
];

/// Largest number of modifiers of one object.
pub const MAX_MODIFIERS: usize = 64;

/// A modifier of an object: a mesh that is not printed. Inside its volume
/// the object prints with `values` over its settings; only those of
/// [`MODIFIER_SETTINGS`] count. Where modifiers of an object overlap, the
/// later one counts. A modifier reaches only its own object, never other
/// objects it overlaps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Modifier {
    /// The volume, in the same coordinates as the mesh of its object; it
    /// is moved with the object when that is put on the bed.
    pub mesh: Mesh,
    /// Settings by name with their values as text, in the form
    /// [`Settings::set`] takes.
    pub values: Vec<(String, String)>,
}

impl Modifier {
    /// A modifier of the volume `mesh` that changes nothing yet.
    pub fn new(mesh: Mesh) -> Self {
        Self {
            mesh,
            values: Vec::new(),
        }
    }

    /// Sets `key` to `value`, replacing an earlier value of it.
    fn put(&mut self, key: &str, value: String) {
        match self.values.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.values.push((key.to_string(), value)),
        }
    }
}

/// The settings inside `modifier`: `object`, the settings of its object on
/// a layer, with the values of the modifier that are settings of
/// [`MODIFIER_SETTINGS`] over them.
pub fn modifier_settings(object: &Settings, modifier: &Modifier) -> Settings {
    let mut settings = object.clone();
    let values = modifier
        .values
        .iter()
        .filter(|(key, _)| MODIFIER_SETTINGS.contains(&key.as_str()))
        .map(|(key, value)| (key.as_str(), value.as_str()));
    // The values were checked when the modifier was made; values that do
    // not fit leave the object's settings as they are, as does lightning
    // infill, which no modifier plans.
    if settings.set_all(values).is_err()
        || (settings.infill_pattern == InfillPattern::Lightning
            && object.infill_pattern != InfillPattern::Lightning)
    {
        return object.clone();
    }
    settings
}

/// Why `key` = `value` cannot be set per modifier, if it cannot.
fn not_per_modifier(key: &str, value: &Settings) -> Option<String> {
    if !MODIFIER_SETTINGS.contains(&key) {
        return Some(format!("'{key}' cannot be set per modifier"));
    }
    (key == "infill_pattern" && value.infill_pattern == InfillPattern::Lightning)
        .then(|| "lightning infill cannot be set per modifier".to_string())
}

/// Layers a settings file in TOML form over `modifier`, a modifier of an
/// object whose settings are `object`, as the command line and the server
/// do. Fails, changing nothing, if the file names a setting outside
/// [`MODIFIER_SETTINGS`] or asks for lightning infill
/// (`settings.not_per_modifier`), or holds a value that is not valid.
pub fn apply_modifier_toml(
    modifier: &mut Modifier,
    object: &Settings,
    text: &str,
) -> Result<(), Diagnostic> {
    let keys = Settings::toml_keys(text).map_err(|e| e.diagnostic())?;
    let refuse = |key: &str, reason: String| {
        let allowed = MODIFIER_SETTINGS.join(", ");
        Diagnostic::error(
            "settings.not_per_modifier",
            format!("{reason} (allowed: {allowed}, not lightning infill)"),
        )
        .with("key", key)
        .with("allowed", allowed)
    };
    for key in &keys {
        if !MODIFIER_SETTINGS.contains(&key.as_str()) {
            return Err(refuse(
                key,
                format!("setting '{key}' cannot be set per modifier"),
            ));
        }
    }
    let mut trial = modifier_settings(object, modifier);
    trial.apply_toml(text).map_err(|e| e.diagnostic())?;
    let values = trial.values();
    for key in &keys {
        if let Some(reason) = not_per_modifier(key, &trial) {
            return Err(refuse(key, reason));
        }
    }
    for key in keys {
        if let Some(value) = values.get(&key) {
            modifier.put(&key, value.clone());
        }
    }
    Ok(())
}

/// Layers settings by the names of Orca Slicer and Bambu Studio, as their
/// project files store them for a modifier of an object, over `modifier`;
/// `object` are the settings of the object. Only settings of
/// [`MODIFIER_SETTINGS`] are taken; the others are returned by their Orca
/// name with the reason, as by [`apply_object_orca`].
pub fn apply_modifier_orca(
    modifier: &mut Modifier,
    object: &Settings,
    values: &[(String, String)],
) -> Vec<(String, String)> {
    let mut trial = modifier_settings(object, modifier);
    let pairs = values
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()));
    let import = slicer_profiles::import_orca_values(pairs, &mut trial.clone());
    let mut skipped = import.skipped;
    for (ours, value, orca) in import.taken {
        let mut next = trial.clone();
        if let Err(error) = next.set(&ours, &value) {
            skipped.push((orca, error.to_string()));
        } else if let Some(reason) = not_per_modifier(&ours, &next) {
            skipped.push((orca, reason));
        } else {
            trial = next;
            modifier.put(&ours, value);
        }
    }
    skipped
}

/// The settings of a group of objects: those of their whole height and
/// those of their height ranges.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Group {
    /// The settings outside the ranges.
    pub(crate) settings: Settings,
    /// Where a range starts and ends (see [`HeightRange`]) and the
    /// settings of its layers, which differ from `settings`. Where ranges
    /// overlap, the later one counts.
    pub(crate) ranges: Vec<(f64, f64, Settings)>,
}

impl Group {
    /// A group with `settings` over its whole height.
    pub(crate) fn plain(settings: Settings) -> Self {
        Self {
            settings,
            ranges: Vec::new(),
        }
    }

    /// The settings of the layer whose middle lies `z` above the bed.
    pub(crate) fn at(&self, z: f64) -> &Settings {
        self.ranges
            .iter()
            .rev()
            .find(|(from, to, _)| *from <= z && z < *to)
            .map_or(&self.settings, |(_, _, settings)| settings)
    }

    /// Whether `test` holds for the settings of some height of the group.
    pub(crate) fn any(&self, test: impl Fn(&Settings) -> bool) -> bool {
        test(&self.settings) || self.ranges.iter().any(|(_, _, s)| test(s))
    }
}

/// `settings` with the values an object may set for itself that only the
/// G-code writer uses taken from `values`: speeds, fan, flow ratio and
/// retraction. They change how the writer prints the lines of an object
/// (and whether its overhanging walls are slowed down), not where the
/// lines go.
fn with_writer_values(settings: &Settings, values: &Settings) -> Settings {
    Settings {
        outer_wall_speed: values.outer_wall_speed,
        inner_wall_speed: values.inner_wall_speed,
        infill_speed: values.infill_speed,
        bridge_speed: values.bridge_speed,
        overhang_speed: values.overhang_speed,
        overhang_speed_full: values.overhang_speed_full,
        ironing_speed: values.ironing_speed,
        fan_speed: values.fan_speed,
        bridge_fan_speed: values.bridge_fan_speed,
        overhang_fan_speed: values.overhang_fan_speed,
        fan_off_layers: values.fan_off_layers,
        fan_layer_time: values.fan_layer_time,
        fan_max_speed: values.fan_max_speed,
        flow_ratio: values.flow_ratio,
        retraction_length: values.retraction_length,
        retraction_speed: values.retraction_speed,
        z_hop: values.z_hop,
        wipe_distance: values.wipe_distance,
        ..settings.clone()
    }
}

/// The settings an object prints with: `plate` with the values of
/// [`OBJECT_SETTINGS`] taken from `own`.
pub fn object_settings(plate: &Settings, own: &Settings) -> Settings {
    let merged = Settings {
        wall_loops: own.wall_loops,
        wall_order: own.wall_order,
        seam_position: own.seam_position,
        top_layers: own.top_layers,
        bottom_layers: own.bottom_layers,
        infill_density: own.infill_density,
        infill_pattern: own.infill_pattern,
        infill_angle: own.infill_angle,
        infill_connect: own.infill_connect,
        infill_overlap: own.infill_overlap,
        supports: own.supports,
        ..plate.clone()
    };
    with_writer_values(&merged, own)
}

/// The settings of every object (`own[i]` over `plate` for object `i`, the
/// plate's without) for [`crate::Toolpaths::object_settings`]; empty if
/// every object prints with the plate's speeds, fan, flow and retraction,
/// and in vase mode, which leaves the settings of objects out.
pub(crate) fn speed_table(
    count: usize,
    own: &[Option<Settings>],
    plate: &Settings,
) -> Vec<Settings> {
    if plate.spiral_vase {
        return Vec::new();
    }
    let table: Vec<Settings> = (0..count)
        .map(|index| match own.get(index) {
            Some(Some(own)) => object_settings(plate, own),
            _ => plate.clone(),
        })
        .collect();
    if table.iter().all(|s| with_writer_values(s, plate) == *s) {
        return Vec::new();
    }
    table
}

/// Layers a settings file in TOML form over `own`, the settings of one
/// object, as the command line and the server do. Fails, changing nothing,
/// if the file names a setting outside [`OBJECT_SETTINGS`] or holds a value
/// that is not valid.
pub fn apply_object_toml(own: &mut Settings, text: &str) -> Result<(), Diagnostic> {
    for key in Settings::toml_keys(text).map_err(|e| e.diagnostic())? {
        if !OBJECT_SETTINGS.contains(&key.as_str()) {
            let allowed = OBJECT_SETTINGS.join(", ");
            return Err(Diagnostic::error(
                "settings.not_per_object",
                format!("setting '{key}' cannot be set per object (allowed: {allowed})"),
            )
            .with("key", key)
            .with("allowed", allowed));
        }
    }
    own.apply_toml(text).map_err(|e| e.diagnostic())
}

/// Layers settings by the names of Orca Slicer and Bambu Studio, as their
/// project files store them for a single object, over `own`, the settings
/// of that object. Only settings of [`OBJECT_SETTINGS`] are taken; those
/// without a counterpart, with values that are not valid and those that
/// cannot be set per object are returned by their Orca name with the
/// reason.
pub fn apply_object_orca(own: &mut Settings, values: &[(String, String)]) -> Vec<(String, String)> {
    let mut trial = own.clone();
    let pairs = values
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()));
    let import = slicer_profiles::import_orca_values(pairs, &mut trial);
    let mut skipped = import.skipped;
    for (ours, value, orca) in import.taken {
        if !OBJECT_SETTINGS.contains(&ours.as_str()) {
            skipped.push((orca, format!("'{ours}' cannot be set per object")));
        } else if let Err(error) = own.set(&ours, &value) {
            skipped.push((orca, error.to_string()));
        }
    }
    skipped
}

/// The different settings among `count` objects, the first of them those
/// of the first object, and for every object the index of its settings.
/// Object `i` has `own[i]` if given, otherwise the plate's, and the height
/// ranges `ranges[i]` (none if the slice is shorter); ranges that change
/// nothing are left out. Speeds, fan, flow and retraction do not count:
/// every group has the plate's, and the G-code writer takes those of every
/// object from [`speed_table`], so objects that differ only in them are
/// laid out exactly as if they did not.
pub(crate) fn groups(
    count: usize,
    own: &[Option<Settings>],
    ranges: &[Vec<HeightRange>],
    plate: &Settings,
) -> (Vec<Group>, Vec<usize>) {
    let mut groups: Vec<Group> = Vec::new();
    let mut membership = Vec::with_capacity(count);
    for index in 0..count {
        let settings = match own.get(index) {
            Some(Some(own)) => with_writer_values(&object_settings(plate, own), plate),
            _ => plate.clone(),
        };
        let ranges = ranges
            .get(index)
            .into_iter()
            .flatten()
            .filter_map(|range| {
                let own = range_settings(&settings, range);
                (own != settings).then_some((range.from, range.to, own))
            })
            .collect();
        let settings = Group { settings, ranges };
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
        groups.push(Group::plain(plate.clone()));
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

/// On every layer (cut at `cut_heights`), everything that groups whose
/// supports are `on` on that layer take, with the parts of their objects
/// inside `modifiers` that say otherwise taken out or added.
pub(crate) fn with_supports(
    regions: &[Vec<Shapes>],
    groups: &[Group],
    modifiers: &[ModifierArea],
    cut_heights: &[f64],
    on: bool,
) -> Vec<Shapes> {
    let layers = regions.first().map_or(0, Vec::len);
    parallel_map(layers, |index| {
        let z = cut_heights.get(index).copied().unwrap_or(0.0);
        let mut area = Vec::new();
        for (number, (region, group)) in regions.iter().zip(groups).enumerate() {
            let region = &region[index];
            let mut own = if group.at(z).supports == on {
                Some(std::borrow::Cow::Borrowed(region))
            } else {
                None
            };
            // Later modifiers count over earlier ones.
            for modifier in modifiers.iter().filter(|m| m.group == number) {
                let inside = &modifier.area[index];
                if inside.is_empty() {
                    continue;
                }
                let none = Vec::new();
                let current = own.as_deref().unwrap_or(&none);
                let next = if modifier.settings.at(z).supports == on {
                    polygon::union(current, &polygon::intersection(region, inside))
                } else {
                    polygon::difference(current, inside)
                };
                own = Some(std::borrow::Cow::Owned(next));
            }
            if let Some(own) = own {
                area = polygon::union(&area, &own);
            }
        }
        area
    })
}

/// A modifier of an object (see [`Modifier`]) as the layers see it: the
/// group of its object, the settings inside it on every height (those of
/// the group with the modifier's values over them) and the part of its
/// object inside it on every layer.
#[derive(Debug, Clone)]
pub(crate) struct ModifierArea {
    /// Index of the group of its object.
    pub(crate) group: usize,
    /// The settings inside it by height.
    pub(crate) settings: Group,
    /// The part of its object inside it on every layer; empty until
    /// [`modifier_areas`] cuts it.
    pub(crate) area: Vec<Shapes>,
    /// Index of its object.
    object: usize,
    /// Index of the modifier among those of its object.
    number: usize,
}

/// Whether `test` holds for the settings of some height of a group or
/// inside one of `modifiers`.
pub(crate) fn any_setting(
    groups: &[Group],
    modifiers: &[ModifierArea],
    test: impl Fn(&Settings) -> bool,
) -> bool {
    groups.iter().any(|g| g.any(&test)) || modifiers.iter().any(|m| m.settings.any(&test))
}

/// The modifiers of every object (`modifiers[i]` those of object `i`,
/// whose group is `membership[i]`) that change anything, with the settings
/// inside them, in the order of the objects and their modifiers; not yet
/// cut (see [`modifier_areas`]).
pub(crate) fn modifier_groups(
    modifiers: &[Vec<Modifier>],
    groups: &[Group],
    membership: &[usize],
) -> Vec<ModifierArea> {
    let mut found = Vec::new();
    for (object, list) in modifiers.iter().enumerate() {
        let Some(group_index) = membership.get(object).copied() else {
            continue;
        };
        let Some(group) = groups.get(group_index) else {
            continue;
        };
        for (number, modifier) in list.iter().enumerate().take(MAX_MODIFIERS) {
            let settings = Group {
                settings: modifier_settings(&group.settings, modifier),
                ranges: group
                    .ranges
                    .iter()
                    .map(|(from, to, s)| (*from, *to, modifier_settings(s, modifier)))
                    .collect(),
            };
            if settings != *group {
                found.push(ModifierArea {
                    group: group_index,
                    settings,
                    area: Vec::new(),
                    object,
                    number,
                });
            }
        }
    }
    found
}

/// Cuts the modifiers found by [`modifier_groups`] at `cut_heights`, after
/// they were put on the bed with their objects as for the whole plate and
/// moved by `shift`: on every layer the part of its object inside it.
pub(crate) fn modifier_areas(
    found: &mut [ModifierArea],
    objects: &[Mesh],
    modifiers: &[Vec<Modifier>],
    placement: Placement,
    shift: Option<[f64; 3]>,
    cut_heights: &[f64],
) {
    // Repaired, put on the bed with the object and cut, as the plate is.
    let cut = |mut mesh: Mesh, lift: f64| {
        mesh.orient();
        mesh.remove_degenerate(DEGENERATE_HEIGHT);
        mesh.close_holes(MAX_HOLE_EDGES);
        let [x, y, z] = shift.unwrap_or([0.0; 3]);
        mesh.translate([x, y, z - lift]);
        slice_mesh(&mesh, cut_heights)
    };
    let mut current: Option<(usize, Vec<Shapes>)> = None;
    for modifier in found.iter_mut() {
        let (Some(object), Some(volume)) = (
            objects.get(modifier.object),
            modifiers
                .get(modifier.object)
                .and_then(|list| list.get(modifier.number)),
        ) else {
            continue;
        };
        // An object placed as given stands on the bed on its own; its
        // modifiers go down with it.
        let lift = match (placement, object.bounds()) {
            (Placement::AsGiven, Some((min, _))) => min[2],
            _ => 0.0,
        };
        if current.as_ref().is_none_or(|(o, _)| *o != modifier.object) {
            current = Some((modifier.object, cut(object.clone(), lift)));
        }
        let Some((_, own)) = &current else {
            continue;
        };
        let inside = cut(volume.mesh.clone(), lift);
        modifier.area = parallel_map(cut_heights.len(), |index| {
            match (own.get(index), inside.get(index)) {
                (Some(own), Some(inside)) if !own.is_empty() && !inside.is_empty() => {
                    polygon::intersection(own, inside)
                }
                _ => Vec::new(),
            }
        });
    }
}

/// The modifiers of the objects of group `group` on layer `index`, whose
/// middle lies `z` above the bed: the part of the layer that each one
/// takes, with later ones taking where they overlap earlier ones, and the
/// settings there. Modifiers that change nothing on this layer are left
/// out.
pub(crate) fn modifiers_at(
    modifiers: &[ModifierArea],
    group: usize,
    index: usize,
    z: f64,
    settings: &Settings,
) -> Vec<(Shapes, Settings)> {
    let mut found: Vec<(Shapes, Settings)> = Vec::new();
    for modifier in modifiers.iter().filter(|m| m.group == group) {
        let Some(area) = modifier.area.get(index).filter(|a| !a.is_empty()) else {
            continue;
        };
        // A later modifier takes its part from earlier ones, even if it
        // gives it the object's own settings again.
        for (earlier, _) in &mut found {
            *earlier = polygon::difference(earlier, area);
        }
        let inside = modifier.settings.at(z);
        if inside != settings {
            found.push((area.clone(), inside.clone()));
        }
    }
    found.retain(|(area, _)| !area.is_empty());
    found
}

/// The regions to print with their extruder, the index of their group and
/// its settings: the regions of the extruders (`extruders`, empty if one
/// extruder prints everything) split by the regions of the groups of
/// objects (`objects`, empty if all objects have the same settings, which
/// are `single`, group 0). Without either, empty.
pub(crate) fn split<'a>(
    extruders: Vec<(u32, Vec<Shapes>)>,
    objects: Vec<Vec<Shapes>>,
    groups: &'a [Group],
    extruder: u32,
    single: &'a Group,
) -> Vec<(u32, usize, &'a Group, Vec<Shapes>)> {
    if objects.is_empty() {
        return extruders
            .into_iter()
            .map(|(e, region)| (e, 0, single, region))
            .collect();
    }
    if extruders.is_empty() {
        return objects
            .into_iter()
            .zip(groups)
            .enumerate()
            .map(|(number, (region, group))| (extruder, number, group, region))
            .collect();
    }
    let mut regions = Vec::new();
    for (e, region) in &extruders {
        for (number, (area, group)) in objects.iter().zip(groups).enumerate() {
            let part = parallel_map(region.len(), |index| {
                polygon::intersection(&region[index], &area[index])
            });
            regions.push((*e, number, group, part));
        }
    }
    regions
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::AtomicBool;

    use slicer_core::Diagnostics;
    use slicer_profiles::{InfillPattern, SeamPosition, WallOrder};

    use super::*;
    use crate::{
        Layer, OwnSettings, PathKind, Progress, Toolpaths, generate_plate_with_object_settings,
        generate_plate_with_own_settings,
    };

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
    fn only_objects_with_an_overhang_speed_get_overhang_walls() {
        let objects = [tee(20.0), tee(140.0)];
        let slow = Settings {
            overhang_speed: 15.0,
            ..Settings::default()
        };
        let plain = Settings::default();
        let overhangs = |toolpaths: &Toolpaths, left: bool| {
            toolpaths
                .layers
                .iter()
                .any(|layer| kinds(layer, left).contains(&PathKind::OverhangWall))
        };
        // The plate has none, the first object asks for them; then the
        // other way round.
        for (plate, own) in [(&plain, &slow), (&slow, &plain)] {
            let (toolpaths, _) = slice(&objects, &[Some(own.clone()), None], plate);
            assert_eq!(toolpaths.object_settings.len(), 2);
            assert_eq!(overhangs(&toolpaths, true), own.overhang_speed > 0.0);
            assert_eq!(overhangs(&toolpaths, false), plate.overhang_speed > 0.0);
            // Speeds alone leave the layout as it was: the groups are the
            // same, only the overhangs are split.
            let (alone, _) = slice(&objects, &[], plate);
            assert_eq!(toolpaths.layers.len(), alone.layers.len());
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
            seam_position: SeamPosition::Corner,
            top_layers: 2,
            bottom_layers: 7,
            infill_density: 100.0,
            infill_pattern: InfillPattern::Gyroid,
            infill_angle: 10.0,
            infill_connect: false,
            infill_overlap: 30.0,
            supports: true,
            outer_wall_speed: 21.0,
            inner_wall_speed: 22.0,
            infill_speed: 23.0,
            bridge_speed: 24.0,
            overhang_speed: 25.0,
            overhang_speed_full: 26.0,
            ironing_speed: 27.0,
            fan_speed: 30.0,
            bridge_fan_speed: 40.0,
            overhang_fan_speed: 50.0,
            fan_off_layers: 3,
            fan_layer_time: 12.0,
            fan_max_speed: 60.0,
            flow_ratio: 0.9,
            retraction_length: 2.0,
            retraction_speed: 25.0,
            z_hop: 0.4,
            wipe_distance: 1.5,
            layer_height: 0.1,
            brim_width: 5.0,
            first_layer_speed: 10.0,
            travel_speed: 100.0,
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
    fn project_values_of_an_object_are_restricted_to_its_keys() {
        let mut own = Settings::default();
        let values: Vec<(String, String)> = [
            ("sparse_infill_density", "35%"),
            ("wall_loops", "5"),
            ("layer_height", "0.1"),
            ("support_type", "tree(auto)"),
            ("top_shell_layers", "-3"),
            ("fan_max_speed", "40"),
            ("filament_flow_ratio", "0.95"),
            ("filament_retraction_length", "1.5"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let skipped = apply_object_orca(&mut own, &values);
        assert_eq!(own.infill_density, 35.0);
        assert_eq!(own.wall_loops, 5);
        // Fan, flow and retraction of the filament are taken per object too.
        assert_eq!(own.fan_speed, 40.0);
        assert_eq!(own.flow_ratio, 0.95);
        assert_eq!(own.retraction_length, 1.5);
        assert_eq!(own.layer_height, Settings::default().layer_height);
        let mut keys: Vec<&str> = skipped.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["layer_height", "support_type", "top_shell_layers"]);
        assert!(
            skipped
                .iter()
                .any(|(key, reason)| key == "layer_height" && reason.contains("per object"))
        );
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
        let (groups, membership) = groups(5, &own, &[], &plate);
        assert_eq!(membership, [0, 1, 0, 1, 0]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], Group::plain(plate.clone()));
        assert_eq!(groups[1].settings.infill_density, 100.0);
        assert_eq!(
            super::groups(0, &[], &[], &plate).0,
            [Group::plain(plate.clone())]
        );
        // Ranges that change nothing are none; others make a group of
        // their own.
        let mut same = HeightRange::new(5.0, 10.0);
        same.put("infill_density", plate.infill_density.to_string());
        let mut walls = HeightRange::new(5.0, 10.0);
        walls.put("wall_loops", "5".into());
        let ranges = [vec![], vec![], vec![same], vec![walls]];
        let (groups, membership) = super::groups(4, &own, &ranges, &plate);
        assert_eq!(membership, [0, 1, 0, 2]);
        assert_eq!(groups[2].settings, groups[1].settings);
        assert_eq!(groups[2].at(4.9).wall_loops, plate.wall_loops);
        assert_eq!(groups[2].at(5.0).wall_loops, 5);
        assert_eq!(groups[2].at(9.9).infill_density, 100.0);
        assert_eq!(groups[2].at(10.0).wall_loops, plate.wall_loops);
    }

    /// Slices `objects` where they are, object `i` with the height ranges
    /// `ranges[i]`.
    fn slice_ranges(
        objects: &[Mesh],
        ranges: &[Vec<HeightRange>],
        settings: &Settings,
    ) -> (Toolpaths, Diagnostics) {
        let mut diagnostics = Diagnostics::new();
        let own = OwnSettings {
            ranges,
            ..OwnSettings::default()
        };
        let toolpaths = generate_plate_with_own_settings(
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

    /// A range from `from` to `to` millimetres with the settings `text`
    /// over `settings`.
    fn range(from: f64, to: f64, settings: &Settings, text: &str) -> HeightRange {
        let mut range = HeightRange::new(from, to);
        apply_range_toml(&mut range, settings, text).unwrap();
        range
    }

    /// Height of the middle of `layer` above the bed.
    fn middle(layer: &Layer) -> f64 {
        layer.z - layer.height / 2.0
    }

    #[test]
    fn layers_in_a_height_range_print_with_its_settings() {
        let settings = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let objects = [cube(20.0)];
        let (plain, plain_warnings) = slice(&objects, &[], &settings);
        let dense = range(
            10.0,
            20.0,
            &settings,
            "infill_density = 100.0\nwall_loops = 4",
        );
        let (toolpaths, warnings) = slice_ranges(&objects, &[vec![dense]], &settings);
        assert_eq!(warnings.codes(), plain_warnings.codes());
        assert_eq!(toolpaths.layers.len(), plain.layers.len());
        use PathKind::*;
        let inner = |layer: &Layer| layer.paths.iter().filter(|p| p.kind == InnerWall).count();
        let below: Vec<usize> = (0..plain.layers.len())
            .filter(|&i| middle(&plain.layers[i]) < 10.0)
            .collect();
        let border = below.len();
        // Below the range everything is as without it, up to the last layer
        // under the border, which stays sparse: no top surface there.
        assert_eq!(toolpaths.layers[..border], plain.layers[..border]);
        assert_eq!(
            kinds(&toolpaths.layers[border - 1], true),
            [OuterWall, InnerWall, SparseInfill]
        );
        // From the first layer whose middle lies above 10 mm, the range's.
        for layer in &toolpaths.layers[border..toolpaths.layers.len() - 6] {
            assert!(middle(layer) >= 10.0);
            assert_eq!(kinds(layer, true), [OuterWall, InnerWall, SolidInfill]);
            assert_eq!(inner(layer), 3);
        }
        assert_eq!(inner(&plain.layers[border]), 1);

        // Without infill above the border, the first layer of the range is
        // no bottom surface either.
        let empty = range(10.0, 20.0, &settings, "infill_density = 0.0");
        let (toolpaths, _) = slice_ranges(&objects, &[vec![empty]], &settings);
        assert_eq!(toolpaths.layers[..border], plain.layers[..border]);
        assert_eq!(
            kinds(&toolpaths.layers[border], true),
            [OuterWall, InnerWall]
        );
        assert_eq!(
            kinds(&toolpaths.layers[border + 5], true),
            [OuterWall, InnerWall]
        );
        // The top stays solid.
        let top = toolpaths.layers.last().unwrap();
        assert_eq!(kinds(top, true), [OuterWall, InnerWall, SolidInfill]);
    }

    #[test]
    fn ranges_that_change_nothing_print_as_before() {
        let settings = Settings::default();
        let objects = [cube(20.0), tee(140.0)];
        let (plain, warnings) = slice(&objects, &[], &settings);
        // The plate's own values, and no ranges at all.
        let text = settings.to_toml_keys(&["wall_loops", "infill_density", "supports"]);
        let same = range(5.0, 15.0, &settings, &text);
        assert_eq!(same.values.len(), 3);
        for ranges in [vec![vec![same.clone()], vec![same]], vec![vec![], vec![]]] {
            let (own, own_warnings) = slice_ranges(&objects, &ranges, &settings);
            assert_eq!(own, plain);
            assert_eq!(own_warnings.codes(), warnings.codes());
        }
        // A range above the object makes it a group of its own, which
        // prints the same kinds of paths.
        let outside = range(30.0, 40.0, &settings, "wall_loops = 5");
        let (own, _) = slice_ranges(&objects, &[vec![], vec![outside]], &settings);
        assert_eq!(own.layers.len(), plain.layers.len());
        for (layer, before) in own.layers.iter().zip(&plain.layers) {
            for left in [true, false] {
                assert_eq!(kinds(layer, left), kinds(before, left));
            }
        }
    }

    #[test]
    fn supports_follow_the_height_ranges() {
        let objects = [tee(20.0)];
        let plate = Settings::default();
        let supported = |toolpaths: &Toolpaths| {
            toolpaths
                .layers
                .iter()
                .any(|layer| kinds(layer, true).contains(&PathKind::Support))
        };
        // The overhang is at 10 mm: a range around it brings supports, one
        // below it does not.
        let around = range(9.0, 20.0, &plate, "supports = true");
        let (toolpaths, warnings) = slice_ranges(&objects, &[vec![around]], &plate);
        assert!(supported(&toolpaths));
        assert!(warnings.is_empty(), "{:?}", warnings.codes());
        let low = range(0.0, 5.0, &plate, "supports = true");
        let (toolpaths, warnings) = slice_ranges(&objects, &[vec![low]], &plate);
        assert!(!supported(&toolpaths));
        assert_eq!(warnings.codes(), ["slicing.unsupported_overhangs"]);
        // And the other way round.
        let with = Settings {
            supports: true,
            ..plate.clone()
        };
        let around = range(9.0, 20.0, &with, "supports = false");
        let (toolpaths, _) = slice_ranges(&objects, &[vec![around]], &with);
        assert!(!supported(&toolpaths));
    }

    #[test]
    fn ranges_take_only_their_own_keys() {
        let object = Settings {
            wall_loops: 3,
            ..Settings::default()
        };
        let mut range = HeightRange::new(10.0, 20.0);
        apply_range_toml(
            &mut range,
            &object,
            "infill_density = 100.0\ntop_layers = 2",
        )
        .unwrap();
        apply_range_toml(&mut range, &object, "infill_density = 50.0").unwrap();
        assert_eq!(range.values.len(), 2);
        let layers = range_settings(&object, &range);
        assert_eq!(layers.infill_density, 50.0);
        assert_eq!(layers.top_layers, 2);
        assert_eq!(layers.wall_loops, 3);
        // Speeds and layer heights stay those of the object.
        for text in ["infill_speed = 20.0", "layer_height = 0.1"] {
            let error = apply_range_toml(&mut range, &object, text).unwrap_err();
            assert_eq!(error.code, "settings.not_per_range");
        }
        assert!(apply_range_toml(&mut range, &object, "wall_loops = -1").is_err());
        assert_eq!(range.values.len(), 2);
        // Values set by hand outside the keys are ignored.
        range.put("layer_height", "0.1".into());
        assert_eq!(
            range_settings(&object, &range).layer_height,
            object.layer_height
        );
        for key in RANGE_SETTINGS {
            assert!(OBJECT_SETTINGS.contains(key), "{key}");
        }
        assert!(range.contains(10.0) && range.contains(19.99) && !range.contains(20.0));
    }

    #[test]
    fn project_values_of_a_range_are_restricted_to_its_keys() {
        let object = Settings::default();
        let mut range = HeightRange::new(0.0, 5.0);
        let values: Vec<(String, String)> = [
            ("sparse_infill_density", "100%"),
            ("wall_loops", "4"),
            ("layer_height", "0.1"),
            ("outer_wall_speed", "30"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let skipped = apply_range_orca(&mut range, &object, &values);
        let layers = range_settings(&object, &range);
        assert_eq!((layers.infill_density, layers.wall_loops), (100.0, 4));
        let mut keys: Vec<&str> = skipped.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["layer_height", "outer_wall_speed"]);
        assert!(
            skipped
                .iter()
                .all(|(_, reason)| reason.contains("height range"))
        );
    }

    /// Slices `objects` placed as `placement`, object `i` with the
    /// modifiers `modifiers[i]`.
    fn slice_modifiers(
        objects: &[Mesh],
        modifiers: &[Vec<Modifier>],
        placement: Placement,
        settings: &Settings,
    ) -> (Toolpaths, Diagnostics) {
        let mut diagnostics = Diagnostics::new();
        let own = OwnSettings {
            modifiers,
            ..OwnSettings::default()
        };
        let toolpaths = generate_plate_with_own_settings(
            objects,
            own,
            placement,
            settings,
            &mut diagnostics,
            &AtomicBool::new(false),
            &Progress::new(),
        )
        .unwrap();
        (toolpaths, diagnostics)
    }

    /// A cube of `size` mm with its smallest corner at `corner`, as a
    /// modifier with the settings `text` over `settings`.
    fn modifier(corner: [f64; 3], size: f64, settings: &Settings, text: &str) -> Modifier {
        let mut mesh = Mesh::cuboid(size, size, size);
        mesh.translate(corner);
        let mut modifier = Modifier::new(mesh);
        apply_modifier_toml(&mut modifier, settings, text).unwrap();
        modifier
    }

    /// Whether every point of `path` lies within `margin` of the square
    /// from `lo` to `hi` (X and Y).
    fn within(path: &crate::Path, lo: [f64; 2], hi: [f64; 2], margin: f64) -> bool {
        path.points
            .iter()
            .all(|p| (0..2).all(|a| p[a] >= lo[a] - margin && p[a] <= hi[a] + margin))
    }

    #[test]
    fn modifiers_change_the_infill_inside_them_only() {
        let settings = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let objects = [cube(20.0)];
        let (plain, plain_warnings) = slice(&objects, &[], &settings);
        // A 10 mm cube in the middle of the 20 mm cube, from 5 to 15 mm.
        let (lo, hi) = ([25.0, 55.0], [35.0, 65.0]);
        let width = settings.line_width;
        use PathKind::*;
        for text in ["infill_density = 100.0", "infill_pattern = \"gyroid\""] {
            let inside = modifier([25.0, 55.0, 5.0], 10.0, &settings, text);
            let (toolpaths, warnings) =
                slice_modifiers(&objects, &[vec![inside]], Placement::AsGiven, &settings);
            assert_eq!(warnings.codes(), plain_warnings.codes(), "{text}");
            assert_eq!(toolpaths.layers.len(), plain.layers.len());
            for (layer, before) in toolpaths.layers.iter().zip(&plain.layers) {
                let z = middle(layer);
                if !(5.0..15.0).contains(&z) {
                    // Outside the modifier nothing changes: no shells
                    // above or below it.
                    assert_eq!(layer, before, "{text} at {z}");
                    continue;
                }
                // The modifier is not printed: the walls stay those of the
                // cube.
                let walls = |l: &Layer| -> Vec<crate::Path> {
                    l.paths
                        .iter()
                        .filter(|p| matches!(p.kind, OuterWall | InnerWall))
                        .cloned()
                        .collect()
                };
                assert_eq!(walls(layer), walls(before), "{text} at {z}");
                assert_ne!(layer, before, "{text} at {z}");
                // Sparse infill is left outside the modifier; inside it is
                // solid or gyroid.
                let sparse: Vec<&crate::Path> = layer
                    .paths
                    .iter()
                    .filter(|p| p.kind == SparseInfill)
                    .collect();
                assert!(!sparse.is_empty());
                if text.contains("100") {
                    assert!(kinds(before, true).iter().all(|k| *k != SolidInfill));
                    let solid: Vec<_> = layer
                        .paths
                        .iter()
                        .filter(|p| p.kind == SolidInfill)
                        .collect();
                    assert!(!solid.is_empty());
                    assert!(solid.iter().all(|p| within(p, lo, hi, width)));
                    // Rectilinear lines outside keep their places: the
                    // sparse infill of the cube without the modifier's
                    // square.
                    assert!(sparse.iter().all(|p| !within(p, lo, hi, -width)));
                } else {
                    let polylines = sparse.iter().filter(|p| p.points.len() > 2).count();
                    assert!(polylines > 0, "gyroid curves at {z}");
                }
            }
        }
    }

    #[test]
    fn modifiers_follow_their_object_when_it_is_centred() {
        let settings = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let objects = [cube(20.0)];
        let inside = modifier([25.0, 55.0, 5.0], 10.0, &settings, "infill_density = 100.0");
        let (toolpaths, _) =
            slice_modifiers(&objects, &[vec![inside]], Placement::Centre, &settings);
        let centre = [settings.bed_size_x / 2.0, settings.bed_size_y / 2.0];
        let lo = [centre[0] - 5.0, centre[1] - 5.0];
        let hi = [centre[0] + 5.0, centre[1] + 5.0];
        let layer = &toolpaths.layers[50];
        let solid: Vec<_> = layer
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::SolidInfill)
            .collect();
        assert!(!solid.is_empty());
        assert!(solid.iter().all(|p| within(p, lo, hi, settings.line_width)));
    }

    #[test]
    fn objects_without_modifiers_print_as_before() {
        let settings = Settings::default();
        let objects = [cube(20.0), tee(140.0)];
        let (plain, warnings) = slice(&objects, &[], &settings);
        // No modifiers, and modifiers that change nothing or lie outside
        // their object, or overlap another object.
        let text = settings.to_toml_keys(&["infill_density", "supports"]);
        let same = modifier([25.0, 55.0, 5.0], 10.0, &settings, &text);
        let outside = modifier([60.0, 55.0, 5.0], 10.0, &settings, "infill_density = 100.0");
        let other = modifier(
            [140.0, 50.0, 0.0],
            30.0,
            &settings,
            "infill_density = 100.0",
        );
        let lists = [
            vec![],
            vec![vec![], vec![]],
            vec![vec![same], vec![]],
            vec![vec![outside, other], vec![]],
        ];
        for modifiers in lists {
            let (own, own_warnings) =
                slice_modifiers(&objects, &modifiers, Placement::AsGiven, &settings);
            assert_eq!(own, plain);
            assert_eq!(own_warnings.codes(), warnings.codes());
        }
    }

    #[test]
    fn supports_and_shells_follow_the_modifiers() {
        let objects = [tee(20.0)];
        let plate = Settings::default();
        let supported = |toolpaths: &Toolpaths| {
            toolpaths
                .layers
                .iter()
                .any(|layer| kinds(layer, true).contains(&PathKind::Support))
        };
        // A modifier around the overhangs at 10 mm brings supports; the
        // object itself has none.
        let around = modifier([15.0, 45.0, 8.0], 40.0, &plate, "supports = true");
        let (toolpaths, warnings) =
            slice_modifiers(&objects, &[vec![around]], Placement::AsGiven, &plate);
        assert!(supported(&toolpaths));
        assert!(warnings.is_empty(), "{:?}", warnings.codes());
        // And the other way round.
        let with = Settings {
            supports: true,
            ..plate.clone()
        };
        let around = modifier([15.0, 45.0, 8.0], 40.0, &with, "supports = false");
        let (toolpaths, _) = slice_modifiers(&objects, &[vec![around]], Placement::AsGiven, &with);
        assert!(!supported(&toolpaths));

        // Thicker top shells inside a modifier around the whole cube.
        let settings = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let cubes = [cube(20.0)];
        let solid_layers = |toolpaths: &Toolpaths| {
            toolpaths
                .layers
                .iter()
                .filter(|layer| kinds(layer, true).contains(&PathKind::SolidInfill))
                .count()
        };
        let (plain, _) = slice(&cubes, &[], &settings);
        let thick = modifier([10.0, 40.0, -1.0], 40.0, &settings, "top_layers = 10");
        let (toolpaths, _) = slice_modifiers(&cubes, &[vec![thick]], Placement::AsGiven, &settings);
        assert_eq!(solid_layers(&toolpaths), solid_layers(&plain) + 5);
    }

    #[test]
    fn modifiers_take_only_their_own_keys() {
        let object = Settings::default();
        let mut inside = Modifier::default();
        apply_modifier_toml(
            &mut inside,
            &object,
            "infill_density = 40.0\nsupports = true",
        )
        .unwrap();
        apply_modifier_toml(&mut inside, &object, "infill_density = 50.0").unwrap();
        assert_eq!(inside.values.len(), 2);
        let settings = modifier_settings(&object, &inside);
        assert_eq!((settings.infill_density, settings.supports), (50.0, true));
        for text in [
            "wall_loops = 4",
            "infill_speed = 20.0",
            "infill_pattern = \"lightning\"",
        ] {
            let error = apply_modifier_toml(&mut inside, &object, text).unwrap_err();
            assert_eq!(error.code, "settings.not_per_modifier", "{text}");
        }
        assert!(apply_modifier_toml(&mut inside, &object, "infill_density = -5.0").is_err());
        assert_eq!(inside.values.len(), 2);
        for key in MODIFIER_SETTINGS {
            assert!(RANGE_SETTINGS.contains(key), "{key}");
        }
        // From a project: keys a modifier cannot have are returned.
        let values: Vec<(String, String)> = [
            ("sparse_infill_density", "100%"),
            ("sparse_infill_pattern", "lightning"),
            ("wall_loops", "4"),
            ("top_shell_layers", "6"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let mut stored = Modifier::default();
        let skipped = apply_modifier_orca(&mut stored, &object, &values);
        let settings = modifier_settings(&object, &stored);
        assert_eq!((settings.infill_density, settings.top_layers), (100.0, 6));
        assert_eq!(settings.infill_pattern, object.infill_pattern);
        let mut keys: Vec<&str> = skipped.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["sparse_infill_pattern", "wall_loops"]);
    }
}
