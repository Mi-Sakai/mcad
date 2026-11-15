# mcad — a 2D CAD written in Rust

*[日本語版 README](./README.md)*

A 2D CAD application built with Rust and egui. Current version: **v1.0.0** (first stable release).
The goal is to take one mechanical drawing from first line to PDF entirely in mcad.

This is a personal project started in July 2026, prompted by the release of Claude Fable 5, to find out how far
AI-assisted development can go in building a 2D CAD that is actually usable. The overall design, the architectural
rules, the drafting requirements, and the accept/reject decisions during review are the author's; Claude Code is used
for the implementation.

> This is an independent personal project and is not affiliated with Anthropic.

> **Note on language.** The specification, the design documents and the changelog are written in Japanese, and the
> application UI is partly in Japanese. This file is a short English guide so that the project can be evaluated
> without reading Japanese. **The detailed specification is the Japanese [`docs/mcad仕様書.md`](./docs/mcad仕様書.md).**

## Features

- **Drawing and editing**: lines, circles, arcs, polylines, rectangles, regular polygons, construction lines and text
  (CJK supported). Move, duplicate, rotate, mirror, offset, trim, extend, fillet, chamfer, split, join and grip editing.
  Numeric input, object snaps, orthogonal mode, polar tracking and isometric drawing aids
- **Dimensions**: linear, radial, diameter, angular and ordinate, with auxiliary symbols, size tolerances, reference and
  theoretically-exact notation, and a per-document dimension style
- **Drawings**: layers (color, linetype, line width, lock, stacking order), drawing frame and title block, tables and a
  parts list, and geometric constraints on lines, circles and arcs
- **Files**: `.mcad` (JSON; files remain readable across 1.x), DXF import and export, true-scale SVG/PDF export

Features, operation and the file formats are described in the [**specification**](./docs/mcad仕様書.md) (Japanese).
Frequently used sections: [key bindings](./docs/mcad仕様書.md#a-キーの一覧),
[layers of a new drawing](./docs/mcad仕様書.md#43-新規図面のレイヤーと自動割り当て),
[the `.mcad` format](./docs/mcad仕様書.md#11-mcad-形式) and its [compatibility promise](./docs/mcad仕様書.md#115-互換性の約束),
[DXF support and limitations](./docs/mcad仕様書.md#12-dxf), [the settings file](./docs/mcad仕様書.md#13-設定configjson).

## Getting started

```bash
cargo run -p mcad-app             # launch
cargo run --release -p mcad-app   # a release build is recommended for everyday use
```

For tests, linting, formatting checks and other development steps, see [`AGENTS.md`](./AGENTS.md) (Japanese).

## Documentation

All of the following are written in Japanese.

| Document | Contents |
|---|---|
| [`docs/mcad仕様書.md`](./docs/mcad仕様書.md) | Specification: behavior as seen by the user, key bindings, the `.mcad` format and its [compatibility promise](./docs/mcad仕様書.md#115-互換性の約束), DXF and SVG/PDF output, the settings file |
| [`DESIGN.md`](./DESIGN.md) | Design: principles, structure, design decisions, roadmap |
| [`CHANGELOG.md`](./CHANGELOG.md) | Per-version change history |
| [`AGENTS.md`](./AGENTS.md) | Development conventions (architectural invariants, build and verification, coding rules) |

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

To render CJK text in drawings and the Japanese parts of the UI, [Noto Sans JP](https://fonts.google.com/noto/specimen/Noto+Sans+JP)
Regular is embedded in the binary. Noto Sans JP is provided under the SIL Open Font License 1.1. The full license text
is bundled at [`crates/mcad-app/assets/fonts/LICENSE-OFL.txt`](./crates/mcad-app/assets/fonts/LICENSE-OFL.txt).

> Noto Sans JP is licensed under the SIL Open Font License, Version 1.1.
> © 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'.
> "Noto" is a trademark of Google LLC.
