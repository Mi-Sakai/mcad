//! 幾何プリミティブ（[`LineSeg`] / [`Circle`] / [`Arc`] / [`Polyline`]）、
//! それらをまとめる [`Shape`]、および最近点・距離クエリ。

use std::f64::consts::TAU;

use serde::{Deserialize, Serialize};

use crate::{Aabb, Point2, Vec2};

/// 角度 `a`（ラジアン）を `[0, TAU)` に正規化する。
///
/// `rem_euclid` を使うため負角も正しく折り返す。
#[inline]
#[must_use]
pub(crate) fn wrap_2pi(a: f64) -> f64 {
    a.rem_euclid(TAU)
}

/// 線分。始点 `a` と終点 `b`。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LineSeg {
    /// 始点。
    pub a: Point2,
    /// 終点。
    pub b: Point2,
}

impl LineSeg {
    /// 始点・終点から線分を作る。
    #[inline]
    #[must_use]
    pub const fn new(a: Point2, b: Point2) -> Self {
        Self { a, b }
    }

    /// 始点から終点への方向ベクトル（正規化しない）。
    #[inline]
    #[must_use]
    pub fn direction(&self) -> Vec2 {
        self.b - self.a
    }

    /// 長さ。
    #[inline]
    #[must_use]
    pub fn length(&self) -> f64 {
        self.direction().length()
    }

    /// 軸並行境界ボックス。
    #[inline]
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        Aabb::from_corners(self.a, self.b)
    }

    /// 点 `p` に最も近い線分上の点。
    ///
    /// 直線へ射影したパラメータ `t` を `[0, 1]` にクランプする。
    /// 退化した（長さ 0 の）線分では始点を返す。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        let d = self.direction();
        let len2 = d.length_squared();
        if len2 <= crate::EPS * crate::EPS {
            return self.a;
        }
        let t = ((p - self.a).dot(d) / len2).clamp(0.0, 1.0);
        self.a + d * t
    }

    /// 変位 `delta` だけ平行移動した新しい線分。
    #[inline]
    #[must_use]
    pub fn translated(self, delta: Vec2) -> LineSeg {
        LineSeg::new(self.a + delta, self.b + delta)
    }
}

/// 円。中心 `center` と半径 `radius`。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    /// 中心。
    pub center: Point2,
    /// 半径（非負を想定）。
    pub radius: f64,
}

impl Circle {
    /// 中心・半径から円を作る。
    #[inline]
    #[must_use]
    pub const fn new(center: Point2, radius: f64) -> Self {
        Self { center, radius }
    }

    /// 角度 `theta`（ラジアン）における円周上の点。
    #[inline]
    #[must_use]
    pub fn point_at_angle(&self, theta: f64) -> Point2 {
        self.center + Vec2::new(theta.cos(), theta.sin()) * self.radius
    }

    /// 軸並行境界ボックス。
    #[inline]
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        let r = Vec2::new(self.radius, self.radius);
        Aabb {
            min: self.center - r,
            max: self.center + r,
        }
    }

    /// 点 `p` に最も近い円周上の点。
    ///
    /// `p` が中心とほぼ一致する場合は方向が定まらないため、角度 0 の点
    /// （`center + (radius, 0)`）を返す。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        match (p - self.center).normalize() {
            Some(dir) => self.center + dir * self.radius,
            None => self.center + Vec2::new(self.radius, 0.0),
        }
    }

    /// 変位 `delta` だけ平行移動した新しい円（中心のみ動き、半径は不変）。
    #[inline]
    #[must_use]
    pub fn translated(self, delta: Vec2) -> Circle {
        Circle::new(self.center + delta, self.radius)
    }
}

