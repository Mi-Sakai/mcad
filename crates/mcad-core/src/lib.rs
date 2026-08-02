//! `mcad-core` — ドキュメントモデル・レイヤー・undo/redo。
//!
//! `mcad-geom` の幾何型（[`mcad_geom::Shape`] など）の上に、GUI 非依存の CAD
//! ドキュメントモデルを構築する層。
//!
//! # 公開 API の概要
//!
//! - ID: [`EntityId`], [`LayerId`]（`slotmap` キー。undo/redo をまたいでも安定）
//! - 値型: [`Rgb`], [`WidthMm`], [`Linetype`], [`Style`], [`Layer`], [`Entity`]
//! - 図面メタデータ: [`SheetMeta`]（[`Scale`] / [`PaperSize`] / [`Orientation`] /
//!   [`TitleBlockKind`] / [`TitleBlockFields`]）と表題欄様式 [`TitleBlockTemplate`]
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
mod document;
mod entity;
mod entity_geom;
mod error;
mod id;
mod layer;
mod sheet;
mod style;
mod title_block;

pub use command::Command;
pub use document::{Document, NewIds};
pub use entity::Entity;
pub use entity_geom::{DimLinear, DimRadial, EntityGeom, TextGeom};
pub use error::CoreError;
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
