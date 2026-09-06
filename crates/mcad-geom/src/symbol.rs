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

/// 寸法矢先の種類（DESIGN.md 7章「随時対応」の「寸法矢印のブロック化」設計確定
/// 1・3）。`.mcad` v6 で `mcad-core::DimStyle::arrow_kind`（文書単位で1つ）が
/// この型を保持する。
///
/// JIS Z 8317-1:2008 附属書A の図示記号を優先し、AutoCAD の `DIMBLK` 系互換名は
/// 参考に留める（設計確定3）。形状の生成は純関数 [`arrow_glyph`]（M10 タスク63）。
///
/// `#[non_exhaustive]`: 種別は今後も増えうる。未知バリアントは呼び出し側が
/// [`ArrowKind::ClosedFilled`] として扱う（設計確定3。M9 判断1 と同じ「黙って
/// 壊れるより保守的な既定」）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ArrowKind {
    /// 閉じた塗りつぶし矢（現行・既定）。
    #[default]
    ClosedFilled,
    /// 閉じた白抜き矢（輪郭 stroke のみ。モノクロ出力・青図で背景色に依存しない）。
    ClosedBlank,
    /// 開いた矢、全開き角 30°（JIS Z 8317-1:2008 附属書A）。
    Open30,
    /// 開いた矢、全開き角 90°（JIS Z 8317-1:2008 附属書A）。
    Open90,
    /// 斜線（建築用チック、45°）。
    Oblique,
    /// 小円（塗りつぶし）。
    Dot,
    /// 矢先なし。
    None,
}

/// 閉じた矢（[`ArrowKind::ClosedFilled`] / [`ArrowKind::ClosedBlank`]）の全開き角（度）。
///
/// JIS Z 8317-1:2008 附属書A の開いた矢（30°）と揃えた値（DESIGN.md 7章「随時対応」の
/// 「寸法矢印のブロック化」設計確定5）。M9 まで app 層の定数 `ARROW_HALF_WIDTH_RATIO` が
/// 持っていた 20° より太い（意図した可視差。タスク63）。
const CLOSED_ARROW_INCLUDED_ANGLE_DEG: f64 = 30.0;

/// [`ArrowKind::Open30`] の全開き角（度）。JIS Z 8317-1:2008 附属書A。
const OPEN_ARROW_NARROW_INCLUDED_ANGLE_DEG: f64 = 30.0;

/// [`ArrowKind::Open90`] の寸法線に沿った到達距離の `len` に対する比。全幅 = 2·len·この比 = len。
const OPEN90_REACH_RATIO: f64 = 0.5;

/// [`ArrowKind::Open90`] の全開き角（度）。JIS Z 8317-1:2008 附属書A。
const OPEN_ARROW_WIDE_INCLUDED_ANGLE_DEG: f64 = 90.0;

/// [`ArrowKind::Oblique`]（建築用チック）が寸法線となす角（度）。
const OBLIQUE_ANGLE_DEG: f64 = 45.0;

/// [`ArrowKind::Dot`] の円の直径 ÷ 矢先の長さ `len`（設計確定5）。
const DOT_DIAMETER_RATIO: f64 = 0.5;

/// [`ArrowKind::Dot`] の円を近似する正多角形の頂点数。
///
/// 点は塗りつぶし専用（画面は `convex_polygon`、SVG/PDF は fill パス）なので、
/// [`Shape::Circle`] のベジエ近似ではなく多角形で持つ（[`ArrowGlyph`] が
/// 「塗る多角形＋描く線分」の 2 つだけで閉じるため）。矢先長さ 5mm なら直径 2.5mm で、
/// 16 角形の最大偏差は `r(1 - cos(π/16))` ≒ 半径の 1.9%（≒ 0.024mm）＝ 線幅
/// （0.25mm 級）より十分小さい。
const DOT_POLYGON_SEGMENTS: u32 = 16;

