//! エンティティ型。

use crate::{EntityGeom, LayerId, Style};

/// 図形エンティティ。幾何・所属レイヤー・スタイルの組。
///
/// # serde について
///
/// `Entity` は所属レイヤーを [`LayerId`]（`slotmap` の生キー）で参照するため、
/// この型自体には `Serialize`/`Deserialize` を導出していない。生キーはファイルへ
/// そのまま書ける安定 ID ではなく、`.mcad` 保存/読込（mcad-io / DESIGN 3.3）は
/// 別途ポータブルな表現へ変換する必要がある。素の値型（[`Style`], [`crate::Layer`],
/// [`crate::Rgb`]）とジオメトリ（[`EntityGeom`]）は個別に `Serialize` 可能。
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    /// 幾何形状。
    pub geom: EntityGeom,
    /// 所属レイヤー。
    pub layer: LayerId,
    /// 描画スタイル。
    pub style: Style,
}

impl Entity {
    /// 各要素からエンティティを作る。
    ///
    /// `geom` は [`EntityGeom`] へ変換できる型を受け取る。[`mcad_geom::Shape`] は
    /// [`From<Shape>`](EntityGeom) 実装により暗黙変換されるため、Shape 系エンティティを
    /// 作る既存コードは `Entity::new(shape, ..)` のまま変更不要。
    #[must_use]
    pub fn new(geom: impl Into<EntityGeom>, layer: LayerId, style: Style) -> Self {
        Self {
            geom: geom.into(),
            layer,
            style,
        }
    }
}
