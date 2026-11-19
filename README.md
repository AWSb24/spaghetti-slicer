# Spaghetti Slicer

A 3D printing slicer written entirely in Rust, for desktop and server use.

**Status: early prototype. The G-code has never been run on a printer. Do not print it unattended.**

What works: STL and 3MF import (geometry only, one plate of a Bambu Studio project), closing small holes in a mesh, slicing, walls, solid top/bottom layers, sparse infill (rectilinear, grid, triangles, gyroid, honeycomb, cubic, concentric, lightning), optional adaptive layer height, thin walls and gap fill with lines of their own width, several objects on one plate, several extruders with tool changes and a prime tower, tree supports, skirt, brim, retraction, G-code for Marlin and Klipper, layered settings files with a set of [profiles](profiles/README.md), time and material estimates, per-layer SVG preview, sending to Klipper (Moonraker) and OctoPrint, an HTTP server with a job API, a container image, a desktop app that also runs in the browser, and a first resin (MSLA) pipeline.

## Known limitations

- Supports are simple: straight columns of lines with a loop around them under everything that overhangs by more than half a line width per layer, except short bridges (`supports = true`, off by default), filled densely in the layers right under the surface they carry, with one empty layer between support and model above and below. Tree supports (`support_style = tree`) start at points `support_spacing` apart under the dense interface; their branches lean towards each other, keep the gap to the model (looking 16 layers ahead, so that they move aside early), merge into trunks and stand on the bed or on the model; they start thin and thicken on the way down. The gaps cannot be set. Bridges (solid areas with nothing below) are filled with lines that run from one side that carries them to the other, in the best of every 15° and along or across the bridge's longest edge; they have their own speed, flow and fan setting (`bridge_speed`, `bridge_flow`, `bridge_fan_speed`). Supports print at `infill_speed` unless `support_speed` is set. Their dense interface (`support_interface_layers` under the surface they carry, and with `support_bottom_interface_layers` also where a support stands on the model; none by default) is a line type of its own, "Support interface" in the G-code and in the preview, for grid and tree supports alike: it has its own speed (`support_interface_speed`, 0 = `support_speed`), fan (`support_interface_fan_speed` in percent, for example 100 so that it comes off the model more easily; -1, the default, keeps the fan of the layer), pattern (`support_interface_pattern`: `lines` across the support below, `concentric` loops, or a `grid` of both diagonals) and spacing (`support_interface_spacing`, 0 = two line widths). The loop around a support stays support, and the gap to the model stays one layer. With supports switched on, bridges up to `support_bridge_length` (10 mm by default) get none. The seam is lined up at the back of the model, put at the sharpest corner of every loop (`seam_position = corner`; corners are measured over two line widths of path, so rounded ones count too, and inward corners are preferred because the seam hides better there) or left where the nozzle happens to be; it cannot be placed by hand.
- The G-code for Marlin and Klipper is the same. For Bambu Lab printers there is a project file (`-o name.gcode.3mf`) and an X1C profile, but neither has ever been tried on a printer: the start sequence uses common commands only, and it is not known whether the printer accepts the file.
- Colours stored in a 3MF file are ignored; the print settings of an Orca Slicer or Bambu Studio project are used only on request (`--project-settings`, see below). The extruder that a Bambu Studio project names per object is read. With `extruder_count` above 1 every object prints with its extruder (tool change with `T<n>` and `toolchange_gcode`, optional prime tower); with one extruder everything prints with the lowest one (warning `slicing.multiple_extruders`). Tool changes and the prime tower have never run on a printer.
- The time estimate has not been compared with a real print and ignores heating.
- The desktop app is a first version.
- The resin output (`slice-sla`) is a first step: an open archive of layer images, not a file any printer reads, and without supports or hollowing.
- Server jobs that are still queued or running are lost when the server stops; finished ones survive only with `SPAGHETTI_DATA_DIR`.

## Trying it on a printer

