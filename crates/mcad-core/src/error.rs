//! コア操作のエラー型。

use crate::{EntityId, LayerId};

/// [`crate::Document::apply`] などコア操作が返しうるエラー。
///
/// これらのエラーが返るとき、ドキュメントの状態は **変更されない**
/// （部分適用による不整合状態を作らない）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// 指定した [`EntityId`] のエンティティが存在しない（または既に削除済み）。
    #[error("entity not found: {0:?}")]
    EntityNotFound(EntityId),

    /// 指定した [`LayerId`] のレイヤーが存在しない（または既に削除済み）。
    #[error("layer not found: {0:?}")]
    LayerNotFound(LayerId),

    /// デフォルトレイヤー（レイヤー0相当）は削除できない。
    #[error("the default layer cannot be deleted")]
    CannotDeleteDefaultLayer,

    /// カレントレイヤーは削除できない（先に別レイヤーへ切り替える必要がある）。
    #[error("the current layer cannot be deleted")]
    CannotDeleteCurrentLayer,

    /// エンティティを保持しているレイヤーは削除できない。
    ///
    /// MVP ではカスケード削除（レイヤーとその中身を同時に消す）を行わず、
    /// 宙に浮いた `layer` 参照を防ぐために非空レイヤーの削除を禁止する。
    #[error("layer is not empty and cannot be deleted: {0:?}")]
    LayerNotEmpty(LayerId),

    /// 対象エンティティが所属するレイヤーがロック（[`crate::Layer::locked`]）されているため、
    /// このエンティティへの変更・削除はできない。
    #[error("layer is locked: {0:?}")]
    LayerLocked(LayerId),

    /// ジオメトリが不正（NaN/∞座標、負半径など）で追加・変更を拒否した。
    ///
    /// 判定条件は [`mcad_geom::Shape::validate`]（DESIGN.md M4 タスク15）。
    /// `Command::AddEntity` / `Command::ModifyEntity` の実行前チェックで返る。
    #[error("invalid geometry: {0}")]
    InvalidGeometry(String),

    /// 図面尺度が不正（`num`/`den` に 0 を含む、上限超過）で拒否した。
    ///
    /// 判定条件は [`crate::Scale::new`]（DESIGN.md M8 設計判断2）。
    #[error("invalid scale: {0}")]
    InvalidScale(String),

    /// 線幅が不正（0・負・非有限・許容範囲外）で拒否した。
    ///
    /// 判定条件は [`crate::WidthMm::new`]（DESIGN.md M8 設計判断5）。
    #[error("invalid line width: {0}")]
    InvalidLineWidth(String),

    /// 図面メタデータが不正（ユーザー定義の表題欄様式の寸法が非有限・非正など）で
    /// 拒否した。
    ///
    /// 判定条件は [`crate::SheetMeta::validate`]。`Command::SetSheet` の実行前
    /// チェックで返る。
    #[error("invalid sheet metadata: {0}")]
    InvalidSheet(String),
}
