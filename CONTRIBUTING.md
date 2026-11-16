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
| Mesh repair: closing holes (ear clipping after Meisters' two-ears theorem, plane normal by Newell's method), removing triangles without area (edge collapse and edge flip) | Textbook methods (O'Rourke, *Computational Geometry in C*; Botsch et al., *Polygon Mesh Processing*), own design |
| Polygon booleans and offsetting | `i_overlay` crate (MIT OR Apache-2.0); see `docs/decisions/0001-polygon-library.md` |
| Hatch infill (scanline with even-odd pairing) | Standard scanline fill, own design |
| Triangle infill (three hatchings 60° apart, the middle one shifted by half a spacing so that all three meet in points) | Own design |
| Gyroid infill (cross-section of sin x cos y + sin y cos z + sin z cos x = 0, traced with marching squares, saddles decided by the cell centre; length per area measured numerically) | Gyroid surface after A. H. Schoen (NASA TN D-5541, 1970) in its common trigonometric approximation; marching squares is the standard contouring method; own design. Clipping of the curves by `i_overlay` |
| Thinning gyroid polylines (Ramer-Douglas-Peucker on open lines) and turning the pattern by the infill angle | Douglas and Peucker, "Algorithms for the reduction of the number of points required to represent a digitized line or its caricature", The Canadian Cartographer 10(2), 1973; own design |
| Connected rectilinear infill (each line end joined to the next line end along the boundary of the area, if that stretch is at most two spacings long and its line is not yet taken) | Own design |
| Top/bottom detection (intersection with neighbouring layers) | Own design |
| Adaptive layer height (cusp height bounded with the triangle normals, layers ending on flat surfaces) | Cusp height from Dolenc and Mäkelä, "Slicing procedures for layered manufacturing techniques", Computer-Aided Design 26(2), 1994; planning own design |
| Extrusion amount (line width × layer height per millimetre) | Volume conservation, own design |
| STL parsing | Public STL format description |
| 3MF parsing | 3MF Core Specification and Production Extension (3MF Consortium, public); `zip` (MIT) and `quick-xml` (MIT) crates |
| Sliced project file for Bambu Lab printers (`.gcode.3mf`) | Worked out from a file exported by Bambu Studio (names of the parts, checksum file, description files); no source code read. MD5 from RFC 1321 |
| Build plates and object extruders of Bambu Studio projects | Worked out from the `Metadata/model_settings.config` part of project files written by the program; no source code read |
| HTTP server | `axum` and `tokio` (MIT), `uuid` (MIT OR Apache-2.0); API is an own design |
| Sending G-code to Moonraker and OctoPrint (`slicer-send`) | Public API documentation: Moonraker file manager and authorization pages (moonraker.readthedocs.io/en/latest/external_api/file_manager/, …/authorization/), OctoPrint file and general API pages (docs.octoprint.org/en/master/api/files.html, …/general.html); multipart body after RFC 7578; HTTP client `ureq` (MIT OR Apache-2.0) with `rustls`/`ring` (Apache-2.0/ISC/MIT) and the Mozilla root certificates from `webpki-roots` (CDLA-Permissive-2.0); never tried against a real printer |
| G-code commands | Marlin and RepRap G-code documentation |
| `;TYPE:` and `;LAYER_CHANGE` comments | Naming convention read by common G-code viewers, used for interoperability |
| Tree supports (contact points on a grid, branches leaning a bounded step per layer towards their nearest neighbour, pushed out of the model, merged when they touch, radius from the carried area; circles from own sine and cosine) | Idea of supports that start at sampled points and join on the way down within a bounded angle from Vanek, Galicia and Benes, "Clever Support: Efficient Support Structure Generation for Digital Fabrication", Computer Graphics Forum 33(5), 2014; algorithm own design |
| Several extruders: a region per extruder (the higher extruder takes overlaps), ordering that starts each layer with the extruder that ended the last one, tool change with `T<n>` and custom G-code, square prime tower shared line by line among the extruders changed to | `T<n>` from the Marlin G-code documentation; everything else own design |
| Thin walls and gap fill | Centre line by shrinking a region and averaging its two sides; skip test by Steiner's formula for parallel sets; own design |
| Folded walls in regions one to two lines wide | Morphological opening (shrink, then grow back) of a wall's centre area finds where its loop's sides lie closer than a line; textbook mathematical morphology (Serra), own design |
| Desktop app | `eframe`/`egui` (MIT OR Apache-2.0), `rfd` (MIT); layout own design |
| Default start and end G-code | Generic Marlin commands, not taken from any profile |
| Resin layer images (scanline fill with non-zero winding, supersampling) | Standard scanline fill, own design |
| PNG writer for resin layers | PNG specification (ISO/IEC 15948); `flate2` and `crc32fast` crates (MIT OR Apache-2.0) |
| Resin print archive (`.sla.zip`) | Own design, described in `docs/sla-format.md` |
| Anycubic Photon Mono X screen and build volume | Manufacturer's product page |
| Resin supports, raft and hollowing (grid support points, cone tips, 45° braces, hollowing as erosion by a ball, drain holes at cavity minima) | Own design; erosion from textbook mathematical morphology |
| Import of Orca Slicer profiles | Key names and value formats read from profiles a user saved in Orca Slicer, as data files; no source code or shipped profile read |

The initial implementation was written with an AI coding assistant, without consulting the source code of other slicers.

## License of contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed under MIT and Apache-2.0, without any additional terms or conditions.
