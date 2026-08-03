# mcad — a 2D CAD written in Rust

*[日本語版 README](./README.md)*

A 2D CAD application built with Rust and egui. Current version: **v0.7.2**.

> **Note on language.** The project's design documents (`DESIGN.md`, `AGENTS.md`,
> `CHANGELOG.md`) are written in Japanese, and the application UI is being migrated
> to Japanese as well. This file is a translation of [`README.md`](./README.md)
> provided so that the project can be evaluated without reading Japanese.

## Background

This is a personal project started in July 2026, prompted by the release of
Claude Fable 5, to find out how far AI-assisted development can go in building a
2D CAD that is actually usable. The overall design, the architectural rules, the
drafting requirements, and the accept/reject decisions during review are the
author's; Claude Code is used for the implementation. See [`DESIGN.md`](./DESIGN.md)
for the design and [`AGENTS.md`](./AGENTS.md) for the development conventions
(both in Japanese).

> This is an independent personal project and is not affiliated with Anthropic.

## Features

- **Drawing**: points, line segments, circles, arcs (three-point: start / through / end), polylines, text (CJK supported), dimensions (linear and radial)
- **Editing**: selection (click, rubber band, additive), move, duplicate, rotate, mirror, offset, **trim, extend, fillet, split**, delete
- **Snapping**: endpoint, intersection, midpoint, center and grid candidates chosen by priority, with a distinct marker per kind
- **Layers**: color, visibility, lock, stacking order (front / back buttons), managed in a dedicated panel
- **Undo/redo**: built on the command pattern
- **File I/O**: save and load `.mcad` (JSON), import and export DXF
- **Viewport**: cursor-centered zoom, pan, and a grid that follows the zoom level

## Getting started

```bash
cargo run -p mcad-app
```

For everyday use, a release build is recommended: `cargo run --release -p mcad-app`.

## Key bindings

