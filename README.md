# Spaghetti Slicer

A 3D printing slicer written entirely in Rust, for desktop and server use.

**Status: early prototype. The G-code has never been run on a printer. Do not print it unattended.**

What works: STL and 3MF import (geometry only, one plate of a Bambu Studio project), closing small holes in a mesh, slicing, walls, solid top/bottom layers, sparse infill (rectilinear, grid, triangles, gyroid), optional adaptive layer height, skirt, brim, retraction, G-code for Marlin and Klipper, layered settings files with a set of [profiles](profiles/README.md), time and material estimates, per-layer SVG preview, an HTTP server with a job API, a container image.

## Known limitations

- Supports are simple: straight columns of lines with a loop around them under everything that overhangs by more than half a line width per layer, except short bridges (`supports = true`, off by default), filled densely in the layers right under the surface they carry, with one empty layer between support and model above and below. No tree supports, and the gaps cannot be set. Bridges (solid areas with nothing below) are filled with lines that run from one side that carries them to the other, in the best of every 15° and along or across the bridge's longest edge; they have their own speed, flow and fan setting (`bridge_speed`, `bridge_flow`, `bridge_fan_speed`). With supports switched on, bridges up to `support_bridge_length` (10 mm by default) get none. The seam is lined up at the back of the model, put at the sharpest corner of every loop (`seam_position = corner`; corners are measured over two line widths of path, so rounded ones count too, and inward corners are preferred because the seam hides better there) or left where the nozzle happens to be; it cannot be placed by hand.
- The G-code for Marlin and Klipper is the same. For Bambu Lab printers there is a project file (`-o name.gcode.3mf`) and an X1C profile, but neither has ever been tried on a printer: the start sequence uses common commands only, and it is not known whether the printer accepts the file.
- Colours and settings stored in a 3MF file are ignored. The extruder that a Bambu Studio project names per object is read, but everything prints with one extruder (warning `slicing.multiple_extruders`).
- The time estimate has not been compared with a real print and ignores heating.
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

Only settings with a clear counterpart are taken; every other one is listed on stderr with the reason. A saved Orca profile usually holds only what differs from the Orca preset it builds on (`inherits`). Those inherited values are not in the file and are not imported, so combine the result with a printer profile from `profiles/`, for example `-c profiles/printers/bambu-x1c.toml -c from-orca.toml`. The bed temperature is taken from the setting for the high-temperature plate (`hot_plate_temp`); start and end G-code are not taken, since Orca's placeholders differ.

Thinner layers where the surface is shallow, thicker ones on steep walls (top and bottom shells then keep their thickness, `top_layers`/`bottom_layers` × `layer_height`):

```bash
cargo run --release -p slicer-cli -- slice model.stl -s adaptive_layers=true -s min_layer_height=0.1 -s max_layer_height=0.3
```

### Resin (MSLA) printers (experimental)

`slice-sla` writes one PNG per layer and a JSON manifest into a ZIP archive, described in [docs/sla-format.md](docs/sla-format.md). The defaults follow the screen of the Anycubic Photon Mono X; exposure and lift values are typical starting points, not tested with a resin. No printer reads this archive directly yet.

```bash
cargo run --release -p slicer-cli -- slice-sla model.stl -o model.sla.zip --set exposure_time=2.5
```

Errors go to stderr with exit code 1. With `--json`, errors and warnings are objects with a stable `code`, a `message` and a `context`:

| Code | Meaning |
|---|---|
| `model.unrecognized`, `model.truncated`, `model.invalid_vertex`, `model.incomplete_triangle`, `model.non_finite`, `model.no_triangles` | The STL file cannot be read |
| `model.archive`, `model.missing_part`, `model.xml`, `model.invalid_attribute`, `model.unsupported_unit`, `model.invalid_index`, `model.unknown_object`, `model.component_cycle`, `model.too_large` | The 3MF file cannot be read |
| `settings.parse`, `settings.unknown_key`, `settings.out_of_range` | A settings file, `--set` value or Orca profile is invalid |
| `slicing.empty_model`, `slicing.too_large`, `slicing.nothing_to_print` | The model cannot be sliced |
| `gcode.unknown_placeholder`, `gcode.unclosed_placeholder` | Custom start or end G-code is invalid |
| `io.read`, `io.write`, `cli.invalid_override` | A file cannot be accessed or an argument is malformed |
| `model.unknown_plate` | The build plate chosen with `--plate` does not exist in the file |
| `model.multiple_plates` (warning) | The project file has several build plates; only the first is sliced unless one is chosen |
| `model.unsupported_extension` (warning) | The 3MF file requires extensions that are not supported; they are ignored |
| `model.skipped_objects` (warning) | The 3MF file has objects that are not of type `model`; they are not printed |
| `mesh.repaired` (warning) | Small holes in the surface of the mesh were closed before slicing |
| `mesh.reoriented` (warning) | Triangles that faced the other way than their neighbours were turned around before slicing |
| `mesh.non_manifold` (warning) | Edges of the mesh belong to more than two triangles |
| `mesh.inside_out` (warning) | All triangles face inward; the model is sliced as if they faced outward |
| `mesh.not_watertight` (warning) | The mesh has open or inconsistently oriented edges |
| `slicing.open_contours` (warning) | Some layers have contours that do not close and were dropped |
| `slicing.unsupported_overhangs` (warning) | Some layers reach more than one line width beyond the layer below (overhangs or bridges) and supports are switched off |
| `mesh.degenerate_removed` (warning) | Triangles without area were removed before slicing; their neighbours were joined so the surface stays closed |
| `slicing.multiple_extruders` (warning) | Objects of the model ask for different extruders (Bambu Studio projects); everything is printed with the lowest one |
| `slicing.thin_regions` (warning) | Some layers have parts narrower than the line width that are not printed |

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

| Request | Purpose |
|---|---|
| `GET /health` | Liveness check, never needs a token |
| `GET /openapi.json` | Description of this API (OpenAPI 3.1), never needs a token |
| `POST /v1/jobs` | Create a job (multipart fields `model`, `settings`, `set`, `plate`; several `settings` files are layered in order); answers `202` at once |
| `GET /v1/jobs/{id}` | Status (`queued`, `running`, `done`, `failed`, `cancelled`), statistics, warnings or the error |
| `GET /v1/jobs/{id}/gcode` | The G-code of a finished job |
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

A tag `v*` runs `.github/workflows/release.yml`: it builds archives with both programs and the profiles for Linux, Windows and macOS, a container image for amd64 and arm64 (pushed to `ghcr.io`), and publishes a GitHub release with checksums and generated notes. Started by hand, the workflow only builds, as a rehearsal.

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
