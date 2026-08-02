//! スナップエンジン（DESIGN.md 3.4）。
//!
//! カーソルのワールド座標とワールド単位の探索半径を受け取り、半径内のスナップ
//! 候補点を種別付きで列挙して **1 点だけ** 返す。GUI（egui）には一切依存せず、
//! [`Document`] と幾何プリミティブ（`mcad-geom`）だけで完結するため、候補列挙・
//! 優先度判定は headless で単体テストできる（本ファイル末尾の `tests` を参照）。
//!
//! # 優先度
//!
//! DESIGN.md 3.4 の優先順位に厳密に従う:
//!
//! > 端点 > 交点 > 中点 > 中心 > グリッド
//!
//! 半径内に複数種別の候補があれば **優先度が高い種別** を採用し、同種別内では
//! カーソルに **最も近い** ものを選ぶ（[`Best::consider`]）。半径内に候補が
//! 何も無ければ [`None`]（＝スナップなし。呼び出し側は素のカーソル位置を使う）。
//!
//! # 交点候補の事前絞り込み（AABB カリング）
//!
//! 交点計算は全エンティティの全ペアに掛けると O(n^2) になるため、「カーソル近傍
//! AABB（カーソル位置を探索半径ぶん拡張したボックス）と交差する」エンティティのみを
//! 対象に総当たりする。この絞り込みは **無損失**: スナップ候補は定義上カーソルから
//! 半径内の点に限られ、交点は両エンティティの AABB 内に必ずあるため、半径内の交点を
//! 持つペアは両方とも必ずカーソル近傍 AABB と交差する（Chebyshev 距離 ≦ Euclid 距離）。
//! カーソルは画面内にあるので、これは DESIGN.md 3.4 の「画面内エンティティのみ対象に
//! AABB で事前絞り込み」よりさらに狭い、上位互換の絞り込みである。マウス移動のたびに
//! 呼ばれる関数なので、画面内エンティティ数 k に対する O(k^2) をカーソル近傍の
//! ごく少数に抑えることがフレームレート維持に効く。
//!
//! 一方、端点・中点・中心は 1 エンティティあたり定数個で列挙コストが O(n) に
//! 収まるため、可視 AABB での事前絞り込みは行わず、可視レイヤーの全エンティティを
//! 走査する（DESIGN.md が AABB 事前絞り込みを要求しているのは交点のみ）。半径内
//! （数ピクセル相当）に入る端点・中点・中心は、オンスクリーンのカーソル近傍に
//! ある以上その所属エンティティも実質的に画面内なので、絞り込みの有無で結果は
//! 変わらない。

use mcad_core::{Document, Entity, EntityGeom};
use mcad_geom::{Aabb, Point2, Shape, intersect};

/// スナップ候補の種別。優先度は Endpoint > Intersection > Midpoint > Center > Grid
/// （DESIGN.md 3.4）。数値優先度は [`SnapKind::priority`] が返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapKind {
    /// 端点（線分の端・ポリライン頂点・円弧の始終点・点エンティティ）。
    Endpoint,
    /// 交点（2 エンティティの交差点）。
    Intersection,
    /// 中点（線分・ポリライン各辺の中点）。
    Midpoint,
    /// 中心（円・円弧の中心）。
    Center,
    /// グリッド交点。
    Grid,
}

impl SnapKind {
    /// 数値優先度（小さいほど高優先）。同順位内はカーソルとの距離で決める。
    #[must_use]
    fn priority(self) -> u8 {
        match self {
            SnapKind::Endpoint => 0,
            SnapKind::Intersection => 1,
            SnapKind::Midpoint => 2,
            SnapKind::Center => 3,
            SnapKind::Grid => 4,
        }
    }
}

/// スナップ結果。採用した候補の種別と、スナップ先のワールド座標。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapResult {
    /// 採用した候補の種別（マーカー描画で見た目を変えるのに使う）。
    pub kind: SnapKind,
    /// スナップ先のワールド座標。
    pub point: Point2,
}

