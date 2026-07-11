//! エンティティ・レイヤーの識別子（`slotmap` キー型）。

slotmap::new_key_type! {
    /// [`crate::Entity`] を一意に識別するキー。
    ///
    /// `slotmap` の世代付きキーなので、ドキュメント内で安定して使える。
    /// 本クレートはドキュメント編集中に `SlotMap::remove` を呼ばない
    /// （削除はスロットの墓標化で表現する。[`crate::Document`] のモジュール解説を参照）ため、
    /// 一度発行された `EntityId` は undo/redo をまたいでも同じ値であり続ける。
    pub struct EntityId;

    /// [`crate::Layer`] を一意に識別するキー。
    ///
    /// 安定性の保証は [`EntityId`] と同じ。
    pub struct LayerId;
}