Pick the files for your printer, material and layer height from [profiles/](profiles/README.md), slice a small model first, and look at the result before printing, for example with the SVG preview (`--svg`) or a G-code viewer:

```bash
cargo run --release -p slicer-cli -- slice examples/cube_20mm.stl -c profiles/printers/generic-220.toml -c profiles/materials/pla.toml -c profiles/processes/standard-0.20.toml -o cube.gcode
```

For a Bambu Lab printer, name the output `cube.gcode.3mf` to get a project file instead of plain G-code. Check the start and end G-code of the printer file against what your machine needs. Stay next to the printer for the first layers.

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

All settings with defaults, limits and meaning are listed in [docs/settings.md](docs/settings.md). A settings file only needs the keys it changes. Several files are layered in the order given, so printer, material and process can live in separate files; a later file overrides an earlier one, and `--set` overrides them all:

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

The layers, from bottom to top: the defaults, the project's settings (of several project files in the order given), the `-c` files, the `--set` overrides. On a single object its settings from the project go over the plate's, and `--object` files over those; so `--set infill_density=30` does not change an object to which the project gives an infill of its own. Of the settings of an object only those that an object can have here are taken (see [several models](#several-models-on-one-plate) below); an object with settings of its own is sliced as an object of its own, so the project file then counts as several objects that keep their places relative to each other, and `--object N:FILE` applies to all objects of the N-th file. Of values per filament or extruder the first is taken. Everything that is left out is reported as a warning with the setting names in `keys`: `project.settings_skipped` for the project (project files saved by Bambu Studio hold 440 to 500 settings, of which about 40 are taken), `project.object_settings_skipped` per object, `project.part_settings_skipped` for settings of parts and modifiers, which are not read; a 3MF without such settings gives `project.no_settings`. Height ranges of an object (height range modifiers) are read as `<range min_z="…" max_z="…">` elements inside the object in `Metadata/model_settings.config`, with their settings as `<metadata key value>` entries or `<option opt_key>` elements, and used like `--range` for that object, below the `--range` files; an object with ranges is sliced as an object of its own. Settings a range cannot have, typically its layer height, are reported as `project.range_settings_skipped`, ranges without valid heights as `project.range_invalid`. This layout is inferred: none of the project files examined so far had height ranges, so it has only been tested with a file built to match. Without the option the stored settings are ignored and the G-code stays the same. The server does the same with the form field `project_settings=true` (see [Server](#server)); the desktop app does not read them yet.

Thinner layers where the surface is shallow, thicker ones on steep walls (top and bottom shells then keep their thickness, `top_layers`/`bottom_layers` × `layer_height`):

```bash
cargo run --release -p slicer-cli -- slice model.stl -s adaptive_layers=true -s min_layer_height=0.1 -s max_layer_height=0.3
```

More print options, all off by default (see `docs/settings.md`):

- `min_layer_time = 8` slows extrusion on small layers so that each takes at least 8 seconds to cool, but not below `min_print_speed`.
- `pressure_advance = 0.04` writes `M900 K0.04` (Marlin) or `SET_PRESSURE_ADVANCE ADVANCE=0.04` (Klipper) at the start.
- `elephant_foot_compensation = 0.15` prints the first layer that much smaller all round.
- `spiral_vase = true` prints the outer wall alone above the solid bottom, as one line rising continuously, without seams.
- `top_surface_pattern = "monotonic"` prints the lines of top surfaces all in the same direction, each beside the one before, for an even sheen; `"concentric"` prints loops following the outline instead. `bottom_surface_pattern` does the same for the first layer. Other solid infill and bridges keep their lines.
- `ironing = true` passes over every top surface once more with 10 % of the material (`ironing_flow`), lines `ironing_spacing` apart at `ironing_speed`; with `ironing_type = top` only the topmost surface of every part, the top of its last layer.
- `avoid_crossing_walls = true` sends travel moves round holes and notches inside the part instead of over open air, which also saves their retraction.
- `fuzzy_skin = true` gives the outer wall a rough texture: its points are moved in and out at random by up to half of `fuzzy_skin_thickness`, one every `fuzzy_skin_point_distance`, the same on every run. The first layer stays smooth.
- `wipe_distance = 2` retracts while going back 2 mm along the line just printed.
- `arc_fitting = true` writes runs of short lines that lie on a circle within `arc_tolerance` (0.01 mm) as one `G2`/`G3` arc; the G-code of Benchy shrinks from 2.49 to 1.66 MB. Klipper needs `[gcode_arcs]` in `printer.cfg`.
- `fan_layer_time = 30` turns the fan up on layers that take less than 30 seconds, the shorter the more, up to `fan_max_speed` at `min_layer_time`.
- `overhang_speed = 15` prints the parts of walls where more than half the line hangs over the layer below at 15 mm/s and with `overhang_fan_speed`; they show as "Overhang wall" in the G-code and the preview. `overhang_speed_full = 8` prints the parts that hang free entirely (the middle of the line more than half a line width beyond the layer below) slower still.
- `label_objects = true` names every object in the G-code (`object_1`, `object_2`, …), so that the printer can leave one out mid-print: Klipper gets `EXCLUDE_OBJECT_DEFINE` with the outline of each object's first layer and `EXCLUDE_OBJECT_START`/`_END` around its lines on every layer (needs `[exclude_object]` in `printer.cfg`), Marlin `M486 T`, `M486 S<n>` and `M486 S-1`.
- `thumbnails = "48x48,300x300"` puts pictures of the plate into the G-code header, as base64 PNG in `; thumbnail begin 300x300 <length>` … `; thumbnail end` comment blocks, which Mainsail and Fluidd (through Moonraker), OctoPrint plugins and many printer screens show. They are drawn in software (front right, from above, framed to the models) and add about 0.05 s to slicing Benchy. A `.gcode.3mf` always gets `Metadata/plate_1.png` (512×512) and a 128×128 one, and projects saved in the app `Metadata/thumbnail.png`. Whether Moonraker shows pictures from a slicer it does not know by name has not been checked.
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

arranges the models side by side (`--gap` sets the distance, default 5 mm plus the brim). `--keep-positions` keeps the coordinates of the files, for example the build item positions of a 3MF. By default arranging fills rows of bounding rectangles. `arrange_outlines = true` packs the convex outlines seen from above instead (bottom-left fill with no-fit polygons, largest first, never worse than the rows), so round, slanted and triangular models stand closer: on a 256 mm bed 56 instead of 49 cylinders 30 mm across fit. `arrange_rotation = true` also turns models about Z in steps of 45° where that packs them closer, for example triangles in pairs into squares (21 instead of 16 right triangles with 60 mm legs); it takes up to about 150 ms for 50 models. The same settings apply to the server and to the Arrange button of the desktop app, which adds the turns to the models' rotation.

A model can have settings of its own that override the plate's process: wall loops, wall order and seam position (`seam_position`), top and bottom layers, sparse infill (density, pattern, angle, connection, overlap), supports on or off, the speeds of its lines (`outer_wall_speed`, `inner_wall_speed`, `infill_speed`, `bridge_speed`, `overhang_speed`, `overhang_speed_full`, `ironing_speed`), its fan (`fan_speed`, `bridge_fan_speed`, `overhang_fan_speed`, `fan_off_layers`, `fan_layer_time`, `fan_max_speed`), its `flow_ratio` and its retraction (`retraction_length`, `retraction_speed`, `z_hop`, `wipe_distance`). In the desktop app they are under "Object settings" below the selected model; models with own values are marked with ✱ in the object list. In Rust, `slicer_fdm::generate_plate_with_object_settings` takes them per object; own speeds, fan, flow and retraction travel to the G-code writer in `Toolpaths::object_settings` and only change how that model's lines are printed (models that differ only in these are laid out exactly as without): their feed rates, the fan while they print (`M106` only where the value changes; `fan_layer_time` goes by the time of the whole layer), the filament they get (counted so in the statistics), and the retraction, wipe and Z hop of the travel after one of its lines; the filament then goes forward again by as much as was pulled back, whatever the next line's model asks for. Layer heights, line width, skirt, brim, the kind and speed of supports (`support_speed`), the first layer speed, travel, the shortest travel that retracts and the flow of bridges and ironing stay the same for the whole plate, as does the slowdown for `min_layer_time` (which slows all lines of a layer alike); skirt, brim, supports and prime tower print with the plate's fan, flow and retraction, and models without own values print exactly as before. On the command line, `--object N:FILE` layers a settings file over the plate's settings for the N-th model file only (1 is the first); it may be given several times, also for the same model, and a key outside those settings is refused (`settings.not_per_object`):

```bash
printf 'infill_density = 40\nwall_loops = 4\n' > strong.toml
cargo run --release -p slicer-cli -- slice a.stl b.stl --object 2:strong.toml -o plate.gcode
```

Settings can also change with the height of a model ("height ranges"): `--range [N:]Z1-Z2:FILE` layers a settings file over the model's settings for the layers whose middle lies at least Z1 and less than Z2 millimetres above the bed, of the N-th model file only or, without `N:`, of every model. A range may set wall loops, wall order, seam position, top and bottom layers, sparse infill (density, pattern, angle, connection, overlap) and supports on or off (`slicer_fdm::RANGE_SETTINGS`); other keys, among them the layer height and the speeds, are refused (`settings.not_per_range`), as are malformed arguments (`cli.invalid_range`). Files for the same heights are layered in order; where ranges overlap, the later one counts. A range belongs to the model's region on every layer, so top and bottom surfaces are found across its borders as without it: a denser infill from 10 mm up does not make the layer below a top surface, and no infill from 10 mm up does not make the first layer above a bottom surface. Models without ranges, or with ranges that change nothing, print as before. In Rust, `slicer_fdm::generate_plate_with_own_settings` takes the ranges per object (`slicer_fdm::HeightRange`). The desktop app does not offer height ranges yet.

```bash
printf 'infill_density = 100\nwall_loops = 4\n' > solid.toml
cargo run --release -p slicer-cli -- slice a.stl b.stl --range 2:10-20:solid.toml -o plate.gcode
```

### Sending to a printer

```bash
cargo run --release -p slicer-cli -- slice model.stl --send moonraker://printer.local:7125 --start
```

uploads the G-code to Klipper (Moonraker) and starts it; `octoprint://host` targets OctoPrint, `moonraker+https://` and `octoprint+https://` use TLS. `send file.gcode --to PRINTER` uploads an existing file. The API key comes from `--api-key` or `SPAGHETTI_PRINTER_API_KEY`. Only tested against a stand-in, never against a real printer. Bambu Lab printers are not supported yet.

Errors go to stderr with exit code 1. With `--json`, errors and warnings are objects with a stable `code`, a `message` and a `context`:

| Code | Meaning |
|---|---|
| `model.unrecognized`, `model.truncated`, `model.invalid_vertex`, `model.incomplete_triangle`, `model.non_finite`, `model.no_triangles` | The STL file cannot be read |
| `model.archive`, `model.missing_part`, `model.xml`, `model.invalid_attribute`, `model.unsupported_unit`, `model.invalid_index`, `model.unknown_object`, `model.component_cycle`, `model.too_large` | The 3MF file cannot be read |
| `settings.parse`, `settings.unknown_key`, `settings.out_of_range` | A settings file, `--set` value or Orca profile is invalid |
| `slicing.empty_model`, `slicing.too_large`, `slicing.nothing_to_print` | The model cannot be sliced |
| `gcode.unknown_placeholder`, `gcode.unclosed_placeholder` | Custom start or end G-code is invalid |
| `io.read`, `io.write`, `cli.invalid_override`, `cli.invalid_object`, `cli.invalid_range` | A file cannot be accessed or an argument is malformed |
| `settings.not_per_object` | A settings file for one object (`--object`, `object_settings_<n>`) sets a key that only the whole plate can have |
| `settings.not_per_range`, `settings.too_many_ranges` | A settings file for a height range (`--range`, `range_settings_...`) sets a key that a range cannot have, or an object has more than 256 ranges |
| `model.unknown_plate` | The build plate chosen with `--plate` does not exist in the file |
| `model.multiple_plates` (warning) | The project file has several build plates; only the first is sliced unless one is chosen |
| `model.unsupported_extension` (warning) | The 3MF file requires extensions that are not supported; they are ignored |
| `model.skipped_objects` (warning) | The 3MF file has objects that are not of type `model`; they are not printed |
| `project.settings_skipped`, `project.object_settings_skipped`, `project.part_settings_skipped`, `project.no_settings` (warnings) | With `--project-settings`: settings stored in the project file without a counterpart, not settable per object, on parts or modifiers, or none at all |
| `project.range_settings_skipped`, `project.range_invalid` (warnings) | With `--project-settings`: settings of a height range of an object that a range cannot have here, or a range without valid heights |
| `mesh.repaired` (warning) | Small holes in the surface of the mesh were closed before slicing |
| `mesh.reoriented` (warning) | Triangles that faced the other way than their neighbours were turned around before slicing |
| `mesh.non_manifold` (warning) | Edges of the mesh belong to more than two triangles |
| `mesh.inside_out` (warning) | All triangles face inward; the model is sliced as if they faced outward |
| `mesh.not_watertight` (warning) | The mesh has open or inconsistently oriented edges |
| `slicing.open_contours` (warning) | Some layers have contours that do not close and were dropped |
| `slicing.unsupported_overhangs` (warning) | Some layers reach more than one line width beyond the layer below (overhangs or bridges) and supports are switched off |
| `mesh.degenerate_removed` (warning) | Triangles without area were removed before slicing; their neighbours were joined so the surface stays closed |
| `slicing.multiple_extruders` (warning) | Objects of the model ask for different extruders while `extruder_count` is 1; everything is printed with the lowest one |
| `slicing.missing_extruder` (warning) | Objects ask for an extruder beyond `extruder_count`; they are printed with extruder 1 |
| `slicing.prime_tower_overlap` (warning) | The prime tower overlaps the model; move it with `prime_tower_x`/`prime_tower_y` |
| `slicing.outside_bed` | An object placed as given (`--keep-positions`, the desktop app) reaches over the edge of the bed, or arranged objects do not fit |
| `slicing.objects_overlap` (warning) | The outlines of two objects overlap seen from above; they are printed as one |
| `calibration.invalid_range` | Start, end and step of a calibration print are invalid, out of range, or give more than 20 values |
| `send.invalid_target`, `send.connection`, `send.unauthorized`, `send.too_large`, `send.rejected` | A printer address is malformed, the printer cannot be reached, refused the API key, refused the file as too large, or refused the upload otherwise |
| `send.not_started` (warning) | The file was uploaded but the printer did not start it |
| `slicing.thin_regions` (warning) | Some layers have parts narrower than half a line width that are not printed (parts between half and one line width get a single line of their own width) |

## Desktop app (preview)

```bash
cargo run --release -p slicer-ui -- [model.stl] [settings.toml ...]
```

Opens a window laid out like the desktop slicers users know (Orca Slicer, Bambu Studio), drawn with its own icons: a top bar with the file menu, the Prepare and Preview tabs and the slice and export buttons; on the left printer, filament, objects and the process settings in tabs (Quality, Strength, Speed, Support, Others) with units; the plate in 3D with a toolbar (add, arrange, undo, redo, duplicate, lay on face, auto orient, mirror, cut, centre, turn, delete). The selected model has a ring to turn it and a corner box to scale it by dragging. Cut splits a model at a height into two closed parts (both or one kept, the upper one beside the lower, optionally upside down), with the cutting plane shown on the plate. On the first start a setup assistant asks which printers (with which nozzles) and filaments you have; 66 printer models of 12 makers and 11 filament types are built in, and the File menu opens it again. The chosen profiles and the settings in use are kept for the next start (in the browser in local storage). Below the object list are position, rotation, scale, size, mirror along X, Y or Z and auto orient (least overhang, most contact with the bed); lay on face turns the model onto the face clicked next. Click a model to select it and drag it across the bed; drag elsewhere to turn the view, with the right button or Shift to pan, and zoom with the wheel towards the pointer. The preview draws every extrusion as a solid strand of its width and layer height on the GPU, with an upright layer slider on the right, a slider along the bottom for the moves within the layer, and a legend of the line types with share and length that can hide each type. At the bottom left the G-code of the shown layer is listed, with the line near the last shown move marked. The preview can also colour the strands by speed, layer height or line width, and the legend shows the estimated time of every line type. Undo and redo (Ctrl+Z, Ctrl+Y), copy, paste and duplicate (Ctrl+C, Ctrl+V, Ctrl+D), Delete, Escape, the arrow keys (1 mm, with Shift 10 mm) and A to arrange work on the plate; a right click on a model opens a menu with the same actions. The button beside the printer and the filament opens their settings, which can be saved as a new profile. A session can hold several plates: the numbered buttons above the 3D view show a plate, + adds one and a right click removes one; each plate has its own models and its own slice, so slicing, arranging and exporting work on the plate shown (the file name gets the plate number), and the context menu of a model moves it to another plate or a new one. Undo and redo cover all plates. Messages appear at the bottom right. Building it on Linux needs no extra system packages.

File → Save project… (Ctrl+S) writes the session as a 3MF file: every model as it is on the plate (also after cutting, mirroring or laying it on a face), placed by its build item, so other programs that read 3MF open the models where they lie. The settings files in use (with their text), the values of the side panel, the list of all settings and the extra lines are kept in the part `Metadata/spaghetti-project.toml`; its layout is described in `crates/slicer-ui/src/project.rs`. File → Open project… (Ctrl+O) replaces the plate and the settings with those of the file. A dropped or opened `.3mf` with that part opens as a project, any other 3MF as a model. In the browser saving downloads the file.

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

The field `project_settings=true` uses the print settings stored in project files of Orca Slicer and Bambu Studio, as `--project-settings` does on the command line: the project's settings below the `settings` and `set` fields, the settings the project gives single objects over the plate's and below `object_settings_<n>`, the height ranges of objects below `range_settings_...`, and a project whose objects have settings or height ranges of their own is sliced as those objects. What is left out appears in the job's `warnings` (`project.settings_skipped`, `project.object_settings_skipped`, `project.part_settings_skipped`, `project.range_settings_skipped`, `project.range_invalid`, `project.no_settings`); with several models their messages start with `model <n>:`. Without the field the stored settings are ignored:

```bash
curl -F model=@project.3mf -F project_settings=true -F set=nozzle_temp=215 localhost:8080/v1/jobs
```

| Request | Purpose |
|---|---|
| `GET /health` | Liveness check, never needs a token |
| `GET /openapi.json` | Description of this API (OpenAPI 3.1), never needs a token |
| `POST /v1/jobs` | Create a job (multipart fields `model`, `settings`, `set`, `plate`, `project_settings`, `object_settings_<n>`, `range_settings_[<n>_]<z1>-<z2>`; several `settings` files are layered in order); answers `202` at once |
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

## Workspace

| Crate | Purpose |
|---|---|
| `slicer-core` | Geometry, meshes, layering, polygon operations. No file system, network or UI. |
| `slicer-fdm` | FDM toolpaths: perimeters, infill, supports, path planning |
| `slicer-gcode` | G-code generation with exchangeable flavors |
| `slicer-formats` | Model and project formats (STL, 3MF) |
| `slicer-profiles` | Printer, material and process profiles |
| `slicer-cli` | Command line interface (`spaghetti-slicer`) |
| `slicer-server` | HTTP API server (`spaghetti-slicer-server`) |

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
