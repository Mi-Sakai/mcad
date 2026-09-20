//! 注記エンティティ（寸法・表）を描画可能な要素へ**展開**する純関数群
//! （DESIGN.md M11 設計判断1、タスク71）。
//!
//! # なぜ core にあるか
//!
//! 寸法と表は、保存データ（[`DimLinear`](crate::DimLinear) / [`TableGeom`](crate::TableGeom)
//! など）と実際に描かれる線・文字が 1 対 1 ではない。「罫線・補助線・矢先・文字を
//! どこへ置くか」を組む処理が要り、それは M10 まで `mcad-app` にあった。
//! その結果 `mcad-io` の DXF 表 export が同じ組版を独立に再実装してしまい、値が
//! 乖離しても検出できない残債になった（M10 タスク61）。M11 で寸法も DXF へ分解
//! export する（タスク72）ため、同じ二重化を繰り返さないよう展開を core へ移した。
//!
//! これで画面（`mcad-app`）・SVG/PDF 出力（`mcad-app` の `plot`）・DXF export
//! （`mcad-io`）が**同じ 1 つの実装**から座標を得る。
//!
//! # 表示状態は持ち込まない
//!
//! このモジュールは GUI 非依存という core の設計方針をそのまま守る。紙基準表示
//! トグル（F9）・ズームといった表示状態は知らず、寸法の展開は
//! [`DimRender`]（ワールド長へ解決済みのパラメータ束）を受け取るだけ、表の展開は
//! 文書尺度 `k` を受け取るだけである。表示モードの解決は `mcad-app` 側
//! （`dimension::dim_sizes` / `dimension::dim_render`）に残る。

mod dim_label;
mod dimension;
mod table;

pub use dim_label::{DimLabel, TextRun, layout_dim_label};
pub use dimension::{
    DimExpansion, DimRender, angular_distance, angular_pick_segments, arrow_kind_occupies_line,
    diameter_distance, diameter_pick_segments, dim_distance, expand_angular, expand_diameter,
    expand_dim, expand_linear, expand_ordinate, expand_radial, label_box_center,
    label_box_contains, linear_distance, linear_pick_segments, ordinate_distance,
    ordinate_pick_segments, radial_distance, radial_pick_segments,
};
/// 長さ寸法・角度寸法の AABB（[`crate::EntityGeom::aabb`]）が展開と同じ骨格を使う
/// ためのクレート内公開。外部 API としては [`linear_pick_segments`] /
/// [`expand_linear`]（角度は [`angular_pick_segments`] / [`expand_angular`]）が
/// 同じ骨格を返すので、これを `pub` にはしない。
pub(crate) use dimension::{angular_frame, linear_degenerate_dir_and_point, linear_frame};
pub use table::{
    CELL_TEXT_PAD_MM, FRAME_BORDER_WIDTH_MM, FRAME_DIVIDER_WIDTH_MM, TableExpansion, TableSegment,
    expand_table, table_world_aabb,
};
