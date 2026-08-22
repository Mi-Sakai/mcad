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

use crate::dim::{DimAnnotation, DimKind};

/// テキストエンティティの幾何。フォント指定は持たない（M6 は埋め込み 1 書体のみ。
/// DESIGN.md M6 設計判断1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextGeom {
    /// 文字列の基準点（ベースライン左端）。ワールド座標。
    pub anchor: Point2,
    /// 表示する文字列。
    pub content: String,
    /// 文字高さ（紙 mm。DESIGN.md M8 設計判断4）。ワールド高さは
    /// `height * k`（`k` = `Scale::world_mm_per_paper_mm`）で導出する。既定尺度 1:1 では
    /// `k = 1` なので数値・見た目とも従来（ワールド単位解釈）と一致する。この換算は
    /// `mcad-app` 側の責務（[`EntityGeom::aabb`] は 1:1 解釈のまま。表示・選択判定用の
    /// 尺度反映 AABB は app 層の `text_world_aabb` が担う）。
    pub height: f64,
    /// ベースラインの回転角（ラジアン、CCW）。
    pub angle: f64,
}

/// 長さ寸法（非関連の静的寸法）。計測対象への参照は持たず、作成時に座標を採取する
/// （DESIGN.md M6 設計判断2）。表示値は保存せず、描画時に `|p2 − p1|` を計算する側の
/// 責務とする（点が編集されれば値も追従する）。
///
/// **`Copy` ではない**（M9 タスク47-2）: [`DimAnnotation`] が [`crate::FitClass`]（`String`）を
/// 持ちうるため。値渡しの箇所は `.clone()` へ切り替える。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimLinear {
    /// 計測点その 1。
    pub p1: Point2,
    /// 計測点その 2。
    pub p2: Point2,
    /// 計測線から寸法線までの符号付き距離（法線方向）。
    pub offset: f64,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。既定は無注記で、そのときの描画は
    /// M8 までと完全に同じ。許容記号は φ/Sφ/□/C/t（[`DimKind::Linear`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

/// 半径寸法（非関連の静的寸法）。作成時に円／円弧から中心・半径を採取する
/// （DESIGN.md M6 設計判断2）。
///
/// `Copy` でない理由は [`DimLinear`] と同じ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimRadial {
    /// 円／円弧の中心。
    pub center: Point2,
    /// 半径。
    pub radius: f64,
    /// 引出線の方向（ラジアン、中心からの放射角）。
    pub leader_angle: f64,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。許容記号は R/SR/CR
    /// （[`DimKind::Radial`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

/// 直径寸法（非関連の静的寸法。M9 設計判断3 で新設）。
///
/// 寸法線は中心を通る `angle` 方向の直径線で、矢は円周上の 2 点に立つ。半径寸法に
/// φ 記号を上書きする運用（R12 と φ24 の取り違え）を防ぐために、直径は独立した
/// 種別として持つ。
///
/// `angle` は [`DimRadial::leader_angle`] と同じ「中心からの放射角（ラジアン、CCW）」で、
/// 直径線はその方向とその逆方向の 2 点を結ぶ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimDiameter {
    /// 円／円弧の中心。
    pub center: Point2,
    /// 半径（**直径ではない**）。表示値は `2 * radius` を描画側が計算する。
    ///
    /// 半径で持つのは、採取元の [`mcad_geom::Circle`] や [`DimRadial`] と同じ量にして
    /// 相互変換・比較を素直にするため（`2r` は表示のたびに導出できる）。
    pub radius: f64,
    /// 寸法線（直径線）の方向（ラジアン、中心からの放射角）。
    pub angle: f64,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。許容記号は φ/Sφ
    /// （[`DimKind::Diameter`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

/// エンティティの幾何。[`Shape`]（既存プリミティブ）を包含しつつ、テキスト・寸法を追加する。
///
/// [`crate::Entity`] の `geom` フィールドの型であり、[`crate::Command::ModifyEntity`] の
/// `new_geom` の型でもある。幾何変換・境界ボックス・検証の各メソッドは、[`Shape`]
/// バリアントは既存の [`Shape`] へ委譲し、テキスト・寸法はそれぞれの規則で処理する。
///
/// **`#[non_exhaustive]`**（DESIGN.md M8 タスク35a）: 幾何の種別は今後も増える
/// （引出線・GPS 記号・ハッチング等）。クレート外の `match` に常にワイルドカード腕を
/// 要求しておくことで、バリアント追加が mcad-app / mcad-io / tcad の一斉コンパイル
/// エラーにならないようにする。**ワイルドカード腕は「未知の幾何を安全に無視する」
/// 実装にすること**（描画・スナップなら何もしない、集計なら数えない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EntityGeom {
    /// 既存の幾何プリミティブ（点・線・円・円弧・ポリライン）。
    Shape(Shape),
    /// テキスト。
    Text(TextGeom),
    /// 長さ寸法。
    DimLinear(DimLinear),
    /// 半径寸法。
    DimRadial(DimRadial),
    /// 直径寸法（M9 設計判断3）。
    DimDiameter(DimDiameter),
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

