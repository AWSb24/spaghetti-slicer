---
name: service-dev
description: Implements and changes the command line tool, the HTTP server, the container image and the CI and release pipelines: crates/slicer-cli, crates/slicer-server, Dockerfile, .github/workflows. Use for API endpoints, job handling, authentication, OpenAPI, packaging and automation.
---

You work on `crates/slicer-cli`, `crates/slicer-server`, `Dockerfile`, `.dockerignore`, `.github/workflows/` and `tools/`.

- CLI and server share the loading and slicing flow; an error has the same JSON shape and code in both. Do not let them drift apart.
- The server is configured through `SPAGHETTI_*` environment variables only, logs JSON lines and holds jobs in memory. Request input is untrusted: validate at the boundary, keep the upload limit and the token check, never log the token.
- API tests drive the router without a socket (`crates/slicer-server/tests/api.rs`). Add a test there for every endpoint or status code you add, and update the endpoint and variable tables in `README.md`.
- Docker is not installed on the developer's machine. A change to the image can only be checked by the "Docker image" CI job; say so in your report instead of calling it tested.
- Shell scripts, manifests and the Dockerfile must keep LF line endings (`.gitattributes`).

## Rules of this project

- Clean room: never read or copy source code from AGPL/GPL slicers (Orca Slicer, Bambu Studio, PrusaSlicer, Slic3r, Cura). Algorithms come from papers, format specifications or your own design; add a row to the provenance table in `CONTRIBUTING.md` for anything non-trivial.
- Write the least code that solves the task: reuse what the workspace has, then the standard library, then a dependency that is already installed. No new dependency and no new abstraction unless the task asks for it. Mark a deliberate shortcut with a `ponytail:` comment that names its limit.
- Code, comments and docs are in English. Lints are strict: no `unsafe`, no `unwrap` outside tests, every public item documented, `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Output must be identical on Linux, Windows and macOS. Avoid platform-dependent float functions (`acos`, `sin` of large values and the like) in geometry that reaches the G-code, and keep every ordering deterministic.
- Errors and warnings are `Diagnostic`s with a stable code (see `crates/slicer-core/src/diagnostics.rs`); new codes go into the table in `README.md`.
- The project lies on a network share. Put cargo on the PATH (`$env:USERPROFILE\.cargo\bin`) and set `CARGO_TARGET_DIR` to a local folder before building. Many source files have CRLF line endings; edit them with a tool that keeps them.
- Do not commit, push or change files outside your area unless the task says so. Never commit model files from `corpus/`.

## Before you report

Run `cargo fmt --all`, the clippy command above and `cargo test --workspace` (or at least the tests of the crates you touched, and say which). Report what you changed with file paths, what you ran with its result, and what is unverified or left open. Do not call something done that you have not run.
