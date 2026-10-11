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
    #[error(
        "unsupported format version: {0} (supported: 1..={max})",
        max = crate::FORMAT_VERSION
    )]
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

    /// 拘束が指すエンティティの添字が `entities` 配列の範囲外（v9）。
    #[error("constraint {index} references missing entity {entity}")]
    BadConstraintRef {
        /// constraints 配列内のインデックス。
        index: usize,
        /// 参照していたエンティティの添字。
        entity: usize,
    },

    /// 拘束が対象にできない図形（ポリライン・文字など）や合わない点の種類を指している
    /// （v9。例: 線分の `Center`、円の `Start`）。
    #[error("constraint {index} is invalid: {reason}")]
    InvalidConstraint {
        /// constraints 配列内のインデックス。
        index: usize,
        /// 不正の内容。
        reason: String,
    },

    /// 拘束が保存された形のままでは満たされていない（v9）。読込時は解かないので、
    /// 手編集等で壊れたファイルは読込エラーにする。
    #[error("constraint {index} is not satisfied by the saved geometry")]
    UnsatisfiedConstraint {
        /// constraints 配列内のインデックス。
        index: usize,
    },

    /// import 中の再構築コマンドをコアが拒否した（バリデーション通過後は
    /// 起きない想定の防御的バリアント）。
    #[error("core rejected command during import: {0}")]
    Core(#[from] CoreError),

    /// DXF の読み書き・構文解析の失敗（`dxf` クレート由来）。
    ///
    /// 個々のエンティティの対応可否・ジオメトリの妥当性の問題はここに含めない
    /// （[`crate::dxf_file::ImportSummary::skipped_entities`] として扱う）。
    /// これはあくまで DXF ファイル全体の構造的な異常（構文エラー・
    /// 読み書き失敗など）を表す。
    #[error("dxf error: {0}")]
    Dxf(#[from] dxf::DxfError),
}