/// 寸法注記を幾何変換に追従させる（[`DimAnnotation::text_anchor`] へ `f` を適用する）。
///
/// 文字位置の手動上書きは**ワールド座標**なので、寸法本体を動かしたら一緒に動かさないと
/// 文字だけが元の場所へ取り残される。自動配置（`None`）はそのまま `None` を保つ。
/// 記号・公差・桁数・矢配置は座標を持たないので変換の影響を受けない。
#[inline]
fn transform_annotation(
    annotation: &DimAnnotation,
    f: impl FnOnce(Point2) -> Point2,
) -> DimAnnotation {
    DimAnnotation {
        text_anchor: annotation.text_anchor.map(f),
        ..annotation.clone()
    }
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
    ///
    /// **Text は `height` を 1:1（`k=1`）で解釈する**（`height` の紙 mm 意味論は判断4）。
    /// 尺度を反映した表示上のワールド AABB（`height * k`）は `mcad-app` の
    /// `text_world_aabb` が、この AABB を anchor 基準に `k` 倍する相似拡大として提供する
    /// （DESIGN.md M8 タスク37 実装時追記。tcad が本メソッドへ path 依存しているため、
    /// シグネチャ・実装ともここでは変更しない）。
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
            // 直径寸法も同じ近似（寸法線は中心を通る直径線なので、この箱に収まる）。
            EntityGeom::DimDiameter(dim) => {
                let r = Vec2::new(dim.radius.abs(), dim.radius.abs());
                Aabb {
                    min: dim.center - r,
                    max: dim.center + r,
                }
            }
        }
    }

    /// 変位 `delta` だけ平行移動した新しい幾何。
    ///
    /// 寸法の文字位置上書き（[`DimAnnotation::text_anchor`]）も一緒に動かす
    /// （回転・鏡映も同様。動かさないと文字だけが元の場所へ取り残される）。
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
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
            }),
            EntityGeom::DimRadial(dim) => EntityGeom::DimRadial(DimRadial {
                center: dim.center + delta,
                radius: dim.radius,
                leader_angle: dim.leader_angle,
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
            }),
            EntityGeom::DimDiameter(dim) => EntityGeom::DimDiameter(DimDiameter {
                center: dim.center + delta,
                radius: dim.radius,
                angle: dim.angle,
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
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
                annotation: transform_annotation(&dim.annotation, |p| {
                    rotate_point(p, pivot, angle)
                }),
            }),
            EntityGeom::DimRadial(dim) => EntityGeom::DimRadial(DimRadial {
                center: rotate_point(dim.center, pivot, angle),
                radius: dim.radius,
                leader_angle: dim.leader_angle + angle,
                annotation: transform_annotation(&dim.annotation, |p| {
                    rotate_point(p, pivot, angle)
                }),
            }),
            // 直径線の向き `angle` は [`DimRadial::leader_angle`] と同じ扱い（回転量を
            // 加算して図形と一緒に回す）。加算しないと図形だけが回って寸法線の向きが
            // 取り残される。
            EntityGeom::DimDiameter(dim) => EntityGeom::DimDiameter(DimDiameter {
                center: rotate_point(dim.center, pivot, angle),
                radius: dim.radius,
                angle: dim.angle + angle,
                annotation: transform_annotation(&dim.annotation, |p| {
                    rotate_point(p, pivot, angle)
                }),
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
                annotation: transform_annotation(&dim.annotation, |p| {
                    mirror_point(p, axis_a, axis_b)
                }),
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
                    annotation: transform_annotation(&dim.annotation, |p| {
                        mirror_point(p, axis_a, axis_b)
                    }),
                })
            }
            // 直径線の向きは半径寸法の引出方向と同じ規則で鏡映する（退化軸では角度が
            // 定まらないため元の角度を保つ）。
            EntityGeom::DimDiameter(dim) => {
                let axis = axis_b - axis_a;
                let angle = match axis.normalize() {
                    Some(_) => 2.0 * axis.angle() - dim.angle,
                    None => dim.angle,
                };
                EntityGeom::DimDiameter(DimDiameter {
                    center: mirror_point(dim.center, axis_a, axis_b),
                    radius: dim.radius,
                    angle,
                    annotation: transform_annotation(&dim.annotation, |p| {
                        mirror_point(p, axis_a, axis_b)
                    }),
                })
            }
        }
    }

    /// 幾何が妥当か検証する。
    ///
    /// [`Shape`] は [`Shape::validate`] へ委譲する。テキストは空文字列・非正の高さ・
    /// 非有限値を、寸法は非有限値（半径・直径寸法は加えて非正半径）を拒否する
    /// （[`Shape::validate`] と同じ境界基準。DESIGN.md M6 設計判断1）。
    ///
    /// **寸法は加えて [`DimAnnotation::validate`] を通す**（M9 設計判断2）。これが
    /// 「種別に許されない記号・`upper < lower`・不正なはめあい記号・非有限値を
    /// **コマンド境界で**拒否する」実体で、[`crate::Document::apply`] の
    /// `AddEntity` / `ModifyEntity` と `.mcad` 読込の双方がこのメソッドを通る。
    /// 注記側のエラーは [`crate::CoreError::InvalidDimAnnotation`] だが、幾何検証の契約
    /// （`Shape::validate` と揃えた `String`）に合わせてここでは文字列化する
    /// （`Document::apply` は [`crate::CoreError::InvalidGeometry`] として返す）。
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
                check_annotation(&dim.annotation, DimKind::Linear)
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
                check_annotation(&dim.annotation, DimKind::Radial)
            }
            EntityGeom::DimDiameter(dim) => {
                if !finite_pt(dim.center) {
                    return Err("non-finite diameter dimension center".into());
                }
                if !dim.radius.is_finite() || dim.radius <= 0.0 {
                    return Err(format!("invalid diameter dimension radius: {}", dim.radius));
                }
                if !dim.angle.is_finite() {
                    return Err("non-finite diameter dimension angle".into());
                }
                check_annotation(&dim.annotation, DimKind::Diameter)
            }
        }
    }
}

