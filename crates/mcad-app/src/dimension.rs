//! 寸法ジオメトリの展開（寸法線・補助線・矢先・文字配置）を計算する純関数群。
//!
//! # 設計方針（DESIGN.md M6 設計判断2）
//!
//! 寸法の **展開ロジックは app 層に閉じる**（＝ mcad-geom には入れない）。データ型
//! （[`DimLinear`] / [`DimRadial`]）は永続化・undo の都合で mcad-core に置くが、矢印・
//! 補助線・文字配置の生成はここ（app 層）の純関数で行い、単体テストで固定する。
//!
//! # 描画とヒットテストの一貫性
//!
//! [`expand_linear`] / [`expand_radial`] が返す [`DimExpansion`] を描画（`main.rs` の
//! `draw_dim_expansion`）とプレビューが共有する。ヒットテスト（`SelectTool::pick`）は
//! [`linear_distance`] / [`radial_distance`] を使い、**保存データ（p1/p2/offset・
//! center/radius/leader_angle）だけで決まるズーム非依存の線分**への最短距離を返す。
//! これにより「tol 以内で最も近いものを拾う」という既存の pick 契約へ素直に合流する
//! （矢印・文字の見かけの大きさに依存しない。M6 タスク23 の Text ヒットテストで得た教訓）。
//!
//! 寸法線・補助線・引出線の **位置は保存データだけで決まる**（矢先と文字の *大きさ* のみ
//! スクリーン固定 px をズームで割ったワールド長で与える）。したがって pick 用の線分
//! （[`linear_pick_segments`] / [`radial_pick_segments`]）は矢印・文字サイズを引数に取らない。

use mcad_core::{DimLinear, DimRadial, TextGeom};
use mcad_geom::{LineSeg, Point2, Vec2};

/// 文字幅の近似係数。寸法値は常に ASCII（数字・`.`・`R`）なので、mcad-core の
/// `text_aabb` と同じ ASCII 係数 0.55×height で幅を近似する（純関数のまま中央寄せ配置を
/// 決めるための割り切り。DESIGN.md M6 設計判断1 の近似方針に沿う）。
const ASCII_CHAR_WIDTH_RATIO: f64 = 0.55;

/// 矢先の半幅と長さの比。
const ARROW_HALF_WIDTH_RATIO: f64 = 0.35;

/// 寸法を描画・プレビュー可能な要素へ展開した結果（すべてワールド座標）。
#[derive(Debug, Clone, PartialEq)]
pub struct DimExpansion {
    /// 線分（寸法線・補助線・引出線）。
    pub segments: Vec<[Point2; 2]>,
    /// 矢先（各要素は塗りつぶす三角形の 3 頂点）。
    pub arrows: Vec<[Point2; 3]>,
    /// 値ラベル。既存の `draw_text` をそのまま再利用できる [`TextGeom`]。
    pub text: TextGeom,
}

/// 先端 `tip`・方向 `dir`（単位ベクトル）・長さ `len` の矢先三角形を作る。
fn arrow_triangle(tip: Point2, dir: Vec2, len: f64) -> [Point2; 3] {
    let back = tip - dir * len;
    let half = dir.perp() * (len * ARROW_HALF_WIDTH_RATIO);
    [tip, back + half, back - half]
}

/// 点 `p` から線分群への最短距離。空なら [`f64::INFINITY`]。
fn min_distance_to_segments(segs: &[[Point2; 2]], p: Point2) -> f64 {
    segs.iter()
        .map(|[a, b]| LineSeg::new(*a, *b).closest_point(p).distance(p))
        .fold(f64::INFINITY, f64::min)
}

// ---------------------------------------------------------------------
// 長さ寸法
// ---------------------------------------------------------------------

/// 長さ寸法の寸法線 2 端点 `(d1, d2)` と、寸法線方向の単位ベクトル・単位法線を返す。
/// 計測 2 点がほぼ同一（法線が定まらない）なら `None`。
fn linear_frame(dim: &DimLinear) -> Option<(Point2, Point2, Vec2, Vec2)> {
    let dir = (dim.p2 - dim.p1).normalize()?;
    let normal = dir.perp();
    let shift = normal * dim.offset;
    Some((dim.p1 + shift, dim.p2 + shift, dir, normal))
}

