//! Printer, material and process profiles with inheritance and validation.
//!
//! Settings are one flat [`Settings`] struct. Profiles are TOML files that
//! each set some of the keys and are applied one after the other, for
//! example a printer, a material and a process file; a later file overrides
//! an earlier one. The files themselves are not tied to one of these roles.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub mod catalog;
pub mod info;
pub mod orca;
pub use info::{SETTINGS_INFO, SettingInfo, UNITS, ValueKind, setting_info};
pub use orca::{OrcaImport, import_orca, import_orca_values};

/// G-code dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    /// Marlin firmware.
    Marlin,
    /// Klipper firmware.
    Klipper,
}

/// Pattern of sparse infill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InfillPattern {
    /// Parallel lines, direction alternating by 90° every layer.
    Rectilinear,
    /// Lines in both directions on every layer.
    Grid,
    /// Lines in three directions 60° apart on every layer, forming
    /// triangles.
    Triangles,
    /// Wavy lines from the cross-section of a gyroid surface; the waves turn
    /// with the height.
    Gyroid,
    /// Hexagons, the same on every layer, so they stack into hexagonal
    /// prisms: zig-zag lines across the area and short lines joining them.
    Honeycomb,
    /// Lines in three directions 60° apart that move sideways with the
    /// height, so they form cubes standing on a corner; strong in every
    /// direction.
    Cubic,
    /// Loops following the outline of the area, one inside the other.
    Concentric,
    /// Only what carries the top surfaces: trees of straight lines below
    /// them that branch out towards the walls and get shorter further
    /// down, like lightning. Areas with nothing above get no infill. Saves
    /// much material and time, but makes the part hardly stronger.
    Lightning,
}

/// Pattern of the solid infill of a top or bottom surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfacePattern {
    /// Parallel lines, printed back and forth in the order that keeps
    /// travel short.
    Rectilinear,
    /// Parallel lines all printed in the same direction, each one next to
    /// the one before, for an even sheen without criss-cross marks.
    Monotonic,
    /// Loops following the outline of the surface, one inside the other.
    Concentric,
}

/// Order in which the walls of a layer are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallOrder {
    /// From the innermost wall outward; the outer wall leans on the others.
    InnerOuter,
    /// The outer wall first, for the most accurate outside dimensions.
    OuterInner,
}

/// Where the walls of a layer start and end. The seam is visible on the
/// print as a small mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeamPosition {
    /// At the rearmost point of every loop, so the seams of all layers line
    /// up at the back of the model.
    Rear,
    /// At the sharpest corner of every loop, where the mark shows least;
    /// among equally sharp corners and on loops without corners at the
    /// rearmost point.
    Corner,
    /// Wherever the nozzle happens to be closest; shortest travel, but the
    /// seams are scattered.
    Nearest,
}

/// Shape of the supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupportStyle {
    /// Straight columns under everything that hangs free.
    Normal,
    /// Branches that grow down from points under the overhangs, lean
    /// towards each other and merge into trunks.
    Tree,
}

/// Pattern of the dense support interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupportInterfacePattern {
    /// Parallel lines, turned by 90 degrees from one layer to the next, so
    /// that every interface layer crosses the one below it.
    Lines,
    /// Loops following the outline of the interface, one inside the other.
    Concentric,
    /// Lines in both diagonal directions, each `support_interface_spacing`
    /// apart, so twice the material of `lines`.
    Grid,
}

/// Which top surfaces are ironed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IroningType {
    /// Only the topmost surface of every part: the top of its last layer,
    /// with nothing of that part above it.
    Top,
    /// Every top surface.
    All,
}

/// The kind of printer settings are meant for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Technology {
    /// Printers that melt filament and lay it down in lines.
    Fdm,
}

/// How the amount of filament is written to the G-code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtrusionMode {
    /// Every move states how much filament it adds (`M83`).
    Relative,
    /// Every move states the total since the start of the layer (`M82`).
    Absolute,
}

