//! レイヤー型。

use serde::{Deserialize, Serialize};

use crate::Rgb;

/// レイヤー。名前・色・表示/非表示・ロック状態・重ね順を持つ。
///
/// レイヤー自体は識別子（[`crate::LayerId`]）を持たない純粋な値であり、
/// [`crate::Document`] がキーと対応付けて保持する。ドキュメントに紐づく
/// レイヤーのプロパティ変更は [`crate::Command::SetLayerProps`] 経由で行う
/// （[`Layer::order`] も他のプロパティと同じくこの経路で変更する）。
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
    ///
    /// ロックが禁じるのは「そのレイヤー上のエンティティ」への変更であり、
    /// レイヤー属性自体（色・表示・ロック・重ね順）の変更は禁止しない。
    pub locked: bool,
    /// 重ね順（描画順）。**値が大きいほど手前**（後に描画される）。
    ///
    /// 一意性は不変条件ではない。同じ値のレイヤーは「同順位」として扱われ、
    /// [`crate::Document::layers_in_order`] が決定的な順序で解決する。値の間隔にも
    /// 意味はないため、「最前面へ」「最背面へ」のような操作は既存の最大値 + 1 /
    /// 最小値 - 1 を割り当てれば足りる（全レイヤーの詰め直しは不要）。
    ///
    /// **`serde(default)` は意図的に付けていない。** 旧 `.mcad`（`order` を持たない
    /// v1/v2）の後方互換は io 側のレイヤー専用の凍結 DTO が担っており、`order` を
    /// 欠いた入力を黙って `0` で埋めてしまうと壊れたファイルを検出できなくなる。
    pub order: i32,
}

impl Layer {
    /// 名前と色からレイヤーを作る（`visible = true`, `locked = false`, `order = 0`）。
    ///
    /// `order` の既定値が `0` なのは「重ね順を気にしない呼び出し側は全レイヤーを
    /// 同順位に置ける」ようにするため。特定の重ね順が要るときは生成後に
    /// [`Layer::order`] を書き換える（`Layer` はドキュメントへ渡す前の純粋な値なので、
    /// [`crate::Command::AddLayer`] へ渡す時点で希望の値にしておけばよい）。
    #[must_use]
    pub fn new(name: impl Into<String>, color: Rgb) -> Self {
        Self {
            name: name.into(),
            color,
            visible: true,
            locked: false,
            order: 0,
        }
    }
}
