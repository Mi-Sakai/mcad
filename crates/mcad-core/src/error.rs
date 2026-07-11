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
}
