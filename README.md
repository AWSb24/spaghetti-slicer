# Spaghetti Slicer

A 3D printing slicer written entirely in Rust, for desktop and server use.

**Status: early prototype. The G-code has never been run on a printer. Do not print it unattended.**

What works: STL and 3MF import (geometry only), slicing, walls, solid top/bottom layers, sparse infill (rectilinear, grid), skirt, brim, retraction, G-code for Marlin and Klipper, per-layer SVG preview, an HTTP server with a job API.

What does not exist yet: supports, 3MF projects with settings or colours, Bambu Lab output, seam control, bridges, a Docker image, the UI.

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

Errors go to stderr with exit code 1. With `--json`, errors and warnings are objects with a stable `code`, a `message` and a `context`:

| Code | Meaning |
|---|---|
| `model.unrecognized`, `model.truncated`, `model.invalid_vertex`, `model.incomplete_triangle`, `model.non_finite`, `model.no_triangles` | The STL file cannot be read |
| `model.archive`, `model.missing_part`, `model.xml`, `model.invalid_attribute`, `model.unsupported_unit`, `model.invalid_index`, `model.unknown_object`, `model.component_cycle`, `model.too_large` | The 3MF file cannot be read |
| `settings.parse`, `settings.unknown_key`, `settings.out_of_range` | A settings file or `--set` value is invalid |
| `slicing.empty_model`, `slicing.too_large`, `slicing.nothing_to_print` | The model cannot be sliced |
| `gcode.unknown_placeholder`, `gcode.unclosed_placeholder` | Custom start or end G-code is invalid |
| `io.read`, `io.write`, `cli.invalid_override` | A file cannot be accessed or an argument is malformed |
| `model.unsupported_extension` (warning) | The 3MF file requires extensions that are not supported; they are ignored |
| `model.skipped_objects` (warning) | The 3MF file has objects that are not of type `model`; they are not printed |
| `mesh.not_watertight` (warning) | The mesh has open or inconsistently oriented edges |
| `slicing.open_contours` (warning) | Some layers have contours that do not close and were dropped |
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
| `POST /v1/jobs` | Create a job (multipart fields `model`, `settings`, `set`); answers `202` at once |
| `GET /v1/jobs/{id}` | Status (`queued`, `running`, `done`, `failed`, `cancelled`), statistics, warnings or the error |
| `GET /v1/jobs/{id}/gcode` | The G-code of a finished job |
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

Without a token anyone who can reach the port can submit jobs. Logs are JSON lines on standard output.

## Reference models

`examples/` holds six generated models that print without supports: a cube, a cylinder, a dome, a tube, a stepped block and a gear with a hub. CI slices all of them on Linux, Windows and macOS and fails if the G-code differs between platforms. Regenerate them with:

```bash
cargo run -p slicer-formats --example reference_models -- examples
```

`examples/golden.tsv` holds size, line count and a checksum of the G-code of each reference model, and the tests fail if the output changes. After an intended change, rewrite the file and commit it with the change:

```bash
UPDATE_GOLDEN=1 cargo test -p slicer-cli --test corpus golden
```

## Real-world models

3DBenchy (CC0) and its parts are used as real-world test models. They are not stored in the repository; see [corpus/README.md](corpus/README.md) for sources, license and how to fetch them.

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