/// 長さ寸法のヒットテスト用線分（寸法線＋補助線 2 本）。矢先・文字の大きさに依らず
/// 保存データ（p1/p2/offset）だけで決まるため、ズーム非依存で pick から使える。
/// 退化（p1≈p2）時は空。
#[must_use]
pub fn linear_pick_segments(dim: &DimLinear) -> Vec<[Point2; 2]> {
    match linear_frame(dim) {
        Some((d1, d2, _, _)) => vec![[d1, d2], [dim.p1, d1], [dim.p2, d2]],
        None => Vec::new(),
    }
}

/// クリック点 `p` から長さ寸法への最短距離（ヒットテスト用）。退化寸法は p1 への距離で
/// 代替し、選択・削除だけはできるようにする。
#[must_use]
pub fn linear_distance(dim: &DimLinear, p: Point2) -> f64 {
    let segs = linear_pick_segments(dim);
    if segs.is_empty() {
        p.distance(dim.p1)
    } else {
        min_distance_to_segments(&segs, p)
    }
}

/// 長さ寸法を展開する。`arrow_len`・`text_height` はワールド長（呼び出し側がスクリーン
/// 固定 px ÷ ズームで与える）。値ラベルは `{:.2}`（DESIGN.md M6 設計判断2）。
#[must_use]
pub fn expand_linear(dim: &DimLinear, arrow_len: f64, text_height: f64) -> DimExpansion {
    // 退化時も破綻しない安全な既定方向（+x）を使う。通常はツールが p1≈p2 を弾く。
    let (d1, d2, dir, normal) =
        linear_frame(dim).unwrap_or((dim.p1, dim.p2, Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)));

    let segments = vec![[d1, d2], [dim.p1, d1], [dim.p2, d2]];
    // 矢先は寸法線の両端で外向き（d1 で −dir、d2 で +dir）。
    let arrows = vec![
        arrow_triangle(d1, -dir, arrow_len),
        arrow_triangle(d2, dir, arrow_len),
    ];

    let value = format!("{:.2}", (dim.p2 - dim.p1).length());
    let text = place_linear_text(d1, d2, dir, normal, dim.offset, value, text_height);

    DimExpansion {
        segments,
        arrows,
        text,
    }
}

/// 長さ寸法の値ラベルを配置する。ベースラインは寸法線方向に合わせ（左向きは反転して
/// 読みやすくする）、文字ブロックは寸法線の外側（計測線から遠い側 = `offset` の符号側）に
/// 中心を置く。近似幅で中央寄せの [`TextGeom`]（ベースライン左端アンカー）を返す。
fn place_linear_text(
    d1: Point2,
    d2: Point2,
    dir: Vec2,
    normal: Vec2,
    offset: f64,
    value: String,
    height: f64,
) -> TextGeom {
    let mid = d1 + (d2 - d1) * 0.5;
    // 読みやすい向き（左向きベースラインは反転）。
    let along = if dir.x < 0.0 { -dir } else { dir };
    let angle = along.angle();

    // 外側 = 計測線から遠い側。offset の符号で決める（0 のときは +法線側）。
    let sign = if offset >= 0.0 { 1.0 } else { -1.0 };
    let outward = normal * sign;

    let width = value.chars().count() as f64 * ASCII_CHAR_WIDTH_RATIO * height;
    let gap = height * 0.4;
    // 文字ブロック（width×height）の中心を寸法線の外側へ置く。ここから左下アンカーへ戻す。
    let block_center = mid + outward * (gap + height * 0.5);
    let up = along.perp();
    let anchor = block_center - along * (width * 0.5) - up * (height * 0.5);

    TextGeom {
        anchor,
        content: value,
        height,
        angle,
    }
}

// ---------------------------------------------------------------------
// 半径寸法
// ---------------------------------------------------------------------

/// 引出方向の単位ベクトルと円周上の点 `pc`（中心から半径方向へ radius 進んだ点）。
fn radial_frame(dim: &DimRadial) -> (Vec2, Point2) {
    let dir = Vec2::new(dim.leader_angle.cos(), dim.leader_angle.sin());
    (dir, dim.center + dir * dim.radius)
}

/// 半径寸法のヒットテスト用線分（中心→円周点の半径線）。保存データ（center/radius/
/// leader_angle）だけで決まるためズーム非依存で pick から使える。描画の引出線も同じ
/// 線分なので、描画とヒットテストの位置が一致する。
#[must_use]
pub fn radial_pick_segments(dim: &DimRadial) -> Vec<[Point2; 2]> {
    let (_, pc) = radial_frame(dim);
    vec![[dim.center, pc]]
}

