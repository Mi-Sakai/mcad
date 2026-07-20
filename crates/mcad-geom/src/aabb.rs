//! 軸並行境界ボックス（AABB）。

use serde::{Deserialize, Serialize};

use crate::{Point2, Vec2};

/// 軸並行境界ボックス。`min <= max`（各成分）を不変条件とする。
///
/// ビューポートカリング・矩形選択・交点候補の事前絞り込みに使う。
/// コンストラクタ [`Aabb::new`] / [`Aabb::from_corners`] / [`Aabb::from_points`]
/// はいずれも成分ごとに min/max を取り直すため、不変条件は常に保たれる。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    /// 最小コーナー（各成分が最小）。
    pub min: Point2,
    /// 最大コーナー（各成分が最大）。
    pub max: Point2,
}

impl Aabb {
    /// 2 点から作る。順序は問わない（成分ごとに min/max を取る）。
    #[inline]
    #[must_use]
    pub fn new(a: Point2, b: Point2) -> Self {
        Self::from_corners(a, b)
    }

    /// 対角の 2 点から作る。順序は問わない。
    #[inline]
    #[must_use]
    pub fn from_corners(a: Point2, b: Point2) -> Self {
        Aabb {
            min: Point2::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point2::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// 1 点だけを含む退化した AABB。
    #[inline]
    #[must_use]
    pub fn from_point(p: Point2) -> Self {
        Aabb { min: p, max: p }
    }

    /// 点列を包む最小の AABB。点が空なら `None`。
    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = Point2>) -> Option<Self> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let mut bb = Aabb::from_point(first);
        for p in iter {
            bb = bb.extended(p);
        }
        Some(bb)
    }

    /// 中心点。
    #[inline]
    #[must_use]
    pub fn center(&self) -> Point2 {
        self.min.midpoint(self.max)
    }

    /// 幅（x 方向のサイズ）。
    #[inline]
    #[must_use]
    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }

    /// 高さ（y 方向のサイズ）。
    #[inline]
    #[must_use]
    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }

    /// サイズ（幅・高さをまとめたベクトル）。
    #[inline]
    #[must_use]
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// 点を含むか（境界上を含む閉領域）。
    #[inline]
    #[must_use]
    pub fn contains_point(&self, p: Point2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// 別の AABB を完全に含むか。
    #[inline]
    #[must_use]
    pub fn contains(&self, other: &Aabb) -> bool {
        self.contains_point(other.min) && self.contains_point(other.max)
    }

    /// 別の AABB と交差（重なり）するか。境界の接触も交差とみなす。
    #[inline]
    #[must_use]
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
    }

    /// 2 つの AABB を包む最小の AABB。
    #[inline]
    #[must_use]
    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min: Point2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    /// 点 `p` を含むように拡張した AABB を返す。
    #[inline]
    #[must_use]
    pub fn extended(&self, p: Point2) -> Aabb {
        Aabb {
            min: Point2::new(self.min.x.min(p.x), self.min.y.min(p.y)),
            max: Point2::new(self.max.x.max(p.x), self.max.y.max(p.y)),
        }
    }

    /// 各方向に `margin` だけ広げた AABB を返す（ピック許容量の付与などに使う）。
    #[inline]
    #[must_use]
    pub fn expanded(&self, margin: f64) -> Aabb {
        let m = Vec2::new(margin, margin);
        Aabb {
            min: self.min - m,
            max: self.max + m,
        }
    }

    /// 点 `p` から AABB までの符号なし距離。内部（境界上を含む）なら `0.0`。
    ///
    /// ピック処理で近似形状（Text の境界枠など）を実形状と同じ「許容量以内で
    /// 最も近い」比較にかけるために使う。
    #[inline]
    #[must_use]
    pub fn distance_to_point(&self, p: Point2) -> f64 {
        let dx = (self.min.x - p.x).max(0.0).max(p.x - self.max.x);
        let dy = (self.min.y - p.y).max(0.0).max(p.y - self.max.y);
        (dx * dx + dy * dy).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_corners_normalizes() {
        let bb = Aabb::from_corners(Point2::new(4.0, 5.0), Point2::new(1.0, 2.0));
        assert_eq!(bb.min, Point2::new(1.0, 2.0));
        assert_eq!(bb.max, Point2::new(4.0, 5.0));
    }

    #[test]
    fn from_points_bounds_all() {
        let pts = [
            Point2::new(0.0, 0.0),
            Point2::new(-2.0, 3.0),
            Point2::new(5.0, -1.0),
        ];
        let bb = Aabb::from_points(pts).unwrap();
        assert_eq!(bb.min, Point2::new(-2.0, -1.0));
        assert_eq!(bb.max, Point2::new(5.0, 3.0));
    }

    #[test]
    fn from_points_empty_is_none() {
        assert!(Aabb::from_points(std::iter::empty()).is_none());
    }

    #[test]
    fn contains_and_intersects() {
        let a = Aabb::from_corners(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let inner = Aabb::from_corners(Point2::new(2.0, 2.0), Point2::new(5.0, 5.0));
        let overlap = Aabb::from_corners(Point2::new(8.0, 8.0), Point2::new(20.0, 20.0));
        let apart = Aabb::from_corners(Point2::new(20.0, 20.0), Point2::new(30.0, 30.0));

        assert!(a.contains(&inner));
        assert!(!a.contains(&overlap));
        assert!(a.intersects(&inner));
        assert!(a.intersects(&overlap));
        assert!(!a.intersects(&apart));
        assert!(a.contains_point(Point2::new(5.0, 5.0)));
        assert!(!a.contains_point(Point2::new(-1.0, 5.0)));
    }

    #[test]
    fn union_and_expand() {
        let a = Aabb::from_corners(Point2::new(0.0, 0.0), Point2::new(2.0, 2.0));
        let b = Aabb::from_corners(Point2::new(3.0, -1.0), Point2::new(4.0, 1.0));
        let u = a.union(&b);
        assert_eq!(u.min, Point2::new(0.0, -1.0));
        assert_eq!(u.max, Point2::new(4.0, 2.0));

        let e = a.expanded(1.0);
        assert_eq!(e.min, Point2::new(-1.0, -1.0));
        assert_eq!(e.max, Point2::new(3.0, 3.0));
    }

    #[test]
    fn distance_to_point_inside_and_outside() {
        let a = Aabb::from_corners(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));

        // 内部・境界上は 0。
        assert_eq!(a.distance_to_point(Point2::new(5.0, 5.0)), 0.0);
        assert_eq!(a.distance_to_point(Point2::new(0.0, 0.0)), 0.0);
        assert_eq!(a.distance_to_point(Point2::new(10.0, 5.0)), 0.0);

        // 軸方向に直線的に離れている場合はその距離そのもの。
        assert_eq!(a.distance_to_point(Point2::new(15.0, 5.0)), 5.0);
        assert_eq!(a.distance_to_point(Point2::new(5.0, -3.0)), 3.0);

        // 斜め（コーナー外）は 3-4-5 のユークリッド距離。
        assert_eq!(a.distance_to_point(Point2::new(13.0, 14.0)), 5.0);
    }
}
