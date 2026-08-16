//! 寸法記号（φ・Sφ・□・R・SR・CR・C・t）。
//!
//! [`DimSymbol`] は寸法数値の前後に付く記号の種類を表す。このうち幾何形状を必要と
//! するのは φ（[`DimSymbol::Diameter`]）と □（[`DimSymbol::Square`]）だけで、
//! それ以外（R/SR/CR/C/t）は ASCII 文字そのものなので、フォント文字として組版する
//! 側（app 層）に委ねる。[`dim_symbol_glyph`] はその方針に従い、φ・□ のみジオメトリ
//! （[`crate::Shape`]）を生成し、他は空の形状列を返す。

use serde::{Deserialize, Serialize};

use crate::{Circle, LineSeg, Point2, Polyline, Shape, Vec2};

/// φ 記号の円直径 ÷ 文字高さ `h`。
///
/// 製図規定.md で確定した暫定比率（見た目のバランスが取れる値として採用）。
const DIAMETER_CIRCLE_DIAMETER_RATIO: f64 = 0.7;

/// φ 記号の斜線が円の外へはみ出す長さ ÷ 円の半径。
///
/// 一般的な φ 記号の見た目（斜線が円周から少し飛び出す）を再現するための、
/// 製図規定.md で確定した暫定比率。
const DIAMETER_SLASH_OVERHANG_RATIO: f64 = 0.15;

/// φ 記号の斜線が水平となす角度（度）。
///
/// 製図規定.md で確定した暫定値。JIS 系の製図での慣用的な φ 記号の傾きに合わせる。
const DIAMETER_SLASH_ANGLE_DEG: f64 = 65.0;

/// □ 記号の正方形の一辺 ÷ 文字高さ `h`。
///
/// 製図規定.md で確定した暫定比率（φ の円直径比率と揃えている）。
const SQUARE_SIDE_RATIO: f64 = 0.7;

/// ASCII 文字 1 文字あたりの幅 ÷ 文字高さ `h`。
///
/// `mcad-core::entity_geom::text_aabb`・`mcad-app::dimension::ASCII_CHAR_WIDTH_RATIO`
/// と同じ慣行値。フォント文字扱いの記号（R/SR/CR/C/t、および Sφ の "S" 部分）の
/// 送り量をこの比率の文字数倍で見積もる。
const FONT_CHAR_ADVANCE_RATIO: f64 = 0.55;

/// 寸法記号の種類。
///
/// ジオメトリを生成するのは [`DimSymbol::Diameter`]（φ）と [`DimSymbol::Square`]
/// （□）の 2 つだけ。他の 6 種（R/SR/CR/C/t）は ASCII 文字そのものなので、
/// フォント文字として組版する側（app 層、mcad-geom の範囲外）に委ねる。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DimSymbol {
    /// 直径（φ）。ジオメトリを生成する。
    Diameter,
    /// 球の直径（Sφ）。ジオメトリを生成しない（"S" はフォント文字 1 文字、φ は
    /// [`DimSymbol::Diameter`] と同じ形状を app 層が並べる想定）。
    SphereDiameter,
    /// 正方形の辺（□）。ジオメトリを生成する。
    Square,
    /// 半径（R）。フォント文字 1 文字。
    Radius,
    /// 球の半径（SR）。フォント文字 2 文字。
    SphereRadius,
    /// コントロール半径（CR）。フォント文字 2 文字。
    ControlRadius,
    /// 面取り（C）。フォント文字 1 文字。
    Chamfer,
    /// 板厚（t）。フォント文字 1 文字。
    Thickness,
}

/// [`dim_symbol_glyph`] が生成する記号のジオメトリと送り量。
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolGlyph {
    /// 記号を構成する形状。フォント文字扱いの記号（φ・□ 以外）では空。
    pub shapes: Vec<Shape>,
    /// この記号が占める水平幅。後続の数値文字を続けて配置するための送り量。
    pub advance: f64,
}

