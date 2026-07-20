//! エンティティの幾何を表す [`EntityGeom`]（M6「テキストと寸法」で新設）。
//!
//! # 設計方針（DESIGN.md M6）
//!
//! MVP の [`mcad_geom::Shape`]（点・線・円・円弧・ポリライン）に加えて、テキストと
//! 寸法（長さ・半径）を扱えるようにする層。mcad-geom の [`Shape`] は **変更しない**
//! （`Shape` へバリアントを足すと mcad 内の網羅 match と tcad の `EntityKind::of()` を
//! 直接壊すため。DESIGN.md M6 設計判断1）。代わりに mcad-core 側で `Shape` を包含する
//! [`EntityGeom`] を定義し、[`crate::Entity`] の幾何をこの型で持つ。
//!
//! # 幾何変換の規則
//!
//! [`Shape`] バリアントは既存の [`Shape`] のメソッドへそのまま委譲する。テキスト・寸法は
//! それぞれの意味に沿って変換する（各メソッドの doc 参照）。特に **テキストのミラーは
//! アンカーのみ鏡映し、文字グリフは反転しない**（グリフ反転は egui では不可能。
//! AutoCAD の MIRRTEXT=0 相当。DESIGN.md M6 設計判断1）。

use serde::{Deserialize, Serialize};

use mcad_geom::{Aabb, Point2, Shape, Vec2};

/// テキストエンティティの幾何。フォント指定は持たない（M6 は埋め込み 1 書体のみ。
/// DESIGN.md M6 設計判断1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextGeom {
    /// 文字列の基準点（ベースライン左端）。ワールド座標。
    pub anchor: Point2,
    /// 表示する文字列。
    pub content: String,
    /// 文字高さ（ワールド単位）。
    pub height: f64,
    /// ベースラインの回転角（ラジアン、CCW）。
    pub angle: f64,
}

/// 長さ寸法（非関連の静的寸法）。計測対象への参照は持たず、作成時に座標を採取する
/// （DESIGN.md M6 設計判断2）。表示値は保存せず、描画時に `|p2 − p1|` を計算する側の
/// 責務とする（点が編集されれば値も追従する）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DimLinear {
    /// 計測点その 1。
    pub p1: Point2,
    /// 計測点その 2。
    pub p2: Point2,
    /// 計測線から寸法線までの符号付き距離（法線方向）。
    pub offset: f64,
}

/// 半径寸法（非関連の静的寸法）。作成時に円／円弧から中心・半径を採取する
/// （DESIGN.md M6 設計判断2）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DimRadial {
    /// 円／円弧の中心。
    pub center: Point2,
    /// 半径。
    pub radius: f64,
    /// 引出線の方向（ラジアン、中心からの放射角）。
    pub leader_angle: f64,
}

/// エンティティの幾何。[`Shape`]（既存プリミティブ）を包含しつつ、テキスト・寸法を追加する。
///
/// [`crate::Entity`] の `geom` フィールドの型であり、[`crate::Command::ModifyEntity`] の
/// `new_geom` の型でもある。幾何変換・境界ボックス・検証の各メソッドは、[`Shape`]
/// バリアントは既存の [`Shape`] へ委譲し、テキスト・寸法はそれぞれの規則で処理する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EntityGeom {
    /// 既存の幾何プリミティブ（点・線・円・円弧・ポリライン）。
    Shape(Shape),
    /// テキスト。
    Text(TextGeom),
    /// 長さ寸法。
    DimLinear(DimLinear),
    /// 半径寸法。
    DimRadial(DimRadial),
}

/// 点 `p` を `pivot` を中心に CCW へ `angle` ラジアン回転する（公開 Vec2/Point2 API で組む）。
#[inline]
fn rotate_point(p: Point2, pivot: Point2, angle: f64) -> Point2 {
    pivot + (p - pivot).rotated(angle)
}