/// All settings that influence slicing. Lengths in mm, speeds in mm/s,
/// temperatures in °C.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    // Printer
    /// The kind of printer these settings are for. Only filament printers
    /// exist so far; settings for anything else are rejected.
    pub technology: Technology,
    /// Printable bed width (X).
    pub bed_size_x: f64,
    /// Printable bed depth (Y).
    pub bed_size_y: f64,
    /// Maximum print height (Z).
    pub max_print_height: f64,
    /// Nozzle diameter.
    pub nozzle_diameter: f64,
    /// G-code dialect.
    pub flavor: Flavor,
    /// Whether extrusion amounts in the G-code are relative or absolute.
    pub extrusion_mode: ExtrusionMode,
    /// Whether runs of short extrusion moves that lie on a circle are
    /// written as one arc (`G2` clockwise, `G3` counter-clockwise) instead
    /// of many straight `G1` lines: smaller files and smoother curves.
    /// Klipper needs `[gcode_arcs]` in `printer.cfg` for it.
    pub arc_fitting: bool,
    /// With `arc_fitting`: how far any point of the original lines (and
    /// the middle of every line) may lie from the arc that replaces them.
    pub arc_tolerance: f64,
    /// Whether the G-code names the objects on the plate, so that the
    /// printer can leave one out while printing (Klipper's
    /// `EXCLUDE_OBJECT_DEFINE`/`_START`/`_END`, Marlin's `M486`). The
    /// objects are called `object_1`, `object_2` and so on, in the order
    /// they were given.
    pub label_objects: bool,
    /// Pictures of the plate to put into the G-code header, as sizes in
    /// pixels separated by commas, such as `48x48,300x300`; empty for none.
    /// They are PNG images in the comment blocks (`; thumbnail begin`)
    /// that Klipper front-ends (Mainsail, Fluidd), OctoPrint plugins and
    /// many printer screens show.
    pub thumbnails: String,
    /// Number of extruders (tools or filament slots) of the printer. With
    /// more than one, every object prints with the extruder it names (Bambu
    /// Studio projects) and the G-code changes tools with `T<n>`; with one,
    /// everything prints with the lowest extruder the objects name.
    pub extruder_count: u32,

    // Material
    /// Filament diameter.
    pub filament_diameter: f64,
    /// Filament density in g/cm³, used for the weight estimate.
    pub filament_density: f64,
    /// Multiplier on the extruded amount.
    pub flow_ratio: f64,
    /// Nozzle temperature after the first layer.
    pub nozzle_temp: f64,
    /// Nozzle temperature on the first layer.
    pub nozzle_temp_first_layer: f64,
    /// Bed temperature after the first layer.
    pub bed_temp: f64,
    /// Bed temperature on the first layer.
    pub bed_temp_first_layer: f64,
    /// Part cooling fan speed in percent.
    pub fan_speed: f64,
    /// Number of initial layers printed with the fan off.
    pub fan_off_layers: u32,
    /// Part cooling fan speed in percent while printing bridges.
    pub bridge_fan_speed: f64,
    /// Layers that take less than this many seconds get more fan than
    /// `fan_speed`, the more the shorter they are, up to `fan_max_speed`
    /// at `min_layer_time` (or at no time at all without it). 0 turns it
    /// off.
    pub fan_layer_time: f64,
    /// Fan speed in percent for the shortest layers, see `fan_layer_time`.
    pub fan_max_speed: f64,
    /// Fan speed in percent while printing overhanging walls, see
    /// `overhang_speed`.
    pub overhang_fan_speed: f64,
    /// Fan speed in percent while printing the dense support interface,
    /// for example 100 so that it bonds less to the model and comes off
    /// more easily; -1 keeps the fan of the layer.
    pub support_interface_fan_speed: f64,
    /// Shortest time in seconds a layer may take, so that it can cool
    /// before the next one goes on top; smaller layers are printed slower.
    /// 0 turns it off.
    pub min_layer_time: f64,
    /// Slowest speed in mm/s that `min_layer_time` slows extrusion down to.
    pub min_print_speed: f64,
    /// Pressure advance (linear advance) factor written at the start of the
    /// print: `M900 K` for Marlin, `SET_PRESSURE_ADVANCE` for Klipper. 0
    /// leaves the printer's own value.
    pub pressure_advance: f64,
    /// Colour of the filament of every extruder, as `#RRGGBB` separated by
    /// commas in the order of the extruders, such as `#FF8000,#202020`;
    /// empty for none. With more than one extruder (`extruder_count`) at
    /// most one colour per extruder; extruders without one keep their
    /// colour from the built-in palette. With one extruder any number is
    /// accepted (as projects of Bambu Studio bring them) and the first one
    /// is used. Thumbnails draw every object in the colour of its extruder,
    /// a `.gcode.3mf` names the colour of every filament, and
    /// `flush_from_colours` derives purge volumes from them. Changes
    /// nothing in the G-code itself.
    pub filament_colours: String,
    /// Kind of the filament of every extruder, such as `PLA` or `PETG`,
    /// separated by commas in the order of the extruders; empty for none.
    /// Letters, digits, spaces and `+-_.` only; at most one per extruder
    /// as with `filament_colours`. Only written to the `.gcode.3mf`
    /// container (`PLA` where none is given).
    pub filament_type: String,

    // Process
    /// Layer height.
    pub layer_height: f64,
    /// Height of the first layer.
    pub first_layer_height: f64,
    /// Whether layers after the first one get thinner where the surface of
    /// the model is shallow and thicker where it is steep, between
    /// `min_layer_height` and `max_layer_height`. `layer_height` then only
    /// sets the thickness of the top and bottom shells (`top_layers` or
    /// `bottom_layers` times `layer_height`) and the steepest overhang that
    /// needs no support.
    pub adaptive_layers: bool,
    /// Thinnest layer with adaptive layers, used where the surface is nearly
    /// flat. On a sloped surface layers are only so thick that the steps
    /// they leave are no higher than on a flat one printed at this height.
    pub min_layer_height: f64,
    /// Thickest layer with adaptive layers, used where the walls are steep.
    pub max_layer_height: f64,
    /// Width of one extruded line.
    pub line_width: f64,
    /// Number of wall loops.
    pub wall_loops: u32,
    /// Order in which the walls of a layer are printed.
    pub wall_order: WallOrder,
    /// Where the walls of a layer start and end.
    pub seam_position: SeamPosition,
    /// Whether travel moves go round holes and notches inside the part
    /// instead of crossing them over open air, which also spares their
    /// retraction.
    pub avoid_crossing_walls: bool,
    /// Whether the outer wall gets a rough, textured surface: its points
    /// are moved in and out at random.
    pub fuzzy_skin: bool,
    /// How far fuzzy skin moves the points of the outer wall, in and out
    /// together.
    pub fuzzy_skin_thickness: f64,
    /// Distance between the moved points of fuzzy skin.
    pub fuzzy_skin_point_distance: f64,
    /// Number of solid layers at the top.
    pub top_layers: u32,
    /// Number of solid layers at the bottom.
    pub bottom_layers: u32,
    /// Pattern of the solid infill of top surfaces: the parts of a layer
    /// with nothing above them. Other solid infill and bridges keep
    /// rectilinear lines.
    pub top_surface_pattern: SurfacePattern,
    /// Pattern of the solid infill of the first layer, the bottom surface
    /// on the bed.
    pub bottom_surface_pattern: SurfacePattern,
    /// Vase mode: above the solid bottom layers, the outer wall alone is
    /// printed as one line rising continuously, without seams, infill or
    /// top layers. One wall loop, no supports.
    pub spiral_vase: bool,
    /// Sparse infill density in percent.
    pub infill_density: f64,
    /// Sparse infill pattern.
    pub infill_pattern: InfillPattern,
    /// Direction of the infill lines in degrees from the X axis; every
    /// other layer is turned by 90 degrees. Triangles, the gyroid,
    /// honeycomb and cubic keep it on every layer; concentric loops and
    /// lightning have no direction.
    pub infill_angle: f64,
    /// Whether the lines of rectilinear sparse infill are joined into
    /// zig-zags along the walls. Saves travel moves and retractions but
    /// uses more material (on small areas up to a third more infill).
    /// Along solid infill and bridges the lines stay apart.
    pub infill_connect: bool,
    /// How far infill reaches into the innermost wall, in percent of the line width.
    pub infill_overlap: f64,
    /// Whether top surfaces are ironed: the hot nozzle passes over them
    /// once more with very little material to smooth them.
    pub ironing: bool,
    /// Which top surfaces are ironed: only the topmost one of every part
    /// (`top`) or every one (`all`).
    pub ironing_type: IroningType,
    /// Material extruded while ironing, in percent of a normal line.
    pub ironing_flow: f64,
    /// Distance between the lines of ironing.
    pub ironing_spacing: f64,
    /// Number of skirt loops around the first layer.
    pub skirt_loops: u32,
    /// Distance between skirt and model.
    pub skirt_distance: f64,
    /// Width of the brim attached to the first layer; 0 disables it.
    pub brim_width: f64,
    /// How far the outline of the first layer is moved inwards, against
    /// the first layer squashing out wider than the model (elephant foot).
    /// 0 turns it off.
    pub elephant_foot_compensation: f64,
    /// Whether to print supports under overhangs and bridges.
    pub supports: bool,
    /// Shape of the supports: straight columns (`normal`) or branches that
    /// merge into trunks (`tree`).
    pub support_style: SupportStyle,
    /// Distance between the lines of a support. Tree supports also touch
    /// the surface they carry at points this far apart.
    pub support_spacing: f64,
    /// Number of densely filled support layers right under the surface they carry.
    pub support_interface_layers: u32,
    /// Number of densely filled support layers where a support stands on
    /// the model (bottom interface), so that it leaves a smoother surface
    /// there; 0 turns them off.
    pub support_bottom_interface_layers: u32,
    /// Pattern of the dense interface layers: `lines`, `concentric` or
    /// `grid`.
    pub support_interface_pattern: SupportInterfacePattern,
    /// Distance between the lines (or loops) of the dense interface, at
    /// least one line width; 0 means two line widths.
    pub support_interface_spacing: f64,
    /// Bridges up to this length in millimetres get no supports; 0 supports
    /// every bridge.
    pub support_bridge_length: f64,
    /// Gap between the top of a support and the surface it carries, in
    /// millimetres, rounded to whole layers of `layer_height`, at least one
    /// layer; 0 means one layer, whatever its thickness.
    pub support_top_z_distance: f64,
    /// Gap between the model a support stands on and the support, in
    /// millimetres, rounded to whole layers of `layer_height`, at least one
    /// layer; 0 means one layer, whatever its thickness.
    pub support_bottom_z_distance: f64,
    /// Distance between a support and the sides of the model, in
    /// millimetres; 0 means one line width.
    pub support_xy_distance: f64,
    /// Side of the square tower that every tool change primes the new
    /// extruder into, on every layer up to the last tool change; 0 disables
    /// it. Only used with more than one extruder.
    pub prime_tower_width: f64,
    /// X of the centre of the prime tower on the bed.
    pub prime_tower_x: f64,
    /// Y of the centre of the prime tower on the bed.
    pub prime_tower_y: f64,
    /// How much a tool change purges into the prime tower, in mm³ of
    /// filament, for every pair of extruders: `extruder_count` ×
    /// `extruder_count` numbers separated by commas, row by row, row i
    /// being the change from extruder i (counted from 0) and column j the
    /// change to extruder j, such as `0,50,200,0` for two extruders; a `;`
    /// may end a row (`0,50;200,0`). The diagonal is ignored. With one
    /// extruder any square table is accepted and not used. Empty: every
    /// tool change shares the solid tower layer with the other changes of
    /// its layer.
    pub flush_volumes: String,
    /// Factor on every value of `flush_volumes`, also on the volumes that
    /// `flush_from_colours` derives.
    pub flush_multiplier: f64,
    /// Whether, while `flush_volumes` is empty and there is more than one
    /// extruder, the purge volumes are derived from `filament_colours`,
    /// which then needs a colour for every extruder. A change from colour
    /// A to colour B purges 50 + 500 × max(0, L(B) − L(A)) + 100 × D mm³
    /// (times `flush_multiplier`), where L is the lightness of a colour
    /// from 0 (black) to 1 (white), the square root of 0.2126 r² + 0.7152
    /// g² + 0.0722 b² with r, g, b from 0 to 1, and D the distance of the
    /// two colours from 0 (same) to 1 (black and white), the square root
    /// of the mean of the squared differences of r, g and b. So black to
    /// white purges 650 mm³, white to black 150 and a change between equal
    /// colours 50: a dark colour shows through a light one. Own rule of
    /// thumb, not measured.
    pub flush_from_colours: bool,
    /// Whether every object on the plate is printed completely, all its
    /// layers, before the next one starts, lowest first; between objects
    /// the nozzle rises above everything printed. The objects must stand
    /// `extruder_clearance_radius` apart, and all but the last one printed
    /// must be no taller than `gantry_height`. Every object gets its own
    /// brim; the skirt goes round the first object only, and there is no
    /// prime tower.
    pub sequential_print: bool,
    /// With `sequential_print`: radius around the nozzle that the print
    /// head takes up below the height of the objects; objects must be at
    /// least this far apart.
    pub extruder_clearance_radius: f64,
    /// With `sequential_print`: height of the lowest part of the gantry
    /// (the X axis) above the tip of the nozzle; every object but the last
    /// one printed must be lower.
    pub gantry_height: f64,
    /// Whether arranging packs the objects by their convex outlines seen
    /// from above instead of their bounding rectangles, so round, slanted
    /// and triangular objects stand closer together. Off, arranging fills
    /// rows of rectangles as before.
    pub arrange_outlines: bool,
    /// Whether arranging may also turn objects about Z in steps of 45° where
    /// that packs them closer; the slicer turns the models it arranges, the
    /// desktop app adds the turn to their rotation. Packs by outlines even
    /// without `arrange_outlines`.
    pub arrange_rotation: bool,

    // Speeds
    /// Speed of the outermost wall.
    pub outer_wall_speed: f64,
    /// Speed of inner walls.
    pub inner_wall_speed: f64,
    /// Speed of solid and sparse infill.
    pub infill_speed: f64,
    /// Speed of bridges: solid infill with nothing below it.
    pub bridge_speed: f64,
    /// Speed of walls where more than half the line hangs over the edge of
    /// the layer below; these parts of walls are printed as overhang walls
    /// with `overhang_fan_speed`. 0 turns it off: walls keep their speed.
    pub overhang_speed: f64,
    /// Speed of walls that hang free entirely, the middle of the line more
    /// than half a line width beyond the edge of the layer below; only with
    /// `overhang_speed`. 0 turns it off: they print at `overhang_speed`.
    pub overhang_speed_full: f64,
    /// Speed of ironing.
    pub ironing_speed: f64,
    /// Speed of supports; 0 prints them at `infill_speed`.
    pub support_speed: f64,
    /// Speed of the dense support interface layers; 0 prints them at
    /// `support_speed`.
    pub support_interface_speed: f64,
    /// Multiplier on the extruded amount of bridges, on top of `flow_ratio`.
    pub bridge_flow: f64,
    /// Speed of all extrusion on the first layer.
    pub first_layer_speed: f64,
    /// Speed of moves without extrusion.
    pub travel_speed: f64,
    /// Acceleration of the printer in mm/s². Only used to estimate the
    /// print time; it is not written to the G-code.
    pub acceleration: f64,
    /// Largest speed change in mm/s the printer makes at once, for example
    /// at a corner. Only used to estimate the print time.
    pub jerk: f64,
    /// Seconds one `G28` (homing) in the start and end G-code takes. Only
    /// used to estimate the print time.
    pub homing_time: f64,
    /// How fast the nozzle heats up (and cools down), in °C per second.
    /// Only used to estimate the print time: waits for the nozzle (`M109`,
    /// `M116`) count with this rate. 0 leaves heating the nozzle out of the
    /// estimate.
    pub nozzle_heat_rate: f64,
    /// How fast the bed heats up (and cools down), in °C per second. Only
    /// used to estimate the print time: waits for the bed (`M190`, `M116`)
    /// count with this rate. 0 leaves heating the bed out of the estimate.
    pub bed_heat_rate: f64,
    /// Temperature of the nozzle and the bed before the print, for the
    /// time estimate of heating.
    pub ambient_temp: f64,

    // Retraction
    /// Filament length pulled back before a travel move; 0 disables retraction.
    pub retraction_length: f64,
    /// Speed of retraction and unretraction.
    pub retraction_speed: f64,
    /// Travel moves shorter than this do not retract.
    pub retraction_min_travel: f64,
    /// Nozzle lift during retracted travel moves; 0 disables it.
    pub z_hop: f64,
    /// Before a travel, the nozzle goes back this far along the line just
    /// printed while it retracts, wiping off what oozes; 0 turns it off.
    pub wipe_distance: f64,

    // Custom G-code
    /// G-code inserted before the print. `{setting_name}` placeholders are replaced.
    pub start_gcode: String,
    /// G-code inserted after the print. `{setting_name}` placeholders are replaced.
    pub end_gcode: String,
    /// G-code inserted at every tool change, after the old extruder has
    /// retracted and before `T<n>` selects the new one. Besides
    /// `{setting_name}`, `{previous_extruder}` and `{next_extruder}` are
    /// replaced, counted from 0 as in `T<n>`.
    pub toolchange_gcode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            technology: Technology::Fdm,
            bed_size_x: 220.0,
            bed_size_y: 220.0,
            max_print_height: 250.0,
            nozzle_diameter: 0.4,
            flavor: Flavor::Marlin,
            extrusion_mode: ExtrusionMode::Relative,
            arc_fitting: false,
            arc_tolerance: 0.01,
            label_objects: false,
            thumbnails: String::new(),
            extruder_count: 1,
            filament_diameter: 1.75,
            filament_density: 1.24,
            flow_ratio: 1.0,
            nozzle_temp: 210.0,
            nozzle_temp_first_layer: 215.0,
            bed_temp: 60.0,
            bed_temp_first_layer: 60.0,
            fan_speed: 100.0,
            fan_off_layers: 1,
            bridge_fan_speed: 100.0,
            fan_layer_time: 0.0,
            fan_max_speed: 100.0,
            overhang_fan_speed: 100.0,
            support_interface_fan_speed: -1.0,
            min_layer_time: 0.0,
            min_print_speed: 10.0,
            pressure_advance: 0.0,
            filament_colours: String::new(),
            filament_type: String::new(),
            layer_height: 0.2,
            first_layer_height: 0.2,
            adaptive_layers: false,
            min_layer_height: 0.1,
            max_layer_height: 0.3,
            line_width: 0.45,
            wall_loops: 2,
            wall_order: WallOrder::InnerOuter,
            seam_position: SeamPosition::Rear,
            avoid_crossing_walls: false,
            fuzzy_skin: false,
            fuzzy_skin_thickness: 0.3,
            fuzzy_skin_point_distance: 0.8,
            top_layers: 5,
            bottom_layers: 4,
            top_surface_pattern: SurfacePattern::Rectilinear,
            bottom_surface_pattern: SurfacePattern::Rectilinear,
            spiral_vase: false,
            infill_density: 15.0,
            infill_pattern: InfillPattern::Grid,
            infill_angle: 45.0,
            infill_connect: true,
            infill_overlap: 15.0,
            ironing: false,
            ironing_type: IroningType::All,
            ironing_flow: 10.0,
            ironing_spacing: 0.1,
            skirt_loops: 2,
            skirt_distance: 4.0,
            brim_width: 0.0,
            elephant_foot_compensation: 0.0,
            supports: false,
            support_style: SupportStyle::Normal,
            support_spacing: 2.5,
            support_interface_layers: 2,
            support_bottom_interface_layers: 0,
            support_interface_pattern: SupportInterfacePattern::Lines,
            support_interface_spacing: 0.0,
            support_bridge_length: 10.0,
            support_top_z_distance: 0.0,
            support_bottom_z_distance: 0.0,
            support_xy_distance: 0.0,
            prime_tower_width: 15.0,
            prime_tower_x: 190.0,
            prime_tower_y: 190.0,
            flush_volumes: String::new(),
            flush_multiplier: 1.0,
            flush_from_colours: false,
            sequential_print: false,
            extruder_clearance_radius: 35.0,
            gantry_height: 20.0,
            arrange_outlines: false,
            arrange_rotation: false,
            outer_wall_speed: 40.0,
            inner_wall_speed: 60.0,
            infill_speed: 80.0,
            bridge_speed: 30.0,
            overhang_speed: 0.0,
            overhang_speed_full: 0.0,
            ironing_speed: 20.0,
            support_speed: 0.0,
            support_interface_speed: 0.0,
            bridge_flow: 1.0,
            first_layer_speed: 25.0,
            travel_speed: 150.0,
            acceleration: 1000.0,
            jerk: 8.0,
            homing_time: 10.0,
            nozzle_heat_rate: 0.0,
            bed_heat_rate: 0.0,
            ambient_temp: 25.0,
            retraction_length: 0.8,
            retraction_speed: 35.0,
            retraction_min_travel: 1.5,
            z_hop: 0.0,
            wipe_distance: 0.0,
            start_gcode: "M140 S{bed_temp_first_layer}\nM104 S{nozzle_temp_first_layer}\nG28\nM190 S{bed_temp_first_layer}\nM109 S{nozzle_temp_first_layer}"
                .to_owned(),
            end_gcode: "M104 S0\nM140 S0\nM107\nG91\nG1 Z10 F600\nG90\nM84".to_owned(),
            toolchange_gcode: String::new(),
        }
    }
}

