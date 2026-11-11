# Spaghetti Slicer

A 3D printing slicer written entirely in Rust, for desktop and server use.

**Status: early prototype. The G-code has never been run on a printer. Do not print it unattended.**

What works: STL import, slicing, walls, solid top/bottom layers, sparse infill (rectilinear, grid), skirt, brim, retraction, G-code for Marlin and Klipper, per-layer SVG preview.

What does not exist yet: supports, 3MF, Bambu Lab output, seam control, bridges, the HTTP server, the UI.

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

Errors go to stderr with exit code 1; with `--json` they are machine-readable.

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