/// 点 `p` を `axis_a`→`axis_b` を通る直線に対して鏡映する。
#[inline]
fn mirror_point(p: Point2, axis_a: Point2, axis_b: Point2) -> Point2 {
    axis_a + (p - axis_a).reflected(axis_b - axis_a)
}

impl From<Shape> for EntityGeom {
    /// 既存コード（Shape のみを作る作図ツール・ファイル入出力）との互換のため、
    /// `Shape` から [`EntityGeom::Shape`] への変換を提供する。
    #[inline]
    fn from(shape: Shape) -> Self {
        EntityGeom::Shape(shape)
    }
}

impl EntityGeom {
    /// [`EntityGeom::Shape`] のとき内側の [`Shape`] を借用する。それ以外は `None`。
    ///
    /// テキスト・寸法にまだ対応していない Shape 専用の経路（描画・スナップ・オフセット・
    /// DXF export など）が、機械的に既存ロジックへ委譲するために使う。
    #[inline]
    #[must_use]
    pub fn as_shape(&self) -> Option<&Shape> {
        match self {
            EntityGeom::Shape(shape) => Some(shape),
            _ => None,
        }
    }

    /// 軸並行境界ボックス。
    ///
    /// テキストの境界は文字数×高さの **近似**（CJK≈1.0×height、ASCII≈0.55×height）で、
    /// zoom fit 用途を想定する。正確な選択判定は app 層の責務（DESIGN.md M6 設計判断1）。
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        match self {
            EntityGeom::Shape(shape) => shape.aabb(),
            EntityGeom::Text(text) => text_aabb(text),
            EntityGeom::DimLinear(dim) => dim_linear_aabb(dim),
            EntityGeom::DimRadial(dim) => {
                // 計測対象の円／円弧を包む近似ボックス（中心 ± 半径）。引出線・文字は
                // その内外へ僅かに出るが、zoom fit 用途では十分な近似とする。
                let r = Vec2::new(dim.radius.abs(), dim.radius.abs());
                Aabb {
                    min: dim.center - r,
                    max: dim.center + r,
                }
            }
        }
    }

    /// 変位 `delta` だけ平行移動した新しい幾何。
    #[must_use]
    pub fn translated(&self, delta: Vec2) -> EntityGeom {
        match self {
            EntityGeom::Shape(shape) => EntityGeom::Shape(shape.translated(delta)),
            EntityGeom::Text(text) => EntityGeom::Text(TextGeom {
                anchor: text.anchor + delta,
                ..text.clone()
            }),
            EntityGeom::DimLinear(dim) => EntityGeom::DimLinear(DimLinear {
                p1: dim.p1 + delta,
                p2: dim.p2 + delta,
                offset: dim.offset,
            }),
            EntityGeom::DimRadial(dim) => EntityGeom::DimRadial(DimRadial {
                center: dim.center + delta,
                ..*dim
            }),
        }
    }

    /// `pivot` を中心に CCW へ `angle` ラジアン回転した新しい幾何。
    ///
    /// テキストはアンカーを回転し、ベースライン角へ `angle` を加える。
    #[must_use]
    pub fn rotated(&self, pivot: Point2, angle: f64) -> EntityGeom {
        match self {
            EntityGeom::Shape(shape) => EntityGeom::Shape(shape.rotated(pivot, angle)),
            EntityGeom::Text(text) => EntityGeom::Text(TextGeom {
                anchor: rotate_point(text.anchor, pivot, angle),
                angle: text.angle + angle,
                ..text.clone()
            }),
            EntityGeom::DimLinear(dim) => EntityGeom::DimLinear(DimLinear {
                p1: rotate_point(dim.p1, pivot, angle),
                p2: rotate_point(dim.p2, pivot, angle),
                offset: dim.offset,
            }),
            EntityGeom::DimRadial(dim) => EntityGeom::DimRadial(DimRadial {
                center: rotate_point(dim.center, pivot, angle),
                radius: dim.radius,
                leader_angle: dim.leader_angle + angle,
            }),
        }
    }

    /// `axis_a`→`axis_b` を通る直線に対して鏡映した新しい幾何。
    ///
    /// **テキストはアンカーのみ鏡映し、文字グリフは反転しない**（AutoCAD の MIRRTEXT=0
    /// 相当。DESIGN.md M6 設計判断1）。ベースライン角は方向ベクトルを軸に対して鏡映した
    /// 角度（`2·alpha − angle`、`alpha` は軸の方向角）にする。退化軸（2 点がほぼ同一）では
    /// 角度が定まらないためアンカーのみ鏡映し角度は保つ。
    #[must_use]
    pub fn mirrored(&self, axis_a: Point2, axis_b: Point2) -> EntityGeom {
        match self {
            EntityGeom::Shape(shape) => EntityGeom::Shape(shape.mirrored(axis_a, axis_b)),
            EntityGeom::Text(text) => {
                let axis = axis_b - axis_a;
                let angle = match axis.normalize() {
                    // 軸方向角 alpha に対し、方向ベクトルの反射角は 2·alpha − angle。
                    Some(_) => 2.0 * axis.angle() - text.angle,
                    // 退化軸では角度が定まらないため元の角度を保つ。
                    None => text.angle,
                };
                EntityGeom::Text(TextGeom {
                    anchor: mirror_point(text.anchor, axis_a, axis_b),
                    angle,
                    ..text.clone()
                })
            }
            EntityGeom::DimLinear(dim) => EntityGeom::DimLinear(DimLinear {
                p1: mirror_point(dim.p1, axis_a, axis_b),
                p2: mirror_point(dim.p2, axis_a, axis_b),
                // 符号付きオフセットは鏡映で向きが反転する。
                offset: -dim.offset,
            }),
            EntityGeom::DimRadial(dim) => {
                let axis = axis_b - axis_a;
                let leader_angle = match axis.normalize() {
                    Some(_) => 2.0 * axis.angle() - dim.leader_angle,
                    None => dim.leader_angle,
                };
                EntityGeom::DimRadial(DimRadial {
                    center: mirror_point(dim.center, axis_a, axis_b),
                    radius: dim.radius,
                    leader_angle,
                })
            }
        }
    }

    /// 幾何が妥当か検証する。
    ///
    /// [`Shape`] は [`Shape::validate`] へ委譲する。テキストは空文字列・非正の高さ・
    /// 非有限値を、寸法は非有限値（半径寸法は加えて非正半径）を拒否する
    /// （[`Shape::validate`] と同じ境界基準。DESIGN.md M6 設計判断1）。
    ///
    /// # Errors
    ///
    /// 不正な理由を人が読める `String` で返す。
    pub fn validate(&self) -> Result<(), String> {
        let finite_pt = |p: Point2| p.x.is_finite() && p.y.is_finite();
        match self {
            EntityGeom::Shape(shape) => shape.validate(),
            EntityGeom::Text(text) => {
                if text.content.is_empty() {
                    return Err("empty text content".into());
                }
                if !finite_pt(text.anchor) {
                    return Err("non-finite text anchor".into());
                }
                if !text.height.is_finite() || text.height <= 0.0 {
                    return Err(format!("invalid text height: {}", text.height));
                }
                if !text.angle.is_finite() {
                    return Err("non-finite text angle".into());
                }
                Ok(())
            }
            EntityGeom::DimLinear(dim) => {
                if !finite_pt(dim.p1) || !finite_pt(dim.p2) {
                    return Err("non-finite linear dimension points".into());
                }
                if !dim.offset.is_finite() {
                    return Err("non-finite linear dimension offset".into());
                }
                Ok(())
            }
            EntityGeom::DimRadial(dim) => {
                if !finite_pt(dim.center) {
                    return Err("non-finite radial dimension center".into());
                }
                if !dim.radius.is_finite() || dim.radius <= 0.0 {
                    return Err(format!("invalid radial dimension radius: {}", dim.radius));
                }
                if !dim.leader_angle.is_finite() {
                    return Err("non-finite radial dimension leader angle".into());
                }
                Ok(())
            }
        }
    }
}