/// 円弧。中心・半径・開始角・終了角。掃引は開始角から **反時計回り（CCW）**。
///
/// # 角度の扱い
///
/// 掃引角は `wrap_2pi(end_angle - start_angle)`（`[0, TAU)`）として解釈する。
/// したがって `start_angle == end_angle` は掃引 0 の退化した弧（端点のみ）を意味し、
/// 全周（360°）は本型では表現しない（全周は [`Circle`] を使う）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Arc {
    /// 中心。
    pub center: Point2,
    /// 半径。
    pub radius: f64,
    /// 開始角（ラジアン）。
    pub start_angle: f64,
    /// 終了角（ラジアン）。開始角から CCW に掃引する。
    pub end_angle: f64,
}

impl Arc {
    /// 各要素から円弧を作る。
    #[inline]
    #[must_use]
    pub const fn new(center: Point2, radius: f64, start_angle: f64, end_angle: f64) -> Self {
        Self {
            center,
            radius,
            start_angle,
            end_angle,
        }
    }

    /// 台となる円。
    #[inline]
    #[must_use]
    pub fn circle(&self) -> Circle {
        Circle::new(self.center, self.radius)
    }

    /// 掃引角（`[0, TAU)`）。
    #[inline]
    #[must_use]
    pub fn sweep(&self) -> f64 {
        wrap_2pi(self.end_angle - self.start_angle)
    }

    /// 始点。
    #[inline]
    #[must_use]
    pub fn start_point(&self) -> Point2 {
        self.circle().point_at_angle(self.start_angle)
    }

    /// 終点。
    #[inline]
    #[must_use]
    pub fn end_point(&self) -> Point2 {
        self.circle().point_at_angle(self.end_angle)
    }

    /// 角度 `theta` が弧の掃引範囲内か（両端点を許容量込みで含む）。
    #[must_use]
    pub fn contains_angle(&self, theta: f64) -> bool {
        let sweep = self.sweep();
        let d = wrap_2pi(theta - self.start_angle);
        // d は [0, TAU)。範囲内（d <= sweep）に加え、始点直前（theta が
        // start をわずかに下回り d ≒ TAU となるケース）も端点として許容する。
        d <= sweep + crate::EPS || d >= TAU - crate::EPS
    }

    /// 軸並行境界ボックス。
    ///
    /// 両端点に加え、弧の範囲内に入る軸方向の極値（角 0, π/2, π, 3π/2）を含める。
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        let mut bb = Aabb::from_corners(self.start_point(), self.end_point());
        // 軸方向の極値: それぞれ +x, +y, -x, -y の最遠点。
        const CARDINALS: [f64; 4] = [
            0.0,
            std::f64::consts::FRAC_PI_2,
            std::f64::consts::PI,
            3.0 * std::f64::consts::FRAC_PI_2,
        ];
        for &a in &CARDINALS {
            if self.contains_angle(a) {
                bb = bb.extended(self.circle().point_at_angle(a));
            }
        }
        bb
    }

    /// 点 `p` に最も近い弧上の点。
    ///
    /// `p` の方向角が弧の範囲内なら円周への射影点を、範囲外なら近い方の端点を返す。
    /// `p` が中心とほぼ一致する場合は始点を返す（弧上のどの点も等距離のため）。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        let dir = p - self.center;
        if dir.length() <= crate::EPS {
            return self.start_point();
        }
        let theta = dir.angle();
        if self.contains_angle(theta) {
            self.circle().point_at_angle(theta)
        } else {
            let s = self.start_point();
            let e = self.end_point();
            if p.distance_squared(s) <= p.distance_squared(e) {
                s
            } else {
                e
            }
        }
    }

    /// 変位 `delta` だけ平行移動した新しい円弧（中心のみ動き、半径・開始/終了角は不変）。
    #[inline]
    #[must_use]
    pub fn translated(self, delta: Vec2) -> Arc {
        Arc::new(
            self.center + delta,
            self.radius,
            self.start_angle,
            self.end_angle,
        )
    }
}