/// カーソル位置 `cursor` から半径 `radius`（ワールド単位）内のスナップ候補を
/// 列挙し、優先度・距離規則で 1 点を選んで返す。候補が無ければ `None`。
///
/// - `document`: 候補の元となるエンティティ（可視レイヤーのみ対象）。
/// - `cursor`: カーソルのワールド座標。
/// - `radius`: 探索半径（ワールド単位。呼び出し側で `半径px / zoom` に換算済み）。
/// - `grid_step`: グリッド間隔（ワールド単位）。`0` 以下・非有限ならグリッド候補なし。
/// - `extra_points`: 作図中（未確定）ツールの頂点列（[`crate::tool::Tool::snap_points`]）。
///   `Document` にまだ存在しない点だが、[`SnapKind::Endpoint`]（最優先）候補として
///   他の端点と同じ優先度・最近傍規則で扱う。ポリライン作図中に自分自身の始点へ
///   スナップして自動クローズできるようにするための拡張。
#[must_use]
pub fn snap(
    document: &Document,
    cursor: Point2,
    radius: f64,
    grid_step: f64,
    extra_points: &[Point2],
) -> Option<SnapResult> {
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let r2 = radius * radius;
    let mut best = Best::default();

    // 作図中(未確定)ツールの頂点: Document にはまだ無いが端点として最優先で扱う。
    for &p in extra_points {
        best.consider(SnapKind::Endpoint, p, cursor, r2);
    }

    // 端点・中点・中心: 可視レイヤーの全エンティティを走査（O(n)）。
    for (_, entity) in document.entities() {
        if !layer_visible(document, entity) {
            continue;
        }
        match &entity.geom {
            EntityGeom::Shape(shape) => enumerate_features(shape, cursor, r2, &mut best),
            // Text はアンカー点のみをスナップ源にする（端点扱い。DESIGN.md M6 L386）。
            EntityGeom::Text(text) => best.consider(SnapKind::Endpoint, text.anchor, cursor, r2),
            // 寸法はスナップ源にしない（DESIGN.md M6 L386）。`EntityGeom` は
            // `#[non_exhaustive]` なので、未知の幾何も同じくスナップ源にしない。
            _ => {}
        }
    }

    // 交点: カーソル近傍 AABB（カーソル±半径のボックス）と交差するエンティティのみを
    // 事前絞り込みし、その集合内の全ペアで交点を求める。絞り込みが無損失である理由は
    // モジュール doc を参照。
    let cursor_box = Aabb::from_point(cursor).expanded(radius);
    let near: Vec<&Shape> = document
        .entities()
        .filter(|(_, e)| layer_visible(document, e))
        .filter(|(_, e)| e.geom.aabb().intersects(&cursor_box))
        // M6: 交点計算は Shape 系のみを対象にする（寸法・テキストは交点源にしない）。
        .filter_map(|(_, e)| e.geom.as_shape())
        .collect();
    for i in 0..near.len() {
        for j in (i + 1)..near.len() {
            for p in intersect(near[i], near[j]) {
                best.consider(SnapKind::Intersection, p, cursor, r2);
            }
        }
    }

    // グリッド: カーソルに最も近いグリッド交点。最低優先度なので他候補が無いときの
    // フォールバックになる。
    if grid_step.is_finite() && grid_step > 0.0 {
        let gx = (cursor.x / grid_step).round() * grid_step;
        let gy = (cursor.y / grid_step).round() * grid_step;
        best.consider(SnapKind::Grid, Point2::new(gx, gy), cursor, r2);
    }

    best.finish()
}