/// A problem with settings.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsError {
    /// The TOML text could not be parsed or contains unknown keys or wrong types.
    Parse(String),
    /// A key that does not exist.
    UnknownKey(String),
    /// A value outside its allowed range.
    OutOfRange {
        /// Name of the setting.
        key: &'static str,
        /// The rejected value.
        value: f64,
        /// Human-readable description of the allowed range.
        allowed: String,
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "invalid settings: {message}"),
            Self::UnknownKey(key) => write!(f, "unknown setting '{key}'"),
            Self::OutOfRange {
                key,
                value,
                allowed,
            } => {
                write!(
                    f,
                    "setting '{key}' = {value} is out of range (allowed: {allowed})"
                )
            }
        }
    }
}

impl std::error::Error for SettingsError {}

impl slicer_core::Diagnose for SettingsError {
    fn code(&self) -> &'static str {
        match self {
            Self::Parse(_) => "settings.parse",
            Self::UnknownKey(_) => "settings.unknown_key",
            Self::OutOfRange { .. } => "settings.out_of_range",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::Parse(_) => Vec::new(),
            Self::UnknownKey(key) => vec![("key", key.clone())],
            Self::OutOfRange {
                key,
                value,
                allowed,
            } => vec![
                ("key", (*key).to_owned()),
                ("value", value.to_string()),
                ("allowed", allowed.clone()),
            ],
        }
    }
}