/// ポリライン。頂点列と、閉じているか（始点と終点を結ぶか）のフラグ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polyline {
    /// 頂点列。
    pub vertices: Vec<Point2>,
    /// 閉じているか。`true` なら末尾頂点と先頭頂点を結ぶ閉ループ。
    pub closed: bool,
}

impl Polyline {
    /// 頂点列とクローズフラグからポリラインを作る。
    #[inline]
    #[must_use]
    pub fn new(vertices: Vec<Point2>, closed: bool) -> Self {
        Self { vertices, closed }
    }

    /// 構成する線分を順に列挙する。閉じている場合は末尾→先頭の閉じ線分も含む。
    ///
    /// 頂点が 2 未満なら線分は 0 本。
    pub fn segments(&self) -> impl Iterator<Item = LineSeg> + '_ {
        let n = self.vertices.len();
        let open_count = n.saturating_sub(1);
        // 閉じていて頂点が 2 以上なら閉じ線分を 1 本追加する。
        let closing = usize::from(self.closed && n >= 2);
        (0..open_count + closing).map(move |i| {
            let a = self.vertices[i];
            let b = self.vertices[(i + 1) % n];
            LineSeg::new(a, b)
        })
    }

    /// 軸並行境界ボックス。頂点が空なら `None`。
    #[must_use]
    pub fn aabb(&self) -> Option<Aabb> {
        Aabb::from_points(self.vertices.iter().copied())
    }

    /// 点 `p` に最も近いポリライン上の点。
    ///
    /// 各線分の最近点のうち最も近いものを返す。頂点が 1 個ならその頂点、
    /// 0 個なら `p` 自身（距離 0）を返す。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        let mut best: Option<(f64, Point2)> = None;
        for seg in self.segments() {
            let cp = seg.closest_point(p);
            let d2 = p.distance_squared(cp);
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, cp));
            }
        }
        match best {
            Some((_, cp)) => cp,
            None => self.vertices.first().copied().unwrap_or(p),
        }
    }

    /// 全頂点を変位 `delta` だけ平行移動した新しいポリライン。
    #[must_use]
    pub fn translated(&self, delta: Vec2) -> Polyline {
        Polyline::new(
            self.vertices.iter().map(|v| *v + delta).collect(),
            self.closed,
        )
    }
}

/// エンティティの幾何を表す列挙型。mcad-core の `Entity { geom: Shape, .. }` が使う。
///
/// # 設計メモ
///
/// DESIGN.md 3.1 が列挙する型（[`LineSeg`] / [`Circle`] / [`Arc`] / [`Polyline`]）に加え、
/// [`Shape::Point`] を持たせている。MVP の作図ツールに「点」ツール（DESIGN.md 3.4 / タスク 5）
/// があり、点エンティティを `Shape` として保持する必要があるため。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    /// 点。
    Point(Point2),
    /// 線分。
    Line(LineSeg),
    /// 円。
    Circle(Circle),
    /// 円弧。
    Arc(Arc),
    /// ポリライン。
    Polyline(Polyline),
}

impl Shape {
    /// 軸並行境界ボックス。
    ///
    /// 頂点が空のポリラインは境界を定義できないため、慣例として原点を含む
    /// 退化 AABB を返す（実運用で空ポリラインは作られない前提）。
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        match self {
            Shape::Point(p) => Aabb::from_point(*p),
            Shape::Line(s) => s.aabb(),
            Shape::Circle(c) => c.aabb(),
            Shape::Arc(a) => a.aabb(),
            Shape::Polyline(pl) => pl
                .aabb()
                .unwrap_or_else(|| Aabb::from_point(Point2::ORIGIN)),
        }
    }

    /// 形状全体を変位 `delta` だけ平行移動した新しい形状を返す純関数。
    ///
    /// 選択エンティティの移動（Move）で、確定時に各エンティティの `Shape` を
    /// 平行移動した幾何（`Command::ModifyEntity { new_geom }` に載せる値）を作るのに使う。
    /// GUI 非依存の再利用可能な幾何演算なので、mcad-geom 側に置く（DESIGN 3.1）。
    #[must_use]
    pub fn translated(&self, delta: Vec2) -> Shape {
        match self {
            Shape::Point(p) => Shape::Point(*p + delta),
            Shape::Line(s) => Shape::Line(s.translated(delta)),
            Shape::Circle(c) => Shape::Circle(c.translated(delta)),
            Shape::Arc(a) => Shape::Arc(a.translated(delta)),
            Shape::Polyline(pl) => Shape::Polyline(pl.translated(delta)),
        }
    }
}

