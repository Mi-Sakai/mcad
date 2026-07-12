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

    /// 複数のサブコマンドを **1 つの操作** としてまとめて適用する複合コマンド。
    ///
    /// 矩形選択で複数エンティティをまとめて移動する、といった「ツール操作1回」で
    /// 複数の変更が生じるケースを、undo/redo の 1 単位として扱うためのもの
    /// （DESIGN 3.2「ツール操作1回 = 1コマンド」）。これがないと対象数だけ
    /// [`Command::ModifyEntity`] を個別に `apply` することになり、Ctrl+Z 1 回で
    /// 最後の 1 件しか戻らない不整合が起きる。
    ///
    /// # 原子性
    ///
    /// サブコマンドを先頭から順に検証・実行し、途中で 1 つでも失敗したら、それまでに
    /// 適用した分をすべて巻き戻して呼び出し前の状態へ完全復帰させたうえで
    /// [`crate::CoreError`] を返す（部分適用ゼロをバッチ全体へ拡張）。
    ///
    /// # undo/redo
    ///
    /// バッチは undo 履歴へ **1 記録** として積まれる。undo はサブコマンドの逆操作を
    /// 逆順に、redo は正順に適用するので、Ctrl+Z 1 回でバッチ全体が戻る。
    ///
    /// # 空バッチと意味的no-op
    ///
    /// 空の [`Command::Batch`] や、サブコマンドがすべて意味的 no-op（ネストした空
    /// [`Command::Batch`] を含む）であるバッチは「操作なし」として扱われる。
    /// [`crate::Document::apply`] はこの場合 `Ok(`[`crate::NewIds`]`::default())` を
    /// 返し、undo/redo 履歴には一切触れない。
    ///
    /// これは `Batch` に固有の扱いではなく、状態を変えないコマンド全般（例: 現在と
    /// 同じ幾何・プロパティ・レイヤーを指定する `ModifyEntity`/`SetLayerProps`/
    /// `SetCurrentLayer`）に共通する。判定基準の詳細は [`crate::Document::apply`] を参照。
    ///
    /// サブコマンドに [`Command::Batch`] を含めるネストも `apply` が再帰的に扱うため
    /// 破綻しないが、MVP で意図しているのはフラットな複数操作を 1 単位にすることである。
    Batch(Vec<Command>),
}
