# Settings

Generated from the code by `cargo test -p slicer-profiles --test reference`; do not edit by hand.

Numbers are in the unit of the Unit column (empty for counts and factors). A settings file is TOML and only needs the keys it changes. Limits marked with * depend on other settings; the values shown hold for the defaults.

| Key | Default | Unit | Allowed | Meaning |
|---|---|---|---|---|
| `technology` | fdm |  | `fdm` | The kind of printer these settings are for. Only filament printers exist so far; settings for anything else are rejected. |
| `bed_size_x` | 220 | mm | 10 to 2000 | Printable bed width (X). |
| `bed_size_y` | 220 | mm | 10 to 2000 | Printable bed depth (Y). |
| `max_print_height` | 250 | mm | 1 to 2000 | Maximum print height (Z). |
| `nozzle_diameter` | 0.4 | mm | 0.1 to 2 | Nozzle diameter. |
| `flavor` | marlin |  | `marlin`, `klipper` | G-code dialect. |
| `extrusion_mode` | relative |  | `relative`, `absolute` | Whether extrusion amounts in the G-code are relative or absolute. |
| `arc_fitting` | false |  | `true`, `false` | Whether runs of short extrusion moves that lie on a circle are written as one arc (`G2` clockwise, `G3` counter-clockwise) instead of many straight `G1` lines: smaller files and smoother curves. Klipper needs `[gcode_arcs]` in `printer.cfg` for it. |
| `arc_tolerance` | 0.01 | mm | 0.001 to 0.5 | With `arc_fitting`: how far any point of the original lines (and the middle of every line) may lie from the arc that replaces them. |
| `label_objects` | false |  | `true`, `false` | Whether the G-code names the objects on the plate, so that the printer can leave one out while printing (Klipper's `EXCLUDE_OBJECT_DEFINE`/`_START`/`_END`, Marlin's `M486`). The objects are called `object_1`, `object_2` and so on, in the order they were given. |
| `thumbnails` |  | px | sizes like `48x48,300x300`, empty for none | Pictures of the plate to put into the G-code header, as sizes in pixels separated by commas, such as `48x48,300x300`; empty for none. They are PNG images in the comment blocks (`; thumbnail begin`) that Klipper front-ends (Mainsail, Fluidd), OctoPrint plugins and many printer screens show. |
| `extruder_count` | 1 |  | 1 to 16 | Number of extruders (tools or filament slots) of the printer. With more than one, every object prints with the extruder it names (Bambu Studio projects) and the G-code changes tools with `T<n>`; with one, everything prints with the lowest extruder the objects name. |
| `filament_diameter` | 1.75 | mm | 1 to 3.5 | Filament diameter. |
| `filament_density` | 1.24 | g/cm³ | 0.1 to 20 | Filament density in g/cm³, used for the weight estimate. |
| `flow_ratio` | 1 |  | 0.5 to 2 | Multiplier on the extruded amount. |
| `nozzle_temp` | 210 | °C | 0 to 500 | Nozzle temperature after the first layer. |
| `nozzle_temp_first_layer` | 215 | °C | 0 to 500 | Nozzle temperature on the first layer. |
| `bed_temp` | 60 | °C | 0 to 200 | Bed temperature after the first layer. |
| `bed_temp_first_layer` | 60 | °C | 0 to 200 | Bed temperature on the first layer. |
| `fan_speed` | 100 | % | 0 to 100 | Part cooling fan speed in percent. |
| `fan_off_layers` | 1 |  | 0 or more | Number of initial layers printed with the fan off. |
| `bridge_fan_speed` | 100 | % | 0 to 100 | Part cooling fan speed in percent while printing bridges. |
| `fan_layer_time` | 0 | s | 0 to 600 | Layers that take less than this many seconds get more fan than `fan_speed`, the more the shorter they are, up to `fan_max_speed` at `min_layer_time` (or at no time at all without it). 0 turns it off. |
| `fan_max_speed` | 100 | % | 0 to 100 | Fan speed in percent for the shortest layers, see `fan_layer_time`. |
| `overhang_fan_speed` | 100 | % | 0 to 100 | Fan speed in percent while printing overhanging walls, see `overhang_speed`. |
| `support_interface_fan_speed` | -1 | % | -1 to 100 | Fan speed in percent while printing the dense support interface, for example 100 so that it bonds less to the model and comes off more easily; -1 keeps the fan of the layer. |
| `min_layer_time` | 0 | s | 0 to 600 | Shortest time in seconds a layer may take, so that it can cool before the next one goes on top; smaller layers are printed slower. 0 turns it off. |
| `min_print_speed` | 10 | mm/s | 1 to 1000 | Slowest speed in mm/s that `min_layer_time` slows extrusion down to. |
| `pressure_advance` | 0 |  | 0 to 2 | Pressure advance (linear advance) factor written at the start of the print: `M900 K` for Marlin, `SET_PRESSURE_ADVANCE` for Klipper. 0 leaves the printer's own value. |
| `filament_colours` |  |  | colours like `#FF8000,#202020`, one per extruder, empty for none | Colour of the filament of every extruder, as `#RRGGBB` separated by commas in the order of the extruders, such as `#FF8000,#202020`; empty for none. With more than one extruder (`extruder_count`) at most one colour per extruder; extruders without one keep their colour from the built-in palette. With one extruder any number is accepted (as projects of Bambu Studio bring them) and the first one is used. Thumbnails draw every object in the colour of its extruder, a `.gcode.3mf` names the colour of every filament, and `flush_from_colours` derives purge volumes from them. Changes nothing in the G-code itself. |
| `filament_type` |  |  | kinds like `PLA,PETG`, one per extruder, empty for none | Kind of the filament of every extruder, such as `PLA` or `PETG`, separated by commas in the order of the extruders; empty for none. Letters, digits, spaces and `+-_.` only; at most one per extruder as with `filament_colours`. Only written to the `.gcode.3mf` container (`PLA` where none is given). |
| `layer_height` | 0.2 | mm | 0.02 to 0.4 * | Layer height. |
| `first_layer_height` | 0.2 | mm | 0.02 to 0.4 * | Height of the first layer. |
| `adaptive_layers` | false |  | `true`, `false` | Whether layers after the first one get thinner where the surface of the model is shallow and thicker where it is steep, between `min_layer_height` and `max_layer_height`. `layer_height` then only sets the thickness of the top and bottom shells (`top_layers` or `bottom_layers` times `layer_height`) and the steepest overhang that needs no support. |
| `min_layer_height` | 0.1 | mm | 0.02 to 0.3 * | Thinnest layer with adaptive layers, used where the surface is nearly flat. On a sloped surface layers are only so thick that the steps they leave are no higher than on a flat one printed at this height. |
| `max_layer_height` | 0.3 | mm | 0.02 to 0.4 * | Thickest layer with adaptive layers, used where the walls are steep. |
| `adaptive_layer_quality` | 100 | % | 0 to 100 | How closely adaptive layers follow a sloped surface, in percent: 100 keeps the steps on a slope no higher than on a flat surface printed at `min_layer_height`; 0 allows steps as high as `max_layer_height`, so that layers only get thinner to end on flat surfaces. In between, the allowed step height lies in proportion between the two. |
| `layer_height_max_change` | 0 | % | 0 to 100 | Largest change of the layer height from one layer to the next with adaptive layers or layer heights of height ranges, in percent of the thinner of the two: at 20 a layer is at most 1.2 times as thick or as thin as the one below, so that a change from thick to thin layers is spread over several layers, which get thinner before thin ones are needed, never thicker than asked for. 0 sets no limit. The first layer keeps its own height and is not included. |
| `line_width` | 0.45 | mm | 0.24 to 1 * | Width of one extruded line. |
| `wall_loops` | 2 |  | 1 to 50 | Number of wall loops. |
| `wall_order` | inner_outer |  | `inner_outer`, `outer_inner` | Order in which the walls of a layer are printed. |
| `seam_position` | rear |  | `rear`, `corner`, `nearest` | Where the walls of a layer start and end. |
| `avoid_crossing_walls` | false |  | `true`, `false` | Whether travel moves go round holes and notches inside the part instead of crossing them over open air, which also spares their retraction. |
| `fuzzy_skin` | false |  | `true`, `false` | Whether the outer wall gets a rough, textured surface: its points are moved in and out at random. |
| `fuzzy_skin_thickness` | 0.3 | mm | 0 to 2 | How far fuzzy skin moves the points of the outer wall, in and out together. |
| `fuzzy_skin_point_distance` | 0.8 | mm | 0.1 to 10 | Distance between the moved points of fuzzy skin. |
| `top_layers` | 5 |  | 0 to 1000 | Number of solid layers at the top. |
| `bottom_layers` | 4 |  | 0 to 1000 | Number of solid layers at the bottom. |
| `top_surface_pattern` | rectilinear |  | `rectilinear`, `monotonic`, `concentric` | Pattern of the solid infill of top surfaces: the parts of a layer with nothing above them. Other solid infill and bridges keep rectilinear lines. |
| `bottom_surface_pattern` | rectilinear |  | `rectilinear`, `monotonic`, `concentric` | Pattern of the solid infill of the first layer, the bottom surface on the bed. |
| `spiral_vase` | false |  | `true`, `false` | Vase mode: above the solid bottom layers, the outer wall alone is printed as one line rising continuously, without seams, infill or top layers. One wall loop, no supports. |
| `infill_density` | 15 | % | 0 to 100 | Sparse infill density in percent. |
| `infill_pattern` | grid |  | `rectilinear`, `grid`, `triangles`, `gyroid`, `honeycomb`, `cubic`, `concentric`, `lightning` | Sparse infill pattern. |
| `infill_angle` | 45 | ° | 0 to 180 | Direction of the infill lines in degrees from the X axis; every other layer is turned by 90 degrees. Triangles, the gyroid, honeycomb and cubic keep it on every layer; concentric loops and lightning have no direction. |
| `infill_connect` | true |  | `true`, `false` | Whether the lines of rectilinear sparse infill are joined into zig-zags along the walls. Saves travel moves and retractions but uses more material (on small areas up to a third more infill). Along solid infill and bridges the lines stay apart. |
| `infill_overlap` | 15 | % | 0 to 50 | How far infill reaches into the innermost wall, in percent of the line width. |
| `ironing` | false |  | `true`, `false` | Whether top surfaces are ironed: the hot nozzle passes over them once more with very little material to smooth them. |
| `ironing_type` | all |  | `top`, `all` | Which top surfaces are ironed: only the topmost one of every part (`top`) or every one (`all`). |
| `ironing_flow` | 10 | % | 0 to 100 | Material extruded while ironing, in percent of a normal line. |
| `ironing_spacing` | 0.1 | mm | 0.01 to 2 | Distance between the lines of ironing. |
| `skirt_loops` | 2 |  | 0 to 20 | Number of skirt loops around the first layer. |
| `skirt_distance` | 4 | mm | 0 to 50 | Distance between skirt and model. |
| `brim_width` | 0 | mm | 0 to 50 | Width of the brim attached to the first layer; 0 disables it. |
| `elephant_foot_compensation` | 0 | mm | 0 to 0.45 | How far the outline of the first layer is moved inwards, against the first layer squashing out wider than the model (elephant foot). 0 turns it off. |
| `supports` | false |  | `true`, `false` | Whether to print supports under overhangs and bridges. |
| `support_style` | normal |  | `normal`, `tree` | Shape of the supports: straight columns (`normal`) or branches that merge into trunks (`tree`). |
| `support_spacing` | 2.5 | mm | 0.5 to 20 | Distance between the lines of a support. Tree supports also touch the surface they carry at points this far apart. |
| `support_interface_layers` | 2 |  | 0 to 10 | Number of densely filled support layers right under the surface they carry. |
| `support_bottom_interface_layers` | 0 |  | 0 to 10 | Number of densely filled support layers where a support stands on the model (bottom interface), so that it leaves a smoother surface there; 0 turns them off. |
| `support_interface_pattern` | lines |  | `lines`, `concentric`, `grid` | Pattern of the dense interface layers: `lines`, `concentric` or `grid`. |
| `support_interface_spacing` | 0 | mm | 0 to 20 | Distance between the lines (or loops) of the dense interface, at least one line width; 0 means two line widths. |
| `support_bridge_length` | 10 | mm | 0 to 1000 | Bridges up to this length in millimetres get no supports; 0 supports every bridge. |
| `support_top_z_distance` | 0 | mm | 0 to 10 | Gap between the top of a support and the surface it carries, in millimetres, rounded to whole layers, at least one layer; 0 means one layer, whatever its thickness. Where the layers differ in height (`adaptive_layers`, layer heights of height ranges) the layers right below the surface are counted until their heights add up to the distance, otherwise layers of `layer_height`. With `support_soluble` 0 means no gap and the distance may round to no layer at all. |
| `support_bottom_z_distance` | 0 | mm | 0 to 10 | Gap between the model a support stands on and the support, in millimetres, rounded to whole layers like `support_top_z_distance`, at least one layer; 0 means one layer, whatever its thickness. With `support_soluble` 0 means no gap and the distance may round to no layer at all. |
| `support_xy_distance` | 0 | mm | 0 to 10 | Distance between a support and the sides of the model, in millimetres; 0 means one line width, or no gap with `support_soluble`. |
| `support_soluble` | false |  | `true`, `false` | Whether the supports are printed in a material that dissolves (or otherwise comes off without a gap), so that they may touch the model: 0 in `support_top_z_distance`, `support_bottom_z_distance` and `support_xy_distance` then means no gap at all, and the dense interface lies right under the surface it carries. The loop around a support that lies under a surface is printed as interface too. Usually goes with `support_extruder` or `support_interface_extruder`. |
| `support_extruder` | 0 |  | 0 to 16 | Extruder that prints the supports (counted from 1, like the extruders of objects); 0 keeps the extruder that prints skirt and brim, the lowest one the objects use. Only with more than one extruder (`extruder_count`); a number above it is reported and counts as 0. |
| `support_interface_extruder` | 0 |  | 0 to 16 | Extruder that prints the dense support interface (counted from 1); 0 takes the one of `support_extruder`. With an interface extruder of its own, the loop around a support that lies under a surface is printed as interface too, so that only the interface material touches the model there. |
| `prime_tower_width` | 15 | mm | 0 to 100 | Side of the square tower that every tool change primes the new extruder into, on every layer up to the last tool change; 0 disables it. Only used with more than one extruder. |
| `prime_tower_x` | 190 | mm | 0 to 2000 * | X of the centre of the prime tower on the bed. |
| `prime_tower_y` | 190 | mm | 0 to 2000 * | Y of the centre of the prime tower on the bed. |
| `flush_volumes` |  | mm³ | `extruder_count`² numbers of at least 0 in mm³, empty for none | How much a tool change purges into the prime tower, in mm³ of filament, for every pair of extruders: `extruder_count` × `extruder_count` numbers separated by commas, row by row, row i being the change from extruder i (counted from 0) and column j the change to extruder j, such as `0,50,200,0` for two extruders; a `;` may end a row (`0,50;200,0`). The diagonal is ignored. With one extruder any square table is accepted and not used. Empty: every tool change shares the solid tower layer with the other changes of its layer. |
| `flush_multiplier` | 1 |  | 0 to 10 | Factor on every value of `flush_volumes`, also on the volumes that `flush_from_colours` derives. |
| `flush_from_colours` | false |  | `true`, `false` | Whether, while `flush_volumes` is empty and there is more than one extruder, the purge volumes are derived from `filament_colours`, which then needs a colour for every extruder. A change from colour A to colour B purges 50 + 500 × max(0, L(B) − L(A)) + 100 × D mm³ (times `flush_multiplier`), where L is the lightness of a colour from 0 (black) to 1 (white), the square root of 0.2126 r² + 0.7152 g² + 0.0722 b² with r, g, b from 0 to 1, and D the distance of the two colours from 0 (same) to 1 (black and white), the square root of the mean of the squared differences of r, g and b. So black to white purges 650 mm³, white to black 150 and a change between equal colours 50: a dark colour shows through a light one. Own rule of thumb, not measured. |
| `sequential_print` | false |  | `true`, `false` | Whether every object on the plate is printed completely, all its layers, before the next one starts, lowest first; between objects the nozzle rises above everything printed. The objects must stand `extruder_clearance_radius` apart, and all but the last one printed must be no taller than `gantry_height`. Every object gets its own brim; the skirt goes round the first object only, and there is no prime tower. |
| `extruder_clearance_radius` | 35 | mm | 0 to 500 | With `sequential_print`: radius around the nozzle that the print head takes up below the height of the objects; objects must be at least this far apart. |
| `gantry_height` | 20 | mm | 0 to 2000 | With `sequential_print`: height of the lowest part of the gantry (the X axis) above the tip of the nozzle; every object but the last one printed must be lower. |
| `arrange_outlines` | false |  | `true`, `false` | Whether arranging packs the objects by their convex outlines seen from above instead of their bounding rectangles, so round, slanted and triangular objects stand closer together. Off, arranging fills rows of rectangles as before. |
| `arrange_rotation` | false |  | `true`, `false` | Whether arranging may also turn objects about Z in steps of 45° where that packs them closer; the slicer turns the models it arranges, the desktop app adds the turn to their rotation. Packs by outlines even without `arrange_outlines`. |
| `outer_wall_speed` | 40 | mm/s | 1 to 1000 | Speed of the outermost wall. |
| `inner_wall_speed` | 60 | mm/s | 1 to 1000 | Speed of inner walls. |
| `infill_speed` | 80 | mm/s | 1 to 1000 | Speed of solid and sparse infill. |
| `bridge_speed` | 30 | mm/s | 1 to 1000 | Speed of bridges: solid infill with nothing below it. |
| `overhang_speed` | 0 | mm/s | 0 to 1000 | Speed of walls where more than half the line hangs over the edge of the layer below; these parts of walls are printed as overhang walls with `overhang_fan_speed`. 0 turns it off: walls keep their speed. |
| `overhang_speed_full` | 0 | mm/s | 0 to 1000 | Speed of walls that hang free entirely, the middle of the line more than half a line width beyond the edge of the layer below; only with `overhang_speed`. 0 turns it off: they print at `overhang_speed`. |
| `ironing_speed` | 20 | mm/s | 1 to 1000 | Speed of ironing. |
| `support_speed` | 0 | mm/s | 0 to 1000 | Speed of supports; 0 prints them at `infill_speed`. |
| `support_interface_speed` | 0 | mm/s | 0 to 1000 | Speed of the dense support interface layers; 0 prints them at `support_speed`. |
| `bridge_flow` | 1 |  | 0.5 to 2 | Multiplier on the extruded amount of bridges, on top of `flow_ratio`. |
| `first_layer_speed` | 25 | mm/s | 1 to 1000 | Speed of all extrusion on the first layer. |
| `travel_speed` | 150 | mm/s | 1 to 2000 | Speed of moves without extrusion. |
| `acceleration` | 1000 | mm/s² | 10 to 100000 | Acceleration of the printer in mm/s². Only used to estimate the print time; it is not written to the G-code. |
| `jerk` | 8 | mm/s | 0 to 1000 | Largest speed change in mm/s the printer makes at once, for example at a corner. Only used to estimate the print time. |
| `retraction_length` | 0.8 | mm | 0 to 20 | Filament length pulled back before a travel move; 0 disables retraction. |
| `retraction_speed` | 35 | mm/s | 1 to 200 | Speed of retraction and unretraction. |
| `retraction_min_travel` | 1.5 | mm | 0 to 100 | Travel moves shorter than this do not retract. |
| `z_hop` | 0 | mm | 0 to 10 | Nozzle lift during retracted travel moves; 0 disables it. |
| `wipe_distance` | 0 | mm | 0 to 20 | Before a travel, the nozzle goes back this far along the line just printed while it retracts, wiping off what oozes; 0 turns it off. |
| `start_gcode` | see `default-config` |  | text with `{key}` placeholders | G-code inserted before the print. `{setting_name}` placeholders are replaced. |
| `end_gcode` | see `default-config` |  | text with `{key}` placeholders | G-code inserted after the print. `{setting_name}` placeholders are replaced. |
| `toolchange_gcode` | see `default-config` |  | text with `{key}` placeholders | G-code inserted at every tool change, after the old extruder has retracted and before `T<n>` selects the new one. Besides `{setting_name}`, `{previous_extruder}` and `{next_extruder}` are replaced, counted from 0 as in `T<n>`. |