/// テキストの近似 AABB。文字数×高さの近似幅（CJK≈1.0×height、ASCII≈0.55×height）で
/// 局所ボックスを組み、ベースライン角で回転した 4 隅を包む（DESIGN.md M6 設計判断1）。
fn text_aabb(text: &TextGeom) -> Aabb {
    let width: f64 = text
        .content
        .chars()
        .map(|c| if c.is_ascii() { 0.55 } else { 1.0 } * text.height)
        .sum();
    // 局所座標（アンカー原点、ベースライン +x、上方向 +y）の 4 隅を回転して包む。
    let corners = [
        Vec2::new(0.0, 0.0),
        Vec2::new(width, 0.0),
        Vec2::new(width, text.height),
        Vec2::new(0.0, text.height),
    ];
    let pts = corners
        .into_iter()
        .map(|c| text.anchor + c.rotated(text.angle));
    Aabb::from_points(pts).unwrap_or_else(|| Aabb::from_point(text.anchor))
}

/// 長さ寸法の AABB。計測 2 点と、そこから法線方向へ `offset` ずらした寸法線 2 点を包む。
fn dim_linear_aabb(dim: &DimLinear) -> Aabb {
    let base = Aabb::from_corners(dim.p1, dim.p2);
    match (dim.p2 - dim.p1).perp().normalize() {
        Some(normal) => {
            let shift = normal * dim.offset;
            base.extended(dim.p1 + shift).extended(dim.p2 + shift)
        }
        // 計測 2 点がほぼ同一で法線が定まらない場合は 2 点のみのボックスを返す。
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_geom::{LineSeg, Shape};
    use std::f64::consts::{FRAC_PI_2, PI};

    const T: f64 = 1e-9;

    fn text(anchor: Point2, angle: f64) -> TextGeom {
        TextGeom {
            anchor,
            content: "Ab".into(),
            height: 2.0,
            angle,
        }
    }

    fn approx(a: Point2, b: Point2) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn from_shape_and_as_shape_roundtrip() {
        let shape = Shape::Point(Point2::new(1.0, 2.0));
        let geom: EntityGeom = shape.clone().into();
        assert_eq!(geom, EntityGeom::Shape(shape.clone()));
        assert_eq!(geom.as_shape(), Some(&shape));
        assert_eq!(EntityGeom::Text(text(Point2::ORIGIN, 0.0)).as_shape(), None);
    }

    #[test]
    fn shape_variant_delegates_to_shape() {
        // Shape バリアントは既存 Shape の同名メソッドへ委譲する。
        let shape = Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0)));
        let geom = EntityGeom::Shape(shape.clone());
        let d = Vec2::new(1.0, 2.0);
        assert_eq!(geom.translated(d), EntityGeom::Shape(shape.translated(d)));
        assert_eq!(geom.aabb().min, shape.aabb().min);
    }

    #[test]
    fn text_translate_keeps_angle_moves_anchor() {
        let g = EntityGeom::Text(text(Point2::new(1.0, 1.0), 0.3));
        let EntityGeom::Text(t) = g.translated(Vec2::new(2.0, -3.0)) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(3.0, -2.0)));
        assert!((t.angle - 0.3).abs() < T);
    }

    #[test]
    fn text_rotate_moves_anchor_and_adds_angle() {
        // anchor (1,0)、角 0 を原点まわり +π/2 → anchor (0,1)、角 π/2。
        let g = EntityGeom::Text(text(Point2::new(1.0, 0.0), 0.0));
        let EntityGeom::Text(t) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(0.0, 1.0)));
        assert!((t.angle - FRAC_PI_2).abs() < T);
    }

    #[test]
    fn text_mirror_across_x_axis_keeps_horizontal_baseline() {
        // x 軸鏡映: alpha=0 → new_angle = -angle。角 0 の水平ベースラインは角 0 のまま、
        // anchor の y だけ反転する（グリフは反転しない = 角度が水平を保つ）。
        let g = EntityGeom::Text(text(Point2::new(3.0, 2.0), 0.0));
        let EntityGeom::Text(t) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(3.0, -2.0)));
        assert!((t.angle - 0.0).abs() < T);
    }

    #[test]
    fn text_mirror_across_y_axis_flips_baseline_direction() {
        // y 軸鏡映: alpha=π/2 → new_angle = π − angle。右向き(0)ベースラインは左向き(π)へ。
        let g = EntityGeom::Text(text(Point2::new(1.0, 5.0), 0.0));
        let EntityGeom::Text(t) = g.mirrored(Point2::ORIGIN, Point2::new(0.0, 1.0)) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(-1.0, 5.0)));
        assert!((t.angle - PI).abs() < T);
    }

    #[test]
    fn text_validate_rejects_empty_and_bad_height() {
        let ok = EntityGeom::Text(text(Point2::ORIGIN, 0.0));
        assert!(ok.validate().is_ok());

        let empty = EntityGeom::Text(TextGeom {
            content: String::new(),
            ..text(Point2::ORIGIN, 0.0)
        });
        assert!(empty.validate().is_err());

        let zero_h = EntityGeom::Text(TextGeom {
            height: 0.0,
            ..text(Point2::ORIGIN, 0.0)
        });
        assert!(zero_h.validate().is_err());

        let nan = EntityGeom::Text(text(Point2::new(f64::NAN, 0.0), 0.0));
        assert!(nan.validate().is_err());
    }

    #[test]
    fn text_aabb_is_finite_and_contains_anchor_row() {
        let g = EntityGeom::Text(text(Point2::new(0.0, 0.0), 0.0));
        let bb = g.aabb();
        assert!(bb.min.x.is_finite() && bb.max.x.is_finite());
        // 高さ 2、2 文字（ASCII 0.55×2 ×2 = 2.2 幅）。
        assert!((bb.min.y - 0.0).abs() < T);
        assert!((bb.max.y - 2.0).abs() < T);
        assert!(bb.max.x > 2.0 && bb.max.x < 2.5);
    }

    #[test]
    fn dim_linear_transforms_and_offset_sign() {
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 1.5,
        };
        let g = EntityGeom::DimLinear(dim);

        // 平行移動: 2 点が動き offset は不変。
        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(1.0, 1.0)) else {
            panic!();
        };
        assert!(approx(t.p1, Point2::new(1.0, 1.0)));
        assert!(approx(t.p2, Point2::new(5.0, 1.0)));
        assert!((t.offset - 1.5).abs() < T);

        // x 軸鏡映: 符号付き offset が反転する。
        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!();
        };
        assert!((m.offset + 1.5).abs() < T);
    }

    #[test]
    fn dim_linear_validate_rejects_non_finite() {
        let ok = EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(1.0, 0.0),
            offset: 0.0,
        });
        assert!(ok.validate().is_ok());
        let bad = EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(1.0, 0.0),
            offset: f64::INFINITY,
        });
        assert!(bad.validate().is_err());
    }

    #[test]
    fn dim_radial_rotate_and_validate() {
        let dim = DimRadial {
            center: Point2::new(1.0, 0.0),
            radius: 3.0,
            leader_angle: 0.0,
        };
        let g = EntityGeom::DimRadial(dim);

        // 回転: 中心が動き leader_angle に角が加わる。半径は不変。
        let EntityGeom::DimRadial(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!(approx(r.center, Point2::new(0.0, 1.0)));
        assert!((r.leader_angle - FRAC_PI_2).abs() < T);
        assert!((r.radius - 3.0).abs() < T);

        // validate: 非正半径は拒否。
        let bad = EntityGeom::DimRadial(DimRadial { radius: 0.0, ..dim });
        assert!(bad.validate().is_err());
        assert!(g.validate().is_ok());
    }
}