| Key / action | Function |
|---|---|
| `Ctrl+N` | New document |
| `Ctrl+O` | Open a `.mcad` file |
| `Ctrl+Shift+O` | Import a DXF file |
| `Ctrl+S` | Save |
| `Ctrl+Shift+S` | Save as |
| `Ctrl+E` | Export to DXF |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+D` | Duplicate the selection (two clicks: base point, then destination) |
| `S` | Selection tool |
| `M` | Move the selection (two clicks: base point, then destination) |
| `R` | Rotate the selection (three clicks: pivot, reference, then target angle) |
| `Shift+M` | Mirror the selection (pick two points defining the axis) |
| `O` | Offset the selected entity (single target; click a through point or type a distance) |
| `X` | Trim (two clicks: boundary, then target — the clicked side is removed) |
| `E` | Extend (two clicks: boundary, then target — the target grows to the boundary) |
| `F` | Fillet (enter a radius in the top panel, then click the first and second segments on the side you want to keep; `Esc` cancels) |
| `B` | Split (click a target; it is split at the clicked point) |
| `1` | Point tool |
| `L` / `C` / `A` / `P` | Line / circle / arc / polyline |
| `T` | Text (click the anchor, type the string and height in the top panel, `Enter` to commit, `Esc` to cancel) |
| `D` | Linear dimension (three clicks: two measured points, then the dimension line position; preview follows the cursor) |
| `Shift+D` | Radial dimension (two clicks: a circle or arc, then the leader direction) |
| `Enter` | Commit a polyline (two or more points, left open; clicking the start point closes and commits it) |
| `Del` / `Backspace` | Delete the selected entities |
| `Esc` | Cancel drawing or placement / discard a rubber-band drag / clear the selection |
| `F3` | Toggle snapping |
| `F8` | Toggle orthogonal mode (constrains to horizontal/vertical from the previous point; an available snap candidate takes precedence) |
| Wheel | Cursor-centered zoom |
| Middle-drag / `Space`+left-drag | Pan |
| Click | On a shape: add to the selection (clicking empty space does nothing) |
| `Shift`+click | On a shape: remove it from the selection; on empty space: clear the selection |

Creating a new document, opening a file, or closing the window while there are
unsaved changes brings up a modal asking whether to discard them.

## File formats

- **`.mcad`**: the native JSON format. As of v0.7.2 the schema is v3; v1 and v2 files still load (backward compatible)
- **New drawings** start with two layers, `"0"` and `"Text"`. `"Text"` can be deleted, but `"0"` is the document's default layer and cannot be. Renaming a layer is not yet available in the UI

## Notes on DXF

DXF is treated as an interchange format. After importing, always save in `.mcad`
format — the original DXF file is never overwritten. Be aware of the following.

- **Colors**: approximated to the 9-color ACI palette (RGB is rounded to the nearest color)
- **Layer locks**: lost on a round trip, since DXF has no such field
- **Layer stacking order**: lost on a round trip, since DXF has no z-index field
- **Line width**: not preserved on a round trip; falls back to the default
- **TEXT**: position, height and rotation are mapped. Strings containing CJK are written and restored as UTF-8 (the DXF header is R2007). TEXT with a non-standard justification — anything other than horizontal Left plus vertical Baseline — is skipped
- **DIMENSION**: mcad dimensions are not exported to DXF, as there is no corresponding primitive; the number of skipped entities is shown in the status bar. They are saved normally in `.mcad`. Importing DXF DIMENSION is planned for M9
- **CJK text in other applications**: CJK written by mcad may appear garbled in other CAD software (confirmed with LibreCAD). DXF cannot embed the font itself, so rendering depends on the fonts installed on the receiving side

## Architecture

The Cargo workspace holds four crates, and dependencies flow in one direction only.

```
mcad/
├── crates/
│   ├── mcad-geom/  → GUI-independent geometric primitives and math
│   ├── mcad-core/  → document model and undo/redo (depends on mcad-geom)
│   ├── mcad-io/    → file save/load and DXF conversion (depends on mcad-core)
│   └── mcad-app/   → the egui application (depends on mcad-io)
```

| Crate | Role |
|---|---|
| **mcad-geom** | Geometric primitives (`Point2`, `Vec2`, `LineSeg`, `Circle`, `Arc`, `Polyline`, `Shape`) and calculations (nearest point, distance, intersection). GUI-independent, pure functions only |
| **mcad-core** | The CAD document model (`Document` / `Entity` / `Layer`) and command-pattern undo/redo |
| **mcad-io** | Saving and loading `.mcad` (JSON) and DXF I/O via the `dxf` crate |
| **mcad-app** | The egui GUI: tools, viewport, snapping engine, layer panel and file operations |

Layers are ordered `mcad-app` → `mcad-io` → `mcad-core` → `mcad-geom`, and a higher
crate may depend directly on any lower one (for example `mcad-app` also uses
`mcad-core` and `mcad-geom`). Only the reverse direction is forbidden.

Key design decisions:

- **f64 world coordinates**: coordinate precision matters in CAD. f32 accumulates error on large drawings, so the conversion to f32 happens only at the egui drawing boundary
- **Command-pattern undo**: cheaper in memory than snapshots and viable for large drawings. Every change goes through `Document::apply(Command)`
- **Keeping geom GUI-independent**: geometry stays testable as pure functions, which also makes a future constraint solver easier to add
- **No spatial index**: a full scan with AABB culling is adequate up to a few thousand entities

## Development

```bash
cargo build                                  # build the whole workspace
cargo test --workspace                       # run all tests
cargo clippy --workspace --all-targets       # lint (kept at zero warnings)
cargo fmt --all --check                      # formatting check
```

CI (`.github/workflows/ci.yml`) runs the same three checks. All changes must pass
them before being committed.

GUI changes (dialogs, viewport, panels and so on) cannot be tested automatically
and require a manual smoke test in a real window.

## License

This repository is available under either of the following, at your option:

- MIT License ([`LICENSE-MIT`](./LICENSE-MIT))
- Apache License 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE))

This dual-licensing follows the convention of the Rust ecosystem; you only need to
satisfy the terms of one of them. Unless you state otherwise, any contribution you
submit to this repository is provided under the same dual license, per section 5 of
the Apache-2.0 license.

**The bundled font is not covered.** The fonts under
`crates/mcad-app/assets/fonts/` are third-party works licensed under the SIL Open
Font License 1.1 (see below).

## Font attribution

To render CJK glyphs in document text (Text entities), [Noto Sans JP](https://fonts.google.com/noto/specimen/Noto+Sans+JP)
Regular is embedded in the binary. It is registered as a fallback behind egui's
default fonts, so egui falls back per glyph and both Latin and CJK render correctly.

Noto Sans JP is provided under the SIL Open Font License 1.1. The full license text
is bundled at [`crates/mcad-app/assets/fonts/LICENSE-OFL.txt`](./crates/mcad-app/assets/fonts/LICENSE-OFL.txt).

> Noto Sans JP is licensed under the SIL Open Font License, Version 1.1.
> © 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'.
> "Noto" is a trademark of Google LLC.

## Documentation

All of the following are written in Japanese.

- [`DESIGN.md`](./DESIGN.md) — overall design, rationale for technology choices, task breakdown
- [`CHANGELOG.md`](./CHANGELOG.md) — per-version change history
- [`AGENTS.md`](./AGENTS.md) — development conventions (architectural invariants, coding rules)
