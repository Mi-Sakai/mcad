//! 2 次元の位置（点）型。

use std::ops::{Add, Sub};

use serde::{Deserialize, Serialize};

use crate::Vec2;

/// 2 次元の位置。座標は f64（CAD の座標精度要件のため必須）。
///
/// 変位を表す [`Vec2`] とは別型。点 − 点 = ベクトル、点 + ベクトル = 点、
/// という代数を型で表現する。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    /// x 座標。
    pub x: f64,
    /// y 座標。
    pub y: f64,
}

impl Point2 {
    /// 原点 `(0, 0)`。
    pub const ORIGIN: Point2 = Point2 { x: 0.0, y: 0.0 };

    /// 座標から点を作る。
    #[inline]
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// 原点からの位置ベクトルとして見る。
    #[inline]
    #[must_use]
    pub fn to_vec2(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    /// 位置ベクトルから点を作る。
    #[inline]
    #[must_use]
    pub fn from_vec2(v: Vec2) -> Self {
        Point2::new(v.x, v.y)
    }

    /// 2 点間の距離の 2 乗（平方根を避けたい比較用）。
    #[inline]
    #[must_use]
    pub fn distance_squared(self, other: Point2) -> f64 {
        (other - self).length_squared()
    }

    /// 2 点間のユークリッド距離。
    #[inline]
    #[must_use]
    pub fn distance(self, other: Point2) -> f64 {
        (other - self).length()
    }

    /// 中点。
    #[inline]
    #[must_use]
    pub fn midpoint(self, other: Point2) -> Point2 {
        self.lerp(other, 0.5)
    }

    /// `self` から `other` へ係数 `t` で線形補間する（`t=0` で `self`、`t=1` で `other`）。
    #[inline]
    #[must_use]
    pub fn lerp(self, other: Point2, t: f64) -> Point2 {
        self + (other - self) * t
    }
}

impl Sub for Point2 {
    type Output = Vec2;
    /// 点 − 点 = 変位ベクトル。
    #[inline]
    fn sub(self, rhs: Point2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Add<Vec2> for Point2 {
    type Output = Point2;
    /// 点 + ベクトル = 点。
    #[inline]
    fn add(self, rhs: Vec2) -> Point2 {
        Point2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub<Vec2> for Point2 {
    type Output = Point2;
    /// 点 − ベクトル = 点。
    #[inline]
    fn sub(self, rhs: Vec2) -> Point2 {
        Point2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: f64 = 1e-12;

    #[test]
    fn point_minus_point_is_vec() {
        let a = Point2::new(1.0, 1.0);
        let b = Point2::new(4.0, 5.0);
        assert_eq!(b - a, Vec2::new(3.0, 4.0));
    }

    #[test]
    fn point_plus_and_minus_vec() {
        let p = Point2::new(1.0, 2.0);
        let v = Vec2::new(3.0, 4.0);
        assert_eq!(p + v, Point2::new(4.0, 6.0));
        assert_eq!(p - v, Point2::new(-2.0, -2.0));
    }

    #[test]
    fn distance_and_midpoint() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(3.0, 4.0);
        assert!((a.distance(b) - 5.0).abs() < T);
        assert!((a.distance_squared(b) - 25.0).abs() < T);
        assert_eq!(a.midpoint(b), Point2::new(1.5, 2.0));
    }

    #[test]
    fn lerp_endpoints() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(10.0, 20.0);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        assert_eq!(a.lerp(b, 0.5), Point2::new(5.0, 10.0));
    }
}