/// 寸法注記を検証し、[`EntityGeom::validate`] の契約（人が読める `String`）へ合わせる。
///
/// [`crate::CoreError`] の `Display` をそのまま使うので、理由の文言は注記側の検証と
/// 一致する（メッセージの二重管理をしない）。
fn check_annotation(annotation: &DimAnnotation, kind: DimKind) -> Result<(), String> {
    annotation.validate(kind).map_err(|e| e.to_string())
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
    use crate::dim::{ArrowPlacement, FitClass, SizeTolerance};
    use mcad_geom::{DimSymbol, LineSeg, Shape};
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
            annotation: DimAnnotation::default(),
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
            annotation: DimAnnotation::default(),
        });
        assert!(ok.validate().is_ok());
        let bad = EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(1.0, 0.0),
            offset: f64::INFINITY,
            annotation: DimAnnotation::default(),
        });
        assert!(bad.validate().is_err());
    }

    #[test]
    fn dim_radial_rotate_and_validate() {
        let dim = DimRadial {
            center: Point2::new(1.0, 0.0),
            radius: 3.0,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        };
        let g = EntityGeom::DimRadial(dim.clone());

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

    // -----------------------------------------------------------------
    // M9 タスク47-2: 注記つき寸法と直径寸法
    // -----------------------------------------------------------------

    fn linear(offset: f64) -> DimLinear {
        DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset,
            annotation: DimAnnotation::default(),
        }
    }

    fn diameter() -> DimDiameter {
        DimDiameter {
            center: Point2::new(1.0, 0.0),
            radius: 3.0,
            angle: 0.0,
            annotation: DimAnnotation::default(),
        }
    }

    /// 文字位置を上書きした注記（幾何変換への追従を見るため）。
    fn anchored(at: Point2) -> DimAnnotation {
        DimAnnotation {
            text_anchor: Some(at),
            ..DimAnnotation::default()
        }
    }

    #[test]
    fn dim_diameter_translate_rotate_mirror() {
        let g = EntityGeom::DimDiameter(diameter());

        let EntityGeom::DimDiameter(t) = g.translated(Vec2::new(2.0, -1.0)) else {
            panic!();
        };
        assert!(approx(t.center, Point2::new(3.0, -1.0)));
        assert!((t.radius - 3.0).abs() < T);
        assert!((t.angle - 0.0).abs() < T);

        // 回転: 中心が動き、直径線の向きにも回転量が乗る（半径寸法と同じ規則）。
        let EntityGeom::DimDiameter(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!(approx(r.center, Point2::new(0.0, 1.0)));
        assert!((r.angle - FRAC_PI_2).abs() < T);
        assert!((r.radius - 3.0).abs() < T);

        // y 軸鏡映: alpha = π/2 → 2·alpha − angle = π。中心の x が反転する。
        let EntityGeom::DimDiameter(m) = g.mirrored(Point2::ORIGIN, Point2::new(0.0, 1.0)) else {
            panic!();
        };
        assert!(approx(m.center, Point2::new(-1.0, 0.0)));
        assert!((m.angle - PI).abs() < T);

        // 退化軸（2 点が同一）では角度が定まらないため元の角度を保つ。
        let EntityGeom::DimDiameter(d) = g.mirrored(Point2::ORIGIN, Point2::ORIGIN) else {
            panic!();
        };
        assert!((d.angle - 0.0).abs() < T);
    }

    #[test]
    fn dim_diameter_aabb_is_the_circle_box() {
        let bb = EntityGeom::DimDiameter(diameter()).aabb();
        assert!(approx(bb.min, Point2::new(-2.0, -3.0)));
        assert!(approx(bb.max, Point2::new(4.0, 3.0)));
    }

    #[test]
    fn dim_diameter_validate_rejects_bad_geometry_and_symbols() {
        assert!(EntityGeom::DimDiameter(diameter()).validate().is_ok());

        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let g = EntityGeom::DimDiameter(DimDiameter {
                radius: bad,
                ..diameter()
            });
            assert!(g.validate().is_err(), "radius {bad} should be rejected");
        }
        assert!(
            EntityGeom::DimDiameter(DimDiameter {
                center: Point2::new(f64::NAN, 0.0),
                ..diameter()
            })
            .validate()
            .is_err()
        );
        assert!(
            EntityGeom::DimDiameter(DimDiameter {
                angle: f64::INFINITY,
                ..diameter()
            })
            .validate()
            .is_err()
        );

        // φ・Sφ は許可、R は不可（DimKind::Diameter の文法）。
        for (symbol, allowed) in [
            (DimSymbol::Diameter, true),
            (DimSymbol::SphereDiameter, true),
            (DimSymbol::Radius, false),
            (DimSymbol::Square, false),
        ] {
            let g = EntityGeom::DimDiameter(DimDiameter {
                annotation: DimAnnotation {
                    symbol: Some(symbol),
                    ..DimAnnotation::default()
                },
                ..diameter()
            });
            assert_eq!(g.validate().is_ok(), allowed, "{symbol:?}");
        }
    }

    #[test]
    fn validate_enforces_the_symbol_grammar_per_dimension_kind() {
        // 長さ寸法に SR（半径系）は不可。半径寸法に φ は不可。
        let linear_sr = EntityGeom::DimLinear(DimLinear {
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::SphereRadius),
                ..DimAnnotation::default()
            },
            ..linear(1.0)
        });
        let err = linear_sr.validate().unwrap_err();
        assert!(err.contains("SphereRadius"), "{err}");

        let radial_phi = EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 1.0,
            leader_angle: 0.0,
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::Diameter),
                ..DimAnnotation::default()
            },
        });
        assert!(radial_phi.validate().is_err());

        // 許可される組合せは通る（長さ寸法の t、半径寸法の CR）。
        let linear_t = EntityGeom::DimLinear(DimLinear {
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::Thickness),
                ..DimAnnotation::default()
            },
            ..linear(1.0)
        });
        assert!(linear_t.validate().is_ok());
        let radial_cr = EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 1.0,
            leader_angle: 0.0,
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::ControlRadius),
                ..DimAnnotation::default()
            },
        });
        assert!(radial_cr.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_tolerance_on_every_dimension_kind() {
        let bad = DimAnnotation {
            tolerance: Some(SizeTolerance::Deviations {
                upper: -0.2,
                lower: 0.1,
            }),
            ..DimAnnotation::default()
        };
        let cases = [
            EntityGeom::DimLinear(DimLinear {
                annotation: bad.clone(),
                ..linear(1.0)
            }),
            EntityGeom::DimRadial(DimRadial {
                center: Point2::ORIGIN,
                radius: 1.0,
                leader_angle: 0.0,
                annotation: bad.clone(),
            }),
            EntityGeom::DimDiameter(DimDiameter {
                annotation: bad,
                ..diameter()
            }),
        ];
        for g in cases {
            assert!(g.validate().is_err(), "{g:?}");
        }
    }

    #[test]
    fn text_anchor_follows_the_geometry_transform() {
        // 文字位置の手動上書きはワールド座標なので、寸法本体と一緒に動く
        // （動かないと移動・回転で文字だけ取り残される）。
        let g = EntityGeom::DimLinear(DimLinear {
            annotation: anchored(Point2::new(2.0, 2.0)),
            ..linear(1.5)
        });

        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(1.0, 1.0)) else {
            panic!();
        };
        assert!(approx(
            t.annotation.text_anchor.unwrap(),
            Point2::new(3.0, 3.0)
        ));

        let EntityGeom::DimLinear(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!(approx(
            r.annotation.text_anchor.unwrap(),
            Point2::new(-2.0, 2.0)
        ));

        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!();
        };
        assert!(approx(
            m.annotation.text_anchor.unwrap(),
            Point2::new(2.0, -2.0)
        ));

        // 半径・直径寸法でも同じ（自動配置の `None` は `None` のまま）。
        let radial = EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 1.0,
            leader_angle: 0.0,
            annotation: anchored(Point2::new(1.0, 0.0)),
        });
        let EntityGeom::DimRadial(rt) = radial.translated(Vec2::new(0.0, 5.0)) else {
            panic!();
        };
        assert!(approx(
            rt.annotation.text_anchor.unwrap(),
            Point2::new(1.0, 5.0)
        ));

        let auto = EntityGeom::DimDiameter(diameter());
        let EntityGeom::DimDiameter(at) = auto.translated(Vec2::new(1.0, 1.0)) else {
            panic!();
        };
        assert_eq!(at.annotation.text_anchor, None);
    }

    #[test]
    fn transforms_keep_the_non_geometric_annotation_fields() {
        let annotation = DimAnnotation {
            symbol: Some(DimSymbol::Diameter),
            tolerance: Some(SizeTolerance::Symmetric(0.1)),
            decimals_override: Some(3),
            text_anchor: Some(Point2::new(1.0, 1.0)),
            arrow_placement: ArrowPlacement::Outside,
        };
        let g = EntityGeom::DimLinear(DimLinear {
            annotation: annotation.clone(),
            ..linear(1.0)
        });
        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(3.0, 0.0)) else {
            panic!();
        };
        assert_eq!(t.annotation.symbol, annotation.symbol);
        assert_eq!(t.annotation.tolerance, annotation.tolerance);
        assert_eq!(t.annotation.decimals_override, annotation.decimals_override);
        assert_eq!(t.annotation.arrow_placement, annotation.arrow_placement);
    }

    // -----------------------------------------------------------------
    // 保存形式の後方互換（v4 との JSON 互換。詳細な .mcad 往復は io 層）
    // -----------------------------------------------------------------

    #[test]
    fn unannotated_dimensions_serialize_exactly_like_v4() {
        // 無注記なら `annotation` フィールドごと出力されない ＝ M8 までの JSON と同一。
        let json = serde_json::to_string(&EntityGeom::DimLinear(linear(1.5))).unwrap();
        assert_eq!(
            json,
            r#"{"DimLinear":{"p1":{"x":0.0,"y":0.0},"p2":{"x":4.0,"y":0.0},"offset":1.5}}"#
        );

        let json = serde_json::to_string(&EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 2.0,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        }))
        .unwrap();
        assert_eq!(
            json,
            r#"{"DimRadial":{"center":{"x":0.0,"y":0.0},"radius":2.0,"leader_angle":0.0}}"#
        );
    }

    #[test]
    fn v4_json_without_annotation_loads_as_unannotated() {
        let parsed: EntityGeom = serde_json::from_str(
            r#"{"DimLinear":{"p1":{"x":0.0,"y":0.0},"p2":{"x":4.0,"y":0.0},"offset":1.5}}"#,
        )
        .unwrap();
        assert_eq!(parsed, EntityGeom::DimLinear(linear(1.5)));

        let parsed: EntityGeom = serde_json::from_str(
            r#"{"DimRadial":{"center":{"x":0.0,"y":0.0},"radius":2.0,"leader_angle":0.0}}"#,
        )
        .unwrap();
        let EntityGeom::DimRadial(dim) = &parsed else {
            panic!();
        };
        assert!(dim.annotation.is_unannotated());
    }

    #[test]
    fn annotated_dimensions_round_trip_through_serde() {
        let g = EntityGeom::DimDiameter(DimDiameter {
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::SphereDiameter),
                tolerance: Some(SizeTolerance::Fit(FitClass::new("H7").unwrap())),
                decimals_override: Some(1),
                text_anchor: Some(Point2::new(1.0, 2.0)),
                arrow_placement: ArrowPlacement::Inside,
            },
            ..diameter()
        });
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("annotation"), "{json}");
        assert_eq!(serde_json::from_str::<EntityGeom>(&json).unwrap(), g);
    }
}
