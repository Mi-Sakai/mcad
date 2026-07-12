//! `mcad-io` — `.mcad`（JSON）保存/読込と DXF 入出力。
//!
//! # 公開 API の概要
//!
//! - `.mcad`（JSON）: [`save_mcad`] / [`load_mcad`]（ファイル）、
//!   [`to_json`] / [`from_json`]（文字列）、
//!   [`export_document`] / [`import_document`]（DTO [`FileDocument`] との相互変換）
//! - DXF: [`save_dxf`] / [`load_dxf`]（ファイル）、
//!   [`export_dxf`] / [`import_dxf`]（`dxf::Drawing` との相互変換）、
//!   結果は [`ImportSummary`]（未対応エンティティの無視件数を含む）
//! - エラー: [`IoError`]
//!
//! ファイル形式の設計（ポータブル DTO・ファイル ID 方式・バリデーション）は
//! [`mcad_file`] モジュールの doc を、DXF の対応範囲・色や層ロックの扱い・
//! 未対応エンティティの扱いは [`dxf_file`] モジュールの doc を参照。

mod dxf_file;
mod error;
mod mcad_file;

pub use dxf_file::{ImportSummary, export_dxf, import_dxf, load_dxf, save_dxf};
pub use error::IoError;
pub use mcad_file::{
    FORMAT_VERSION, FileDocument, FileEntity, export_document, from_json, import_document,
    load_mcad, save_mcad, to_json,
};
