# Spaghetti Slicer

A 3D printing slicer written entirely in Rust, for desktop and server use.

**Status: early prototype. The G-code has never been run on a printer. Do not print it unattended.**

What works: STL and 3MF import (geometry, and 3MF materials as extruders; one plate of a Bambu Studio project), repairing broken meshes (see [Mesh repair](#mesh-repair)), slicing, walls, solid top/bottom layers, sparse infill (rectilinear, grid, triangles, gyroid, honeycomb, cubic, concentric, lightning), optional adaptive layer height, thin walls and gap fill with lines of their own width, several objects on one plate, several extruders with tool changes and a prime tower, tree supports, skirt, brim, raft, retraction, G-code for Marlin and Klipper, layered settings files with a set of [profiles](profiles/README.md), time and material estimates, per-layer SVG preview, sending to Klipper (Moonraker) and OctoPrint, an HTTP server with a job API, a container image, a desktop app that also runs in the browser (object tree with parts, modifiers, negative volumes, support blockers and enforcers, and height ranges), and a first resin (MSLA) pipeline.

## Known limitations

- Supports are simple: straight columns of lines with a loop around them under everything that overhangs by more than half a line width per layer, except short bridges (`supports = true`, off by default), filled densely in the layers right under the surface they carry, with a gap to the model above (`support_top_z_distance`), below (`support_bottom_z_distance`) and to the sides (`support_xy_distance`). The gaps above and below are given in millimetres and rounded to whole layers of `layer_height`, at least one; 0, the default, means one layer whatever its thickness, as before. Where the layers differ in height (adaptive layers, layer heights of height ranges) the layers right next to the surface are counted until their heights add up to the gap, so it stays close to the millimetres asked for. The gap to the sides is 0 by default, meaning one line width. Soluble supports (`support_soluble = true`) may touch the model: 0 then means no gap at all above, below and to the sides, the interface lies right under the surface it carries (a support still stays half a line width from a wall right below that surface), and the loop around a support under a surface becomes interface too. Supports and their interface can print with an extruder of their own (`support_extruder`, `support_interface_extruder`, counted from 1, 0 = as before: the lowest extruder the objects use, and the interface that of the supports); tool changes and the prime tower follow, and an interface with an extruder of its own also takes the loops of the supports under a surface, so that only the interface material touches the model. On a 3DBenchy with soluble supports on a second extruder: 1:45:02 instead of 1:09:02 on one extruder, 5.46 m of filament on extruder 1 and 2.52 m on extruder 2 (7.98 m instead of 4.36 m in all: the difference and most of the extra time go into the prime tower), 185 tool changes; with only the interface on the second extruder 15 tool changes and 0.16 m. Orca's `support_top_z_distance`, `support_bottom_z_distance`, `support_object_xy_distance`, `support_filament` and `support_interface_filament` are taken from projects and profiles; Orca's gap of 0 needs `support_soluble` here to mean no gap. Tree supports (`support_style = tree`) start at points `tree_contact_spacing` apart (5 mm by default; the dense interface spans the gaps) under the dense interface; their branches pair up and lean towards each other at up to `tree_branch_angle` from the vertical (45° by default), keep the gap to the model (looking 16 layers ahead, so that they move aside early), merge into trunks and stand on the bed or on the model; they start as thin as `tree_tip_diameter` (0 = two line widths) and thicken on the way down with the area they carry, up to `tree_branch_diameter` (0 = 20 line widths). A branch with no neighbour within four contact spacings leans towards a farther one if it can reach it on the upper half of its way down, and a branch that never merges with another one widens below its tip like a slim cone (1 mm more across every 10 mm) instead of standing as a needle as thin as its tip. Each branch is printed whole before the next one (its loop, then the lines inside it, without retraction in between), so the many branches of a layer are toured once; grid supports print their columns the same way. The travel from one branch or column to the next skips retraction too where it stays over the supports or the model of the layer (joined across the gap between them), and with `avoid_crossing_walls` goes round inside them; over open air it retracts. On the Benchy tree supports take as long as grid supports did before that (1:06:43; grid supports now 1:05:31 with 2318 instead of 3255 retractions) with about a third of their support filament (0.21 instead of 0.61 m). Orca's `tree_support_branch_angle`, `tree_support_branch_diameter`, `tree_support_tip_diameter` and `tree_support_branch_distance` are taken over. Bridges (solid areas with nothing below) are filled with lines that run from one side that carries them to the other, in the best of every 15° and along or across the bridge's longest edge; they have their own speed, flow and fan setting (`bridge_speed`, `bridge_flow`, `bridge_fan_speed`). Supports print at `infill_speed` unless `support_speed` is set. Their dense interface (`support_interface_layers` under the surface they carry, and with `support_bottom_interface_layers` also where a support stands on the model; none by default) is a line type of its own, "Support interface" in the G-code and in the preview, for grid and tree supports alike: it has its own speed (`support_interface_speed`, 0 = `support_speed`), fan (`support_interface_fan_speed` in percent, for example 100 so that it comes off the model more easily; -1, the default, keeps the fan of the layer), pattern (`support_interface_pattern`: `lines` that turn by 90° from one layer to the next, `concentric` loops, or a `grid` of both diagonals) and spacing (`support_interface_spacing`, 0 = two line widths). The loop around a support stays support (except as said above for soluble supports or an interface extruder of its own). With supports switched on, bridges up to `support_bridge_length` (10 mm by default) get none. The seam is lined up at the back of the model, put at the sharpest corner of every loop (`seam_position = corner`; corners are measured over two line widths of path, so rounded ones count too, and inward corners are preferred because the seam hides better there) or left where the nozzle happens to be; it cannot be placed by hand.
- The G-code for Marlin and Klipper is the same. For Bambu Lab printers there is a project file (`-o name.gcode.3mf`) and an X1C profile, but neither has ever been tried on a printer: the start sequence uses common commands only, and it is not known whether the printer accepts the file.
- The materials of a 3MF file choose extruders: base materials (`<basematerials>` of the core specification) and colour groups (`<m:colorgroup>` of the materials extension), given to an object with `pid`/`pindex` or to a triangle with `pid`/`p1`. Every material in use gets an extruder of its own, in the order the triangles first use them (build items and triangles in file order), starting with extruder 1; two entries of the same colour are two materials, and triangles without a material print with extruder 1. A file with several materials gets the warning `model.materials`, which names the extruder of every material and gives their colours (`colours`, `#RRGGBB` in extruder order, `-` where none is given); `filament_colours` is not changed. Materials beyond the 64th print with extruder 1 (`model.too_many_materials`), as do those beyond `extruder_count` when slicing (`slicing.missing_extruder`); triangles whose property is a texture, a composite material or missing print as if they had none (`model.unsupported_properties`), and a triangle with different properties at its corners (a colour gradient) takes its first corner's. The print settings of an Orca Slicer or Bambu Studio project are used only on request (`--project-settings`, see below), but the extruders that a Bambu Studio project names per object and per part of an object (`<metadata key="extruder">` of the `<object>` or of its `<part>` in `Metadata/model_settings.config`; 0 on a part means the object's) are always read: a part's goes over its object's, an object's over its materials. With `extruder_count` above 1 every object prints with its extruder (tool change with `T<n>` and `toolchange_gcode`, optional prime tower); with one extruder everything prints with the lowest one (warning `slicing.multiple_extruders`). The prime tower stands up to the last tool change; on a layer with tool changes the extruders changed to print it, on a layer without the extruder that starts the layer prints it solid. Without `flush_volumes` every tool change layer is solid too and its changes share the lines. With `flush_volumes`, a table of purge volumes in mm³ per pair of extruders (`extruder_count`² numbers row by row, row = from, column = to, e.g. `0,50;200,0`; diagonal ignored; times `flush_multiplier`), every change extrudes its volume into the tower as one stretch of a zig-zag inside the outline (the outline counts towards the first change of the layer); the zig-zag is as dense as the sum of the layer's purges needs, at least a tenth of a solid layer, so the next layer has something to stand on (smaller purges grow to that). The tower keeps its square; purges that need more than a solid layer of it are cut down alike and reported as `slicing.flush_too_large` with the width that would hold them. Filament and time per extruder include the purges; the statistics give the material of the tower as `flush_volume_cm3` and `flush_weight_g` (JSON, and a line in the summary). An Orca Slicer or Bambu Studio project brings its table (`flush_volumes_matrix`, `flush_multiplier`) with `--project-settings`; with one extruder any square table is accepted and kept unused, so set `extruder_count` to the number of filaments to use it. `filament_colours` gives every extruder a colour (`#RRGGBB` separated by commas, e.g. `#FF8000,#202020`; at most `extruder_count` of them, with one extruder any number, of which the first is used) and `filament_type` a kind (`PLA,PETG`); both come from a project with `--project-settings` (`filament_colour`, `filament_type`; opacity dropped), so set `extruder_count` to at least the number of filaments. They change nothing in the G-code: thumbnails draw every object in the colour of its extruder (the built-in palette where none is given), and a `.gcode.3mf` lists every extruder the print uses with its kind, colour, metres and grams (`PLA` and white without them). With `flush_from_colours = true`, an empty `flush_volumes` and more than one extruder, the purge volumes are derived from the colours (a colour for every extruder needed) by an own rule of thumb: 50 + 500 × max(0, L(to) − L(from)) + 100 × D mm³ times `flush_multiplier`, L being the lightness from 0 (black) to 1 (white, the square root of 0.2126 r² + 0.7152 g² + 0.0722 b²) and D the colour distance from 0 to 1, so black → white purges 650 mm³ and white → black 150; off by default. The G-code header names no colours, since Moonraker does not document which comment it reads them from. Tool changes and the prime tower have never run on a printer.
- The time estimate has not been compared with a real print. Heating counts only when you give the heat rates of your printer (`nozzle_heat_rate`, `bed_heat_rate`).
- Mesh repair does not fix parts that touch or overlap along an edge or a face (edges with more than two triangles) and does not check that a patch over a hole stays clear of the rest of the model; such models are sliced as they are and reported as `mesh.not_repairable`. Objects with settings of their own are repaired without removing repeated triangles or turning inside-out shells.
- The desktop app is a first version.
- The resin output (`slice-sla`) is a first step: an open archive of layer images, not a file any printer reads, and without supports or hollowing.
- Server jobs that are still queued or running are lost when the server stops; finished ones survive only with `SPAGHETTI_DATA_DIR`.

## Trying it on a printer

Pick the files for your printer, material and layer height from [profiles/](profiles/README.md), slice a small model first, and look at the result before printing, for example with the SVG preview (`--svg`) or a G-code viewer:

```bash
cargo run --release -p slicer-cli -- slice examples/cube_20mm.stl -c profiles/printers/generic-220.toml -c profiles/materials/pla.toml -c profiles/processes/standard-0.20.toml -o cube.gcode
```

For a Bambu Lab printer, name the output `cube.gcode.3mf` to get a project file instead of plain G-code. Its description lists, for every run of layers, which filaments they use (`<layer_filament_list filament_list="0 1" layer_ranges="0 14" />`, filaments counted from 0); this shape is inferred, not compared with a file from Bambu Studio that uses several filaments. Check the start and end G-code of the printer file against what your machine needs. Stay next to the printer for the first layers.

## Usage

Slice the bundled 20 mm cube and write one SVG preview per layer:

```bash
cargo run --release -p slicer-cli -- slice examples/cube_20mm.stl -o cube.gcode --svg preview
```

Print all settings with their defaults, to use as a starting point for a config file:

```bash
cargo run --release -p slicer-cli -- default-config
```

Use a config file and override single settings:

```bash
cargo run --release -p slicer-cli -- slice model.stl -c settings.toml -s layer_height=0.1 -s infill_density=30 --json
```

All settings with defaults, units, limits and meaning are listed in [docs/settings.md](docs/settings.md). A settings file only needs the keys it changes. Several files are layered in the order given, so printer, material and process can live in separate files; a later file overrides an earlier one, and `--set` overrides them all:

```bash
cargo run --release -p slicer-cli -- slice model.stl -c printer.toml -c pla.toml -c draft.toml
```

Profiles you saved in Orca Slicer (the JSON files in its `user` folder, or exported ones) can be turned into a settings file. Give printer, material and process in this order:

```bash
cargo run --release -p slicer-cli -- import-orca printer.json material.json process.json -o from-orca.toml
```

Only settings with a clear counterpart are taken; every other one is listed on stderr with the reason. A saved Orca profile usually holds only what differs from the Orca preset it builds on (`inherits`). Those inherited values are not in the file and are not imported, so combine the result with a printer profile from `profiles/`, for example `-c profiles/printers/bambu-x1c.toml -c from-orca.toml`. The bed temperature is taken from the setting for the high-temperature plate (`hot_plate_temp`); start and end G-code are not taken, since Orca's placeholders differ. A brim type `no_brim` gives `brim_width = 0`.

Project files (3MF) of Orca Slicer and Bambu Studio store their print settings: those of the whole project in `Metadata/project_settings.config` (printer, filaments and process, by Orca's names) and those changed for single objects in `Metadata/model_settings.config`. `--project-settings` uses them, translated like `import-orca`:

```bash
cargo run --release -p slicer-cli -- slice project.3mf --project-settings -c my-printer.toml -s nozzle_temp=215
```

The layers, from bottom to top: the defaults, the project's settings (of several project files in the order given), the `-c` files, the `--set` overrides. On a single object its settings from the project go over the plate's, and `--object` files over those; so `--set infill_density=30` does not change an object to which the project gives an infill of its own. Of the settings of an object only those that an object can have here are taken (see [several models](#several-models-on-one-plate) below); an object with settings of its own is sliced as an object of its own, so the project file then counts as several objects that keep their places relative to each other, and `--object N:FILE` applies to all objects of the N-th file. Of values per filament or extruder the first is taken. Everything that is left out is reported as a warning with the setting names in `keys`: `project.settings_skipped` for the project (project files saved by Bambu Studio hold 440 to 500 settings, of which about 40 are taken), `project.object_settings_skipped` per object, `project.part_settings_skipped` for settings of parts other than modifiers, which are not read; a 3MF without such settings gives `project.no_settings`. Modifiers of an object are read from its parts with `subtype="modifier_part"` in `Metadata/model_settings.config`: the part's `id` is the object that the matching `<component>` of the object refers to in the 3D model, which gives the mesh of the modifier, and its `<metadata key value>` entries are its settings; they are used like `--modifier` for that object, before the `--modifier` files, and settings a modifier cannot have are reported as `project.modifier_settings_skipped`. An object with modifiers with settings is sliced as an object of its own. Modifier parts, negative parts (`negative_part`) and support blockers and enforcers are never printed, with or without the option; negative parts are cut out of their objects with or without the option, since they are geometry rather than settings (see [negative volumes](#negative-volumes) below; a project with negative parts is therefore always sliced as its objects), blockers or enforcers have no effect yet (`model.parts_ignored`), and modifiers read without the option are reported as `model.modifiers_not_printed`. None of the project files examined had a modifier, so this layout of modifiers is inferred and has been tested with files built to match. Height ranges of an object (height range modifiers) are read as `<range min_z="…" max_z="…">` elements inside the object in `Metadata/model_settings.config`, with their settings as `<metadata key value>` entries or `<option opt_key>` elements, and used like `--range` for that object, below the `--range` files; an object with ranges is sliced as an object of its own. Settings a range cannot have, such as its line width, are reported as `project.range_settings_skipped`, ranges without valid heights as `project.range_invalid`. This layout is inferred: none of the project files examined so far had height ranges, so it has only been tested with a file built to match. Without the option the stored settings are ignored and the G-code stays the same. The server does the same with the form field `project_settings=true` (see [Server](#server)); the desktop app does not read them yet.

Thinner layers where the surface is shallow, thicker ones on steep walls (top and bottom shells then keep their thickness, `top_layers`/`bottom_layers` × `layer_height`):

```bash
cargo run --release -p slicer-cli -- slice model.stl -s adaptive_layers=true -s min_layer_height=0.1 -s max_layer_height=0.3
```

`adaptive_layer_quality` (percent, 100 by default) sets how high the steps on a slope may be: at 100 no higher than on a flat surface printed at `min_layer_height`, at 0 as high as `max_layer_height`, so fewer and thicker layers. `layer_height_max_change` (percent, 0 = no limit, the default) limits how much thicker or thinner a layer may be than the one below, with adaptive layers and with layer heights of height ranges: at 20 the layers change by at most a factor of 1.2 from one to the next, getting thinner before thin layers are needed and never thicker than asked for; flat surfaces the layers cannot end on within the limit are crossed, and the first layer is left out. The Benchy with adaptive layers: 388 layers and 1 h 38 min without a limit (steps up to a factor of 3), 403 layers and 1 h 37 min at 20 % (largest step 1.2), 430 layers and 1 h 40 min at 5 %.

More print options, all off by default (see `docs/settings.md`):

- `min_layer_time = 8` slows extrusion on small layers so that each takes at least 8 seconds to cool, but not below `min_print_speed`.
- `pressure_advance = 0.04` writes `M900 K0.04` (Marlin) or `SET_PRESSURE_ADVANCE ADVANCE=0.04` (Klipper) at the start.
- `min_sparse_infill_area = 15` fills pieces of sparse infill smaller than 15 mm² solid: in narrow corners and small parts sparse lines carry nothing and cost travels (Benchy 45 s longer, 14 mm more filament).
- `skirt_height = 3` prints the skirt on the first three layers, the same loops on each (above the first layer at `outer_wall_speed`, the brim stays on the first); `draft_shield = true` takes it up the whole print as a wall against draughts, around everything the print covers on any layer. On Benchy a shield of the default two loops takes 38 min and 3.2 m of filament more, so `skirt_loops = 1` is worth setting with it.
- `first_layer_line_width = 0.6` prints the first layer (on a raft the raft's lowest layer) with lines 0.6 mm wide instead of `line_width`, so that it sticks better: walls, infill, skirt, brim and supports of that layer are spaced for the wider line and get material for it; the prime tower keeps its lines.
- `elephant_foot_compensation = 0.15` prints the first layer that much smaller all round.
- `xy_hole_compensation = 0.1` makes every hole 0.1 mm larger all round (negative: smaller), for holes that print too tight; `xy_contour_compensation = -0.05` moves the outer outline of every layer inwards (positive: outwards). Both act on the layers right after slicing, so walls, infill and supports follow them; a hole shrunk away closes. On Benchy shrinking the outline by 0.05 mm drops 32 gap fills and 142 retractions in narrow spots and prints 2 min faster.
- `raft_layers = 3` prints a raft under the model and its supports: the lowest layer nearly solid (90 %) to stick to the bed, layers as sparse as the supports (`support_spacing`) in the middle, and `support_interface_layers` solid layers on top, each crossing the one below. It reaches `raft_expansion` (1.5 mm) beyond the outline of the first layer, holes covered, and the model starts `raft_contact_distance` (0.1 mm) above it. The skirt goes around the raft; brim and elephant foot compensation are left out. Supports and the prime tower stand on the raft too; with `sequential_print` every object gets its own. The raft prints with the extruder of skirt and brim. On Benchy three layers add 1 min 41 s and 17 cm of filament.
- `spiral_vase = true` prints the outer wall alone above the solid bottom, as one line rising continuously, without seams. With several islands on a layer (two vases on the plate, or a model that branches) every island rises as a spiral of its own: its loop climbs from the height of the layer below to that of its layer, then the nozzle retracts (and lifts with `z_hop`), travels to the next island and comes down there to climb along that one. Every island starts next to where it started on the layer below (islands are matched from layer to layer by the largest overlap, otherwise by the nearest centre), and every layer starts with the island the layer below ended with. Holes in the outline are left out (warning `slicing.vase_holes`), so a model that already has a wall of its own prints with its outside only. A layer that also has parts too narrow for a wall loop prints flat, one path after the other (warning `slicing.vase_islands`). The time estimate and the preview follow the rising loops.
- `seam_position = "random"` starts every wall loop at another place on every layer, chosen at random but the same on every run, so that the seam marks spread over the surface instead of forming a line (Benchy 1 min 29 s longer for the travels); `"rear"` (default) lines them up at the back, `"corner"` hides them in the sharpest corner, `"nearest"` keeps travel shortest.
- `top_surface_speed = 40` prints top surfaces (the solid infill with nothing above it, "Top surface" in the G-code and its own colour in the preview; the solid infill of the first layer is "Bottom surface") at 40 mm/s instead of `infill_speed`; `gap_infill_speed = 30` does the same for gap infill instead of `inner_wall_speed`. The rest of the solid infill keeps a line's width from a top surface, so their links along the border do not overlap. On Benchy the extra line ends cost 29 s at unchanged speeds.
- `small_perimeter_speed = 20` prints wall loops no longer than a circle of `small_perimeter_threshold` (5 mm) radius at no more than 20 mm/s, so that holes, pins and fine details keep their shape; the preview coloured by speed shows it. Benchy (portholes, chimney, lettering) takes 4 min longer with it, 2 min with a 3 mm threshold.
- `top_surface_pattern = "monotonic"` prints the lines of top surfaces all in the same direction, each beside the one before, for an even sheen; `"concentric"` prints loops following the outline instead. `bottom_surface_pattern` does the same for the first layer. Other solid infill and bridges keep their lines.
- `ironing = true` passes over every top surface once more with 10 % of the material (`ironing_flow`), lines `ironing_spacing` apart at `ironing_speed`; with `ironing_type = top` only the topmost surface of every part, the top of its last layer.
- `avoid_crossing_walls = true` sends travel moves round holes and notches inside the part instead of over open air, which also saves their retraction.
- `fuzzy_skin = true` gives the outer wall a rough texture: its points are moved in and out at random by up to half of `fuzzy_skin_thickness`, one every `fuzzy_skin_point_distance`, the same on every run. The first layer stays smooth.
- `wipe_distance = 2` retracts while going back 2 mm along the line just printed.
- `arc_fitting = true` writes runs of short lines that lie on a circle within `arc_tolerance` (0.01 mm) as one `G2`/`G3` arc; the G-code of Benchy shrinks from 2.49 to 1.66 MB. Klipper needs `[gcode_arcs]` in `printer.cfg`.
- `fan_layer_time = 30` turns the fan up on layers that take less than 30 seconds, the shorter the more, up to `fan_max_speed` at `min_layer_time`.
- `overhang_speed = 15` prints the parts of walls where more than half the line hangs over the layer below at 15 mm/s and with `overhang_fan_speed`; they show as "Overhang wall" in the G-code and the preview. `overhang_speed_full = 8` prints the parts that hang free entirely (the middle of the line more than half a line width beyond the layer below) slower still.
- `label_objects = true` names every object in the G-code (`object_1`, `object_2`, …), so that the printer can leave one out mid-print: Klipper gets `EXCLUDE_OBJECT_DEFINE` with the outline of each object's first layer and `EXCLUDE_OBJECT_START`/`_END` around its lines on every layer (needs `[exclude_object]` in `printer.cfg`), Marlin `M486 T`, `M486 S<n>` and `M486 S-1`.
- `thumbnails = "48x48,300x300"` puts pictures of the plate into the G-code header, as base64 PNG in `; thumbnail begin 300x300 <length>` … `; thumbnail end` comment blocks, which Mainsail and Fluidd (through Moonraker), OctoPrint plugins and many printer screens show. They are drawn in software (front right, from above, framed to the models) and add about 0.05 s to slicing Benchy. Objects are drawn in the colour of their extruder from `filament_colours`, otherwise from a built-in palette (orange first). A `.gcode.3mf` always gets `Metadata/plate_1.png` (512×512) and a 128×128 one, and projects saved in the app `Metadata/thumbnail.png`. Whether Moonraker shows pictures from a slicer it does not know by name has not been checked.
- `nozzle_heat_rate = 2` and `bed_heat_rate = 0.5` (°C per second, measure them on your printer) add the waits for heating to the time estimate: every `M109`, `M190` or `M116` in the start, end and tool change G-code waits until the heater, which started at `ambient_temp` (25 °C) and heats from its `M104`/`M140` on, reaches its target. Temperatures set without waiting, such as those of a temperature tower, take no time. By default both are 0 and heating is left out. The moves, `G4` waits and `G28` homing (`homing_time`, 10 s) of the start and end G-code always count. The estimate in the G-code header, `print_time_s` in the JSON statistics and the prediction of a `.gcode.3mf` are the same figure; the statistics also give `time_start_end_s`, `time_toolchange_s` and `time_heating_s`, the parts of it. Benchy with the defaults: 3268 s (11 s of them start and end G-code); with the heat rates above: 3353 s (85 s heating).
- Machine limits make the time estimate follow what the firmware allows (all 0 by default, which means no limit and the same estimate as before): `max_speed_x`, `_y`, `_z` and `_e` in mm/s (Marlin `M203`) and `max_accel_x`, `_y`, `_z` and `_e` in mm/s² (`M201`) slow every move so that no axis goes beyond its limit: a move whose share along an axis is s goes at most limit / s, so a Z move with `max_speed_z = 5` takes its length over 5 mm/s whatever the G-code asks. `max_accel_extruding` and `max_accel_travel` (`M204 P` and `T`) cap `acceleration` for moves with and without extrusion. With `flavor = klipper` the printer has one acceleration for all moves (the smallest of the three) and one speed limit of the nozzle in any direction (the smaller of `max_speed_x` and `max_speed_y`, Klipper's `max_velocity`), the limits of the filament hold only for moves of the filament alone (`max_extrude_only_velocity`), and corners follow Klipper's `square_corner_velocity` (5 mm/s) instead of `jerk`: the printer goes round a circle that touches both lines at the speed whose centripetal acceleration is the acceleration, with the circle sized so that a 90° corner goes at exactly that speed; `minimum_cruise_ratio` (0.5) keeps that share of every move for cruising by lowering the top speed of short moves, as Klipper's documentation describes. With Marlin, `junction_deviation` (mm, `M205 J`) takes corners the same way instead of `jerk`. `emit_machine_limits = true` writes the limits before the start G-code: as `M201`, `M203`, `M204` and `M205` for Marlin, as `SET_VELOCITY_LIMIT VELOCITY=… ACCEL=… SQUARE_CORNER_VELOCITY=… MINIMUM_CRUISE_RATIO=…` for Klipper (Z and filament limits stay those of `printer.cfg`). Limits that the start G-code sets itself hold for the estimate from that line on, for the start G-code and for the whole print: `M201`, `M203`, `M204` (`P`, `T`, `S`) and `M205` (`J`, or `X`/`Y` as jerk) on Marlin, `M204` and `SET_VELOCITY_LIMIT` on Klipper. The moves and `G4` waits of the tool change G-code count too, from where the nozzle was, and so does the travel from where it leaves the nozzle back to the next path (`time_toolchange_s` in the statistics, "tool changes" in the header line). Benchy: 3268 s with the defaults; with `M203 X500 Y500 Z5 E25` and `M201 X500 Y500 Z100 E5000` as limits 3791 s, adding `junction_deviation = 0.013` 4028 s; with `flavor = klipper` 3456 s, with `max_speed_z = 5` and `max_accel_z = 100` 3471 s, and at `acceleration = 3000` 2678 s. Benchy with `M201 X500 Y500 Z100 E5000`, `M203 X500 Y500 Z5 E25`, `M204 P500 R1000 T1000` and `M205 X8 Y8` in the start G-code: 3919 s (before they were ignored: 3268 s); with Klipper and `SET_VELOCITY_LIMIT VELOCITY=200 ACCEL=500 SQUARE_CORNER_VELOCITY=5` in the start G-code: 4305 s (before 3456 s). Both halves of the dual-print Benchy, one per extruder (189 tool changes, prime tower): 6845 → 6993 s, with a tool change G-code that parks at X5 Y5: 7032 → 7380 s.
- `sequential_print = true` prints every object completely before the next one, lowest first; in between the nozzle rises 2 mm above everything printed so far. Objects must stand `extruder_clearance_radius` (35 mm) apart and all but the tallest must be lower than `gantry_height` (20 mm), otherwise slicing stops with `slicing.objects_too_close` or `slicing.object_too_tall`; arranging keeps that distance. Every object gets its own brim, the skirt goes round the first one only, and there is no prime tower.

None of them has been tried on a printer yet.

### Calibration prints

```bash
cargo run --release -p slicer-cli -- calibrate temperature --start 225 --end 195 --step 5 -c printer.toml -c pla.toml
```

writes `calibration-temperature.gcode`: a tower with one 10 mm block per temperature, the start value at the bottom, and `M104` at the first layer of each block. `calibrate retraction` prints two towers 30 mm apart whose retraction length changes every 5 mm of height; `calibrate flow` prints flat 20 mm blocks side by side, left to right from the front row, each with its own share of the flow ratio (`--start -10 --end 10 --step 5` in percent; multiply `flow_ratio` by 1 + the percentage of the best block / 100); `calibrate pressure-advance` prints a hollow 30 mm tower whose pressure advance factor changes every 4 mm of height (`M900 K` for Marlin, `SET_PRESSURE_ADVANCE` for Klipper; by default 0 to 0.08 in steps of 0.005, or 0 to 0.8 in steps of 0.05 with a retraction over 2 mm as on Bowden extruders) — set `pressure_advance` to the factor of the band with the sharpest corners. Every value is lettered on the model in raised digits 5–6 mm high, so nobody has to count blocks: on the front of each block of the temperature tower, on a tab beside each retraction tower (bands 1, 3, 5, … on the left tab, 2, 4, 6, … on the right one), on top of each flow block near its front edge, and on the pressure advance tower alternately on its front and its right side (its bands are too low for one label above the other). Without `--start`, `--end` and `--step` the values suit the settings. The models are sliced like any other with the settings given; afterwards the G-code gets the temperature commands, or the retraction or extrusion amounts are scaled per band of height or per block. The desktop app has the same four prints in its Calibration menu; they replace the models on the plate, and slicing the plate gives the calibration G-code. None of them has been printed yet.

### Resin (MSLA) printers (experimental)

`slice-sla` writes one PNG per layer and a JSON manifest into a ZIP archive, described in [docs/sla-format.md](docs/sla-format.md). The defaults follow the screen of the Anycubic Photon Mono X; exposure and lift values are typical starting points, not tested with a resin. No printer reads this archive directly yet.

```bash
cargo run --release -p slicer-cli -- slice-sla model.stl -o model.sla.zip --set exposure_time=2.5
```

Add `--set supports=1 --set elevation=5 --set raft_thickness=1` for supports on a raft, and `--set hollow_wall=2` to hollow the model with a 2 mm wall and drain holes.

### Several models on one plate

```bash
cargo run --release -p slicer-cli -- slice a.stl b.stl -o plate.gcode
```

arranges the models side by side (`--gap` sets the distance, default 5 mm plus the brim). `--keep-positions` keeps the coordinates of the files, for example the build item positions of a 3MF. By default arranging fills rows of bounding rectangles. `arrange_outlines = true` packs the convex outlines seen from above instead (bottom-left fill with no-fit polygons, largest first, never worse than the rows), so round, slanted and triangular models stand closer: on a 256 mm bed 56 instead of 49 cylinders 30 mm across fit. `arrange_rotation = true` also turns models about Z in steps of 45° where that packs them closer, for example triangles in pairs into squares (21 instead of 16 right triangles with 60 mm legs). Besides largest first with bottom-left fill, arranging tries the deepest and the widest models first and two more measures (the smallest rectangle around everything placed, grown from the middle of the bed or from its front left corner) and keeps the packing with the most models in the smallest rectangle, so eight right triangles with 90 mm legs fit the 220 mm bed instead of seven. `arrange_nonconvex = true` packs the real outlines seen from above instead of the convex ones (each cut into a few convex pieces, grown by half the gap), so L-shapes reach into each other and small models stand in the hollow of a U or a ring: four L-shapes 60 mm across with 20 mm arms need a rectangle of 85 × 151 instead of 101 × 172 mm with turning. With it the overlap warning compares the real outlines; printed with `sequential_print`, the convex outlines stay, since the print head needs its clearance around them. 50 mixed models with real outlines and turning take about 0.3 s on 16 cores (about 3 s of processor time). The same settings apply to the server and to the Arrange button of the desktop app, which adds the turns to the models' rotation.

A model can have settings of its own that override the plate's process: wall loops, wall order and seam position (`seam_position`), top and bottom layers, sparse infill (density, pattern, angle, connection, overlap), supports on or off, the speeds of its lines (`outer_wall_speed`, `inner_wall_speed`, `infill_speed`, `bridge_speed`, `overhang_speed`, `overhang_speed_full`, `ironing_speed`), its fan (`fan_speed`, `bridge_fan_speed`, `overhang_fan_speed`, `fan_off_layers`, `fan_layer_time`, `fan_max_speed`), its `flow_ratio` and its retraction (`retraction_length`, `retraction_speed`, `z_hop`, `wipe_distance`). In the desktop app they are under "Object settings" below the selected model; models with own values are marked with ✱ in the object list. In Rust, `slicer_fdm::generate_plate_with_object_settings` takes them per object; own speeds, fan, flow and retraction travel to the G-code writer in `Toolpaths::object_settings` and only change how that model's lines are printed (models that differ only in these are laid out exactly as without): their feed rates, the fan while they print (`M106` only where the value changes; `fan_layer_time` goes by the time of the whole layer), the filament they get (counted so in the statistics), and the retraction, wipe and Z hop of the travel after one of its lines; the filament then goes forward again by as much as was pulled back, whatever the next line's model asks for. Layer heights, line width, skirt, brim, the kind and speed of supports (`support_speed`), the first layer speed, travel, the shortest travel that retracts and the flow of bridges and ironing stay the same for the whole plate, as does the slowdown for `min_layer_time` (which slows all lines of a layer alike); skirt, brim, supports and prime tower print with the plate's fan, flow and retraction, and models without own values print exactly as before. On the command line, `--object N:FILE` layers a settings file over the plate's settings for the N-th model file only (1 is the first); it may be given several times, also for the same model, and a key outside those settings is refused (`settings.not_per_object`):

```bash
printf 'infill_density = 40\nwall_loops = 4\n' > strong.toml
cargo run --release -p slicer-cli -- slice a.stl b.stl --object 2:strong.toml -o plate.gcode
```

Settings can also change with the height of a model ("height ranges"): `--range [N:]Z1-Z2:FILE` layers a settings file over the model's settings for the layers whose middle lies at least Z1 and less than Z2 millimetres above the bed, of the N-th model file only or, without `N:`, of every model. A range may set everything a model can set for itself (wall loops, wall order, seam position, top and bottom layers, sparse infill, supports on or off, the speeds of its lines, fan, flow ratio, retraction, wipe and Z hop) and the layer height (`slicer_fdm::RANGE_SETTINGS`); other keys are refused (`settings.not_per_range`), as are malformed arguments (`cli.invalid_range`). Speeds, fan, flow and retraction of a range change only how the G-code writer prints the lines of its model on the layers of the range, which it looks up by model and layer (`slicer_fdm::Toolpaths::range_settings`, `Toolpaths::settings_on`); the minimum layer time slows them down like all others. The layer height of a range sets the layers of the whole plate between its bounds, since all models share the layers: the height between the bounds of such ranges gets as few equally thick layers as keep every one no thicker than asked for, so no layer crosses a bound (a stretch of 10.05 mm at 0.2 mm gets 51 layers of 0.197 mm), and top and bottom shells keep their thickness in millimetres. A layer height in a range of only some of several models reaches the others too (`slicing.range_layer_height_plate`); objects printed one after the other (`sequential_print`) each have their own layers. With adaptive layers a range keeps its layer height and the layers outside are adaptive; `layer_height_max_change` spreads the change from one layer height to the next over several layers (on the side of the thicker layers) instead of putting one thin layer below a bound. Files for the same heights are layered in order; where ranges overlap, the later one counts. A range belongs to the model's region on every layer, so top and bottom surfaces are found across its borders as without it: a denser infill from 10 mm up does not make the layer below a top surface, and no infill from 10 mm up does not make the first layer above a bottom surface. Models without ranges, or with ranges that change nothing, print as before. In Rust, `slicer_fdm::generate_plate_with_own_settings` takes the ranges per object (`slicer_fdm::HeightRange`). The desktop app adds height ranges in the Objects tab and edits their bounds and settings there.

```bash
printf 'infill_density = 100\nwall_loops = 4\n' > solid.toml
cargo run --release -p slicer-cli -- slice a.stl b.stl --range 2:10-20:solid.toml -o plate.gcode
```

Settings can also change inside a volume of a model ("modifiers"): `--modifier N:MESH:SETTINGS` takes a mesh (STL or 3MF, its name ending in `.stl` or `.3mf`) that is not printed; inside it the N-th model prints with the settings file SETTINGS over its own settings. The mesh is in the coordinates of the model file and moves with the model when it is centred or arranged, so a modifier made for a model in a CAD program lines up with it. A modifier may set the top and bottom layers, the sparse infill (density, pattern except lightning, angle, connecting the lines) and supports on or off (`slicer_fdm::MODIFIER_SETTINGS`); other keys, among them the walls, are refused (`settings.not_per_modifier`), as are malformed arguments (`cli.invalid_modifier`) and more than 64 modifiers (`settings.too_many_modifiers`). Where modifiers overlap, the later one counts; a modifier reaches only its own model. The model keeps one region on every layer: its walls and seam stay as they are, and its top and bottom surfaces are found as without the modifier. Only the area inside the walls is split: the sparse infill inside the modifier gets its density, pattern and angle, at 100 % solid infill, so a dense core in a light part adds no shells at the top or bottom of the core; the top and bottom shells inside it are as thick as its `top_layers` and `bottom_layers` ask for, measured from the surfaces of the model; supports go under the parts of the model inside a modifier that asks for them. Models without modifiers, or with modifiers that change nothing, print as before. In Rust, `slicer_fdm::generate_plate_with_own_settings` takes the modifiers per object (`slicer_fdm::Modifier`). The desktop app adds modifiers in the Objects tab, from a file or as a box, and edits their settings there.

```bash
printf 'infill_density = 100\n' > solid.toml
cargo run --release -p slicer-cli -- slice bracket.stl --modifier 1:bracket-core.stl:solid.toml -o bracket.gcode
```

Support blockers and enforcers are volumes of a model that are not printed either: `--support-blocker N:MESH` takes the supports away from every overhang inside the mesh or above it (so that no support passes through it), `--support-enforcer N:MESH` adds supports under every overhang inside it, even with `supports` off and even under short bridges that `support_bridge_length` would leave alone. The meshes are in the coordinates of the model file and move with the N-th model like modifiers; where a blocker and an enforcer overlap, the blocker counts. Both act only on the overhangs of their own model, even where another model reaches into them (columns under the other model's overhangs pass through a blocker), and the supports keep the shape and gaps of the plate's settings. Branches of tree supports keep out of every blocker as they keep out of the model, except straight below overhangs that get supports. Blockers and enforcers of an Orca Slicer or Bambu Studio project (parts of the types support blocker and support enforcer) are taken with `--project-settings`; without it they are left out with the warning `model.support_parts_not_applied`. Vase mode leaves them out. Malformed arguments fail with `cli.invalid_support_volume`, more than 64 per model with `settings.too_many_support_volumes`. In Rust they are `slicer_fdm::SupportVolume`. The desktop app adds support blockers and enforcers in the Objects tab, from a file or as a box.

```bash
cargo run --release -p slicer-cli -- slice bracket.stl --set supports=true --support-blocker 1:bracket-hole.stl -o bracket.gcode
```

#### Negative volumes

A volume can be cut out of a model before it is sliced any further ("negative volumes"): `--negative N:MESH` takes a mesh (STL or 3MF) in the coordinates of the N-th model file, moved with the model like a modifier, and cuts it out of every object of that file on every layer right after slicing. Everything else sees the model with the hole: walls go round it, top and bottom surfaces form where it ends, and a pocket that opens downward is an overhang that gets supports when supports are on (or is bridged when they are off). A negative volume reaches only its own model; where another model reaches into it, that one keeps its part. Layers it cuts away whole stay empty, and a model cut away entirely fails like an empty one (`slicing.nothing_to_print`). Layer heights, the size check and the overlap check still see the whole model. Malformed arguments are refused (`cli.invalid_negative`), as are more than 64 negative volumes for one object (`model.too_many_negatives`). Project files of Orca Slicer and Bambu Studio keep their negative volumes as parts with `subtype="negative_part"` in `Metadata/model_settings.config`; these are always cut out of their objects, with or without `--project-settings`, and come before the `--negative` meshes. Models without negative volumes print as before. In Rust, `slicer_fdm::generate_plate_with_own_settings` takes them per object (`OwnSettings::negatives`). In the desktop app they are added in the Objects tab. A project saved by this program keeps them; a project file of Orca Slicer or Bambu Studio is still read as one mesh, so its negative parts are not cut out yet and are reported as `model.parts_ignored`.

```bash
cargo run --release -p slicer-cli -- slice bracket.stl --negative 1:bracket-bore.stl -o bracket.gcode
```

### Sending to a printer

```bash
cargo run --release -p slicer-cli -- slice model.stl --send moonraker://printer.local:7125 --start
```

uploads the G-code to Klipper (Moonraker) and starts it; `octoprint://host` targets OctoPrint, `moonraker+https://` and `octoprint+https://` use TLS. `send file.gcode --to PRINTER` uploads an existing file. The API key comes from `--api-key` or `SPAGHETTI_PRINTER_API_KEY`. Only tested against a stand-in, never against a real printer. Bambu Lab printers are not supported yet.

### Mesh repair

Before slicing, every model is repaired as far as that is safe (objects with their own settings in the same way, once more for the part of every layer they take; the warning comes once, from the whole plate), in this order: triangles that face the other way than their neighbours are turned; triangles without area are removed (their neighbours are joined so the surface stays closed); repeated triangles are removed, and where the same triangle appears facing both ways (two solids touching along a face) both go, which joins the solids; holes are closed; closed shells that face inward are turned outward, while a shell inside another one (a cavity) keeps its way relative to the shell around it.

A hole is closed when its rim has at most 100000 edges. Rims that meet in one corner are split there. Each rim is projected onto the plane that fits it best (normal after Newell) and filled by ear clipping when it does not cross itself there, which covers concave and moderately curved holes without folds; otherwise by a fan around a new point at the middle of the rim when the rim winds around that point once; otherwise, for rims of up to 64 edges, by a fan from one corner as before. Other holes stay open. The work is bounded on large or broken meshes: sorting instead of hash-map order (the result is the same on every platform), and a budget for ear clipping and for finding nested shells.

The result is one warning: `mesh.repaired` when the mesh is closed and manifold afterwards, with what was done in the context (`holes`, `triangles` turned, `shells` turned, `degenerate`, `duplicates`), or `mesh.not_repairable` when open edges or edges with more than two triangles are left (`open_edges`, `non_manifold_edges`, and the same counts of what was done), with a hint to fix the model in a mesh editor. Slicing goes on in both cases. Of the 40 Thingi10K models the dataset marks as open or not manifold, 22 come out clean (11 before); the other 18 keep edges where parts touch, which mesh repair here does not change. Repair takes about 1 ms per 1000 triangles.

Errors go to stderr with exit code 1. With `--json`, errors and warnings are objects with a stable `code`, a `message` and a `context`:

| Code | Meaning |
|---|---|
| `model.unrecognized`, `model.truncated`, `model.invalid_vertex`, `model.incomplete_triangle`, `model.non_finite`, `model.no_triangles` | The STL file cannot be read |
| `model.archive`, `model.missing_part`, `model.xml`, `model.invalid_attribute`, `model.unsupported_unit`, `model.invalid_index`, `model.unknown_object`, `model.component_cycle`, `model.too_large` | The 3MF file cannot be read |
| `settings.parse`, `settings.unknown_key`, `settings.out_of_range` | A settings file, `--set` value or Orca profile is invalid |
| `slicing.empty_model`, `slicing.too_large`, `slicing.nothing_to_print` | The model cannot be sliced |
| `gcode.unknown_placeholder`, `gcode.unclosed_placeholder` | Custom start or end G-code is invalid |
| `io.read`, `io.write`, `cli.invalid_override`, `cli.invalid_object`, `cli.invalid_range`, `cli.invalid_modifier`, `cli.invalid_negative`, `cli.invalid_support_volume` | A file cannot be accessed or an argument is malformed |
| `settings.not_per_object` | A settings file for one object (`--object`, `object_settings_<n>`) sets a key that only the whole plate can have |
| `settings.not_per_range`, `settings.too_many_ranges` | A settings file for a height range (`--range`, `range_settings_...`) sets a key that a range cannot have, or an object has more than 256 ranges |
| `settings.not_per_modifier`, `settings.too_many_modifiers` | A settings file for a modifier (`--modifier`, `modifier_settings_<n>`) sets a key that a modifier cannot have or lightning infill, or there are more than 64 modifiers |
| `settings.too_many_support_volumes` | An object has more than 64 support blockers and enforcers |
| `model.too_many_negatives` | An object has more than 64 negative volumes (`--negative`, `negative_<n>`, negative parts of a project) |
| `model.unknown_plate` | The build plate chosen with `--plate` does not exist in the file |
| `model.multiple_plates` (warning) | The project file has several build plates; only the first is sliced unless one is chosen |
| `model.unsupported_extension` (warning) | The 3MF file requires extensions that are not supported; they are ignored |
| `model.skipped_objects` (warning) | The 3MF file has objects that are not of type `model`; they are not printed |
| `model.materials` (warning) | The 3MF file has several materials; every one prints with an extruder of its own, named in the message, with their colours in `colours` |
| `model.too_many_materials` (warning) | The 3MF file uses more than 64 materials; the others print with extruder 1 |
| `model.unsupported_properties` (warning) | Triangles of the 3MF file have a property that is not a base material or a colour (a texture, a composite material, or one that does not exist); they print as if they had none |
| `model.parts_ignored`, `model.modifiers_not_printed`, `model.support_parts_not_applied` (warnings) | The project file has parts of a type that is not known or, read by the desktop app, negative parts, which are not printed and have no effect, or modifier parts or support blockers and enforcers, which are not printed and only count with `--project-settings` |
| `project.settings_skipped`, `project.object_settings_skipped`, `project.part_settings_skipped`, `project.no_settings` (warnings) | With `--project-settings`: settings stored in the project file without a counterpart, not settable per object, on parts or modifiers, or none at all |
| `project.range_settings_skipped`, `project.range_invalid` (warnings) | With `--project-settings`: settings of a height range of an object that a range cannot have here, or a range without valid heights |
| `project.modifier_settings_skipped` (warning) | With `--project-settings`: settings of a modifier of an object that a modifier cannot have here |
| `mesh.repaired` (warning) | The mesh was broken and is closed and manifold after repair (see [Mesh repair](#mesh-repair)). Context: `holes` closed, `triangles` and `shells` turned, `degenerate` and `duplicates` triangles removed. Replaces `mesh.reoriented`, `mesh.degenerate_removed` and `mesh.inside_out` |
| `mesh.not_repairable` (warning) | Repair left open edges or edges with more than two triangles; affected layers may be wrong. Context: `open_edges`, `non_manifold_edges` and the counts of `mesh.repaired`. Replaces `mesh.non_manifold` and `mesh.not_watertight` |
| `slicing.open_contours` (warning) | Some layers have contours that do not close and were dropped |
| `slicing.unsupported_overhangs` (warning) | Some layers reach more than one line width beyond the layer below (overhangs or bridges) and supports are switched off |
| `slicing.multiple_extruders` (warning) | Objects of the model ask for different extruders while `extruder_count` is 1; everything is printed with the lowest one |
| `slicing.missing_extruder` (warning) | Objects ask for an extruder beyond `extruder_count`; they are printed with extruder 1 |
| `slicing.missing_support_extruder` (warning) | `support_extruder` or `support_interface_extruder` names an extruder beyond `extruder_count`; supports or interface print as if it were 0 |
| `slicing.range_layer_height_plate` (warning) | A height range of one model sets a layer height that the other models do not have; it sets the layers of the whole plate there |
| `slicing.prime_tower_overlap` (warning) | The prime tower overlaps the model; move it with `prime_tower_x`/`prime_tower_y` |
| `slicing.flush_too_large` (warning) | The purges (`flush_volumes`) of some tool change layers need more than a solid layer of the prime tower; they are cut short. Context: `layers`, `wanted` and `held` (mm³) of the worst layer, `width` that would hold it |
| `slicing.outside_bed` | An object placed as given (`--keep-positions`, the desktop app) reaches over the edge of the bed, or arranged objects do not fit |
| `slicing.objects_overlap` (warning) | The outlines of two objects overlap seen from above; they are printed as one |
| `calibration.invalid_range` | Start, end and step of a calibration print are invalid, out of range, or give more than 20 values |
| `send.invalid_target`, `send.connection`, `send.unauthorized`, `send.too_large`, `send.rejected` | A printer address is malformed, the printer cannot be reached, refused the API key, refused the file as too large, or refused the upload otherwise |
| `send.not_started` (warning) | The file was uploaded but the printer did not start it |
| `slicing.thin_regions` (warning) | Some layers have parts narrower than half a line width that are not printed (parts between half and one line width get a single line of their own width) |
| `slicing.vase_islands` (warning) | In vase mode some layers have lines along parts too narrow for a wall loop besides the outer walls of their islands; they print flat. Context: `layers`, `first_layer`, `last_layer` |
| `slicing.vase_holes` (warning) | In vase mode the outline of some layers has holes; only the outer outline of every island is printed. Context: `layers`, `first_layer`, `last_layer` |

## Desktop app (preview)

```bash
cargo run --release -p slicer-ui -- [model.stl] [settings.toml ...]
```

Opens a window laid out like the desktop slicers users know (Orca Slicer, Bambu Studio), drawn with its own icons: a top bar with the file menu, the Prepare, Preview and Device tabs and the slice and export buttons; on the left printer, filament, objects and the process settings in tabs (Quality, Strength, Speed, Support, Others) with units; the plate in 3D with a toolbar (add, arrange, undo, redo, duplicate, lay on face, auto orient, mirror, cut, centre, turn, delete). The selected model has a ring to turn it and a corner box to scale it by dragging. Cut splits a model at a height into two closed parts (both or one kept, the upper one beside the lower, optionally upside down), with the cutting plane shown on the plate. Modifiers, negative volumes, support blockers and enforcers are drawn in the 3D view as translucent coloured volumes, also through the model, so a volume inside it stays visible; the one the object tree edits is white. On the first start a setup assistant asks which printers (with which nozzles) and filaments you have; 66 printer models of 12 makers and 11 filament types are built in, and the File menu opens it again. The chosen profiles and the settings in use are kept for the next start (in the browser in local storage). Below the object list are position, rotation, scale, size, mirror along X, Y or Z and auto orient (least overhang, most contact with the bed); lay on face turns the model onto the face clicked next. A part or volume selected in the object tree has offset, rotation around X, Y and Z and a uniform scale of its own, in the object's coordinates; its mesh is moved around its own centre, and the 3D view shows the result. Click a model to select it and drag it across the bed; drag elsewhere to turn the view, with the right button or Shift to pan, and zoom with the wheel towards the pointer. The preview draws every extrusion as a solid strand of its width and layer height on the GPU, with an upright layer slider on the right, a slider along the bottom for the moves within the layer, and a legend of the line types with share and length that can hide each type. At the bottom left the G-code of the shown layer is listed, with the line near the last shown move marked. The preview can also colour the strands by speed, layer height or line width, and the legend shows the estimated time of every line type. Undo and redo (Ctrl+Z, Ctrl+Y), copy, paste and duplicate (Ctrl+C, Ctrl+V, Ctrl+D), Delete, Escape, the arrow keys (1 mm, with Shift 10 mm) and A to arrange work on the plate; a right click on a model opens a menu with the same actions. The button beside the printer and the filament opens their settings, which can be saved as a new profile. The filament chips under the filament profile open the filament slots: colour, kind and use of every slot, the number of extruders, and the flush volumes between slots (also derived from the colours). Every object in the tree names its filament (`auto` keeps the parts' and the file's), and a part can print with another slot than its object. A session can hold several plates: the numbered buttons above the 3D view show a plate, + adds one and a right click removes one; each plate has its own models and its own slice, so slicing, arranging and exporting work on the plate shown (the file name gets the plate number), and the context menu of a model moves it to another plate or a new one. Undo and redo cover all plates. Messages appear at the bottom right. Building it on Linux needs no extra system packages.

File → Save project (Ctrl+S) writes the session to the project file it was opened from or saved to last, asking where the first time; File → Save project as… (Ctrl+Shift+S) always asks. The file is a 3MF: every model as it is on the plate (also after cutting, mirroring or laying it on a face), placed by its build item, so other programs that read 3MF open the models where they lie. The settings files in use (with their text), the values of the side panel, the list of all settings and the extra lines are kept in the part `Metadata/spaghetti-project.toml`; its layout is described in `crates/slicer-ui/src/project.rs`. The picture of the first plate uses the project's filament colours. File → Open project… (Ctrl+O) replaces the plate and the settings with those of the file. A dropped or opened `.3mf` with that part opens as this program's project. A project of Orca Slicer or Bambu Studio (a 3MF with `Metadata/project_settings.config`) opens with its print settings, object settings, height ranges, modifiers, negative volumes and support blockers and enforcers; what has no counterpart is reported as a warning, and the original file is never overwritten by Ctrl+S. Any other 3MF opens as a model. In the browser saving downloads the file.

The Device tab sends the G-code of the shown plate to a printer that runs Moonraker (Klipper) or OctoPrint: give `moonraker://host:port` or `octoprint://host:port` (`+https` for TLS), optionally an API key, and whether the printer starts the file after the upload. The upload runs in the background and its result, including warnings such as `send.not_started`, appears in the message list. The address, key and switch are kept between runs on the computer, never in a project. Sending is desktop-only; the browser build explains that in the tab.

The same app runs in a browser. Build it with `bash tools/build-web.sh` (needs `rustup target add wasm32-unknown-unknown` and `wasm-bindgen-cli` in the version from `Cargo.lock`), start the server with `SPAGHETTI_WEB_DIR=target/web` and open `http://localhost:8080/app/`. Models are sliced in the browser, in a Web Worker, so the page keeps reacting, shows the progress and can cancel; nothing is uploaded. In the browser there is no profile folder to choose and no Orca import; the built-in profiles of the setup assistant are there.

## Server

Start the server; it listens on port 8080:

```bash
cargo run --release -p slicer-server
```

Or build and run the container image (runs as an unprivileged user, contains the server and the command line tool):

```bash
docker build -t spaghetti-slicer .
```

```bash
docker run --rm -p 8080:8080 -e SPAGHETTI_TOKEN=change-me spaghetti-slicer
```

Create a job from a model, optionally with a settings file and single overrides. The answer contains the job id:

```bash
curl -F model=@examples/cube_20mm.stl -F settings=@settings.toml -F set=infill_density=30 localhost:8080/v1/jobs
```

Several `model` fields put several objects on the plate, arranged side by side and numbered from 1 in the order sent. A field `object_settings_<n>` holds a settings file for object n only, with the same keys and rules as `--object` on the command line:

```bash
curl -F model=@a.stl -F model=@b.stl -F object_settings_2=@strong.toml localhost:8080/v1/jobs
```

A field `range_settings_<z1>-<z2>` holds a settings file for the layers from z1 up to z2 millimetres of every object, `range_settings_<n>_<z1>-<z2>` for those of object n only, with the same keys and rules as `--range` on the command line:

```bash
curl -F model=@a.stl -F model=@b.stl -F range_settings_2_10-20=@solid.toml localhost:8080/v1/jobs
```

A field `modifier_<n>` holds the mesh of a modifier of object n (STL or 3MF) and `modifier_settings_<n>` its settings file, with the same keys and rules as `--modifier` on the command line; the k-th `modifier_<n>` goes with the k-th `modifier_settings_<n>`:

```bash
curl -F model=@bracket.stl -F modifier_1=@bracket-core.stl -F modifier_settings_1=@solid.toml localhost:8080/v1/jobs
```

Fields `support_blocker_<n>` and `support_enforcer_<n>` hold the meshes of support blockers and enforcers of object n, as `--support-blocker` and `--support-enforcer` do on the command line:

```bash
curl -F model=@bracket.stl -F set=supports=true -F support_blocker_1=@bracket-hole.stl localhost:8080/v1/jobs
```

A field `negative_<n>` holds a negative volume of object n (STL or 3MF), cut out of it as `--negative` does on the command line; negative parts of a project are cut out with or without `project_settings`:

```bash
curl -F model=@bracket.stl -F negative_1=@bracket-bore.stl localhost:8080/v1/jobs
```

The field `project_settings=true` uses the print settings stored in project files of Orca Slicer and Bambu Studio, as `--project-settings` does on the command line: the project's settings below the `settings` and `set` fields, the settings the project gives single objects over the plate's and below `object_settings_<n>`, the height ranges of objects below `range_settings_...`, the modifiers of objects before the `modifier_<n>` fields, the support blockers and enforcers of objects before the `support_blocker_<n>` and `support_enforcer_<n>` fields, and a project whose objects have settings, height ranges, modifiers, support blockers or enforcers of their own is sliced as those objects. What is left out appears in the job's `warnings` (`project.settings_skipped`, `project.object_settings_skipped`, `project.part_settings_skipped`, `project.range_settings_skipped`, `project.range_invalid`, `project.modifier_settings_skipped`, `project.no_settings`); with several models their messages start with `model <n>:`. Without the field the stored settings are ignored:

```bash
curl -F model=@project.3mf -F project_settings=true -F set=nozzle_temp=215 localhost:8080/v1/jobs
```

| Request | Purpose |
|---|---|
| `GET /health` | Liveness check, never needs a token |
| `GET /openapi.json` | Description of this API (OpenAPI 3.1), never needs a token |
| `POST /v1/jobs` | Create a job (multipart fields `model`, `settings`, `set`, `plate`, `project_settings`, `object_settings_<n>`, `range_settings_[<n>_]<z1>-<z2>`, `modifier_<n>`, `modifier_settings_<n>`, `negative_<n>`, `support_blocker_<n>`, `support_enforcer_<n>`; several `settings` files are layered in order); answers `202` at once |
| `GET /v1/jobs/{id}` | Status (`queued`, `running`, `done`, `failed`, `cancelled`), statistics, warnings or the error |
| `GET /v1/jobs/{id}/gcode` | The G-code of a finished job |
| `POST /v1/jobs/{id}/send` | Upload the G-code of a finished job to a printer configured on the server; JSON body `{"printer": NAME, "start": false}`; 502 if the printer fails |
| `GET /app/` | The browser app, if `SPAGHETTI_WEB_DIR` is set |
| `GET /v1/jobs/{id}/gcode.3mf` | The same G-code as a project file for Bambu Lab printers (untested on a printer, like the file the command line tool writes) |
| `DELETE /v1/jobs/{id}` | Cancel an unfinished job or delete a finished one |

Errors have the same shape and codes as the command line tool, plus `request.invalid`, `request.too_large`, `request.unauthorized`, `request.not_found`, `job.not_found`, `job.no_result` and `server.busy`. Jobs live in memory and are lost on restart. Cancelling a job that is already being sliced discards its result but does not stop the computation.

The server is configured through environment variables only:

| Variable | Default | Meaning |
|---|---|---|
| `SPAGHETTI_LISTEN` | `0.0.0.0:8080` | Address and port |
| `SPAGHETTI_WORKERS` | number of CPU cores | Jobs sliced at the same time |
| `SPAGHETTI_MAX_PENDING` | `100` | Unfinished jobs accepted before new ones get `503` |
| `SPAGHETTI_MAX_UPLOAD_BYTES` | `268435456` | Largest request body |
| `SPAGHETTI_RESULT_TTL_SECONDS` | `3600` | How long finished jobs are kept |
| `SPAGHETTI_TOKEN` | unset | If set, `/v1/` requires `Authorization: Bearer <token>` |
| `SPAGHETTI_PRINTERS` | unset | Printers jobs may be sent to, as `name=address,…` (e.g. `shop=moonraker://10.0.0.5:7125`). Callers can only name these; without `SPAGHETTI_TOKEN` anyone who can reach the port can start prints on them |
| `SPAGHETTI_PRINTER_KEY_<NAME>` | unset | API key of printer NAME (upper case) |
| `SPAGHETTI_WEB_DIR` | unset | Directory with the built browser app, served at `/app/` |
| `SPAGHETTI_DATA_DIR` | unset | If set, finished jobs (status and G-code) are kept in this directory and survive a restart, until they expire. Jobs still queued or running when the server stops are lost. In the container, mount a volume writable by user 65532 |

Without a token anyone who can reach the port can submit jobs. Logs are JSON lines on standard output.

## Reference models

`examples/` holds six generated models that print without supports: a cube, a cylinder, a dome, a tube, a stepped block and a gear with a hub. CI slices all of them on Linux, Windows and macOS and fails if the G-code differs between platforms. Regenerate them with:

```bash
cargo run -p slicer-formats --example reference_models -- examples
```

`examples/golden.tsv` holds size, line count and a checksum of the G-code of each reference model, and `examples/golden-layers.tsv` a checksum per layer (layer 0 is the start G-code, the last layer also holds the end G-code and the settings). The tests fail if the output changes and name the first layer that differs. After an intended change, rewrite both files and commit them with the change:

```bash
UPDATE_GOLDEN=1 cargo test -p slicer-cli --test corpus golden
```

## Real-world models

3DBenchy (CC0) with its parts and 200 models from the Thingi10K dataset (CC BY, CC BY-SA or public domain) are used as real-world test models. They are not stored in the repository; see [corpus/README.md](corpus/README.md) for sources, license and how to fetch them.

## Comparison with Orca Slicer

Slicing time, memory and results for five models next to Orca Slicer, with method and findings, are in [docs/benchmarks.md](docs/benchmarks.md).

## Fuzzing

`fuzz/` holds fuzz targets for the STL and 3MF readers. `.github/workflows/fuzz.yml` runs them for a few minutes every week. Locally, with a nightly toolchain and `cargo install cargo-fuzz`:

```bash
cargo +nightly fuzz run stl
```

## Releases

A tag `v*` runs `.github/workflows/release.yml`: it builds archives with the command line tool, the server, the desktop app (`spaghetti-slicer-ui`) and the profiles for Linux, Windows and macOS, a container image for amd64 and arm64 (pushed to `ghcr.io`), and publishes a GitHub release with checksums and generated notes. A tag with a suffix such as `v0.1.0-test` becomes a pre-release and does not move the image tag `latest`. Started by hand, the workflow only builds, as a rehearsal.

The programs are not signed. Windows SmartScreen asks before the first start ("More info", "Run anyway"); on macOS remove the quarantine flag after unpacking with `xattr -dr com.apple.quarantine spaghetti-slicer-macos-arm64`.

## Project documents

- Product requirements: [PRD.md](PRD.md) (German)
- Ticket breakdown for phase 0–1: [TICKETS.md](TICKETS.md) (German)
- Design and dependency decisions: [docs/decisions](docs/decisions) (German)

## Workspace

| Crate | Purpose |
|---|---|
| `slicer-core` | Geometry, meshes, layering, polygon operations. No file system, network or UI. |
| `slicer-fdm` | FDM toolpaths: perimeters, infill, supports, path planning |
| `slicer-gcode` | G-code generation with exchangeable flavors |
| `slicer-formats` | Model and project formats (STL, 3MF) |
| `slicer-profiles` | Printer, material and process profiles |
| `slicer-sla` | Resin (MSLA) pipeline: layer images and an openly documented print archive |
| `slicer-send` | Sending G-code to a printer (Moonraker, OctoPrint) |
| `slicer-cli` | Command line interface (`spaghetti-slicer`) |
| `slicer-server` | HTTP API server (`spaghetti-slicer-server`) |
| `slicer-ui` | Desktop app (`spaghetti-slicer-ui`), also runs in the browser |

## Building

Requires a stable Rust toolchain (installed automatically by `rustup` from `rust-toolchain.toml`).

If the project lies on a network drive, the build fails with "access denied" when it tries to run its build scripts. Point cargo to a local folder first, for example in PowerShell with `$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\cargo-target"`.

```bash
cargo build
```

```bash
cargo test
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all --check
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

The project is written without reference to the source code of other slicers. See [CONTRIBUTING.md](CONTRIBUTING.md) before contributing.