/// [`arrow_glyph`] が生成する矢先 1 個分の形状。
///
/// 座標は**矢先のローカル座標**: 原点＝矢の先端、+x ＝先端が指す向き。したがって矢の胴は
/// `x <= 0` 側（＝寸法線の内側方向が −x）にある。ワールドへの回転・平行移動は呼び出し側
/// （app 層の `dimension::place_arrow`）が行う。
///
/// [`dim_symbol_glyph`] と同じ「geom の純関数が形を決め、画面と SVG/PDF が同じ形を描く」
/// 流儀（DESIGN.md M9 設計判断1・9、7章「随時対応」の「寸法矢印のブロック化」設計確定2）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArrowGlyph {
    /// 塗りつぶす多角形。各要素は頂点列で、**閉じているものとして扱う**
    /// （始点を末尾に繰り返さない）。
    pub fills: Vec<Vec<Point2>>,
    /// ストロークで描く線分。線幅・色は寸法線と同じものを呼び出し側が与える。
    pub strokes: Vec<[Point2; 2]>,
    /// 矢先が**寸法線に沿って**占める長さ。閉じた矢・開いた矢は `len`、
    /// [`ArrowKind::Oblique`] / [`ArrowKind::Dot`] / [`ArrowKind::None`] は `0`。
    ///
    /// app 層の外向き矢印の自動判定（「ラベル幅 + `along_len` × 2 が寸法線長に収まるか」）と、
    /// 外向き時に寸法線を延長する量がこの値を使う。`0` の種別は内外の区別が描画に現れない
    /// ので、自動判定も手動指定（`ArrowPlacement::Outside`）も内向き扱いになる（設計確定4）。
    pub along_len: f64,
}

/// 矢先種別 `kind` のジオメトリを、長さ `len`（ローカル座標）で生成する。
///
/// 戻り値の座標系は [`ArrowGlyph`] の doc のとおり（先端＝原点、+x ＝先端の向き）。
/// `len` の単位は呼び出し側の単位そのまま（ワールド長を渡せばワールド長、紙 mm を渡せば
/// 紙 mm が返る。[`dim_symbol_glyph`] の `h` と同じ「単位は抽象」の契約）。
///
/// # 種別ごとの形（設計確定3・5）
///
/// | 種別 | 形 | `along_len` |
/// |---|---|---|
/// | [`ArrowKind::ClosedFilled`] | 全開き 30° の三角形を塗る | `len` |
/// | [`ArrowKind::ClosedBlank`] | 同じ三角形の輪郭 3 辺を描く | `len` |
/// | [`ArrowKind::Open30`] | 先端から 2 本の線（全開き 30°） | `len` |
/// | [`ArrowKind::Open90`] | 同 90°（到達距離は `len/2`、全幅 = `len`） | `len/2` |
/// | [`ArrowKind::Oblique`] | 45° の斜線 1 本（先端が中点、長さ `len`） | `0` |
/// | [`ArrowKind::Dot`] | 直径 `len × 0.5` の円（塗り、多角形近似） | `0` |
/// | [`ArrowKind::None`] | 空 | `0` |
///
/// 開いた矢の 2 本の線は、同じ全開き角の閉じた矢の**斜辺そのもの**（＝ 閉じた矢から
/// 底辺を除いたもの）である。これにより `along_len` は「寸法線に沿って占める長さ」＝
/// `len` という定義どおりの値になり、閉じた矢と開いた矢の到達距離も揃う。
///
/// # 未知バリアントは `ClosedFilled` として描く
///
/// [`ArrowKind`] は `#[non_exhaustive]` なので、`match` は
/// [`ArrowKind::ClosedFilled`] と未知バリアントを**同じ腕**（ワイルドカード）で扱う
/// （設計確定3。M9 設計判断1 の「黙って壊れるより保守的な既定」と同じ）。
///
/// # 退化
///
/// `len` が非有限または非正のときは空の [`ArrowGlyph`]（`along_len = 0`）を返す。
/// 描くものが無いだけで、呼び出し側は分岐を増やさずに済む。
#[must_use]
pub fn arrow_glyph(kind: ArrowKind, len: f64) -> ArrowGlyph {
    if !(len.is_finite() && len > 0.0) {
        return ArrowGlyph::default();
    }
    match kind {
        ArrowKind::ClosedBlank => closed_arrow_glyph(len, false),
        ArrowKind::Open30 => open_arrow_glyph(len, OPEN_ARROW_NARROW_INCLUDED_ANGLE_DEG),
        // 90° は 30° と同じ到達距離だと全幅 2·len(既定 5.0 で 10mm)になり広すぎる(ユーザー実機、
        // 2026-09-06)。到達距離を半分にして全幅 = len に抑える。
        ArrowKind::Open90 => {
            open_arrow_glyph(len * OPEN90_REACH_RATIO, OPEN_ARROW_WIDE_INCLUDED_ANGLE_DEG)
        }
        ArrowKind::Oblique => oblique_arrow_glyph(len),
        ArrowKind::Dot => dot_arrow_glyph(len),
        ArrowKind::None => ArrowGlyph::default(),
        // `ArrowKind::ClosedFilled` と、将来追加される未知バリアント（`#[non_exhaustive]`）。
        // 明示の腕を作らず 1 つにまとめてあるので、`ClosedFilled` のテストが同時に
        // 未知バリアントの経路を固定する。
        _ => closed_arrow_glyph(len, true),
    }
}