/// 3 点 `a`, `b`, `c` を通る円（外接円）を求める。
///
/// 3 点が同一直線上（またはそれに極めて近い）場合は一意な円が定まらないため
/// `None` を返す。判定は `a` を基準にした相対座標での外積を使い、[`crate::EPS`]
/// による相対許容量（`|cross| <= EPS * |ab| * |ac|`、これは他の平行判定と同じ
/// 基準）で行う。
///
/// 数値安定性のため、絶対座標のまま行列式を組むのではなく `a` を原点とした
/// 相対座標（`ab = b - a`, `ac = c - a`）で外心を計算し、最後に `a` を足し戻す
/// （標準的な外心の公式を相対座標へ適用したもの）。
///
/// 円弧作図ツール（3点指定）が、3点を通る円弧を確定するために使う
/// （円の中心・半径が決まれば、各点の中心からの角度で開始角・終了角を求められる）。
#[must_use]
pub fn circumcircle(a: Point2, b: Point2, c: Point2) -> Option<Circle> {
    let ab = b - a;
    let ac = c - a;
    let d = 2.0 * ab.cross(ac);
    if d.abs() <= crate::EPS * ab.length() * ac.length() {
        return None; // 同一直線上（またはほぼ）で外接円が定まらない。
    }
    let ab2 = ab.length_squared();
    let ac2 = ac.length_squared();
    let ux = (ac.y * ab2 - ab.y * ac2) / d;
    let uy = (ab.x * ac2 - ac.x * ab2) / d;
    let center = a + Vec2::new(ux, uy);
    let radius = center.distance(a);
    Some(Circle::new(center, radius))
}

/// 形状 `shape` 上で、点 `p` に最も近い点を返す。
#[must_use]
pub fn closest_point(shape: &Shape, p: Point2) -> Point2 {
    match shape {
        Shape::Point(q) => *q,
        Shape::Line(s) => s.closest_point(p),
        Shape::Circle(c) => c.closest_point(p),
        Shape::Arc(a) => a.closest_point(p),
        Shape::Polyline(pl) => pl.closest_point(p),
    }
}

