# Contributing

## Clean-room rule

Spaghetti Slicer is licensed under MIT or Apache-2.0. Most existing slicers are licensed under AGPL-3.0 or GPL. Code derived from them cannot be distributed under our license, so this project is written without reference to their source code.

**Do not read, copy, translate or paraphrase source code from these projects while working on Spaghetti Slicer:**

- Orca Slicer, Bambu Studio, PrusaSlicer, SuperSlicer, Slic3r (including `libslic3r`)
- Cura and CuraEngine
- Any other slicer or tool under a copyleft license (GPL, AGPL, LGPL)

This includes their bundled printer, filament and process profiles, their start and end G-code, and their test data.

If you have worked on one of these code bases, do not implement the same feature here from memory. Pick a different area or describe the behaviour for someone else to implement.

### What you may use

- Published papers, textbooks and articles on computational geometry and toolpath planning
- User-facing documentation of other slicers that describes behaviour, not implementation
- Firmware documentation for G-code commands (Marlin, Klipper, RepRap)
- Observing the output of other slicers as a user, for example comparing G-code or print results
- File format specifications (STL, 3MF)
- Your own experiments and measurements on a printer
- Code under MIT, Apache-2.0, BSD, ISC, Zlib, BSL-1.0 or a similarly permissive license, with attribution where required

### Record where algorithms come from

When you add a non-trivial algorithm, say where it comes from in the pull request description: a paper, a specification, a permissively licensed project, or "own design". Put a short note in the code if the source helps the reader.

### Printer-specific G-code and profiles

Start sequences, end sequences and profile values must come from firmware documentation, manufacturer documentation or your own tests on the device. State the source. Do not copy them from another slicer's profiles.

## Dependencies

New dependencies must be under a permissive license (MIT, Apache-2.0, BSD, ISC, Zlib, BSL-1.0, Unicode). No GPL, AGPL or LGPL dependencies. `slicer-core` must not gain dependencies on the file system, network or UI.

## Before you open a pull request

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

All three must pass. New behaviour needs a test.

## Provenance of the existing code

| Area | Source |
|---|---|
| Mesh slicing (triangle–plane intersection, chaining by shared edges) | Standard computational geometry, own design |
| Polygon booleans and offsetting | `i_overlay` crate (MIT OR Apache-2.0); see `docs/decisions/0001-polygon-library.md` |
| Hatch infill (scanline with even-odd pairing) | Standard scanline fill, own design |
| Top/bottom detection (intersection with neighbouring layers) | Own design |
| Extrusion amount (line width × layer height per millimetre) | Volume conservation, own design |
| STL parsing | Public STL format description |
| 3MF parsing | 3MF Core Specification and Production Extension (3MF Consortium, public); `zip` (MIT) and `quick-xml` (MIT) crates |
| Sliced project file for Bambu Lab printers (`.gcode.3mf`) | Worked out from a file exported by Bambu Studio (names of the parts, checksum file, description files); no source code read. MD5 from RFC 1321 |
| Build plates of Bambu Studio projects | Worked out from the `Metadata/model_settings.config` part of project files written by the program; no source code read |
| HTTP server | `axum` and `tokio` (MIT), `uuid` (MIT OR Apache-2.0); API is an own design |
| G-code commands | Marlin and RepRap G-code documentation |
| `;TYPE:` and `;LAYER_CHANGE` comments | Naming convention read by common G-code viewers, used for interoperability |
| Default start and end G-code | Generic Marlin commands, not taken from any profile |

The initial implementation was written with an AI coding assistant, without consulting the source code of other slicers.

## License of contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed under MIT and Apache-2.0, without any additional terms or conditions.
