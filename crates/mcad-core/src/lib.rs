//! `mcad-core` — ドキュメントモデル・レイヤー・undo/redo。
//!
//! `mcad-geom` の幾何型（[`mcad_geom::Shape`] など）の上に、GUI 非依存の CAD
//! ドキュメントモデルを構築する層。
//!
//! # 公開 API の概要
//!
//! - ID: [`EntityId`], [`LayerId`]（`slotmap` キー。undo/redo をまたいでも安定）
//! - 値型: [`Rgb`], [`WidthMm`], [`Linetype`], [`Style`], [`Layer`], [`Entity`]
//! - 長さ寸法の向き: [`DimDirection`]（[`DimLinear::direction`]。整列寸法と回転寸法。
//!   M11 タスク68）
//! - 角度寸法・座標寸法: [`DimAngular`]（頂点と 2 方向・弧の半径。値はラジアンで
//!   持ち度で表示）、[`DimOrdinate`]（基準・計測点・引出線端と [`OrdinateAxis`]。
//!   値は符号つきの座標成分）。M11 タスク69
//! - 寸法の注記とスタイル: [`DimAnnotation`]（[`SizeTolerance`] / [`FitClass`] /
//!   [`ArrowPlacement`]）と文書単位の [`DimStyle`]。文法検証は
//!   [`DimAnnotation::validate`] に [`DimKind`] を渡して行う
//! - 図面メタデータ: [`SheetMeta`]（[`Scale`] / [`PaperSize`] / [`Orientation`] /
//!   [`TitleBlockKind`] / [`TitleBlockFields`]）と表題欄様式 [`TitleBlockTemplate`]
//! - 表: [`TableGeom`]（[`EntityGeom::Table`] の中身。汎用表・部品表の土台。
//!   行列数・寸法の上限は [`MAX_TABLE_ROWS`] / [`MAX_TABLE_COLS`] / [`MAX_TABLE_MM`]）
//! - 展開（`expand` モジュール、M11 タスク71）: 寸法・表を描画可能な線・矢先・文字へ
//!   組む純関数。寸法は [`expand_linear`] / [`expand_radial`] / [`expand_diameter`] /
//!   [`expand_angular`] / [`expand_ordinate`]（パラメータは [`DimRender`]、結果は
//!   [`DimExpansion`]）、表は [`expand_table`]（結果は [`TableExpansion`]）。
//!   **[`EntityGeom`] を渡して種別を問わず展開するなら [`expand_dim`]**（消費側が
//!   種別ごとの表を持たずに済ませるためのディスパッチ。pick 版は [`dim_distance`]）。
//!   ヒットテスト用の線分は [`linear_pick_segments`] 等、
//!   値の組版は [`layout_dim_label`]。**画面・SVG/PDF・DXF export の唯一の出所**
//! - ドキュメント: [`Document`]（エンティティ・レイヤー・カレントレイヤー・
//!   図面メタデータと履歴を保持）
//! - 変更: [`Command`] を [`Document::apply`] に渡す。戻り値の [`NewIds`] で
//!   新規発行された ID を受け取れる。取り消し/やり直しは
//!   [`Document::undo`] / [`Document::redo`]
//! - エラー: [`CoreError`]
//!
//! # 設計方針
//!
//! - ドキュメントの状態変更はすべて [`Command`] 経由（フィールドは非公開、読み取りは
//!   getter/イテレータのみ）。これにより不変条件（デフォルトレイヤーは削除不可 など）を
//!   一元的に守り、あらゆる変更を undo 可能にする。
//! - undo はコマンドパターン（逆操作を履歴スタックへ積む）。スナップショット方式より
//!   省メモリで大図面に耐える（DESIGN 4-2）。
//! - GUI 非依存（eframe/egui へ依存しない）。色は [`Rgb`] の生値で表現する。
//! - 描画・出力へ非有限値やゼロ寸法を伝播させないため、尺度と線幅は **検証済み型**
//!   （[`Scale`] / [`WidthMm`]）で持つ。生成は検証済みコンストラクタのみ、serde も
//!   `try_from` 経由なので、手編集されたファイルの不正値も読込境界で弾かれる
//!   （DESIGN.md M8 設計判断2・5）。

mod command;
mod dim;
mod document;
mod entity;
mod entity_geom;
mod error;
mod expand;
mod id;
mod layer;
mod sheet;
mod style;
mod title_block;

pub use command::Command;
pub use dim::{
    ArrowPlacement, DimAnnotation, DimKind, DimStyle, FitClass, MAX_DIM_DECIMALS, MAX_DIM_STYLE_MM,
    SizeTolerance,
};
pub use document::{Document, NewIds};
pub use entity::Entity;
pub use entity_geom::{
    DimAngular, DimDiameter, DimDirection, DimLinear, DimOrdinate, DimRadial, EntityGeom,
    MAX_TABLE_COLS, MAX_TABLE_MM, MAX_TABLE_ROWS, OrdinateAxis, TableGeom, TextGeom,
    approx_text_width,
};
pub use error::CoreError;
pub use expand::{
    CELL_TEXT_PAD_MM, DimExpansion, DimLabel, DimRender, FRAME_BORDER_WIDTH_MM,
    FRAME_DIVIDER_WIDTH_MM, TableExpansion, TableSegment, TextRun, angular_distance,
    angular_pick_segments, arrow_kind_occupies_line, diameter_distance, diameter_pick_segments,
    dim_distance, expand_angular, expand_diameter, expand_dim, expand_linear, expand_ordinate,
    expand_radial, expand_table, label_box_center, label_box_contains, layout_dim_label,
    linear_distance, linear_pick_segments, ordinate_distance, ordinate_pick_segments,
    radial_distance, radial_pick_segments, table_world_aabb,
};
pub use id::{EntityId, LayerId};
pub use layer::Layer;
pub use sheet::{
    MAX_SCALE_TERM, Orientation, PaperSize, ProjectionMethod, Scale, SheetMeta, TitleBlockFields,
    TitleBlockKind, Unit,
};
pub use style::{Linetype, Rgb, Style, WidthMm};
pub use title_block::{
    MAX_TITLE_BLOCK_CELLS_PER_ROW, MAX_TITLE_BLOCK_MM, MAX_TITLE_BLOCK_ROWS, TitleBlockCell,
    TitleBlockField, TitleBlockRow, TitleBlockTemplate,
};