/// 全開き角 `included_deg` の矢羽の 2 端点（順に +y 側・−y 側）。
///
/// 先端は原点、底辺は `x = -len` の上にあるので、半幅は `len * tan(included_deg / 2)`。
fn barb_ends(len: f64, included_deg: f64) -> [Point2; 2] {
    let half_width = len * (included_deg * 0.5).to_radians().tan();
    [
        Point2::new(-len, half_width),
        Point2::new(-len, -half_width),
    ]
}

/// 閉じた矢（`filled` なら塗りつぶし、そうでなければ輪郭 3 辺のストローク）。
fn closed_arrow_glyph(len: f64, filled: bool) -> ArrowGlyph {
    let [left, right] = barb_ends(len, CLOSED_ARROW_INCLUDED_ANGLE_DEG);
    let tip = Point2::ORIGIN;
    if filled {
        ArrowGlyph {
            fills: vec![vec![tip, left, right]],
            strokes: Vec::new(),
            along_len: len,
        }
    } else {
        ArrowGlyph {
            fills: Vec::new(),
            strokes: vec![[tip, left], [left, right], [right, tip]],
            along_len: len,
        }
    }
}

/// 開いた矢（先端から出る 2 本の線＝閉じた矢の斜辺のみ）。
fn open_arrow_glyph(len: f64, included_deg: f64) -> ArrowGlyph {
    let [left, right] = barb_ends(len, included_deg);
    let tip = Point2::ORIGIN;
    ArrowGlyph {
        fills: Vec::new(),
        strokes: vec![[tip, left], [tip, right]],
        along_len: len,
    }
}

/// 斜線（建築用チック）。先端を中点とする長さ `len` の 45° 線で、寸法線を横切る。
fn oblique_arrow_glyph(len: f64) -> ArrowGlyph {
    let angle = OBLIQUE_ANGLE_DEG.to_radians();
    let half = Vec2::new(angle.cos(), angle.sin()) * (len * 0.5);
    ArrowGlyph {
        fills: Vec::new(),
        strokes: vec![[Point2::ORIGIN - half, Point2::ORIGIN + half]],
        // 寸法線に沿って「場所を取らない」ので 0（設計確定4）。
        along_len: 0.0,
    }
}