/// 1 エンティティの端点・中点・中心の候補を [`Best`] に投入する。
///
/// 各種別の割り当て:
/// - 点 [`Shape::Point`] は点そのものを端点として扱う。
/// - 線分 [`Shape::Line`] は両端が端点、中点が中点。
/// - 円 [`Shape::Circle`] は中心のみ（端点・中点は持たない）。
/// - 円弧 [`Shape::Arc`] は始点・終点が端点、中心が中心（弧の中点は候補にしない）。
/// - ポリライン [`Shape::Polyline`] は各頂点が端点、各辺（閉じている場合は閉じ辺も）の中点が中点。
fn enumerate_features(shape: &Shape, cursor: Point2, r2: f64, best: &mut Best) {
    match shape {
        Shape::Point(p) => best.consider(SnapKind::Endpoint, *p, cursor, r2),
        Shape::Line(s) => {
            best.consider(SnapKind::Endpoint, s.a, cursor, r2);
            best.consider(SnapKind::Endpoint, s.b, cursor, r2);
            best.consider(SnapKind::Midpoint, s.a.midpoint(s.b), cursor, r2);
        }
        Shape::Circle(c) => best.consider(SnapKind::Center, c.center, cursor, r2),
        Shape::Arc(a) => {
            best.consider(SnapKind::Endpoint, a.start_point(), cursor, r2);
            best.consider(SnapKind::Endpoint, a.end_point(), cursor, r2);
            best.consider(SnapKind::Center, a.center, cursor, r2);
        }
        Shape::Polyline(pl) => {
            for &v in &pl.vertices {
                best.consider(SnapKind::Endpoint, v, cursor, r2);
            }
            for seg in pl.segments() {
                best.consider(SnapKind::Midpoint, seg.a.midpoint(seg.b), cursor, r2);
            }
        }
    }
}

/// エンティティの所属レイヤーが可視か（非表示レイヤーはスナップ対象外。
/// `SelectTool` のヒットテストが非表示レイヤーを除外するのと同じ方針）。
fn layer_visible(document: &Document, entity: &Entity) -> bool {
    document.layer(entity.layer).is_some_and(|l| l.visible)
}

/// これまでに見た候補のうち「最良」を保持するアキュムレータ。
///
/// 最良の定義（DESIGN.md 3.4 の優先順位）:
/// 1. 優先度（[`SnapKind::priority`]）が高い（数値が小さい）ものを優先。
/// 2. 優先度が同じなら、カーソルとの距離（の 2 乗）が小さいものを優先。
///
/// 半径外（`d2 > r2`）の候補は無視する。
#[derive(Default)]
struct Best {
    /// `(優先度, カーソルとの距離^2, 結果)`。未確定なら `None`。
    found: Option<(u8, f64, SnapResult)>,
}

impl Best {
    /// 候補 1 点を検討する。半径内かつ現状より良ければ採用する。
    fn consider(&mut self, kind: SnapKind, point: Point2, cursor: Point2, r2: f64) {
        let d2 = cursor.distance_squared(point);
        if d2 > r2 {
            return;
        }
        let rank = kind.priority();
        let better = match self.found {
            None => true,
            Some((best_rank, best_d2, _)) => {
                rank < best_rank || (rank == best_rank && d2 < best_d2)
            }
        };
        if better {
            self.found = Some((rank, d2, SnapResult { kind, point }));
        }
    }