/// 寸法記号 `sym` のジオメトリを、文字高さ `h`（ローカル座標、原点＝ベースライン
/// 左端）で生成する。
///
/// ジオメトリを生成するのは φ（[`DimSymbol::Diameter`]）と □（[`DimSymbol::Square`]）
/// だけ。それ以外（R/SR/CR/C/t、および Sφ の "S" 部分）は ASCII 文字そのものなので、
/// **呼び出し側がフォント文字として組版すること**。ここでは `shapes` を空にし、
/// `advance` には実際の文字数に応じた幅（[`FONT_CHAR_ADVANCE_RATIO`] × 文字数 × `h`）
/// を返す。実際の送り量はフォント側の実測に委ねる目安値である点は変わらない。
#[must_use]
pub fn dim_symbol_glyph(sym: DimSymbol, h: f64) -> SymbolGlyph {
    match sym {
        DimSymbol::Diameter => diameter_glyph(h),
        DimSymbol::Square => square_glyph(h),
        // 1 文字（R / C / t）。
        DimSymbol::Radius | DimSymbol::Chamfer | DimSymbol::Thickness => SymbolGlyph {
            shapes: Vec::new(),
            advance: h * FONT_CHAR_ADVANCE_RATIO,
        },
        // 2 文字（SR / CR）。
        DimSymbol::SphereRadius | DimSymbol::ControlRadius => SymbolGlyph {
            shapes: Vec::new(),
            advance: h * FONT_CHAR_ADVANCE_RATIO * 2.0,
        },
        // Sφ: "S" はフォント文字 1 文字分、φ は diameter_glyph と同じ幅を app 層が
        // 並べる想定なので、両者の advance を合算する。
        DimSymbol::SphereDiameter => SymbolGlyph {
            shapes: Vec::new(),
            advance: h * FONT_CHAR_ADVANCE_RATIO + diameter_glyph(h).advance,
        },
    }
}

/// φ 記号（円 + 貫く斜線）を文字高さ `h` で生成する。
fn diameter_glyph(h: f64) -> SymbolGlyph {
    let diameter = h * DIAMETER_CIRCLE_DIAMETER_RATIO;
    let radius = diameter * 0.5;
    // 記号の中央が概ね文字の中央高さに来るよう、中心をベースラインから h/2 上に置く。
    let center = Point2::new(radius, h * 0.5);

    let circle = Circle::new(center, radius);

    let angle = DIAMETER_SLASH_ANGLE_DEG.to_radians();
    let overhang = radius * DIAMETER_SLASH_OVERHANG_RATIO;
    let half_len = radius + overhang;
    let dir = Vec2::new(angle.cos(), angle.sin());
    let slash = LineSeg::new(center - dir * half_len, center + dir * half_len);

    SymbolGlyph {
        shapes: vec![Shape::Circle(circle), Shape::Line(slash)],
        advance: diameter,
    }
}

