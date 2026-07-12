//! mcad-io のエラー型。

use mcad_core::CoreError;

/// `.mcad` 保存/読込・DXF 入出力が返しうるエラー。
///
/// import は「壊れたファイルを黙って読める形に丸めない」方針で、フォーマット上の
/// 異常（未知バージョン・欠落レイヤー参照・非有限数・負半径など）をすべて明示的な
/// バリアントとして返す。
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    /// JSON のシリアライズ/デシリアライズ失敗（構文エラー・型不一致など）。
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// ファイル読み書きの失敗。
    #[error("file error: {0}")]
    File(#[from] std::io::Error),

    /// 対応していないフォーマットバージョン。
    #[error("unsupported format version: {0} (supported: 1)")]
    UnsupportedVersion(u32),

    /// レイヤーが 1 つもない（`Document` はデフォルトレイヤーを必ず持つため、
    /// 正しい export がこれを生むことはない）。
    #[error("document must have at least one layer")]
    NoLayers,

    /// カレントレイヤーのインデックスがレイヤー配列の範囲外。
    #[error("current layer index out of range: {0}")]
    BadCurrentLayer(usize),

    /// エンティティのレイヤー参照がレイヤー配列の範囲外。
    #[error("entity {index} references missing layer {layer}")]
    BadLayerRef {
        /// entities 配列内のインデックス。
        index: usize,
        /// 参照していたレイヤーインデックス。
        layer: usize,
    },

    /// エンティティのジオメトリが不正（非有限座標・負半径・空ポリラインなど）。
    #[error("entity {index} has invalid geometry: {reason}")]
    InvalidGeometry {
        /// entities 配列内のインデックス。
        index: usize,
        /// 不正の内容。
        reason: String,
    },

    /// import 中の再構築コマンドをコアが拒否した（バリデーション通過後は
    /// 起きない想定の防御的バリアント）。
    #[error("core rejected command during import: {0}")]
    Core(#[from] CoreError),
}
