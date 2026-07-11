//! レイヤー型。

use serde::{Deserialize, Serialize};

use crate::Rgb;

/// レイヤー。名前・色・表示/非表示・ロック状態を持つ。
///
/// レイヤー自体は識別子（[`crate::LayerId`]）を持たない純粋な値であり、
/// [`crate::Document`] がキーと対応付けて保持する。ドキュメントに紐づく
/// レイヤーのプロパティ変更は [`crate::Command::SetLayerProps`] 経由で行う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    /// レイヤー名。
    pub name: String,
    /// レイヤー色。エンティティの [`crate::Style`] が色を継承する場合に使う。
    pub color: Rgb,
    /// 表示するか。`false` なら描画・ヒットテスト対象から外す想定。
    pub visible: bool,
    /// ロックされているか。`true` のレイヤー上のエンティティは追加・変更・削除が
    /// できない。この禁止は [`crate::Document`] が [`crate::Command`] 適用時に
    /// [`crate::CoreError::LayerLocked`] として強制する。
    pub locked: bool,
}

impl Layer {
    /// 名前と色からレイヤーを作る（`visible = true`, `locked = false`）。
    #[must_use]
    pub fn new(name: impl Into<String>, color: Rgb) -> Self {
        Self {
            name: name.into(),
            color,
            visible: true,
            locked: false,
        }
    }
}
