# Resin print archive (`.sla.zip`)

`spaghetti-slicer slice-sla` writes a resin (MSLA) print as a plain ZIP
archive. The format is our own and openly documented here so that other
tools can read it or convert it into a printer's vendor format. It is
**not** read by any printer directly; vendor formats such as Anycubic's
`.pwmx` are out of scope for now (PRD question Q9).

```
spaghetti-slicer slice-sla model.stl -o model.sla.zip --set exposure_time=2.5
```

## Archive layout

| Entry | Compression | Content |
|---|---|---|
| `manifest.json` | deflate | Print parameters and per-layer data, see below |
| `layers/00001.png` … `layers/NNNNN.png` | stored | One image per layer, from the build plate up |

Layer files are numbered from 1 with five digits. All entries carry the
date 1980-01-01 00:00, so slicing the same model with the same settings and
the same program version gives a byte-identical archive.

## Layer images

- PNG (ISO/IEC 15948), 8-bit greyscale (colour type 0), no interlacing.
- Size is the screen resolution (`image.width` × `image.height`).
- 0 is dark, 255 is fully lit. Values in between are edge pixels that the
  model covers only in part (anti-aliasing by supersampling, see
  `settings.antialiasing`). A printer or converter that only knows on/off
  should light pixels from 128 up.
- Orientation: a **top view**. The first row is the edge of the screen with
  the largest Y, columns run in +X. Pixel (column `c`, row `r`) covers
  X from `c · pixel_size[0]` to `(c + 1) · pixel_size[0]` and Y from
  `display_height − (r + 1) · pixel_size[1]` to
  `display_height − r · pixel_size[1]`, measured from the bottom-left
  corner of the screen. The model is centred on the screen. Whether a
  given printer needs the image mirrored has not been checked on a device.

## `manifest.json`

