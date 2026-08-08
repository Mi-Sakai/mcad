# M5 v0.5.0 review / handoff for Claude

Reviewed: 2026-07-18 (JST)  
Baseline commit: `f497273 docs: 長期ロードマップ(M5〜)をDESIGN.md第7章として策定`

## Executive summary

`mcad` is a compact Rust/egui 2D CAD split into four crates:

```text
mcad-app -> mcad-io -> mcad-core -> mcad-geom
```

M4 (`v0.4.0`) is complete.  The repository is in a healthy state: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace` all passed in this review (209 tests total: app 84, core 41, geom unit 51, geom property 10, io 19, integration 4).

The next planned milestone is M5 / `v0.5.0`, “editing operations”: duplicate, rotate, mirror, and offset.  `DESIGN.md` section 7 already contains the accepted high-level design, task decomposition (17–21), and acceptance criteria.  Treat that document as the source of truth; this file explains the actual code seams and the current worktree state so implementation can continue without rediscovery.

## Important worktree state

There are three **uncommitted, passing** changes.  They appear to be an in-progress implementation of M5 task 17.  Do not discard or overwrite them.

```text
M crates/mcad-geom/src/primitives.rs
M crates/mcad-geom/src/vec2.rs
M crates/mcad-geom/tests/properties.rs
```

They add:

- `Vec2::rotated(angle)` and `Vec2::reflected(axis_dir)`;
- `rotated` / `mirrored` methods for `LineSeg`, `Circle`, `Arc`, `Polyline`, and `Shape`;
- unit tests for every primitive and the nontrivial arc-mirror direction case;
- proptests for rotate round-trips, double-mirror round-trips, and radius invariance.

The implementation is mathematically sound for normal finite inputs.  In particular, mirroring an `Arc` reflects both endpoint angles **and swaps them**, preserving mcad’s invariant that arcs sweep CCW from start to end.  This is essential; do not simplify it to independently reflecting `start_angle` and `end_angle`.

Two follow-up details should be addressed deliberately while completing task 17 / using it in task 19:

1. `Vec2::reflected(Vec2::ZERO)` intentionally returns the input unchanged.  This is a safe low-level fallback, but a zero-length mirror axis is not meaningful CAD input.  The UI must reject two identical axis clicks (keep the state active and show an ASCII status error) rather than committing an apparent no-op mirror.
2. The new methods assume finite arguments, like the existing `translated` API.  The `Document::apply(Command::ModifyEntity { .. })` validation remains the authoritative boundary that prevents non-finite geometry from entering a document.  Previews should only use finite values derived from pointer input.

The method naming is intentionally consistent with existing `Shape::translated`: use `rotated` / `mirrored`, not a second parallel `rotate` / `mirror` API, unless there is a coordinated public-API reason to rename all three.

## Architecture and invariants that matter for M5

### `mcad-geom`

This crate is GUI-independent and uses `f64` throughout.  `Shape` is the shared geometry enum (`Point`, `Line`, `Circle`, `Arc`, `Polyline`).  It already owns translation, AABB, closest-point, distance, intersection, and validation.  M5 transformations belong here; do not calculate transformed coordinates ad hoc in `mcad-app`.

`Arc` is center/radius/start-angle/end-angle with a CCW sweep.  Angles are allowed to be unnormalized; `Arc::sweep()` interprets them with wrapping.  Preserve that convention in all new geometry operations.

### `mcad-core`

`Document` owns entities, layers, undo/redo, and document generation.  Its fields are private.  All changes must go through `Document::apply(Command)`.

- `Command::ModifyEntity { id, new_geom }` is the correct commit mechanism for rotate, mirror, and future shape modification.
- `Command::AddEntity(entity)` is the correct mechanism for duplication and offset output.
- `Command::Batch(Vec<Command>)` is atomic and represents one undo operation.  If one selected entity is on a locked layer, the entire multi-entity operation fails.  Surface that `Err` in the status bar; never ignore it.
- `Document::apply` returns `NewIds`.  For duplication, its `entities` list is in command order and should become the new selection after a successful batch.  Do not infer IDs by comparing iterators.
- Deleted entities/layers are tombstones, so IDs remain stable across undo/redo.  Continue to clean the selection with `SelectTool::retain_alive` after history operations.
- No-op commands do not create history entries or change the generation.  Avoid manually manipulating dirty state.

### `mcad-app`

`McadApp` and its shortcut routing live in `crates/mcad-app/src/main.rs`; `SelectTool` lives in `crates/mcad-app/src/tool.rs`.  Drawing tools implement `Tool`, but selection/move/delete are intentionally a separate `SelectTool` path.  Rotation/mirroring act on an existing selection, so their click-state machine should extend `SelectTool` (or another select-mode state owned by it), rather than be forced into the drawing-tool trait.

Useful existing integration points:

- Shortcut collection: `McadApp::ui` calls `handle_tool_shortcut_keys` only when no unsaved-changes modal is active.  It already excludes Ctrl/Cmd-modified keys, so `Ctrl+D` should be handled alongside the existing file/history shortcuts before tool-key routing.
- Select input: `handle_select_input` owns Delete, Escape, pan avoidance, clicks, and drag-to-move.  Add R/M/O handling so it cannot accidentally coexist with a move drag.
- Commit/error path: existing code calls `document.apply(...)`, then reports `CoreError` through the timed status message.  Follow that exact pattern for every M5 action.
- Rendering: `draw_selection` and `SelectTool::drag_preview` show selected geometry and drag feedback.  Add transform previews there or in an equivalent focused helper, using `Shape::rotated` / `Shape::mirrored` and the existing shape-drawing functions.  Preview must never mutate `Document`.
- Snap: `apply_snap` and `snap::snap` are used for drawing coordinates.  Reuse them for rotation pivot, rotation-angle point, and both mirror-axis points.  Clear/update `snap_marker` consistently.

All egui-visible text must remain ASCII.  Japanese is fine in comments and documentation, but not panel labels or status text.

## M5 implementation guidance

### Task 17: geometry transforms (in progress)

Finish/review the dirty `mcad-geom` changes first.  Retain the property tests and add targeted tests if an API adjustment is made.  At minimum, keep these invariants fixed:

- rotation followed by inverse rotation returns the original geometry within tolerance;
- mirror across the same nondegenerate axis twice returns the original geometry;
- circle/arc radius is unchanged;
- mirrored arc geometry has the same locus and sweep magnitude, while retaining CCW semantics;
- point, line, circle, arc, and polyline all dispatch through `Shape`.

Because `mcad-geom` and `mcad-core` are path dependencies of sibling repository `../tcad`, any public API change in either crate must also pass `cargo test --workspace` from `../tcad`.  The sibling worktree was clean during this review, but its tests were not run as part of this review.

### Task 18: duplicate

The approved behavior is `Ctrl+D`: duplicate the selected set with an offset, as one `Batch(AddEntity(...))`; replace the selection with the returned new IDs.  Preserve each entity’s layer and style by cloning the full `Entity`, then replacing only geometry if the chosen offset requires it.  Applying the batch is the lock check and atomicity guarantee.

The phrase “offset付き複製” needs a small UX decision before coding: choose and document either a deterministic default displacement (for example, a fixed world delta) or a two-click copy placement.  A fixed *screen-pixel* displacement is unstable under zoom; a fixed world displacement is deterministic but may be too small/large.  If no decision has been made externally, two-click placement gives the least surprising CAD behavior, but it expands task 18 into a state machine and should be reflected in `DESIGN.md` before implementation.

### Task 19: rotate and mirror

Suggested state semantics, which resolve the current wording “two-stage state machine”:

- `R` with a nonempty selection: click a pivot, then click a reference/target direction point; the angle is `atan2(target - pivot)`.  During the second stage, preview the transformed selection.  Clicking the pivot again is a zero angle and should be treated as a no-op/cancel with clear feedback rather than a history entry.
- `M` with a nonempty selection: click axis point A, then axis point B.  During the second stage, preview.  Reject A == B using a geometry-scale tolerance.
- `Esc` cancels the state without changing the document.  Switching tools, file replacement, and modal entry must also clear it.
- Commit one `Command::Batch` of `ModifyEntity` commands.  Keep the selection on the transformed original entities; IDs do not change.

Before adding R/M behavior, ensure ordinary click/drag selection and pan are gated while the transform state is active.  Add unit tests to `tool.rs` for state transitions and command construction, then use core tests to demonstrate atomic failure on a mixed locked/unlocked selection.

### Task 20: offset

This is the riskiest M5 item because the roadmap intentionally limits the geometry but does not yet define the exact user interaction or all edge cases.  Before implementation, add a short specification to `DESIGN.md` covering:

- how the distance and side are entered (click-derived signed distance is likely simplest in the current no-command-line UI);
- whether an offset replaces or adds geometry (CAD convention and the current wording suggest it adds a new entity; state this explicitly);
- zero distance, negative radius after inward circle/arc offset, and a click exactly on the source;
- polyline joins: M5 allows only shifted segments plus intersections of adjacent offset segments, and explicitly permits self-intersections.  Define the fallback for parallel adjacent segments and open-polyline endpoints.

Keep the algorithm in `mcad-geom` as a reusable pure API, not in the UI.  Test exact line/circle/arc cases and polyline corner/end cases separately from UI tests.  Unsupported shapes (currently Point) need an explicit status message and must not partially commit a multi-selection request.

### Task 21: documentation/release

Update `README.md`, `DESIGN.md`, and `CHANGELOG.md` in the same change set as the feature.  Keep the keybinding table and the in-app ASCII shortcut help synchronized.  Per `AGENTS.md`, update the root `Cargo.toml` version only in its two designated places when cutting `v0.5.0`; do not add per-crate version strings.

GUI changes require a real-window smoke test, requested from the user and recorded in the release documentation.  Suggested M5 smoke cases: duplicate/undo/redo; rotated and mirrored geometry with snap; zero-length mirror-axis rejection; locked-layer atomic failure; every supported offset shape; cancel during each in-progress operation; save/undo dirty marker behavior after each operation.

## Review findings and priorities

No release-blocking defect was found in the reviewed M4 baseline or the current task-17 changes.  The main implementation risks are design completeness rather than failing infrastructure:

| Priority | Finding | Required response |
|---|---|---|
| High | Offset interaction and degenerate/join semantics are underspecified. | Write the narrow M5 contract in `DESIGN.md` before coding task 20. |
| High | A mirror axis with coincident points silently becomes identity in the current low-level helper. | Reject it in select-mode UI; do not commit it. |
| High | Multi-entity operations can include locked layers. | Always use one `Command::Batch`, preserve atomic failure, and display the error. |
| Medium | Transform state can conflict with existing selection drag/pan/keyboard flow. | Make state precedence explicit and test Esc/tool-switch/reset behavior. |
| Medium | `mcad-geom` / `mcad-core` are used by `../tcad`. | Run tcad workspace tests before finalizing public-API changes. |
| Low | `main.rs` (about 1900 lines) already centralizes much UI wiring. | Extract focused helpers/state types while adding M5 instead of further flattening `ui()`. |

## Required verification before commit

From this repository:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For public `mcad-geom` / `mcad-core` changes, also from `../tcad`:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Finally perform and record the GUI smoke test.  Do not commit user-managed root `A-*.md` review notes; they are intentionally ignored.