fn range(key: &'static str, value: f64, min: f64, max: f64) -> Result<(), SettingsError> {
    if value.is_finite() && value >= min && value <= max {
        Ok(())
    } else {
        Err(SettingsError::OutOfRange {
            key,
            value,
            allowed: format!("{min} to {max}"),
        })
    }
}

/// Largest width or height of a picture in `thumbnails`, in pixels.
pub const MAX_THUMBNAIL_SIZE: u32 = 1024;

/// A colour written as `#RRGGBB` (hexadecimal digits in either case) as
/// red, green and blue.
pub fn parse_colour(text: &str) -> Option<[u8; 3]> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// A colour as `#RRGGBB` with upper-case digits.
pub fn colour_hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Purge volume in mm³ that a change from filament colour `from` to `to`
/// needs by the rule of thumb of `flush_from_colours`, before
/// `flush_multiplier`: 50 + 500 × max(0, L(to) − L(from)) + 100 × D, with
/// the lightness L and the colour distance D both from 0 to 1. Only
/// multiplications, additions and square roots, so the same on every
/// platform.
pub fn colour_flush_volume(from: [u8; 3], to: [u8; 3]) -> f64 {
    /// Purge of every change, also between equal colours.
    const BASE: f64 = 50.0;
    /// Purge for a change from black to white on top of the rest.
    const LIGHTER: f64 = 500.0;
    /// Purge for the largest colour distance.
    const DIFFERENT: f64 = 100.0;
    let unit = |colour: [u8; 3]| colour.map(|c| f64::from(c) / 255.0);
    let (a, b) = (unit(from), unit(to));
    // ponytail: squaring stands in for the sRGB curve (exponent 2.4), and
    // the square root of the luminance for perceived lightness.
    let lightness = |[r, g, b]: [f64; 3]| (0.2126 * r * r + 0.7152 * g * g + 0.0722 * b * b).sqrt();
    let squares: f64 = (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum();
    let distance = (squares / 3.0).sqrt();
    BASE + LIGHTER * (lightness(b) - lightness(a)).max(0.0) + DIFFERENT * distance
}

impl Settings {
    /// Parses settings from TOML. Missing keys take their default value,
    /// unknown keys are rejected.
    pub fn from_toml(text: &str) -> Result<Self, SettingsError> {
        let mut settings = Settings::default();
        settings.apply_toml(text)?;
        Ok(settings)
    }

    /// Overrides the settings named in a TOML text and keeps all others.
    /// Applying several files in a row layers them: the last one wins.
    /// Unknown keys are rejected; on any error nothing is changed.
    pub fn apply_toml(&mut self, text: &str) -> Result<(), SettingsError> {
        let parse = |e: toml::de::Error| SettingsError::Parse(e.message().to_owned());
        let mut table = self.table();
        table.extend(text.parse::<toml::Table>().map_err(parse)?);
        let updated: Settings = table.try_into().map_err(parse)?;
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    /// The names of the settings a TOML text sets, without checking that
    /// they exist.
    pub fn toml_keys(text: &str) -> Result<Vec<String>, SettingsError> {
        let table = text
            .parse::<toml::Table>()
            .map_err(|e| SettingsError::Parse(e.message().to_owned()))?;
        Ok(table.into_iter().map(|(key, _)| key).collect())
    }

    /// Serializes all settings as TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    /// Serializes only the named settings as TOML.
    pub fn to_toml_keys(&self, keys: &[&str]) -> String {
        let mut table = self.table();
        table.retain(|key, _| {
            let key: &str = key;
            keys.contains(&key)
        });
        toml::to_string(&table).unwrap_or_default()
    }

    /// Overrides a single setting from its textual value, as given on the
    /// command line.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), SettingsError> {
        self.set_all([(key, value)])
    }

    /// Overrides several settings from their textual values, like
    /// [`Settings::set`], and checks the limits only when all are set, so
    /// that values that depend on each other can change together. On any
    /// error nothing is changed.
    pub fn set_all<'a>(
        &mut self,
        values: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<(), SettingsError> {
        let mut table = self.table();
        for (key, value) in values {
            let Some(current) = table.get(key) else {
                return Err(SettingsError::UnknownKey(key.to_owned()));
            };
            let parsed = if current.is_str() {
                toml::Value::String(value.to_owned())
            } else {
                format!("v = {value}")
                    .parse::<toml::Table>()
                    .ok()
                    .and_then(|mut t| t.remove("v"))
                    .ok_or_else(|| {
                        SettingsError::Parse(format!("cannot parse '{value}' for '{key}'"))
                    })?
            };
            // A value of the wrong type names the setting.
            let mut probe = self.table();
            probe.insert(key.to_owned(), parsed.clone());
            probe.try_into::<Settings>().map_err(|e: toml::de::Error| {
                SettingsError::Parse(format!("{key}: {}", e.message()))
            })?;
            table.insert(key.to_owned(), parsed);
        }
        let updated: Settings = table
            .try_into()
            .map_err(|e: toml::de::Error| SettingsError::Parse(e.message().to_owned()))?;
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    fn table(&self) -> toml::Table {
        toml::Table::try_from(self).unwrap_or_default()
    }

    /// All settings as text, keyed by name. Used to fill `{name}`
    /// placeholders in custom G-code.
    pub fn placeholder_values(&self) -> BTreeMap<String, String> {
        let mut values = self.values();
        values.retain(|key, _| !key.ends_with("_gcode"));
        values
    }

    /// Every setting as text, keyed by name, in the form [`Settings::set`]
    /// accepts.
    pub fn values(&self) -> BTreeMap<String, String> {
        self.table()
            .into_iter()
            .map(|(key, value)| {
                let text = match value {
                    toml::Value::String(s) => s,
                    toml::Value::Float(f) => f.to_string(),
                    other => other.to_string(),
                };
                (key, text)
            })
            .collect()
    }

    /// Checks every value against its allowed range and the settings against
    /// each other.
    pub fn validate(&self) -> Result<(), SettingsError> {
        for (key, value, min, max) in self.limits() {
            range(key, value, min, max)?;
        }
        self.thumbnail_sizes()?;
        self.filament_colours()?;
        self.filament_types()?;
        self.flush_matrix()?;
        Ok(())
    }

    /// The colours of `filament_colours` as red, green and blue, in the
    /// order of the extruders; empty if none are given.
    pub fn filament_colours(&self) -> Result<Vec<[u8; 3]>, SettingsError> {
        let colours = self
            .per_extruder("filament_colours", &self.filament_colours)?
            .into_iter()
            .map(|entry| {
                parse_colour(entry).ok_or_else(|| {
                    SettingsError::Parse(format!(
                        "filament_colours: '{entry}' is not a colour like #FF8000"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(colours)
    }

    /// The kinds of `filament_type` in the order of the extruders; empty if
    /// none are given.
    pub fn filament_types(&self) -> Result<Vec<&str>, SettingsError> {
        let types = self.per_extruder("filament_type", &self.filament_type)?;
        let allowed = |c: char| c.is_ascii_alphanumeric() || " +-_.".contains(c);
        if let Some(entry) = types.iter().find(|entry| !entry.chars().all(allowed)) {
            return Err(SettingsError::Parse(format!(
                "filament_type: '{entry}' may only hold letters, digits, spaces and +-_."
            )));
        }
        Ok(types)
    }

    /// The entries of a list with one value per extruder, separated by
    /// commas and trimmed; empty for an empty text. No entry may be empty,
    /// and with more than one extruder there may be at most one per
    /// extruder.
    fn per_extruder<'a>(&self, key: &str, text: &'a str) -> Result<Vec<&'a str>, SettingsError> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let entries: Vec<&str> = text.split(',').map(str::trim).collect();
        if entries.iter().any(|entry| entry.is_empty()) {
            return Err(SettingsError::Parse(format!(
                "{key}: an entry between commas is empty"
            )));
        }
        if self.extruder_count > 1 && entries.len() > self.extruder_count as usize {
            return Err(SettingsError::Parse(format!(
                "{key}: {} entries, but only {} extruders (extruder_count)",
                entries.len(),
                self.extruder_count
            )));
        }
        Ok(entries)
    }

    /// The purge volumes of `flush_volumes` in mm³, already multiplied by
    /// `flush_multiplier`: `matrix[i][j]` for the change from extruder `i`
    /// to extruder `j`, with zeros on the diagonal. With `flush_volumes`
    /// empty they come from `filament_colours` if `flush_from_colours` is
    /// on (see [`colour_flush_volume`]). Empty if there is only one
    /// extruder, or if `flush_volumes` is empty and not derived.
    pub fn flush_matrix(&self) -> Result<Vec<Vec<f64>>, SettingsError> {
        let invalid = |message: String| SettingsError::Parse(format!("flush_volumes: {message}"));
        let text = self.flush_volumes.trim();
        if text.is_empty() {
            return self.flush_matrix_from_colours();
        }
        let rows: Vec<&str> = text.trim_end_matches(';').split(';').collect();
        let mut values = Vec::new();
        let mut row_lengths = Vec::new();
        for row in &rows {
            let mut length = 0;
            for entry in row.split(',').map(str::trim) {
                let value = entry
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or_else(|| {
                        invalid(format!(
                            "'{entry}' is not a volume in mm³ (a number of at least 0)"
                        ))
                    })?;
                values.push(value);
                length += 1;
            }
            row_lengths.push(length);
        }
        // The side of the table: the whole number whose square is the count.
        let side = (1..=values.len())
            .find(|n| n * n >= values.len())
            .unwrap_or(0);
        let several = self.extruder_count > 1;
        let wanted = if several {
            self.extruder_count as usize
        } else {
            side
        };
        if values.len() != wanted * wanted {
            return Err(invalid(if several {
                format!(
                    "{} values, but {} extruders (extruder_count) need {} ({wanted} rows of {wanted})",
                    values.len(),
                    wanted,
                    wanted * wanted
                )
            } else {
                format!(
                    "{} values do not form a square table (rows of as many values as there are rows)",
                    values.len()
                )
            }));
        }
        if rows.len() > 1 && row_lengths.iter().any(|&n| n != wanted) {
            return Err(invalid(format!(
                "every row separated by ';' needs {wanted} values"
            )));
        }
        if !several {
            return Ok(Vec::new());
        }
        Ok(values
            .chunks(wanted)
            .enumerate()
            .map(|(from, row)| {
                row.iter()
                    .enumerate()
                    .map(|(to, v)| {
                        if from == to {
                            0.0
                        } else {
                            v * self.flush_multiplier
                        }
                    })
                    .collect()
            })
            .collect())
    }

    /// The matrix of [`Settings::flush_matrix`] derived from the colours,
    /// if `flush_from_colours` asks for it and there are several extruders.
    fn flush_matrix_from_colours(&self) -> Result<Vec<Vec<f64>>, SettingsError> {
        if !self.flush_from_colours || self.extruder_count < 2 {
            return Ok(Vec::new());
        }
        let colours = self.filament_colours()?;
        let count = self.extruder_count as usize;
        if colours.len() < count {
            return Err(SettingsError::Parse(format!(
                "flush_from_colours: {} colours in filament_colours, but {count} extruders (extruder_count) need one each",
                colours.len()
            )));
        }
        Ok((0..count)
            .map(|from| {
                (0..count)
                    .map(|to| {
                        if from == to {
                            0.0
                        } else {
                            colour_flush_volume(colours[from], colours[to]) * self.flush_multiplier
                        }
                    })
                    .collect()
            })
            .collect())
    }

    /// The sizes of `thumbnails` as width and height in pixels, in the
    /// order given. Each side must be from 1 to [`MAX_THUMBNAIL_SIZE`]; a
    /// `/PNG` after a size, as other programs write it, is accepted.
    pub fn thumbnail_sizes(&self) -> Result<Vec<[u32; 2]>, SettingsError> {
        let invalid = |entry: &str| {
            SettingsError::Parse(format!(
                "thumbnails: '{entry}' is not a size like 300x300 (each side 1 to {MAX_THUMBNAIL_SIZE})"
            ))
        };
        let mut sizes = Vec::new();
        for entry in self.thumbnails.split(',').map(str::trim) {
            if entry.is_empty() {
                continue;
            }
            let size = entry
                .strip_suffix("/PNG")
                .or_else(|| entry.strip_suffix("/png"))
                .unwrap_or(entry);
            let side = |text: &str| {
                text.trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|side| (1..=MAX_THUMBNAIL_SIZE).contains(side))
            };
            match size.split_once(['x', 'X']) {
                Some((width, height)) => match (side(width), side(height)) {
                    (Some(width), Some(height)) => sizes.push([width, height]),
                    _ => return Err(invalid(entry)),
                },
                None => return Err(invalid(entry)),
            }
        }
        Ok(sizes)
    }

    /// Every numeric setting with its value and the smallest and largest
    /// value allowed. Some limits depend on other settings.
    pub fn limits(&self) -> Vec<(&'static str, f64, f64, f64)> {
        let mut thickest = self.layer_height.max(self.first_layer_height);
        // The limits of adaptive layers only bind while they are on, so that
        // the defaults stay valid for nozzles thinner than they are thick.
        let tallest = if self.adaptive_layers {
            thickest = thickest.max(self.max_layer_height);
            self.nozzle_diameter
        } else {
            self.nozzle_diameter.max(self.max_layer_height)
        };
        // The prime tower must stand on the bed only while it is printed, so
        // that the defaults stay valid on small beds.
        let tower = self.extruder_count > 1 && self.prime_tower_width > 0.0;
        let tower_range = |bed: f64| {
            if tower {
                (
                    self.prime_tower_width / 2.0,
                    bed - self.prime_tower_width / 2.0,
                )
            } else {
                (0.0, 2000.0)
            }
        };
        let (tower_x, tower_y) = (tower_range(self.bed_size_x), tower_range(self.bed_size_y));
        vec![
            ("bed_size_x", self.bed_size_x, 10.0, 2000.0),
            ("bed_size_y", self.bed_size_y, 10.0, 2000.0),
            ("max_print_height", self.max_print_height, 1.0, 2000.0),
            ("nozzle_diameter", self.nozzle_diameter, 0.1, 2.0),
            ("extruder_count", f64::from(self.extruder_count), 1.0, 16.0),
            ("filament_diameter", self.filament_diameter, 1.0, 3.5),
            ("filament_density", self.filament_density, 0.1, 20.0),
            ("flow_ratio", self.flow_ratio, 0.5, 2.0),
            ("nozzle_temp", self.nozzle_temp, 0.0, 500.0),
            (
                "nozzle_temp_first_layer",
                self.nozzle_temp_first_layer,
                0.0,
                500.0,
            ),
            ("bed_temp", self.bed_temp, 0.0, 200.0),
            (
                "bed_temp_first_layer",
                self.bed_temp_first_layer,
                0.0,
                200.0,
            ),
            ("fan_speed", self.fan_speed, 0.0, 100.0),
            (
                "layer_height",
                self.layer_height,
                0.02,
                self.nozzle_diameter,
            ),
            (
                "first_layer_height",
                self.first_layer_height,
                0.02,
                self.nozzle_diameter,
            ),
            (
                "min_layer_height",
                self.min_layer_height,
                0.02,
                self.max_layer_height,
            ),
            ("max_layer_height", self.max_layer_height, 0.02, tallest),
            (
                "line_width",
                self.line_width,
                (0.6 * self.nozzle_diameter).max(1.05 * thickest),
                2.5 * self.nozzle_diameter,
            ),
            ("wall_loops", f64::from(self.wall_loops), 1.0, 50.0),
            ("top_layers", f64::from(self.top_layers), 0.0, 1000.0),
            ("bottom_layers", f64::from(self.bottom_layers), 0.0, 1000.0),
            ("infill_density", self.infill_density, 0.0, 100.0),
            ("infill_overlap", self.infill_overlap, 0.0, 50.0),
            ("skirt_loops", f64::from(self.skirt_loops), 0.0, 20.0),
            ("skirt_distance", self.skirt_distance, 0.0, 50.0),
            ("brim_width", self.brim_width, 0.0, 50.0),
            ("infill_angle", self.infill_angle, 0.0, 180.0),
            ("support_spacing", self.support_spacing, 0.5, 20.0),
            (
                "support_bridge_length",
                self.support_bridge_length,
                0.0,
                1000.0,
            ),
            (
                "support_interface_layers",
                f64::from(self.support_interface_layers),
                0.0,
                10.0,
            ),
            (
                "support_bottom_interface_layers",
                f64::from(self.support_bottom_interface_layers),
                0.0,
                10.0,
            ),
            (
                "support_interface_spacing",
                self.support_interface_spacing,
                0.0,
                20.0,
            ),
            (
                "support_top_z_distance",
                self.support_top_z_distance,
                0.0,
                10.0,
            ),
            (
                "support_bottom_z_distance",
                self.support_bottom_z_distance,
                0.0,
                10.0,
            ),
            ("support_xy_distance", self.support_xy_distance, 0.0, 10.0),
            ("prime_tower_width", self.prime_tower_width, 0.0, 100.0),
            ("prime_tower_x", self.prime_tower_x, tower_x.0, tower_x.1),
            ("prime_tower_y", self.prime_tower_y, tower_y.0, tower_y.1),
            ("flush_multiplier", self.flush_multiplier, 0.0, 10.0),
            ("outer_wall_speed", self.outer_wall_speed, 1.0, 1000.0),
            ("inner_wall_speed", self.inner_wall_speed, 1.0, 1000.0),
            ("infill_speed", self.infill_speed, 1.0, 1000.0),
            ("bridge_speed", self.bridge_speed, 1.0, 1000.0),
            ("bridge_flow", self.bridge_flow, 0.5, 2.0),
            ("bridge_fan_speed", self.bridge_fan_speed, 0.0, 100.0),
            ("fan_layer_time", self.fan_layer_time, 0.0, 600.0),
            ("fan_max_speed", self.fan_max_speed, 0.0, 100.0),
            ("overhang_fan_speed", self.overhang_fan_speed, 0.0, 100.0),
            ("overhang_speed", self.overhang_speed, 0.0, 1000.0),
            ("overhang_speed_full", self.overhang_speed_full, 0.0, 1000.0),
            ("first_layer_speed", self.first_layer_speed, 1.0, 1000.0),
            ("travel_speed", self.travel_speed, 1.0, 2000.0),
            ("acceleration", self.acceleration, 10.0, 100_000.0),
            ("jerk", self.jerk, 0.0, 1000.0),
            ("homing_time", self.homing_time, 0.0, 600.0),
            ("nozzle_heat_rate", self.nozzle_heat_rate, 0.0, 100.0),
            ("bed_heat_rate", self.bed_heat_rate, 0.0, 100.0),
            ("ambient_temp", self.ambient_temp, -20.0, 80.0),
            ("retraction_length", self.retraction_length, 0.0, 20.0),
            ("retraction_speed", self.retraction_speed, 1.0, 200.0),
            (
                "retraction_min_travel",
                self.retraction_min_travel,
                0.0,
                100.0,
            ),
            ("z_hop", self.z_hop, 0.0, 10.0),
            ("min_layer_time", self.min_layer_time, 0.0, 600.0),
            ("min_print_speed", self.min_print_speed, 1.0, 1000.0),
            ("pressure_advance", self.pressure_advance, 0.0, 2.0),
            (
                "elephant_foot_compensation",
                self.elephant_foot_compensation,
                0.0,
                self.line_width,
            ),
            ("ironing_flow", self.ironing_flow, 0.0, 100.0),
            ("ironing_spacing", self.ironing_spacing, 0.01, 2.0),
            ("ironing_speed", self.ironing_speed, 1.0, 1000.0),
            ("support_speed", self.support_speed, 0.0, 1000.0),
            (
                "support_interface_speed",
                self.support_interface_speed,
                0.0,
                1000.0,
            ),
            (
                "support_interface_fan_speed",
                self.support_interface_fan_speed,
                -1.0,
                100.0,
            ),
            ("fuzzy_skin_thickness", self.fuzzy_skin_thickness, 0.0, 2.0),
            (
                "fuzzy_skin_point_distance",
                self.fuzzy_skin_point_distance,
                0.1,
                10.0,
            ),
            ("wipe_distance", self.wipe_distance, 0.0, 20.0),
            ("arc_tolerance", self.arc_tolerance, 0.001, 0.5),
            (
                "extruder_clearance_radius",
                self.extruder_clearance_radius,
                0.0,
                500.0,
            ),
            ("gantry_height", self.gantry_height, 0.0, 2000.0),
        ]
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn defaults_are_valid_and_roundtrip() {
        let defaults = Settings::default();
        defaults.validate().unwrap();
        assert_eq!(Settings::from_toml(&defaults.to_toml()).unwrap(), defaults);
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let s = Settings::from_toml("layer_height = 0.1\nflavor = \"klipper\"\n").unwrap();
        assert_eq!(s.layer_height, 0.1);
        assert_eq!(s.flavor, Flavor::Klipper);
        assert_eq!(s.wall_loops, 2);
    }

    #[test]
    fn files_are_layered_and_the_last_one_wins() {
        let mut settings = Settings::default();
        settings
            .apply_toml("bed_size_x = 256.0\nnozzle_temp = 200.0")
            .unwrap();
        settings
            .apply_toml("nozzle_temp = 240.0\nbed_temp = 80.0")
            .unwrap();
        settings.apply_toml("layer_height = 0.12").unwrap();
        assert_eq!(settings.bed_size_x, 256.0);
        assert_eq!(settings.nozzle_temp, 240.0);
        assert_eq!(settings.bed_temp, 80.0);
        assert_eq!(settings.layer_height, 0.12);
        assert_eq!(settings.line_width, Settings::default().line_width);

        // A broken layer changes nothing.
        let before = settings.clone();
        assert!(
            settings
                .apply_toml("layer_height = 0.12\nnope = 1")
                .is_err()
        );
        assert!(settings.apply_toml("layer_height = 9.0").is_err());
        assert!(settings.apply_toml("layer_height = \"thin\"").is_err());
        assert_eq!(settings, before);
    }

    #[test]
    fn thumbnail_sizes_are_read_and_checked() {
        let mut settings = Settings::default();
        assert_eq!(settings.thumbnail_sizes().unwrap(), Vec::<[u32; 2]>::new());
        settings.set("thumbnails", "48x48, 300X200/PNG,,").unwrap();
        assert_eq!(
            settings.thumbnail_sizes().unwrap(),
            vec![[48, 48], [300, 200]]
        );
        for wrong in ["48", "0x48", "48x2000", "axb", "32x32/QOI"] {
            let error = settings.set("thumbnails", wrong).unwrap_err();
            assert!(error.to_string().contains("thumbnails"), "{error}");
        }
        assert_eq!(settings.thumbnails, "48x48, 300X200/PNG,,");
    }

    #[test]
    fn flush_volumes_are_read_and_checked() {
        let mut settings = Settings::default();
        assert!(settings.flush_matrix().unwrap().is_empty());
        // With one extruder any square table is kept and not used.
        settings.set("flush_volumes", "0,1,2;3,4,5;6,7,8").unwrap();
        assert!(settings.flush_matrix().unwrap().is_empty());
        settings
            .set_all([("extruder_count", "2"), ("flush_volumes", "0, 50, 200, 0")])
            .unwrap();
        assert_eq!(
            settings.flush_matrix().unwrap(),
            [[0.0, 50.0], [200.0, 0.0]]
        );
        // The diagonal is ignored, a row may end with ';', the multiplier
        // scales.
        settings.set("flush_volumes", "9,50;200,9;").unwrap();
        settings.set("flush_multiplier", "1.5").unwrap();
        assert_eq!(
            settings.flush_matrix().unwrap(),
            [[0.0, 75.0], [300.0, 0.0]]
        );
        for wrong in [
            "0,50,200",
            "0,-5,1,0",
            "0,x,1,0",
            "0,,1,0",
            "0,inf,1,0",
            "0,50,200;0",
            "0,1,2;3,4,5;6,7,8",
        ] {
            let error = settings.set("flush_volumes", wrong).unwrap_err();
            assert!(error.to_string().contains("flush_volumes"), "{error}");
        }
        let error = settings.set("flush_volumes", "0,1,2,3,4").unwrap_err();
        assert!(error.to_string().contains("extruder_count"), "{error}");
        assert_eq!(settings.flush_volumes, "9,50;200,9;");
        // A third extruder needs a bigger table.
        assert!(settings.set("extruder_count", "3").is_err());
        settings
            .set_all([("extruder_count", "3"), ("flush_volumes", "")])
            .unwrap();
        assert!(settings.set("flush_multiplier", "-1").is_err());
    }

    #[test]
    fn filament_colours_and_types_are_read_and_checked() {
        let mut settings = Settings::default();
        assert!(settings.filament_colours().unwrap().is_empty());
        assert!(settings.filament_types().unwrap().is_empty());
        // With one extruder any number is accepted, as projects bring them.
        settings
            .set("filament_colours", " #ff8000, #202020 ,#0000FF")
            .unwrap();
        assert_eq!(
            settings.filament_colours().unwrap(),
            [[0xFF, 0x80, 0x00], [0x20, 0x20, 0x20], [0, 0, 0xFF]]
        );
        settings
            .set("filament_type", "PLA, PETG HF,PLA-CF")
            .unwrap();
        assert_eq!(
            settings.filament_types().unwrap(),
            ["PLA", "PETG HF", "PLA-CF"]
        );
        for wrong in [
            "FF8000",
            "#FF800",
            "#FF80000",
            "#GG8000",
            "#FF8000,,#000000",
            "red",
        ] {
            let error = settings.set("filament_colours", wrong).unwrap_err();
            assert!(error.to_string().contains("filament_colours"), "{error}");
        }
        for wrong in ["PLA,<x>", "PLA,", "a\"b"] {
            let error = settings.set("filament_type", wrong).unwrap_err();
            assert!(error.to_string().contains("filament_type"), "{error}");
        }
        // With several extruders at most one each.
        assert!(settings.set("extruder_count", "2").is_err());
        settings
            .set_all([
                ("extruder_count", "2"),
                ("filament_colours", "#FF8000"),
                ("filament_type", "PLA,PETG"),
            ])
            .unwrap();
        assert_eq!(settings.filament_colours().unwrap(), [[0xFF, 0x80, 0]]);
        let error = settings
            .set("filament_colours", "#000000,#111111,#222222")
            .unwrap_err();
        assert!(error.to_string().contains("extruder_count"), "{error}");
        assert_eq!(colour_hex([0xFF, 0x80, 0x0A]), "#FF800A");
        assert_eq!(parse_colour("#ff800a"), Some([0xFF, 0x80, 0x0A]));
    }

    #[test]
    fn flush_volumes_come_from_colours_only_when_asked() {
        let mut settings = Settings::default();
        settings
            .set_all([
                ("extruder_count", "3"),
                ("filament_colours", "#101010,#F0F0F0,#FF0000"),
            ])
            .unwrap();
        // Off by default: no table, as before.
        assert!(settings.flush_matrix().unwrap().is_empty());
        settings.set("flush_from_colours", "true").unwrap();
        let matrix = settings.flush_matrix().unwrap();
        assert_eq!(matrix.len(), 3);
        for (i, row) in matrix.iter().enumerate() {
            assert_eq!(row.len(), 3);
            assert_eq!(row[i], 0.0);
        }
        // Dark to light purges more than light to dark.
        assert!(matrix[0][1] > matrix[1][0], "{matrix:?}");
        assert!(matrix[0][2] > matrix[2][0], "{matrix:?}");
        assert!(matrix[2][1] > matrix[1][2], "{matrix:?}");
        assert!(matrix.iter().flatten().all(|v| *v >= 0.0 && *v <= 650.001));
        // The ends of the scale.
        let (black, white) = ([0, 0, 0], [255, 255, 255]);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(near(colour_flush_volume(black, white), 650.0));
        assert!(near(colour_flush_volume(white, black), 150.0));
        assert!(near(colour_flush_volume(white, white), 50.0));
        // The multiplier scales them, a table of its own wins.
        let before = matrix[0][1];
        settings.set("flush_multiplier", "2").unwrap();
        assert_eq!(settings.flush_matrix().unwrap()[0][1], 2.0 * before);
        settings.set("flush_volumes", "0,1,2;3,0,5;6,7,0").unwrap();
        assert_eq!(settings.flush_matrix().unwrap()[0][1], 2.0);
        // Every extruder needs a colour.
        settings.set("flush_volumes", "").unwrap();
        let error = settings
            .set("filament_colours", "#101010,#F0F0F0")
            .unwrap_err();
        assert!(error.to_string().contains("flush_from_colours"), "{error}");
        // One extruder purges nothing.
        settings
            .set_all([("extruder_count", "1"), ("filament_colours", "#101010")])
            .unwrap();
        assert!(settings.flush_matrix().unwrap().is_empty());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let error = Settings::from_toml("layer_hight = 0.1\n").unwrap_err();
        assert!(error.to_string().contains("layer_hight"), "{error}");
    }

    #[test]
    fn out_of_range_names_key_value_and_range() {
        let error = Settings::from_toml("layer_height = 0.9\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "setting 'layer_height' = 0.9 is out of range (allowed: 0.02 to 0.4)"
        );
    }

    #[test]
    fn set_overrides_single_values() {
        let mut s = Settings::default();
        s.set("infill_density", "100").unwrap();
        s.set("wall_loops", "3").unwrap();
        s.set("infill_pattern", "rectilinear").unwrap();
        s.set("start_gcode", "G28").unwrap();
        assert_eq!(s.infill_density, 100.0);
        assert_eq!(s.wall_loops, 3);
        assert_eq!(s.infill_pattern, InfillPattern::Rectilinear);
        assert_eq!(s.start_gcode, "G28");
        assert_eq!(
            s.set("nope", "1"),
            Err(SettingsError::UnknownKey("nope".into()))
        );
        assert!(s.set("wall_loops", "many").is_err());
        assert!(s.set("technology", "fdm").is_ok());
        assert!(s.set("technology", "sla").is_err());
        assert!(s.set("infill_density", "150").is_err());
        assert_eq!(s.infill_density, 100.0);
    }

    #[test]
    fn set_all_checks_the_limits_once_at_the_end() {
        let mut s = Settings::default();
        // One after the other the layer is too thick for the old nozzle.
        assert!(s.clone().set("layer_height", "0.5").is_err());
        s.set_all([
            ("layer_height", "0.5"),
            ("line_width", "0.8"),
            ("nozzle_diameter", "0.8"),
        ])
        .unwrap();
        assert_eq!((s.layer_height, s.nozzle_diameter), (0.5, 0.8));
        let before = s.clone();
        let error = s
            .set_all([("wall_loops", "3"), ("bed_temp", "hot")])
            .unwrap_err();
        assert!(error.to_string().contains("bed_temp"), "{error}");
        assert_eq!(s, before);
        // Values read back are accepted as they are.
        let values = Settings::default().values();
        assert_eq!(values["end_gcode"], Settings::default().end_gcode);
        let mut copy = s.clone();
        copy.set_all(values.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .unwrap();
        assert_eq!(copy, Settings::default());
    }

    #[test]
    fn adaptive_layer_limits_bind_only_while_on() {
        // A thin nozzle keeps the defaults valid while adaptive layers are off.
        let mut s = Settings::from_toml("nozzle_diameter = 0.25\nline_width = 0.25\nlayer_height = 0.1\nfirst_layer_height = 0.15").unwrap();
        assert!(s.set("adaptive_layers", "true").is_err());
        s.set("max_layer_height", "0.25").unwrap();
        assert!(s.set("adaptive_layers", "true").is_err(), "line too narrow");
        s.set("line_width", "0.3").unwrap();
        s.set("adaptive_layers", "true").unwrap();
        assert!(s.set("min_layer_height", "0.3").is_err());
    }

    #[test]
    fn prime_tower_must_stand_on_the_bed_only_with_several_extruders() {
        let mut s = Settings::from_toml("bed_size_x = 150.0").unwrap();
        assert_eq!(s.extruder_count, 1);
        assert!(s.set("extruder_count", "2").is_err(), "tower off the bed");
        s.set("prime_tower_x", "140").unwrap();
        s.set("extruder_count", "2").unwrap();
        assert!(s.set("prime_tower_x", "145").is_err());
        s.set("prime_tower_width", "0").unwrap();
        s.set("prime_tower_x", "145").unwrap();
        assert!(s.set("extruder_count", "17").is_err());
    }

    #[test]
    fn placeholders_use_plain_numbers() {
        let values = Settings::default().placeholder_values();
        assert_eq!(values["nozzle_temp"], "210");
        assert_eq!(values["layer_height"], "0.2");
        assert_eq!(values["flavor"], "marlin");
        assert!(!values.contains_key("start_gcode"));
    }
}