/// 形状 `shape` と点 `p` の最短距離。ヒットテスト（ピック許容量との比較）に使う。
#[must_use]
pub fn distance_to(shape: &Shape, p: Point2) -> f64 {
    p.distance(closest_point(shape, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: f64 = 1e-9;

    fn approx(a: Point2, b: Point2) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn lineseg_aabb_and_closest() {
        let s = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        let bb = s.aabb();
        assert_eq!(bb.min, Point2::new(0.0, 0.0));
        assert_eq!(bb.max, Point2::new(10.0, 0.0));

        // 線分の外側（上方）→ 垂線の足。
        assert!(approx(
            s.closest_point(Point2::new(3.0, 5.0)),
            Point2::new(3.0, 0.0)
        ));
        // 端の外 → 端点にクランプ。
        assert!(approx(
            s.closest_point(Point2::new(-4.0, 1.0)),
            Point2::new(0.0, 0.0)
        ));
        assert!(approx(
            s.closest_point(Point2::new(20.0, 1.0)),
            Point2::new(10.0, 0.0)
        ));
    }

    #[test]
    fn degenerate_lineseg_closest_is_start() {
        let s = LineSeg::new(Point2::new(2.0, 2.0), Point2::new(2.0, 2.0));
        assert_eq!(
            s.closest_point(Point2::new(9.0, 9.0)),
            Point2::new(2.0, 2.0)
        );
    }

    #[test]
    fn circle_closest_and_distance() {
        let c = Circle::new(Point2::new(0.0, 0.0), 5.0);
        assert!(approx(
            c.closest_point(Point2::new(10.0, 0.0)),
            Point2::new(5.0, 0.0)
        ));
        // 中心 → 角度 0 の点。
        assert!(approx(
            c.closest_point(Point2::new(0.0, 0.0)),
            Point2::new(5.0, 0.0)
        ));

        let shape = Shape::Circle(c);
        assert!((distance_to(&shape, Point2::new(10.0, 0.0)) - 5.0).abs() < T);
        // 内側の点も円周までの距離。
        assert!((distance_to(&shape, Point2::new(2.0, 0.0)) - 3.0).abs() < T);
    }

    #[test]
    fn arc_contains_angle() {
        // 0 から π/2 の弧（第 1 象限）。
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, std::f64::consts::FRAC_PI_2);
        assert!(a.contains_angle(std::f64::consts::FRAC_PI_4));
        assert!(a.contains_angle(0.0));
        assert!(a.contains_angle(std::f64::consts::FRAC_PI_2));
        assert!(!a.contains_angle(std::f64::consts::PI));
        assert!(!a.contains_angle(-std::f64::consts::FRAC_PI_4));
    }

    #[test]
    fn arc_wraps_across_zero() {
        // 7π/4 から π/4（0 をまたぐ 90° 弧）。
        let a = Arc::new(
            Point2::ORIGIN,
            1.0,
            7.0 * std::f64::consts::FRAC_PI_4,
            std::f64::consts::FRAC_PI_4,
        );
        assert!((a.sweep() - std::f64::consts::FRAC_PI_2).abs() < T);
        assert!(a.contains_angle(0.0));
        assert!(!a.contains_angle(std::f64::consts::PI));
    }

    #[test]
    fn arc_aabb_includes_cardinal_extreme() {
        // -π/4 から π/4 の弧。x の最大が (1,0) で、上下端は端点。
        let a = Arc::new(
            Point2::ORIGIN,
            1.0,
            -std::f64::consts::FRAC_PI_4,
            std::f64::consts::FRAC_PI_4,
        );
        let bb = a.aabb();
        // 角 0 が範囲内なので max.x == 1。
        assert!((bb.max.x - 1.0).abs() < 1e-9);
        // y 範囲は端点 ±sin(π/4)。
        assert!((bb.max.y - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert!((bb.min.y + std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn arc_closest_point_out_of_range_returns_endpoint() {
        // 0 から π/2 の弧。第 3 象限の点は始点(1,0)か終点(0,1)の近い方。
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, std::f64::consts::FRAC_PI_2);
        // (2,-3) は角度が範囲外で、始点(1,0)により近い。
        let cp = a.closest_point(Point2::new(2.0, -3.0));
        assert!(approx(cp, Point2::new(1.0, 0.0)));
    }

    #[test]
    fn polyline_segments_open_and_closed() {
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
        ];
        let open = Polyline::new(pts.clone(), false);
        assert_eq!(open.segments().count(), 2);
        let closed = Polyline::new(pts, true);
        assert_eq!(closed.segments().count(), 3);
    }

    #[test]
    fn polyline_closest_point() {
        let pl = Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(10.0, 10.0),
            ],
            false,
        );
        assert!(approx(
            pl.closest_point(Point2::new(5.0, 3.0)),
            Point2::new(5.0, 0.0)
        ));
        assert!(approx(
            pl.closest_point(Point2::new(13.0, 5.0)),
            Point2::new(10.0, 5.0)
        ));
    }

    #[test]
    fn circumcircle_of_points_on_unit_circle() {
        // 単位円上の3点（角度 0, 2π/3, 4π/3）→ 中心 (0,0)、半径 1 が求まるはず。
        let a = Point2::new(1.0, 0.0);
        let b = Point2::new(
            (2.0 * std::f64::consts::FRAC_PI_3).cos(),
            (2.0 * std::f64::consts::FRAC_PI_3).sin(),
        );
        let c = Point2::new(
            (4.0 * std::f64::consts::FRAC_PI_3).cos(),
            (4.0 * std::f64::consts::FRAC_PI_3).sin(),
        );
        let circle = circumcircle(a, b, c).expect("3点は同一直線上ではない");
        assert!(approx(circle.center, Point2::ORIGIN));
        assert!((circle.radius - 1.0).abs() < 1e-9);
    }

    #[test]
    fn circumcircle_of_axis_points() {
        // (0,0),(2,0),(0,2) の外接円は中心(1,1)、半径 sqrt(2)。
        let circle = circumcircle(
            Point2::new(0.0, 0.0),
            Point2::new(2.0, 0.0),
            Point2::new(0.0, 2.0),
        )
        .expect("直角三角形の外心は一意");
        assert!(approx(circle.center, Point2::new(1.0, 1.0)));
        assert!((circle.radius - std::f64::consts::SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn circumcircle_of_collinear_points_is_none() {
        let result = circumcircle(
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(2.0, 2.0),
        );
        assert!(result.is_none());
    }

    #[test]
    fn shape_aabb_dispatch() {
        let s = Shape::Line(LineSeg::new(Point2::new(1.0, 1.0), Point2::new(3.0, 5.0)));
        let bb = s.aabb();
        assert_eq!(bb.min, Point2::new(1.0, 1.0));
        assert_eq!(bb.max, Point2::new(3.0, 5.0));
    }

    #[test]
    fn translated_moves_each_primitive_kind() {
        let d = Vec2::new(2.0, -3.0);

        // Point
        assert_eq!(
            Shape::Point(Point2::new(1.0, 1.0)).translated(d),
            Shape::Point(Point2::new(3.0, -2.0))
        );

        // Line: 両端点が動く
        let line = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0));
        assert_eq!(
            line.translated(d),
            LineSeg::new(Point2::new(2.0, -3.0), Point2::new(6.0, -3.0))
        );

        // Circle: 中心のみ動き半径は不変
        let circle = Circle::new(Point2::new(0.0, 0.0), 5.0);
        let moved = circle.translated(d);
        assert_eq!(moved.center, Point2::new(2.0, -3.0));
        assert!((moved.radius - 5.0).abs() < T);

        // Arc: 中心のみ動き、半径・開始/終了角は不変
        let arc = Arc::new(Point2::new(1.0, 1.0), 2.0, 0.0, std::f64::consts::PI);
        let arc_moved = arc.translated(d);
        assert_eq!(arc_moved.center, Point2::new(3.0, -2.0));
        assert!((arc_moved.radius - 2.0).abs() < T);
        assert_eq!(arc_moved.start_angle, arc.start_angle);
        assert_eq!(arc_moved.end_angle, arc.end_angle);

        // Polyline: 全頂点が動き、closed フラグは不変
        let pl = Polyline::new(vec![Point2::new(0.0, 0.0), Point2::new(1.0, 2.0)], true);
        let pl_moved = pl.translated(d);
        assert_eq!(
            pl_moved.vertices,
            vec![Point2::new(2.0, -3.0), Point2::new(3.0, -1.0)]
        );
        assert!(pl_moved.closed);
    }

    #[test]
    fn translated_by_zero_is_identity() {
        let arc = Shape::Arc(Arc::new(Point2::new(1.0, 2.0), 3.0, 0.5, 2.0));
        assert_eq!(arc.translated(Vec2::ZERO), arc);
    }
}