/// クリック点 `p` から半径寸法（引出線）への最短距離（ヒットテスト用）。
#[must_use]
pub fn radial_distance(dim: &DimRadial, p: Point2) -> f64 {
    min_distance_to_segments(&radial_pick_segments(dim), p)
}

/// 半径寸法を展開する。引出線は中心→円周点の半径線、矢先は円周点で外向き、値ラベルは
/// `R{:.2}` を円周点の少し外側へ水平配置する（DESIGN.md M6 設計判断2）。
#[must_use]
pub fn expand_radial(dim: &DimRadial, arrow_len: f64, text_height: f64) -> DimExpansion {
    let (dir, pc) = radial_frame(dim);
    let segments = vec![[dim.center, pc]];
    let arrows = vec![arrow_triangle(pc, dir, arrow_len)];

    let value = format!("R{:.2}", dim.radius);
    let width = value.chars().count() as f64 * ASCII_CHAR_WIDTH_RATIO * text_height;
    let gap = text_height * 0.4;
    // 文字は水平（angle 0）。円周点から矢先＋隙間ぶん外側の点を近端にし、そこから
    // 引出向きに応じて左右へ伸ばす（右向き引出は右へ、左向きは左へ）。縦方向は近端に中央寄せ。
    let near = pc + dir * (arrow_len + gap);
    let anchor_x = if dir.x >= 0.0 { near.x } else { near.x - width };
    let anchor = Point2::new(anchor_x, near.y - text_height * 0.5);

    DimExpansion {
        segments,
        arrows,
        text: TextGeom {
            anchor,
            content: value,
            height: text_height,
            angle: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    const T: f64 = 1e-9;

    fn approx(a: Point2, b: Point2) -> bool {
        a.distance(b) < 1e-6
    }

    // --- 長さ寸法 ---

    #[test]
    fn linear_pick_segments_offset_positive_side() {
        // 水平な計測 (0,0)-(4,0)、offset +2 → 寸法線は y=2 側（法線 = (0,1)）。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 2.0,
        };
        let segs = linear_pick_segments(&dim);
        assert_eq!(segs.len(), 3);
        // 寸法線。
        assert!(approx(segs[0][0], Point2::new(0.0, 2.0)));
        assert!(approx(segs[0][1], Point2::new(4.0, 2.0)));
        // 補助線（計測点 → 寸法線端）。
        assert!(approx(segs[1][0], Point2::new(0.0, 0.0)));
        assert!(approx(segs[1][1], Point2::new(0.0, 2.0)));
        assert!(approx(segs[2][0], Point2::new(4.0, 0.0)));
        assert!(approx(segs[2][1], Point2::new(4.0, 2.0)));
    }

    #[test]
    fn linear_offset_sign_flips_dimension_line_side() {
        // 同じ計測で offset の符号を反転すると寸法線が反対側（y=-2）へ出る。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: -2.0,
        };
        let segs = linear_pick_segments(&dim);
        assert!(approx(segs[0][0], Point2::new(0.0, -2.0)));
        assert!(approx(segs[0][1], Point2::new(4.0, -2.0)));
    }

    #[test]
    fn linear_degenerate_has_no_segments_but_distance_falls_back() {
        // p1≈p2 は法線が定まらず線分なし。距離は p1 への距離で代替（選択可能に保つ）。
        let dim = DimLinear {
            p1: Point2::new(1.0, 1.0),
            p2: Point2::new(1.0, 1.0),
            offset: 3.0,
        };
        assert!(linear_pick_segments(&dim).is_empty());
        assert!((linear_distance(&dim, Point2::new(1.0, 4.0)) - 3.0).abs() < T);
    }

    #[test]
    fn linear_distance_is_zero_on_dimension_line() {
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 2.0,
        };
        // 寸法線 (0,2)-(4,2) 上の点は距離 0。
        assert!(linear_distance(&dim, Point2::new(2.0, 2.0)) < T);
        // 補助線 (0,0)-(0,2) 上の点も距離 0。
        assert!(linear_distance(&dim, Point2::new(0.0, 1.0)) < T);
        // 離れた点は正の距離（寸法線から 1）。
        assert!((linear_distance(&dim, Point2::new(2.0, 3.0)) - 1.0).abs() < T);
    }

    #[test]
    fn expand_linear_value_and_arrow_count() {
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(3.0, 4.0), // 長さ 5
            offset: 1.0,
        };
        let ex = expand_linear(&dim, 0.5, 1.0);
        assert_eq!(ex.segments.len(), 3);
        assert_eq!(ex.arrows.len(), 2);
        assert_eq!(ex.text.content, "5.00");
    }

    #[test]
    fn expand_linear_arrow_tips_sit_on_dimension_line_ends() {
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 2.0,
        };
        let ex = expand_linear(&dim, 0.5, 1.0);
        // 矢先の先端（各三角形の第 1 頂点）は寸法線の両端。
        assert!(approx(ex.arrows[0][0], Point2::new(0.0, 2.0)));
        assert!(approx(ex.arrows[1][0], Point2::new(4.0, 2.0)));
    }

    #[test]
    fn expand_linear_text_on_outward_side() {
        // offset +2 → 文字ブロックは寸法線 y=2 のさらに外側（y>2）。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 2.0,
        };
        let ex = expand_linear(&dim, 0.5, 1.0);
        // 水平ベースライン（角 0）。
        assert!(ex.text.angle.abs() < T);
        // アンカー（左下）は寸法線 y=2 より上。
        assert!(ex.text.anchor.y > 2.0);
        // offset −2 なら反対側（y<−2 側）。
        let dim2 = DimLinear {
            offset: -2.0,
            ..dim
        };
        let ex2 = expand_linear(&dim2, 0.5, 1.0);
        assert!(ex2.text.anchor.y < -2.0);
    }

    #[test]
    fn expand_linear_flips_baseline_for_leftward_measure() {
        // 右→左の計測（dir.x<0）はベースラインを反転して読みやすい向き（角 0）にする。
        let dim = DimLinear {
            p1: Point2::new(4.0, 0.0),
            p2: Point2::new(0.0, 0.0),
            offset: 2.0,
        };
        let ex = expand_linear(&dim, 0.5, 1.0);
        assert!(ex.text.angle.abs() < T);
    }

    #[test]
    fn expand_linear_vertical_measure_rotates_text() {
        // 垂直な計測（下→上）はベースラインが +π/2。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(0.0, 4.0),
            offset: 2.0,
        };
        let ex = expand_linear(&dim, 0.5, 1.0);
        assert!((ex.text.angle - FRAC_PI_2).abs() < T);
    }

    // --- 半径寸法 ---

    #[test]
    fn radial_pick_segment_is_radius_line() {
        // 中心原点・半径 5・引出角 0 → 円周点 (5,0)。引出線は (0,0)-(5,0)。
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 5.0,
            leader_angle: 0.0,
        };
        let segs = radial_pick_segments(&dim);
        assert_eq!(segs.len(), 1);
        assert!(approx(segs[0][0], Point2::ORIGIN));
        assert!(approx(segs[0][1], Point2::new(5.0, 0.0)));
    }

    #[test]
    fn radial_distance_zero_on_leader_line() {
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 5.0,
            leader_angle: 0.0,
        };
        // 引出線 (0,0)-(5,0) 上。
        assert!(radial_distance(&dim, Point2::new(2.0, 0.0)) < T);
        // 引出線から 1 離れた点。
        assert!((radial_distance(&dim, Point2::new(2.0, 1.0)) - 1.0).abs() < T);
    }

    #[test]
    fn expand_radial_value_prefix_and_arrow() {
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 12.5,
            leader_angle: 0.0,
        };
        let ex = expand_radial(&dim, 0.5, 1.0);
        assert_eq!(ex.segments.len(), 1);
        assert_eq!(ex.arrows.len(), 1);
        // 半径寸法は接頭辞 R。
        assert_eq!(ex.text.content, "R12.50");
        // 矢先の先端は円周点 (12.5,0)。
        assert!(approx(ex.arrows[0][0], Point2::new(12.5, 0.0)));
        // 文字は水平（角 0）で円周点より外側（x>12.5）。
        assert!(ex.text.angle.abs() < T);
        assert!(ex.text.anchor.x > 12.5);
    }

    #[test]
    fn expand_radial_leader_direction_follows_angle() {
        // 引出角 +π/2 → 円周点 (0, radius)。
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 3.0,
            leader_angle: FRAC_PI_2,
        };
        let ex = expand_radial(&dim, 0.5, 1.0);
        assert!(approx(ex.segments[0][1], Point2::new(0.0, 3.0)));
    }

    #[test]
    fn expand_radial_leftward_leader() {
        // 引出角 π → 円周点 (−radius, 0)。
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 4.0,
            leader_angle: PI,
        };
        let ex = expand_radial(&dim, 0.5, 1.0);
        assert!(approx(ex.segments[0][1], Point2::new(-4.0, 0.0)));
    }
}
