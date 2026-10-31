# mcad — a 2D CAD written in Rust

*[日本語版 README](./README.md)*

A 2D CAD application built with Rust and egui. Current version: **v0.13.0**.

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

- **Drawing**: points, line segments, circles, arcs (three-point: start / through / end), polylines, rectangles (two corners), regular polygons (center → vertex, inscribed/circumscribed toggle, 3–64 sides), construction lines (infinite in both directions, a drafting aid), text (CJK supported), **dimensions** (linear, radial, diameter, angular and ordinate; linear dimensions support aligned/horizontal/vertical/oblique direction; angular dimensions can be defined by three points or by two lines — including lines meeting at a corner cut by a fillet or chamfer; ordinate dimensions auto-detect X/Y; auxiliary symbols φ/Sφ/□/R/SR/CR/C/t; tolerance support; annotation value style (reference/theoretically-exact), prefix/suffix and text rotation (auto/horizontal/oblique); style editing; seven arrowhead styles; drag text position; chained/parallel continuous input)
- **Editing**: selection (click, rubber band, additive), move, duplicate, rotate, mirror, offset, **trim, extend (both can use a construction line as the boundary), fillet, chamfer, join, explode, split**, dimension grip editing (drag endpoints or the dimension line position; a right-panel button clears appearance overrides in bulk), shape grip editing (for the selected lines, circles and arcs, drag a line's endpoints, a circle's or arc's center and radius, or an arc's endpoints; constrained shapes that follow are previewed, the edit commits on release, and one undo reverts it), delete
- **Geometric constraints**: lines, circles and arcs can be given coincident, horizontal, vertical, distance, parallel, perpendicular, equal-length, equal-radius, radius, angle, tangent and fix constraints (`Q`), and shapes follow when edited. An edit that cannot be solved is rejected with a reason and leaves the drawing unchanged. The right panel's "拘束" (constraints) section lists constraints and lets you change values, delete them and read the degrees of freedom. Constraint symbols appear on screen only. Solving stops at a work budget, so edits can be rejected in drawings with very large connected constraints (rough targets: about 2 s on commit, about 16 ms for the preview while dragging)
- **Numeric input**: while a drawing tool that accepts a point is active (point, line, circle, arc, polyline, isometric circle, rectangle, polygon, construction line) or during the move/duplicate placement in selection mode, typing opens an input box next to the cursor for `x,y` (absolute), `@dx,dy` (relative to the base point), `L<A` (length `L` and angle `A` degrees from the base point; `@L<A` is the same), or `L` (length only, along the cursor direction from the base point, snapping to the polar-tracking direction while it is active) — equivalent to AutoCAD's dynamic input
- **Polar tracking** (`F10`): snaps the cursor direction from the base point to the nearest multiple of a step angle (15°/30°/45°/90°, default 15°, saved to `config.json`), showing a dotted guide line and the angle. Mutually exclusive with orthogonal mode (`F8`)
- **Isometric drawing aids**: isometric grid (`F5`), isometric axes for orthogonal mode, and an isometric circle tool (`I`, four-centre method). No Z axis or true ellipse; everything stays 2D arcs
- **Snapping**: endpoint, intersection, midpoint, center, quadrant, perpendicular, tangent, virtual intersection (between the extensions of lines/circles), nearest-point and grid candidates chosen by priority, with a distinct marker per kind. Each kind can be toggled independently (top-panel "Snap types" menu, saved to `config.json`; nearest-point defaults to OFF, the rest to ON). Split (`B`) has its own snapping limited to midpoints, intersections and interior vertices of the target entity, and shows the actual split point as a hover marker (trim/extend/fillet only use snapping to decide the pick target and show no hover marker)
- **Tables and parts lists**: a general table entity (`K`, with cells, column addition/removal, column widths, per-column text alignment (left/center/right) and row heights edited via "表を編集…" (edit table)) and a parts-list preset from the drafting standard (part no. (balloon) / item no. / name / quantity / material / mass [kg] / remarks, with those columns centered; the B-format title block omits remarks). A drawing holds at most one parts list; a per-drawing "use a parts list" setting (default ON) makes `K` place one automatically when none exists yet (right above the title block the instant `K` is pressed, if the drawing frame is enabled; otherwise at the click position) — with it OFF, or once a parts list already exists, `K` places a general table instead. A parts list is edited through two separate dialogs, "記入…" (entry: cell values and adding/removing part rows only) and "部品欄の設定…" (parts-list settings: columns, widths, alignment, row/text height, snapping to the title block, reapplying the preset); duplicating a selection that includes the parts list (`Ctrl+D`) is rejected. Entries are typed by hand; nothing is aggregated from the drawing automatically
- **Layers**: color, visibility, lock, stacking order (front / back buttons), managed in a dedicated panel. Construction lines go on a default "構築線" (construction line) layer; a "delete all construction lines" button at the bottom of the panel removes them in bulk regardless of layer visibility
- **Undo/redo**: built on the command pattern
- **Drawing frame and title block**: paper size, scale and title-block style (A/B/C) live in the sheet metadata and are drawn automatically; fields are filled in via a dedicated dialog
- **Line width and linetype**: per-layer or per-entity line width (ISO 128 series) and linetype (continuous / dashed / dash-dot / dash-dot-dot), resolved against paper mm and reflected on screen
- **File I/O**: save and load `.mcad` (JSON), import and export DXF, export to SVG/PDF (true scale)
- **Viewport**: cursor-centered zoom, pan, a grid that follows the zoom level, and manual zoom-to-fit

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
| `Ctrl+Shift+E` | Export to SVG |
| `Ctrl+P` | Export to PDF |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+D` | Duplicate the selection (two clicks: base point, then destination) |
| `S` | Selection tool |
| `M` | Move the selection (two clicks: base point, then destination) |
| `R` | Rotate the selection (three clicks: pivot, reference, then target angle) |
| `Shift+M` | Mirror the selection (pick two points defining the axis) |
| `O` | Offset the selected entity (single target; click a through point or type a distance). Offsetting a construction line instead produces a finite **line segment** on the current layer, joining the two nearest intersections (with other construction lines or visible shapes) that straddle the clicked point on the parallel line (if one side has no intersection, that end reaches the paper edge; the offset is cancelled if neither side can be resolved, or if the click is on an intersection) |
| `J` | Join (selected line segments / open polylines are merged into a single polyline only if they form one chain end-to-end, closing into a loop if the ends meet; rejected if the selection branches, has gaps, includes an arc, or mixes layers/styles) |
| `Shift+J` | Explode (the selected polyline becomes one line segment per edge, closing edge included, keeping its layer/style; non-polyline selections are left untouched) |
| `X` | Trim (two clicks: boundary, then target — the clicked side is removed; a construction line can be used as the boundary) |
| `E` | Extend (two clicks: boundary, then target — the target grows to the boundary; a construction line can be used as the boundary) |
| `F` | Fillet (enter a radius in the top panel, then click the first and second segments on the side you want to keep; `Esc` cancels) |
| `Shift+F` | Chamfer (line segments only; enter distances in the top panel — two distances, equal by default — then click the first and second segments on the side you want to keep) |
| `B` | Split (click a target; it is split at the clicked point) |
| `Q` | Constraint tool (choose the type in the top-panel combo, then click endpoints/centers or lines, circles and arcs in turn. The first pick stays put and the later one moves. Distance, radius and angle come from the top panel's `Value` field (mm and degrees; empty means the current value). The first `Esc` discards the picks, the second exits) |
| `1` | Point tool |
| `L` / `C` / `A` / `P` | Line / circle / arc / polyline |
| `Shift+L` | Construction line (infinite in both directions; two clicks — base point, then a point it passes through — after which further through-points can be placed from the same base point; the first `Esc` restarts from the base point, the second ends the tool. The through-point click accepts numeric input, orthogonal mode, polar tracking and snapping. Drawn clipped to the visible area on screen and to the paper on SVG/PDF export; it round-trips through DXF `XLINE`. Snaps to intersections with other shapes and construction lines. The base point is marked on screen only, with a short tick perpendicular to the line. Placed on a "構築線" (construction line) layer when one exists. Move/rotate/mirror/duplicate work; it can be used as a trim/extend boundary; offsetting it produces a line segment as described above. A construction line itself is not a target for trim/extend/fillet/chamfer/split/join/explode. Box selection picks it when the base point falls inside the box) |
| `Shift+C` | Regular polygon (two clicks: center, then a vertex; `Tab` toggles inscribed/circumscribed, side count (3–64) is set in the top panel) |
| `Shift+P` | Rectangle (two opposite corners; drawn as a closed polyline. The numeric-input format `@w,h` sets width and height directly) |
| `T` | Text (click the anchor, type the string and height in the top panel, `Enter` to commit, `Esc` to cancel) |
| `D` | Linear dimension (three clicks: two measured points, then the dimension line position; preview follows the cursor). Direction (aligned/horizontal/vertical) cycles with `Tab` or the top-panel combo; with orthogonal mode (`F8`) on and direction set to aligned, it snaps to the nearest axis of the two measured points. The top-panel angle field (degrees) sets an extension-line angle. Oblique direction and changing an existing dimension's direction are done from the right panel "寸法" (dimensions) section. The top-panel combo (single/series/parallel) switches to continuous input: series chains the next dimension's first point to the previous one's second point and keeps the dimension line collinear; parallel keeps the first dimension's first point fixed and only asks for a new dimension-line position each time |
| `Shift+D` | Radial dimension (two clicks: a circle or arc, then the leader direction) |
| `G` | Diameter dimension (two clicks: a circle or arc, then the direction of the dimension line) |
| `Shift+G` | Angular dimension. The top-panel combo or `Tab` switches between "3 points" and "2 lines". 3 points: four clicks — vertex, a point on each leg, then the arc position (parallel, same-direction or degenerate legs are rejected). 2 lines: three clicks — line 1, line 2, then the arc position; the vertex is the two lines' intersection (possibly on an extension), and the angle measured is the one on the side where the arc was placed (parallel lines are rejected) |
| `Shift+V` | Ordinate dimension (three clicks: origin, measured point, leader end; after `Esc` the origin stays fixed and further dimensions only need the two remaining clicks). X/Y is auto-detected from the leader direction; `Tab` or the top-panel combo switches auto/X/Y |
| `I` | Isometric circle (click the centre, then a point at the radius; drawn as four arcs by the four-centre method. `Tab` during the preview cycles the face Top/Left/Right) |
| `K` | Table / parts list. If "use a parts list" (default ON) is on and the drawing has none yet, places a parts list (right above the title block the instant `K` is pressed and selected, if the drawing frame is enabled — or otherwise at the click position, as a header row plus 3 empty part rows). Otherwise (OFF, or a parts list already exists) places an empty 3 × 3 general table at the click position. The right panel "表" (table) section shows type-specific controls: a general table gets "表を編集…" (edit table: cells, adding/removing rows and columns, column widths, per-column alignment, row height, text height); a parts list gets "記入…" (entry: cell values and adding/removing part rows only) and "部品欄の設定…" (parts-list settings: columns, widths, alignment, row/text height, snap to the title block, reapply the preset) |
| `0`–`9` `.` `-` `@` | Opens the numeric input box (only while a tool that accepts it, or a move/duplicate placement, is active). Type `x,y` / `@dx,dy` / `L<A` / `L` and press `Enter` to commit; an empty `Enter` falls through to the usual commit action; the first `Esc` closes the box |
| `Enter` | Commit a polyline (two or more points, left open; clicking the start point closes and commits it) |
| `Del` / `Backspace` | Delete the selected entities |
| `Esc` | Cancel drawing or placement / discard a rubber-band drag / clear the selection |
| `F3` | Toggle snapping (individual snap kinds are toggled from the top-panel "Snap types" menu) |
| `F5` | Toggle grid mode (rectangular / isometric; the isometric grid is the 30°, 90° and 150° line families, and snapping and orthogonal mode follow its lattice and axes) |
| `F8` | Toggle orthogonal mode (constrains to horizontal/vertical from the previous point, or to the nearest of 30°/90°/150° on the isometric grid; an available snap candidate takes precedence). Mutually exclusive with polar tracking (`F10`) |
| `F9` | Toggle paper-based display (line width and annotation size shown at paper mm scale vs. fixed screen px) |
| `F10` | Toggle polar tracking (step angle chosen from a top-panel combo: 15°/30°/45°/90°). Mutually exclusive with orthogonal mode (`F8`) |
| `Home` | Manual zoom-to-fit (fits the whole drawing in view) |
| Wheel | Cursor-centered zoom |
| Middle-drag / `Space`+left-drag | Pan |
| Click | On a shape: add to the selection (clicking empty space does nothing) |
| `Shift`+click | On a shape: remove it from the selection; on empty space: clear the selection |

Creating a new document, opening a file, or closing the window while there are
unsaved changes brings up a modal asking whether to discard them. The state of
`F3`/`F8`/`F9` and the default paper size/scale/title-block style are saved to
a settings file and restored on the next launch (see "Settings" below).

## File formats

- **`.mcad`**: the native JSON format. As of v0.13.0 the schema is v9, which stores geometric constraints; v1 through v8 files still load, with no constraints (backward compatible: v7 has no table kind, per-column alignment or "use a parts list" flag, so tables load as ordinary tables, all columns left-aligned, and "use a parts list" as ON — a legacy parts list is never guessed from its contents. v6 files get the dimension types and annotation fields added in v7 filled in with defaults, v5 and earlier have no tables, and v4 and earlier get dimension settings filled in with the defaults of their era). v8 added the table kind (table / parts list, `TableGeom.kind`), a per-drawing "use a parts list" flag (`SheetMeta.use_bom`), per-column text alignment for tables (`TableGeom.col_aligns`), and construction lines (`EntityGeom::XLine`)
- **New drawings** start with `"0"` plus five layers matching the drafting standard's line table (centre line, hidden line, outline, dimension line, text, with their linetypes and widths) and a bottommost `構築線` (construction line) layer (continuous, 0.13mm), six layers in all, and the current layer is the outline layer. Dimensions, text and construction lines are placed automatically on layers named `寸法線` (dimension line), `文字` (text) and `構築線` (construction line) when those exist (in a loaded drawing without them, the current layer is used). The standard layers can be deleted, but `"0"` is the document's default layer and cannot be. Renaming a layer is not yet available in the UI

## Settings

Application settings are stored separately from `.mcad` files, in the OS's
standard config directory (via the `dirs` crate; `~/.config/mcad/config.json`
on Linux). No drawing content is ever stored there.

- Snap (`F3`) / orthogonal mode (`F8`) / paper-based display (`F9`) / polar tracking (`F10`, including its step angle) on/off state
- Per-kind snap on/off state (top-panel "Snap types" menu; nearest-point defaults to OFF)
- Constraint symbol visibility (a checkbox in the right panel's "拘束" section; default ON)
- Default paper size, orientation, scale and title-block style for new documents
- Recently used files (the "Recent" menu in the top panel, up to 5 entries; updated on every open/save, missing files are excluded)
- Dimension trailing-zero trimming (default ON; toggled in the dimension style dialog and saved with the document. `120.00` displays as `120`)

If the settings file is missing or corrupted, mcad falls back to defaults and
still starts normally.

## Notes on DXF

DXF is treated as an interchange format. After importing, always save in `.mcad`
format — the original DXF file is never overwritten. Be aware of the following.

- **Colors**: approximated to the 9-color ACI palette (RGB is rounded to the nearest color)
- **Layer locks**: lost on a round trip, since DXF has no such field
- **Layer stacking order**: lost on a round trip, since DXF has no z-index field
- **Line width**: per-entity line width round-trips, but a layer's default line width is not preserved and falls back to the default (0.35mm) — a limitation of the `dxf` crate 0.6.1
- **Linetype**: continuous / dashed / dash-dot / dash-dot-dot round-trips for both layers and entities. Unknown linetype names fall back to continuous
- **TEXT**: position, height and rotation are mapped. Height is converted against the drawing's scale, in paper mm. Strings containing CJK are written and restored as UTF-8 (the DXF header is R2007). TEXT with a non-standard justification — anything other than horizontal Left plus vertical Baseline — is skipped (this includes cell text from centered/right-aligned table columns; see "Tables" below)
- **DIMENSION**: since DXF has no primitive that corresponds to an mcad dimension, dimensions are exported decomposed into their dimension line, extension lines, arrowheads and text, written as `LINE`/`SOLID`/`ARC`/`TEXT` (the same **one-way round trip** as tables — reimporting does not rebuild the dimension entity; the `SOLID` used for the filled arrowhead is counted in the skipped-entity total on reimport). They are saved normally in `.mcad`. Importing DXF `DIMENSION` written by other CAD software is supported (aligned, rotated, radial, diameter, three-point angular and ordinate dimensions; two-line angular and arc-length dimensions are not)
- **Tables**: a table is exported decomposed into its rules (`LINE`) and cell texts (`TEXT`). This is a **one-way trip** — reimporting does not rebuild the table entity (the distinction in rule thickness is also lost). Centered/right-aligned columns are written using `TEXT` horizontal justification (group 72/11), so on reimport that column's cell text counts as a skipped non-standard-justification `TEXT` (left-aligned columns still import as ordinary text). `ACAD_TABLE` written by other software is skipped, since the `dxf` crate 0.6.1 has no type for it (it is not even counted as skipped)
- **Construction lines (XLINE)**: written and read as `XLINE` (base point and unit direction) — a **symmetric** round trip. The direction is written rounded to 12 decimal digits and renormalized on import. An `XLINE` with a zero direction is counted as skipped. The half-line `RAY` entity is not supported and is counted as skipped without being read
- **Geometric constraints**: not saved to DXF (a status bar notice appears on export)
- **Drawing frame and title block**: these are not exported to DXF, because DXF has nowhere to carry the sheet metadata. A reimported drawing draws its frame from the default paper and scale, so the frame no longer lines up with the geometry the way it did in the original
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
| **mcad-geom** | Geometric primitives (`Point2`, `Vec2`, `LineSeg`, `Circle`, `Arc`, `Polyline`, `Shape`) and calculations (nearest point, distance, intersection, the general constraint solver `solve`). GUI-independent, pure functions only |
| **mcad-core** | The CAD document model (`Document` / `Entity` / `Layer`) and command-pattern undo/redo |
| **mcad-io** | Saving and loading `.mcad` (JSON) and DXF I/O via the `dxf` crate |
| **mcad-app** | The egui GUI: tools, viewport, snapping engine, layer panel and file operations |

Layers are ordered `mcad-app` → `mcad-io` → `mcad-core` → `mcad-geom`, and a higher
crate may depend directly on any lower one (for example `mcad-app` also uses
`mcad-core` and `mcad-geom`). Only the reverse direction is forbidden.

Key design decisions:

- **f64 world coordinates**: coordinate precision matters in CAD. f32 accumulates error on large drawings, so the conversion to f32 happens only at the egui drawing boundary
- **Command-pattern undo**: cheaper in memory than snapshots and viable for large drawings. Every change goes through `Document::apply(Command)`
- **Keeping geom GUI-independent**: geometry stays testable as pure functions, and the constraint solver sits on this layer too
- **No spatial index**: a full scan with AABB culling is used (measured in v0.11.0: picking, box selection and culling stay around 3 ms even at 20,000 entities)

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
default fonts, so egui falls back per glyph and both Latin and CJK render correctly. The application UI is being
migrated to Japanese region by region; currently the right panel (layers, selected style, sheet info, dimensions and
tables) and the title-block editor, the dimension style dialog, and the table dialogs (edit table / parts-list entry /
parts-list settings) are in Japanese, while the rest (top panel, confirmation modals, etc.) remain in English.

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
