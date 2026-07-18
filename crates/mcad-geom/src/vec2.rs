//! 2 次元ベクトル型と基本演算。

use std::ops::{Add, Div, Mul, Neg, Sub};

use serde::{Deserialize, Serialize};

/// 2 次元ベクトル（方向・変位）。座標は f64。
///
/// 「位置」を表す [`crate::Point2`] とは意図的に別型にしている。
/// 点どうしの差が [`Vec2`]、点 + ベクトルが点、という代数を型で担保する。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    /// x 成分。
    pub x: f64,
    /// y 成分。
    pub y: f64,
}

impl Vec2 {
    /// ゼロベクトル。
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    /// 成分からベクトルを作る。
    #[inline]
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// 内積 `self · other`。
    #[inline]
    #[must_use]
    pub fn dot(self, other: Vec2) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// 2 次元の外積（擬スカラー）`self × other = self.x*other.y - self.y*other.x`。
    ///
    /// 符号は反時計回り（CCW）が正。平行判定や左右判定に使う。
    #[inline]
    #[must_use]
    pub fn cross(self, other: Vec2) -> f64 {
        self.x * other.y - self.y * other.x
    }

    /// 長さの 2 乗（平方根を避けたい比較用）。
    #[inline]
    #[must_use]
    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    /// 長さ（ノルム）。
    #[inline]
    #[must_use]
    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    /// 左に 90 度回した垂直ベクトル `(-y, x)`。
    #[inline]
    #[must_use]
    pub fn perp(self) -> Vec2 {
        Vec2::new(-self.y, self.x)
    }

    /// x 軸からの角度（ラジアン、`atan2` ベース、範囲は `(-π, π]`）。
    #[inline]
    #[must_use]
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }

    /// 正規化した単位ベクトルを返す。長さがほぼ 0 の場合は `None`。
    ///
    /// 長さ 0 での 0 除算による NaN を避けるため、[`crate::EPS`] で判定する。
    #[inline]
    #[must_use]
    pub fn normalize(self) -> Option<Vec2> {
        let len = self.length();
        if len <= crate::EPS {
            None
        } else {
            Some(self / len)
        }
    }

    /// 原点まわりに反時計回り（CCW）で `angle` ラジアン回転したベクトル。
    ///
    /// 標準の回転行列 `(x·cos − y·sin, x·sin + y·cos)` を適用する。
    /// 点の回転は「中心を原点へ移す変位に対して本メソッドを適用し、中心を足し戻す」
    /// 形で組み立てる（各プリミティブの `rotated` を参照）。
    #[inline]
    #[must_use]
    pub fn rotated(self, angle: f64) -> Vec2 {
        let (sin, cos) = angle.sin_cos();
        Vec2::new(self.x * cos - self.y * sin, self.x * sin + self.y * cos)
    }

    /// 原点を通り方向 `axis_dir` の直線に対して反射したベクトル。
    ///
    /// `axis_dir` は正規化されていなくてよい（内部で正規化する）。軸方向の成分を
    /// 保持し、それに直交する成分を反転する（`2·proj − self`）。
    ///
    /// `axis_dir` が退化（ほぼゼロベクトル）で軸の向きが定まらない場合は、安全な
    /// フォールバックとして `self` をそのまま返す（恒等写像）。呼び出し側で異常系を
    /// 作らないための措置。
    #[inline]
    #[must_use]
    pub fn reflected(self, axis_dir: Vec2) -> Vec2 {
        match axis_dir.normalize() {
            Some(d) => d * (2.0 * self.dot(d)) - self,
            None => self,
        }
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, s: f64) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;
    #[inline]
    fn mul(self, v: Vec2) -> Vec2 {
        v * self
    }
}

impl Div<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, s: f64) -> Vec2 {
        Vec2::new(self.x / s, self.y / s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: f64 = 1e-12;

    #[test]
    fn dot_and_cross() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(3.0, 4.0);
        assert!((a.dot(b) - 11.0).abs() < T);
        assert!((a.cross(b) - (-2.0)).abs() < T);
    }

    #[test]
    fn cross_sign_is_ccw_positive() {
        let x = Vec2::new(1.0, 0.0);
        let y = Vec2::new(0.0, 1.0);
        assert!(x.cross(y) > 0.0);
    }

    #[test]
    fn length_and_normalize() {
        let v = Vec2::new(3.0, 4.0);
        assert!((v.length() - 5.0).abs() < T);
        let u = v.normalize().unwrap();
        assert!((u.length() - 1.0).abs() < T);
    }

    #[test]
    fn zero_normalize_is_none() {
        assert!(Vec2::ZERO.normalize().is_none());
    }

    #[test]
    fn arithmetic() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(4.0, 6.0);
        assert_eq!(a + b, Vec2::new(5.0, 8.0));
        assert_eq!(b - a, Vec2::new(3.0, 4.0));
        assert_eq!(-a, Vec2::new(-1.0, -2.0));
        assert_eq!(a * 2.0, Vec2::new(2.0, 4.0));
        assert_eq!(2.0 * a, Vec2::new(2.0, 4.0));
        assert_eq!(b / 2.0, Vec2::new(2.0, 3.0));
    }

    #[test]
    fn perp_is_left_turn() {
        assert_eq!(Vec2::new(1.0, 0.0).perp(), Vec2::new(0.0, 1.0));
    }
}