```json
{
  "format": "spaghetti-sla",
  "format_version": 1,
  "generator": "spaghetti-slicer 0.1.0",
  "image": {
    "width": 3840,
    "height": 2400,
    "pixel_size": [0.05, 0.05],
    "encoding": "png-gray8",
    "orientation": "top-view"
  },
  "settings": { "resolution_x": 3840, "...": "all settings, see below" },
  "stats": {
    "layers": 200,
    "resin_volume_ml": 1.0,
    "print_time_s": 1628.0
  },
  "layers": [
    {
      "file": "layers/00001.png",
      "z": 0.05,
      "exposure_time": 40.0,
      "lit_pixels": 40000,
      "resin_volume_mm3": 5.0
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `format`, `format_version` | Always `spaghetti-sla` and `1` for this description. Readers should reject other versions. |
| `generator` | Program and version that wrote the archive. |
| `image.pixel_size` | Width and height of one pixel in mm. |
| `settings` | Every setting the print was sliced with (table below). Settings of a feature that is switched off are left out: `elevation` and `raft_thickness` when 0, `supports` and all `support_*` keys without supports, `hollow_wall` and `drain_hole_diameter` without hollowing. A reader should take the defaults for missing keys. |
| `stats.layers` | Number of layers, raft and elevation included. |
| `stats.resin_volume_ml` | Cured resin of model, raft and supports together, after hollowing; partly lit pixels count in proportion. |
| `stats.print_time_s` | Exposure of every layer plus lifting and lowering the plate after it. Acceleration, pauses and the light-off delay of the printer are not included. |
| `layers[].file` | Image of the layer inside the archive. |
| `layers[].z` | Top of the layer above the build plate in mm. |
| `layers[].exposure_time` | Seconds to expose this layer. |
| `layers[].lit_pixels` | Pixels with a value above 0. |
| `layers[].resin_volume_mm3` | Resin cured in this layer. |

Unknown fields must be ignored by readers; new fields may be added without
raising `format_version`.

## Settings

Set with `--set key=value`. Lengths in mm, times in seconds, speeds in mm/s.

| Key | Default | Allowed | Meaning |
|---|---|---|---|
| `resolution_x` | 3840 | 1–20000 | Screen width in pixels |
| `resolution_y` | 2400 | 1–20000 | Screen height in pixels |
| `display_width` | 192 | 1–1000 | Width of the lit screen area |
| `display_height` | 120 | 1–1000 | Height of the lit screen area |
| `max_print_height` | 245 | 1–1000 | Highest printable model |
| `layer_height` | 0.05 | 0.005–0.5 | Thickness of every layer |
| `exposure_time` | 2 | 0.01–600 | Exposure of a normal layer |
| `bottom_exposure_time` | 40 | 0.01–600 | Exposure of a bottom layer |
| `bottom_layers` | 6 | 0–1000 | Number of bottom layers |
| `lift_height` | 6 | 0–100 | Plate lift after each layer |
| `lift_speed` | 2 | 0.01–100 | Speed of the plate going up |
| `retract_speed` | 3 | 0.01–100 | Speed of the plate going down |
| `antialiasing` | 4 | 1–16 | Samples per pixel along each axis; 1 turns anti-aliasing off |
| `elevation` | 0 | 0–100 | Gap between the top of the raft (or the plate) and the lowest point of the model; needs supports |
| `raft_thickness` | 0 | 0–10 | Raft under model and supports; 0 prints none |
| `supports` | 0 | 0 or 1 | 1 prints supports from the raft up to overhangs |
| `support_overhang_angle` | 45 | 0–89 | Downward-facing surfaces tilted further than this (degrees) from the vertical get supports |
| `support_spacing` | 3 | 0.5–20 | Distance between support points on an overhang |
| `support_tip_diameter` | 0.4 | 0.05–5 | Diameter where a support touches the model |
| `support_tip_length` | 2 | 0.1–20 | Length of the cone from tip to pillar |
| `support_pillar_diameter` | 1.2 | 0.2–10 | Diameter of a pillar |
| `support_brace_height` | 10 | 0–1000 | Neighbouring pillars at least this tall get crossed braces; 0 none |
| `hollow_wall` | 0 | 0–20 | Wall thickness of a hollowed model; 0 prints it solid |
| `drain_hole_diameter` | 2 | 0–20 | Drain holes at the lowest points of each cavity; 0 none |

Screen and height defaults are those of the reference printer, the Anycubic
Photon Mono X: 8.9" monochrome LCD with 3840 × 2400 pixels, XY resolution
0.050 mm, build volume 192 × 120 × 245 mm (Anycubic product pages
store.anycubic.com/products/photon-mono-x-resin-printer and
anycubic.au/products/anycubic-photon-mono-x, read November 2026 through
search-engine excerpts; 3840 × 0.05 = 192 and 2400 × 0.05 = 120 agree).
Not to be confused with the later Photon Mono X 6K (5760 × 3600). The exposure and
lift defaults are typical starting values for standard resin and depend on
the resin; they have not been verified on the device.

## How the layers are made

1. The mesh is centred on the screen. Its lowest point is placed
   `raft_thickness + elevation` above the build plate.
2. Layer `i` (from 0) covers `i · layer_height` to `(i + 1) · layer_height`
   and is cut at its mid-height; the number of layers is the height of the
   model's top above the plate divided by the layer height, rounded down.
3. Hollowing (`hollow_wall` > 0): a point of a layer becomes cavity when a
   ball of radius `hollow_wall` around it lies inside the model, so walls,
   top and bottom keep that thickness. Per layer this is the layer shrunk
   by the wall, intersected with the layers up to `hollow_wall` above and
   below shrunk by `sqrt(hollow_wall² − dz²)` (at most 8 layers on each side
   are compared, evenly spread). Wherever a cavity region does not overlap
   the cavity of the layer below, it has a local lowest point: a vertical
   drain hole of `drain_hole_diameter` is cut from there straight down
   through the floor until it leaves the model. With `elevation` 0 such a
   hole faces the plate and is closed by it (warning `sla.drain_on_plate`);
   without holes the warning is `sla.sealed_cavity`.
4. Supports (`supports` = 1): support points are the points of a grid of
   `support_spacing` (anchored at the screen's corner) under every triangle
   facing down more than `support_overhang_angle` from the vertical, plus
   every vertex that no neighbour lies below (local minimum), at most one
   per grid cell. A point is dropped when it is less than one layer above
   the raft, when it is near a drain hole, or when the model lies below it
   (checked at its centre and at four points on the pillar's edge). Each
   remaining point gets a support drawn as discs in every layer: from the
   raft up a pillar of `support_pillar_diameter`, then over
   `support_tip_length` a cone narrowing to `support_tip_diameter`, which
   reaches one tip radius into the model. Every pillar and its nearest
   neighbour within twice the spacing get crossed braces rising at 45°
   (half the pillar diameter thick) when both pillars' straight parts are
   at least `support_brace_height` tall and the model is not in between.
   Circles are 16-sided polygons (32 for drain holes).
5. Raft (`raft_thickness` > 0): the convex hull of the pillar feet and the
   model's lowest layer, grown by half a pillar diameter, in every layer
   below `raft_thickness`.
6. Each layer's contours are filled with the non-zero winding rule (outer
   contours counter-clockwise, holes clockwise), sampled at
   `antialiasing × antialiasing` evenly spaced points per pixel; the pixel
   value is the covered fraction times 255, rounded.

Raft, elevation and supports add layers, so they add exposure and plate
moves to `stats.print_time_s`; raft, supports and hollowing change
`stats.resin_volume_ml`. Both are computed from the images as before.

Not done yet: auto-orientation; supports standing on the model (a point
above another part of the model gets none, nor do the ceilings of
cavities); small overhang islands between grid points that hold no local
minimum; drain holes at positions given by the user; a chamfered raft
edge; anti-aliasing by grey-level correction for a given resin; vendor
formats. None of it has been printed.
