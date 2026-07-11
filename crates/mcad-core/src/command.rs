//! ドキュメントを変更する唯一の手段である [`Command`]。

use mcad_geom::Shape;

use crate::{Entity, EntityId, Layer, LayerId};

/// ドキュメントへの変更を表すコマンド。
///
/// [`crate::Document`] の状態変更はすべて [`crate::Document::apply`] にこの型を
/// 渡す形で行う（フィールドを直接触らせない）。`apply` はコマンドを実行しつつ、
/// 逆操作に必要な情報を内部の undo 履歴へ積む。
///
/// # 「ツール操作1回 = 1コマンド」
///
/// ドラッグなどの連続操作は、確定時の最終状態を 1 コマンドとして表現する。
/// たとえば移動は「移動後の幾何」を持つ [`Command::ModifyEntity`] 1 回で表し、
/// ドラッグ中の中間状態は履歴へ積まない。[`Command::ModifyEntity`] が変更前の
/// 幾何を保持しないのはこのためで、`apply` が実行時に現在の幾何を記録して逆操作を
/// 構成する（DESIGN 3.2 の `Modify { before, after }` の before は内部生成する）。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// エンティティを追加する。
    AddEntity(Entity),

    /// エンティティを削除する。
    RemoveEntity(EntityId),

    /// エンティティの幾何を差し替える（移動・変形などの確定操作）。
    ModifyEntity {
        /// 対象エンティティ。
        id: EntityId,
        /// 差し替え後の幾何。
        new_geom: Shape,
    },

    /// レイヤーを追加する。
    AddLayer(Layer),

    /// レイヤーを削除する。
    ///
    /// デフォルトレイヤー・カレントレイヤー・非空レイヤーは削除できず、
    /// それぞれ [`crate::CoreError`] を返す。
    RemoveLayer(LayerId),

    /// レイヤーのプロパティ（名前・色・表示・ロック）をまとめて差し替える。
    SetLayerProps {
        /// 対象レイヤー。
        id: LayerId,
        /// 差し替え後のプロパティ一式。
        props: Layer,
    },

    /// カレントレイヤーを切り替える。
    SetCurrentLayer(LayerId),
}
