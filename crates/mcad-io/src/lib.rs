//! `mcad-io` — `.mcad`（JSON）保存/読込と DXF 入出力。
//!
//! # 公開 API の概要
//!
//! - `.mcad`（JSON）: [`save_mcad`] / [`load_mcad`]（ファイル）、
//!   [`to_json`] / [`from_json`]（文字列）、
//!   [`export_document`] / [`import_document`]（DTO [`FileDocument`] との相互変換）
//! - エラー: [`IoError`]
//!
//! ファイル形式の設計（ポータブル DTO・ファイル ID 方式・バリデーション）は
//! [`mcad_file`] モジュールの doc を参照。DXF 入出力（LINE / CIRCLE / ARC /
//! LWPOLYLINE / POINT）は後続タスクで追加する。

mod error;
mod mcad_file;

pub use error::IoError;
pub use mcad_file::{
    FORMAT_VERSION, FileDocument, FileEntity, export_document, from_json, import_document,
    load_mcad, save_mcad, to_json,
};
