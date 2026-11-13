# Settings

Generated from the code by `cargo test -p slicer-profiles --test reference`; do not edit by hand.

Lengths are in mm, speeds in mm/s, temperatures in °C. A settings file is TOML and only needs the keys it changes. Limits marked with * depend on other settings; the values shown hold for the defaults.

| Key | Default | Allowed | Meaning |
|---|---|---|---|
| `bed_size_x` | 220 | 10 to 2000 | Printable bed width (X). |
| `bed_size_y` | 220 | 10 to 2000 | Printable bed depth (Y). |
| `max_print_height` | 250 | 1 to 2000 | Maximum print height (Z). |
| `nozzle_diameter` | 0.4 | 0.1 to 2 | Nozzle diameter. |
| `flavor` | marlin | `marlin`, `klipper` | G-code dialect. |
| `extrusion_mode` | relative | `relative`, `absolute` | Whether extrusion amounts in the G-code are relative or absolute. |
| `filament_diameter` | 1.75 | 1 to 3.5 | Filament diameter. |
| `filament_density` | 1.24 | 0.1 to 20 | Filament density in g/cm³, used for the weight estimate. |
| `flow_ratio` | 1 | 0.5 to 2 | Multiplier on the extruded amount. |
| `nozzle_temp` | 210 | 0 to 500 | Nozzle temperature after the first layer. |
| `nozzle_temp_first_layer` | 215 | 0 to 500 | Nozzle temperature on the first layer. |
| `bed_temp` | 60 | 0 to 200 | Bed temperature after the first layer. |
| `bed_temp_first_layer` | 60 | 0 to 200 | Bed temperature on the first layer. |
| `fan_speed` | 100 | 0 to 100 | Part cooling fan speed in percent. |
| `fan_off_layers` | 1 | 0 or more | Number of initial layers printed with the fan off. |
| `layer_height` | 0.2 | 0.02 to 0.4 * | Layer height. |
| `first_layer_height` | 0.2 | 0.02 to 0.4 * | Height of the first layer. |
| `line_width` | 0.45 | 0.24 to 1 * | Width of one extruded line. |
| `wall_loops` | 2 | 1 to 50 | Number of wall loops. |
| `wall_order` | inner_outer | `inner_outer`, `outer_inner` | Order in which the walls of a layer are printed. |
| `seam_position` | rear | `rear`, `nearest` | Where the walls of a layer start and end. |
| `top_layers` | 5 | 0 to 1000 | Number of solid layers at the top. |
| `bottom_layers` | 4 | 0 to 1000 | Number of solid layers at the bottom. |
| `infill_density` | 15 | 0 to 100 | Sparse infill density in percent. |
| `infill_pattern` | grid | `rectilinear`, `grid` | Sparse infill pattern. |
| `infill_overlap` | 15 | 0 to 50 | How far infill reaches into the innermost wall, in percent of the line width. |
| `skirt_loops` | 2 | 0 to 20 | Number of skirt loops around the first layer. |
| `skirt_distance` | 4 | 0 to 50 | Distance between skirt and model. |
| `brim_width` | 0 | 0 to 50 | Width of the brim attached to the first layer; 0 disables it. |
| `supports` | false | `true`, `false` | Whether to print supports under overhangs and bridges. |
| `support_spacing` | 2.5 | 0.5 to 20 | Distance between the lines of a support. |
| `support_interface_layers` | 2 | 0 to 10 | Number of densely filled support layers right under the surface they carry. |
| `outer_wall_speed` | 40 | 1 to 1000 | Speed of the outermost wall. |
| `inner_wall_speed` | 60 | 1 to 1000 | Speed of inner walls. |
| `infill_speed` | 80 | 1 to 1000 | Speed of solid and sparse infill. |
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