/// □ 記号（正方形の輪郭）を文字高さ `h` で生成する。
fn square_glyph(h: f64) -> SymbolGlyph {
    let side = h * SQUARE_SIDE_RATIO;
    let vertices = vec![
        Point2::new(0.0, 0.0),
        Point2::new(side, 0.0),
        Point2::new(side, side),
        Point2::new(0.0, side),
    ];
    let polyline = Polyline::new(vertices, true);

    SymbolGlyph {
        shapes: vec![Shape::Polyline(polyline)],
        advance: side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_VARIANTS: [DimSymbol; 8] = [
        DimSymbol::Diameter,
        DimSymbol::SphereDiameter,
        DimSymbol::Square,
        DimSymbol::Radius,
        DimSymbol::SphereRadius,
        DimSymbol::ControlRadius,
        DimSymbol::Chamfer,
        DimSymbol::Thickness,
    ];

    #[test]
    fn advance_is_positive_for_all_variants() {
        for sym in ALL_VARIANTS {
            let glyph = dim_symbol_glyph(sym, 3.5);
            assert!(
                glyph.advance > 0.0,
                "advance should be positive for {sym:?}"
            );
        }
    }

    #[test]
    fn diameter_and_square_have_nonempty_shapes() {
        assert!(!dim_symbol_glyph(DimSymbol::Diameter, 3.5).shapes.is_empty());
        assert!(!dim_symbol_glyph(DimSymbol::Square, 3.5).shapes.is_empty());
    }

    #[test]
    fn non_geometric_variants_have_empty_shapes() {
        for sym in [
            DimSymbol::SphereDiameter,
            DimSymbol::Radius,
            DimSymbol::SphereRadius,
            DimSymbol::ControlRadius,
            DimSymbol::Chamfer,
            DimSymbol::Thickness,
        ] {
            assert!(
                dim_symbol_glyph(sym, 3.5).shapes.is_empty(),
                "shapes should be empty for {sym:?}"
            );
        }
    }

    #[test]
    fn diameter_glyph_has_circle_and_slash() {
        let glyph = dim_symbol_glyph(DimSymbol::Diameter, 3.5);
        assert_eq!(glyph.shapes.len(), 2);
        assert!(matches!(glyph.shapes[0], Shape::Circle(_)));
        assert!(matches!(glyph.shapes[1], Shape::Line(_)));
    }

    #[test]
    fn square_glyph_is_closed_polyline_with_four_vertices() {
        let glyph = dim_symbol_glyph(DimSymbol::Square, 3.5);
        assert_eq!(glyph.shapes.len(), 1);
        match &glyph.shapes[0] {
            Shape::Polyline(p) => {
                assert!(p.closed);
                assert_eq!(p.vertices.len(), 4);
            }
            other => panic!("expected Polyline, got {other:?}"),
        }
    }

    #[test]
    fn diameter_glyph_scales_with_h() {
        let h1 = dim_symbol_glyph(DimSymbol::Diameter, 1.0);
        let h2 = dim_symbol_glyph(DimSymbol::Diameter, 2.0);
        assert!((h2.advance - h1.advance * 2.0).abs() < 1e-9);

        let (Shape::Circle(c1), Shape::Circle(c2)) = (&h1.shapes[0], &h2.shapes[0]) else {
            panic!("expected circles");
        };
        assert!((c2.radius - c1.radius * 2.0).abs() < 1e-9);
        assert!((c2.center.x - c1.center.x * 2.0).abs() < 1e-9);
        assert!((c2.center.y - c1.center.y * 2.0).abs() < 1e-9);

        let (Shape::Line(l1), Shape::Line(l2)) = (&h1.shapes[1], &h2.shapes[1]) else {
            panic!("expected lines");
        };
        assert!((l2.a.x - l1.a.x * 2.0).abs() < 1e-9);
        assert!((l2.a.y - l1.a.y * 2.0).abs() < 1e-9);
        assert!((l2.b.x - l1.b.x * 2.0).abs() < 1e-9);
        assert!((l2.b.y - l1.b.y * 2.0).abs() < 1e-9);
    }

    #[test]
    fn two_char_symbols_have_roughly_double_advance_of_one_char_symbols() {
        let h = 3.5;
        let one_char_advance = h * FONT_CHAR_ADVANCE_RATIO;
        for sym in [DimSymbol::Radius, DimSymbol::Chamfer, DimSymbol::Thickness] {
            let glyph = dim_symbol_glyph(sym, h);
            assert!(
                (glyph.advance - one_char_advance).abs() < 1e-9,
                "expected 1-char advance for {sym:?}"
            );
        }
        for sym in [DimSymbol::SphereRadius, DimSymbol::ControlRadius] {
            let glyph = dim_symbol_glyph(sym, h);
            assert!(
                (glyph.advance - one_char_advance * 2.0).abs() < 1e-9,
                "expected 2-char advance for {sym:?}"
            );
        }
    }

    #[test]
    fn sphere_diameter_advance_includes_diameter_glyph_advance() {
        let h = 3.5;
        let one_char_advance = h * FONT_CHAR_ADVANCE_RATIO;
        let sphere_diameter = dim_symbol_glyph(DimSymbol::SphereDiameter, h);
        let diameter = dim_symbol_glyph(DimSymbol::Diameter, h);

        // Sφ の advance は "S" 1 文字分より大きく（φ 分を含む）、
        // "S" 1 文字分 + φ の advance に一致する。
        assert!(sphere_diameter.advance > one_char_advance);
        assert!((sphere_diameter.advance - (one_char_advance + diameter.advance)).abs() < 1e-9);
    }

    #[test]
    fn square_glyph_scales_with_h() {
        let h1 = dim_symbol_glyph(DimSymbol::Square, 1.0);
        let h2 = dim_symbol_glyph(DimSymbol::Square, 2.0);
        assert!((h2.advance - h1.advance * 2.0).abs() < 1e-9);

        let (Shape::Polyline(p1), Shape::Polyline(p2)) = (&h1.shapes[0], &h2.shapes[0]) else {
            panic!("expected polylines");
        };
        for (v1, v2) in p1.vertices.iter().zip(p2.vertices.iter()) {
            assert!((v2.x - v1.x * 2.0).abs() < 1e-9);
            assert!((v2.y - v1.y * 2.0).abs() < 1e-9);
        }
    }
}
