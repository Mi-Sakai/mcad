//! `mcad-core` — ドキュメントモデル・レイヤー・undo/redo。
//!
//! `mcad-geom` の幾何型（[`mcad_geom::Shape`] など）の上に、GUI 非依存の CAD
//! ドキュメントモデルを構築する層。
//!
//! # 公開 API の概要
//!
//! - ID: [`EntityId`], [`LayerId`]（`slotmap` キー。undo/redo をまたいでも安定）
//! - 値型: [`Rgb`], [`Style`], [`Layer`], [`Entity`]
//! - ドキュメント: [`Document`]（エンティティ・レイヤー・カレントレイヤーと履歴を保持）
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

mod command;
mod document;
mod entity;
mod error;
mod id;
mod layer;
mod style;

pub use command::Command;
pub use document::{Document, NewIds};
pub use entity::Entity;
pub use error::CoreError;
pub use id::{EntityId, LayerId};
pub use layer::Layer;
pub use style::{Rgb, Style};
