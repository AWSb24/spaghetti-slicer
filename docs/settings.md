# Settings

Generated from the code by `cargo test -p slicer-profiles --test reference`; do not edit by hand.

Lengths are in mm, speeds in mm/s, temperatures in °C. A settings file is TOML and only needs the keys it changes. Limits marked with * depend on other settings; the values shown hold for the defaults.

| Key | Default | Allowed | Meaning |
|---|---|---|---|
| `technology` | fdm | `fdm` | The kind of printer these settings are for. Only filament printers exist so far; settings for anything else are rejected. |
| `bed_size_x` | 220 | 10 to 2000 | Printable bed width (X). |
| `bed_size_y` | 220 | 10 to 2000 | Printable bed depth (Y). |
| `max_print_height` | 250 | 1 to 2000 | Maximum print height (Z). |
| `nozzle_diameter` | 0.4 | 0.1 to 2 | Nozzle diameter. |
| `flavor` | marlin | `marlin`, `klipper` | G-code dialect. |
| `extrusion_mode` | relative | `relative`, `absolute` | Whether extrusion amounts in the G-code are relative or absolute. |
| `extruder_count` | 1 | 1 to 16 | Number of extruders (tools or filament slots) of the printer. With more than one, every object prints with the extruder it names (Bambu Studio projects) and the G-code changes tools with `T<n>`; with one, everything prints with the lowest extruder the objects name. |
| `filament_diameter` | 1.75 | 1 to 3.5 | Filament diameter. |
| `filament_density` | 1.24 | 0.1 to 20 | Filament density in g/cm³, used for the weight estimate. |
| `flow_ratio` | 1 | 0.5 to 2 | Multiplier on the extruded amount. |
| `nozzle_temp` | 210 | 0 to 500 | Nozzle temperature after the first layer. |
| `nozzle_temp_first_layer` | 215 | 0 to 500 | Nozzle temperature on the first layer. |
| `bed_temp` | 60 | 0 to 200 | Bed temperature after the first layer. |
| `bed_temp_first_layer` | 60 | 0 to 200 | Bed temperature on the first layer. |
| `fan_speed` | 100 | 0 to 100 | Part cooling fan speed in percent. |
| `fan_off_layers` | 1 | 0 or more | Number of initial layers printed with the fan off. |
| `bridge_fan_speed` | 100 | 0 to 100 | Part cooling fan speed in percent while printing bridges. |
| `min_layer_time` | 0 | 0 to 600 | Shortest time in seconds a layer may take, so that it can cool before the next one goes on top; smaller layers are printed slower. 0 turns it off. |
| `min_print_speed` | 10 | 1 to 1000 | Slowest speed in mm/s that `min_layer_time` slows extrusion down to. |
| `pressure_advance` | 0 | 0 to 2 | Pressure advance (linear advance) factor written at the start of the print: `M900 K` for Marlin, `SET_PRESSURE_ADVANCE` for Klipper. 0 leaves the printer's own value. |
| `layer_height` | 0.2 | 0.02 to 0.4 * | Layer height. |
| `first_layer_height` | 0.2 | 0.02 to 0.4 * | Height of the first layer. |
| `adaptive_layers` | false | `true`, `false` | Whether layers after the first one get thinner where the surface of the model is shallow and thicker where it is steep, between `min_layer_height` and `max_layer_height`. `layer_height` then only sets the thickness of the top and bottom shells (`top_layers` or `bottom_layers` times `layer_height`) and the steepest overhang that needs no support. |
| `min_layer_height` | 0.1 | 0.02 to 0.3 * | Thinnest layer with adaptive layers, used where the surface is nearly flat. On a sloped surface layers are only so thick that the steps they leave are no higher than on a flat one printed at this height. |
| `max_layer_height` | 0.3 | 0.02 to 0.4 * | Thickest layer with adaptive layers, used where the walls are steep. |
| `line_width` | 0.45 | 0.24 to 1 * | Width of one extruded line. |
| `wall_loops` | 2 | 1 to 50 | Number of wall loops. |
| `wall_order` | inner_outer | `inner_outer`, `outer_inner` | Order in which the walls of a layer are printed. |
| `seam_position` | rear | `rear`, `corner`, `nearest` | Where the walls of a layer start and end. |
| `top_layers` | 5 | 0 to 1000 | Number of solid layers at the top. |
| `bottom_layers` | 4 | 0 to 1000 | Number of solid layers at the bottom. |
| `spiral_vase` | false | `true`, `false` | Vase mode: above the solid bottom layers, the outer wall alone is printed as one line rising continuously, without seams, infill or top layers. One wall loop, no supports. |
| `infill_density` | 15 | 0 to 100 | Sparse infill density in percent. |
| `infill_pattern` | grid | `rectilinear`, `grid`, `triangles`, `gyroid` | Sparse infill pattern. |
| `infill_angle` | 45 | 0 to 180 | Direction of the infill lines in degrees from the X axis; every other layer is turned by 90 degrees. Triangles and the gyroid keep it on every layer. |
| `infill_connect` | true | `true`, `false` | Whether the lines of rectilinear sparse infill are joined into zig-zags along the walls. Saves travel moves and retractions but uses more material (on small areas up to a third more infill). Along solid infill and bridges the lines stay apart. |
| `infill_overlap` | 15 | 0 to 50 | How far infill reaches into the innermost wall, in percent of the line width. |
| `ironing` | false | `true`, `false` | Whether top surfaces are ironed: the hot nozzle passes over them once more with very little material to smooth them. |
| `ironing_flow` | 10 | 0 to 100 | Material extruded while ironing, in percent of a normal line. |
| `ironing_spacing` | 0.1 | 0.01 to 2 | Distance between the lines of ironing. |
| `skirt_loops` | 2 | 0 to 20 | Number of skirt loops around the first layer. |
| `skirt_distance` | 4 | 0 to 50 | Distance between skirt and model. |
| `brim_width` | 0 | 0 to 50 | Width of the brim attached to the first layer; 0 disables it. |
| `elephant_foot_compensation` | 0 | 0 to 0.45 | How far the outline of the first layer is moved inwards, against the first layer squashing out wider than the model (elephant foot). 0 turns it off. |
| `supports` | false | `true`, `false` | Whether to print supports under overhangs and bridges. |
| `support_style` | normal | `normal`, `tree` | Shape of the supports: straight columns (`normal`) or branches that merge into trunks (`tree`). |
| `support_spacing` | 2.5 | 0.5 to 20 | Distance between the lines of a support. Tree supports also touch the surface they carry at points this far apart. |
| `support_interface_layers` | 2 | 0 to 10 | Number of densely filled support layers right under the surface they carry. |
| `support_bridge_length` | 10 | 0 to 1000 | Bridges up to this length in millimetres get no supports; 0 supports every bridge. |
| `prime_tower_width` | 15 | 0 to 100 | Side of the square tower that every tool change primes the new extruder into, on every layer up to the last tool change; 0 disables it. Only used with more than one extruder. |
| `prime_tower_x` | 190 | 0 to 2000 * | X of the centre of the prime tower on the bed. |
| `prime_tower_y` | 190 | 0 to 2000 * | Y of the centre of the prime tower on the bed. |
| `outer_wall_speed` | 40 | 1 to 1000 | Speed of the outermost wall. |
| `inner_wall_speed` | 60 | 1 to 1000 | Speed of inner walls. |
| `infill_speed` | 80 | 1 to 1000 | Speed of solid and sparse infill. |
| `bridge_speed` | 30 | 1 to 1000 | Speed of bridges: solid infill with nothing below it. |
| `ironing_speed` | 20 | 1 to 1000 | Speed of ironing. |
| `bridge_flow` | 1 | 0.5 to 2 | Multiplier on the extruded amount of bridges, on top of `flow_ratio`. |
| `first_layer_speed` | 25 | 1 to 1000 | Speed of all extrusion on the first layer. |
| `travel_speed` | 150 | 1 to 2000 | Speed of moves without extrusion. |
| `acceleration` | 1000 | 10 to 100000 | Acceleration of the printer in mm/s². Only used to estimate the print time; it is not written to the G-code. |
| `jerk` | 8 | 0 to 1000 | Largest speed change in mm/s the printer makes at once, for example at a corner. Only used to estimate the print time. |
| `retraction_length` | 0.8 | 0 to 20 | Filament length pulled back before a travel move; 0 disables retraction. |
| `retraction_speed` | 35 | 1 to 200 | Speed of retraction and unretraction. |
| `retraction_min_travel` | 1.5 | 0 to 100 | Travel moves shorter than this do not retract. |
| `z_hop` | 0 | 0 to 10 | Nozzle lift during retracted travel moves; 0 disables it. |
| `start_gcode` | see `default-config` | text with `{key}` placeholders | G-code inserted before the print. `{setting_name}` placeholders are replaced. |
| `end_gcode` | see `default-config` | text with `{key}` placeholders | G-code inserted after the print. `{setting_name}` placeholders are replaced. |
| `toolchange_gcode` | see `default-config` | text with `{key}` placeholders | G-code inserted at every tool change, after the old extruder has retracted and before `T<n>` selects the new one. Besides `{setting_name}`, `{previous_extruder}` and `{next_extruder}` are replaced, counted from 0 as in `T<n>`. |
