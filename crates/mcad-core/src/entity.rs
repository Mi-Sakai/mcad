//! エンティティ型。

use mcad_geom::Shape;

use crate::{LayerId, Style};

/// 図形エンティティ。幾何・所属レイヤー・スタイルの組。
///
/// # serde について
///
/// `Entity` は所属レイヤーを [`LayerId`]（`slotmap` の生キー）で参照するため、
/// この型自体には `Serialize`/`Deserialize` を導出していない。生キーはファイルへ
/// そのまま書ける安定 ID ではなく、`.mcad` 保存/読込（mcad-io / DESIGN 3.3）は
/// 別途ポータブルな表現へ変換する必要がある。素の値型（[`Style`], [`crate::Layer`],
/// [`crate::Rgb`]）とジオメトリ（[`Shape`]）は個別に `Serialize` 可能。
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    /// 幾何形状。
    pub geom: Shape,
    /// 所属レイヤー。
    pub layer: LayerId,
    /// 描画スタイル。
    pub style: Style,
}

impl Entity {
    /// 各要素からエンティティを作る。
    #[must_use]
    pub fn new(geom: Shape, layer: LayerId, style: Style) -> Self {
        Self { geom, layer, style }
    }
}
