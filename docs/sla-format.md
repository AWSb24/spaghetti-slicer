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
| `settings` | Every setting the print was sliced with (table below). |
| `stats.layers` | Number of layers. |
| `stats.resin_volume_ml` | Cured resin; partly lit pixels count in proportion. |
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

1. The mesh is centred on the screen and placed on the build plate.
2. Layer `i` (from 0) covers `i · layer_height` to `(i + 1) · layer_height`
   and is cut at its mid-height; the number of layers is the model height
   divided by the layer height, rounded down.
3. Each layer's contours are filled with the non-zero winding rule (outer
   contours counter-clockwise, holes clockwise), sampled at
   `antialiasing × antialiasing` evenly spaced points per pixel; the pixel
   value is the covered fraction times 255, rounded.

Not done yet: supports, hollowing and drain holes, anti-aliasing by
grey-level correction for a given resin, vendor formats.