/// 点（先端を中心とする塗りつぶし円の多角形近似）。
fn dot_arrow_glyph(len: f64) -> ArrowGlyph {
    let radius = len * DOT_DIAMETER_RATIO * 0.5;
    let vertices = (0..DOT_POLYGON_SEGMENTS)
        .map(|i| {
            let t = std::f64::consts::TAU * f64::from(i) / f64::from(DOT_POLYGON_SEGMENTS);
            Point2::new(radius * t.cos(), radius * t.sin())
        })
        .collect();
    ArrowGlyph {
        fills: vec![vertices],
        strokes: Vec::new(),
        along_len: 0.0,
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

    #[test]
    fn arrow_kind_defaults_to_closed_filled() {
        // `.mcad` v1〜v5 の既定値補完先（設計確定1・3、`DimStyle::arrow_kind` の
        // `#[serde(default)]` が使う値）。実際の JSON タグ名を含む serde 往復は
        // `mcad-io`（v6 の実ファイル形式）側の
        // `v6_round_trip_is_lossless_including_table_and_arrow_kind` で固定する
        // （`mcad-geom` は `serde_json` に依存していないため、ここでは
        // `Default` の値だけを固定する）。
        assert_eq!(ArrowKind::default(), ArrowKind::ClosedFilled);
    }

    // -----------------------------------------------------------------
    // 矢先（M10 タスク63）
    // -----------------------------------------------------------------

    /// [`ArrowKind`] の全既知バリアント。`#[non_exhaustive]` なので網羅は
    /// コンパイラではなくこの配列が担保する（`ALL_VARIANTS` と同じ流儀）。
    /// **種別を増やしたらここへ足すこと。**
    const ALL_ARROW_KINDS: [ArrowKind; 7] = [
        ArrowKind::ClosedFilled,
        ArrowKind::ClosedBlank,
        ArrowKind::Open30,
        ArrowKind::Open90,
        ArrowKind::Oblique,
        ArrowKind::Dot,
        ArrowKind::None,
    ];

    /// 矢先が寸法線に沿って場所を取る（＝ `along_len == len`）種別。
    const ARROWS_ALONG_THE_LINE: [ArrowKind; 4] = [
        ArrowKind::ClosedFilled,
        ArrowKind::ClosedBlank,
        ArrowKind::Open30,
        ArrowKind::Open90,
    ];

    /// グリフの全頂点（塗り多角形の頂点＋線分の両端）。
    fn arrow_points(glyph: &ArrowGlyph) -> Vec<Point2> {
        let mut out: Vec<Point2> = glyph.fills.iter().flatten().copied().collect();
        for [a, b] in &glyph.strokes {
            out.push(*a);
            out.push(*b);
        }
        out
    }

    #[test]
    fn every_known_arrow_kind_but_none_has_geometry() {
        for kind in ALL_ARROW_KINDS {
            let glyph = arrow_glyph(kind, 5.0);
            let has_geometry = !glyph.fills.is_empty() || !glyph.strokes.is_empty();
            if kind == ArrowKind::None {
                assert!(!has_geometry, "None は空のはず");
                assert!(glyph.fills.is_empty() && glyph.strokes.is_empty());
            } else {
                assert!(has_geometry, "{kind:?} の形状が空になっている");
            }
        }
    }

    #[test]
    fn along_len_is_len_for_line_occupying_kinds_and_zero_otherwise() {
        let len = 5.0;
        for kind in ALL_ARROW_KINDS {
            let glyph = arrow_glyph(kind, len);
            let expected = if kind == ArrowKind::Open90 {
                // 90° だけ到達距離を半分にしている（全幅 = len）。
                len * OPEN90_REACH_RATIO
            } else if ARROWS_ALONG_THE_LINE.contains(&kind) {
                len
            } else {
                0.0
            };
            assert!(
                (glyph.along_len - expected).abs() < 1e-12,
                "{kind:?}: along_len {} != {expected}",
                glyph.along_len
            );
        }
    }

    /// 先端は常にローカル原点、胴は `x <= 0` 側（[`ArrowGlyph`] の座標契約）。
    ///
    /// 対象は `along_len > 0` の種別（閉じた矢・開いた矢）。[`ArrowKind::Oblique`] と
    /// [`ArrowKind::Dot`] は**先端を中心に置く**設計なので +x 側へもはみ出す
    /// （それぞれ専用テストで形を固定している）。
    #[test]
    fn arrow_bodies_stay_on_the_non_positive_x_side() {
        for kind in ARROWS_ALONG_THE_LINE {
            for p in arrow_points(&arrow_glyph(kind, 5.0)) {
                assert!(p.x <= 1e-12, "{kind:?}: 胴が +x 側へ出た（{p:?}）");
            }
        }
    }

    /// `ClosedFilled` は明示の腕を持たず、未知バリアントと同じワイルドカード腕で
    /// 処理される（設計確定3）。したがってこのテストは「未知バリアントを
    /// `ClosedFilled` として描く」経路も同時に固定している。
    #[test]
    fn closed_filled_is_a_triangle_with_a_30_degree_included_angle() {
        let len = 5.0;
        let glyph = arrow_glyph(ArrowKind::ClosedFilled, len);
        assert_eq!(glyph.fills.len(), 1);
        assert!(glyph.strokes.is_empty(), "塗りだけで輪郭は描かない");
        let tri = &glyph.fills[0];
        assert_eq!(tri.len(), 3);
        // 頂点順は [先端, 後端+半幅, 後端−半幅]（M9 までの `arrow_triangle` と同じ）。
        assert_eq!(tri[0], Point2::ORIGIN);
        let half_width = len * (CLOSED_ARROW_INCLUDED_ANGLE_DEG * 0.5).to_radians().tan();
        assert!((tri[1].x + len).abs() < 1e-12);
        assert!((tri[1].y - half_width).abs() < 1e-12);
        assert!((tri[2].x + len).abs() < 1e-12);
        assert!((tri[2].y + half_width).abs() < 1e-12);
        // 全開き 30°（tan15° ≒ 0.2679）は M9 までの 20°（tan10° ≒ 0.1763）より太い。
        assert!(half_width > len * 0.1763, "20° より太いこと");
    }

    #[test]
    fn closed_blank_is_the_same_triangle_drawn_as_three_edges() {
        let len = 5.0;
        let filled = arrow_glyph(ArrowKind::ClosedFilled, len);
        let blank = arrow_glyph(ArrowKind::ClosedBlank, len);
        assert!(blank.fills.is_empty(), "白抜きは fill を持たない");
        assert_eq!(blank.strokes.len(), 3);
        let tri = &filled.fills[0];
        assert_eq!(blank.strokes[0], [tri[0], tri[1]]);
        assert_eq!(blank.strokes[1], [tri[1], tri[2]]);
        assert_eq!(blank.strokes[2], [tri[2], tri[0]]);
    }

    /// 開いた矢の 2 本は同じ全開き角の閉じた矢の斜辺そのもの（`along_len` の定義が
    /// 「寸法線に沿って占める長さ」であることの担保）。
    #[test]
    fn open_arrows_are_the_slanted_edges_of_the_closed_triangle() {
        let len = 5.0;
        let open30 = arrow_glyph(ArrowKind::Open30, len);
        assert!(open30.fills.is_empty());
        assert_eq!(open30.strokes.len(), 2);
        let closed = arrow_glyph(ArrowKind::ClosedFilled, len);
        let tri = &closed.fills[0];
        // `ClosedFilled` も 30° なので斜辺が一致する。
        assert_eq!(open30.strokes[0], [tri[0], tri[1]]);
        assert_eq!(open30.strokes[1], [tri[0], tri[2]]);

        // 90° は到達距離 len/2 で半幅 = len/2·tan45° = len/2（全幅 = len）。30° より幅広。
        let open90 = arrow_glyph(ArrowKind::Open90, len);
        assert_eq!(open90.strokes.len(), 2);
        assert!((open90.strokes[0][1].y - len * OPEN90_REACH_RATIO).abs() < 1e-12);
        assert!(open90.strokes[0][1].y > open30.strokes[0][1].y);
        // 底辺は x = -along_len（＝寸法線に沿って占める長さ）。
        for glyph in [&open30, &open90] {
            for [_, end] in &glyph.strokes {
                assert!((end.x + glyph.along_len).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn oblique_crosses_the_dimension_line_through_the_tip() {
        let len = 5.0;
        let glyph = arrow_glyph(ArrowKind::Oblique, len);
        assert!(glyph.fills.is_empty());
        assert_eq!(glyph.strokes.len(), 1);
        let [a, b] = glyph.strokes[0];
        // 先端（原点）が中点で、長さは len。
        assert!(a.midpoint(b).distance(Point2::ORIGIN) < 1e-12);
        assert!((a.distance(b) - len).abs() < 1e-12);
        // 寸法線（局所 x 軸）を横切る: 両端が x 軸の反対側にあり、
        // かつ寸法線の内側（−x）と外側（+x）の両方へ出る。
        assert!(a.y * b.y < 0.0, "x 軸を横切っていない");
        assert!(a.x * b.x < 0.0, "y 軸を横切っていない");
        // 45°（|dy/dx| = 1）。
        assert!(((b.y - a.y).abs() - (b.x - a.x).abs()).abs() < 1e-12);
    }

    #[test]
    fn dot_is_a_filled_polygon_of_half_the_arrow_length_in_diameter() {
        let len = 5.0;
        let glyph = arrow_glyph(ArrowKind::Dot, len);
        assert!(glyph.strokes.is_empty());
        assert_eq!(glyph.fills.len(), 1);
        let poly = &glyph.fills[0];
        assert_eq!(poly.len(), DOT_POLYGON_SEGMENTS as usize);
        let radius = len * DOT_DIAMETER_RATIO * 0.5;
        for p in poly {
            assert!(
                (p.distance(Point2::ORIGIN) - radius).abs() < 1e-12,
                "頂点が半径 {radius} の円上にない: {p:?}"
            );
        }
    }

    #[test]
    fn arrow_glyphs_scale_linearly_with_len() {
        for kind in ALL_ARROW_KINDS {
            let one = arrow_glyph(kind, 1.0);
            let two = arrow_glyph(kind, 2.0);
            assert!(
                (two.along_len - one.along_len * 2.0).abs() < 1e-12,
                "{kind:?}"
            );
            let (p1, p2) = (arrow_points(&one), arrow_points(&two));
            assert_eq!(p1.len(), p2.len(), "{kind:?}");
            for (a, b) in p1.iter().zip(p2.iter()) {
                assert!((b.x - a.x * 2.0).abs() < 1e-12, "{kind:?}");
                assert!((b.y - a.y * 2.0).abs() < 1e-12, "{kind:?}");
            }
        }
    }

    #[test]
    fn non_finite_or_non_positive_len_yields_an_empty_glyph() {
        for len in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for kind in ALL_ARROW_KINDS {
                let glyph = arrow_glyph(kind, len);
                assert!(glyph.fills.is_empty(), "{kind:?} / len {len}");
                assert!(glyph.strokes.is_empty(), "{kind:?} / len {len}");
                assert_eq!(glyph.along_len, 0.0, "{kind:?} / len {len}");
            }
        }
    }
}