    /// 採用した最良候補を返す（無ければ `None`）。
    fn finish(self) -> Option<SnapResult> {
        self.found.map(|(_, _, result)| result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::{Command, Entity, Style};
    use mcad_geom::{Circle, LineSeg, Shape};

    /// カレントレイヤーに `shape` を追加する。
    fn add(doc: &mut Document, shape: Shape) {
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            shape,
            layer,
            Style::inherited(),
        )))
        .expect("current layer must accept entity");
    }

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Shape {
        Shape::Line(LineSeg::new(Point2::new(ax, ay), Point2::new(bx, by)))
    }

    /// グリッド無効（`grid_step = 0`）・`extra_points` 無しでスナップする短縮版。
    fn snap_no_grid(doc: &Document, cursor: Point2, radius: f64) -> Option<SnapResult> {
        snap(doc, cursor, radius, 0.0, &[])
    }

    #[test]
    fn no_candidate_within_radius_returns_none() {
        let mut doc = Document::new();
        add(&mut doc, Shape::Point(Point2::new(0.0, 0.0)));
        // カーソルは点から遠い。半径 1 内に候補なし。
        assert_eq!(snap_no_grid(&doc, Point2::new(10.0, 0.0), 1.0), None);
        // 近づけば端点にスナップする。
        let r = snap_no_grid(&doc, Point2::new(0.5, 0.0), 1.0).unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(0.0, 0.0));
    }

    #[test]
    fn endpoint_beats_intersection_and_midpoint() {
        // 線分 A (0,0)-(10,0)、線分 B (5,-5)-(5,5)。交点 (5,0)。
        // A の中点も (5,0)。A の端点 (10,0)。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        add(&mut doc, line(5.0, -5.0, 5.0, 5.0));

        // カーソル (7,0): 交点(5,0)まで 2、端点(10,0)まで 3。半径 5 で両方内。
        // 端点は交点より遠いが、優先度で端点が勝つ。
        let r = snap_no_grid(&doc, Point2::new(7.0, 0.0), 5.0).unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(10.0, 0.0));
    }

    #[test]
    fn intersection_beats_midpoint() {
        // 線分 A (0,0)-(4,0): 中点 (2,0)。線分 B (3,-1)-(3,3): A と (3,0) で交差。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 4.0, 0.0));
        add(&mut doc, line(3.0, -1.0, 3.0, 3.0));

        // カーソル (2.4,0): A 中点(2,0)まで 0.4、交点(3,0)まで 0.6。半径 0.7 で両方内。
        // 交点は中点より遠いが、優先度で交点が勝つ。
        let r = snap_no_grid(&doc, Point2::new(2.4, 0.0), 0.7).unwrap();
        assert_eq!(r.kind, SnapKind::Intersection);
        assert_eq!(r.point, Point2::new(3.0, 0.0));
    }

    #[test]
    fn midpoint_beats_center() {
        // 線分 (0,0)-(4,0): 中点 (2,0)。円 中心 (2.4,0) 半径 5（曲線はカーソルから遠い）。
        // 線分は円の内側に収まるので交点は生じない。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 4.0, 0.0));
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(2.4, 0.0), 5.0)),
        );

        // カーソル (2.3,0): 中心(2.4,0)まで 0.1、中点(2,0)まで 0.3。半径 0.5 で両方内。
        // 中点は中心より遠いが、優先度で中点が勝つ。
        let r = snap_no_grid(&doc, Point2::new(2.3, 0.0), 0.5).unwrap();
        assert_eq!(r.kind, SnapKind::Midpoint);
        assert_eq!(r.point, Point2::new(2.0, 0.0));
    }

    #[test]
    fn center_beats_grid() {
        // 円 中心 (0.3,0.3) 半径 5（曲線は遠い）。グリッド間隔 1.0。
        let mut doc = Document::new();
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(0.3, 0.3), 5.0)),
        );

        // カーソル (0.1,0.1): 最寄りグリッド交点(0,0)まで ≈0.14、中心(0.3,0.3)まで ≈0.28。
        // グリッドの方が近いが、優先度で中心が勝つ。
        let r = snap(&doc, Point2::new(0.1, 0.1), 0.5, 1.0, &[]).unwrap();
        assert_eq!(r.kind, SnapKind::Center);
        assert_eq!(r.point, Point2::new(0.3, 0.3));
    }

    #[test]
    fn grid_is_fallback_when_nothing_else_in_range() {
        // エンティティのない空ドキュメントでもグリッドにはスナップする。
        let doc = Document::new();
        let r = snap(&doc, Point2::new(0.2, -0.1), 0.5, 1.0, &[]).unwrap();
        assert_eq!(r.kind, SnapKind::Grid);
        assert_eq!(r.point, Point2::new(0.0, 0.0));
    }

    #[test]
    fn nearest_within_same_kind_is_chosen() {
        // 交差しない 2 本の水平線分。端点 (0,0) と (0.5,0) がどちらも半径内。
        // A (0,0)-(-5,0): 端点 (0,0),(-5,0)。B (0.5,0)-(5,0): 端点 (0.5,0),(5,0)。
        // 同一直線上だが区間が離れており交点はない。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, -5.0, 0.0));
        add(&mut doc, line(0.5, 0.0, 5.0, 0.0));

        // カーソル (0.3,0): 端点(0,0)まで 0.3、端点(0.5,0)まで 0.2。近い方 (0.5,0)。
        let r = snap_no_grid(&doc, Point2::new(0.3, 0.0), 0.4).unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(0.5, 0.0));
    }

    #[test]
    fn candidate_just_outside_radius_is_ignored() {
        let mut doc = Document::new();
        add(&mut doc, Shape::Point(Point2::new(0.0, 0.0)));
        // 距離 1.0 ちょうどより小さい半径なら候補外。
        assert_eq!(snap_no_grid(&doc, Point2::new(1.0, 0.0), 0.99), None);
        // 半径を距離以上にすれば候補になる。
        assert!(snap_no_grid(&doc, Point2::new(1.0, 0.0), 1.01).is_some());
    }

    #[test]
    fn hidden_layer_entities_do_not_snap() {
        let mut doc = Document::new();
        add(&mut doc, Shape::Point(Point2::new(0.0, 0.0)));
        // カレントレイヤーを非表示にする。
        let layer_id = doc.current_layer();
        let mut props = doc.layer(layer_id).unwrap().clone();
        props.visible = false;
        doc.apply(Command::SetLayerProps {
            id: layer_id,
            props,
        })
        .unwrap();

        assert_eq!(snap_no_grid(&doc, Point2::new(0.0, 0.0), 1.0), None);
    }

    #[test]
    fn intersection_prefilter_keeps_candidate_at_radius_boundary() {
        // カーソル近傍 AABB による事前絞り込みが無損失であること（半径ちょうどの
        // 交点候補を取りこぼさないこと）の境界ケース。
        // 交点 (100,100) で交差する 2 線分。端点・中点は交点から 5 以上離す。
        // A (80,100)-(110,100): 中点 (95,100)。B (100,70)-(100,110): 中点 (100,90)。
        let mut doc = Document::new();
        add(&mut doc, line(80.0, 100.0, 110.0, 100.0));
        add(&mut doc, line(100.0, 70.0, 100.0, 110.0));

        // カーソルを交点から半径ちょうど（距離 3.0、半径 3.0）離しても交点にスナップする。
        let cursor = Point2::new(103.0, 100.0);
        let r = snap(&doc, cursor, 3.0, 0.0, &[]).unwrap();
        assert_eq!(r.kind, SnapKind::Intersection);
        assert_eq!(r.point, Point2::new(100.0, 100.0));
    }

    #[test]
    fn intersections_far_from_cursor_do_not_snap() {
        // 交点 (100,100) を持つ 2 線分があっても、カーソルがそこから半径外に
        // 離れていれば交点候補にならない（事前絞り込み経路でも半径規則が保たれる）。
        let mut doc = Document::new();
        add(&mut doc, line(80.0, 100.0, 110.0, 100.0));
        add(&mut doc, line(100.0, 70.0, 100.0, 110.0));

        // カーソル (0,0): 交点まで距離 ≈141。半径 3 では何にもスナップしない。
        assert_eq!(snap(&doc, Point2::new(0.0, 0.0), 3.0, 0.0, &[]), None);
    }

    #[test]
    fn extra_point_within_radius_snaps_as_endpoint() {
        // Document は空でグリッドのみ候補になる状況で、extra_points（作図中ツールの
        // 未確定頂点）を渡すと、より優先度の高い端点として選ばれる。
        let doc = Document::new();
        let extra = [Point2::new(0.05, 0.0)];
        let r = snap(&doc, Point2::new(0.0, 0.0), 0.5, 1.0, &extra).unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(0.05, 0.0));
    }

    #[test]
    fn text_anchor_snaps_as_endpoint() {
        use mcad_core::TextGeom;
        // Text エンティティのアンカーだけが端点候補になる（DESIGN.md M6 L386）。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(2.0, 3.0),
                content: "abc".to_owned(),
                height: 1.0,
                angle: 0.0,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let r = snap_no_grid(&doc, Point2::new(2.1, 3.0), 0.5).unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(2.0, 3.0));
    }

    #[test]
    fn extra_point_outside_radius_is_ignored() {
        let doc = Document::new();
        let extra = [Point2::new(10.0, 10.0)];
        // 半径外なので extra_points は候補にならず、グリッドにフォールバックする。
        let r = snap(&doc, Point2::new(0.1, -0.1), 0.5, 1.0, &extra).unwrap();
        assert_eq!(r.kind, SnapKind::Grid);
        assert_eq!(r.point, Point2::new(0.0, 0.0));
    }
}
