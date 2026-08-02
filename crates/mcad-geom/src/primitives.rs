//! 幾何プリミティブ（[`LineSeg`] / [`Circle`] / [`Arc`] / [`Polyline`]）、
//! それらをまとめる [`Shape`]、および最近点・距離クエリ。

use std::f64::consts::TAU;

use serde::{Deserialize, Serialize};

use crate::{Aabb, Point2, Vec2, point_tol};

/// 線分が退化しているか（長さが端点の座標スケールに対する相対許容量以下か）。
///
/// ゼロ長の絶対判定（`len <= EPS`）だと、原点から遠い座標では「点の一致許容量より
/// 短い線分」を非退化と誤認し、方向ベクトルが丸め誤差そのものになる。判定は
/// [`crate::point_tol`] に揃える（[`crate::EPS`] の doc 参照）。
#[inline]
#[must_use]
fn seg_is_degenerate(seg: &LineSeg) -> bool {
    let tol = point_tol(seg.a, seg.b);
    seg.direction().length_squared() <= tol * tol
}

/// 線分の単位方向ベクトル。退化（[`seg_is_degenerate`]）していれば `None`。
///
/// [`Vec2::normalize`] 単体では相対許容量を使えないので、退化判定だけを座標スケールに
/// 対して相対化してから正規化する。
#[inline]
#[must_use]
fn seg_unit_direction(seg: &LineSeg) -> Option<Vec2> {
    if seg_is_degenerate(seg) {
        return None;
    }
    seg.direction().normalize()
}

/// 角度 `a`（ラジアン）を `[0, TAU)` に正規化する。
///
/// `rem_euclid` を使うため負角も正しく折り返す。
#[inline]
#[must_use]
pub(crate) fn wrap_2pi(a: f64) -> f64 {
    a.rem_euclid(TAU)
}

/// 点 `p` を `pivot` を中心に CCW へ `angle` ラジアン回転する。
#[inline]
#[must_use]
fn rotate_point(p: Point2, pivot: Point2, angle: f64) -> Point2 {
    pivot + (p - pivot).rotated(angle)
}

/// 点 `p` を `axis_a`→`axis_b` を通る直線に対して鏡映する。
#[inline]
#[must_use]
fn mirror_point(p: Point2, axis_a: Point2, axis_b: Point2) -> Point2 {
    axis_a + (p - axis_a).reflected(axis_b - axis_a)
}

/// オフセット（[`Shape::offset`] および各プリミティブの `offset`）が退化して結果を
/// 作れない理由。
///
/// geom は GUI 非依存なので、ここでは理由の列挙のみを返し、ユーザー向け文言は持たない
/// （UI 側 mcad-app が ASCII ステータスメッセージへ対応付ける）。退化仕様は DESIGN.md
/// M5 設計判断5 に従う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetError {
    /// オフセット距離が正の有限値でない（0・負・NaN・∞）。距離ゼロの no-op を含む。
    NonPositiveDistance,
    /// 円・円弧の内側オフセットで距離が半径以上（半径が消滅・反転する）。
    RadiusCollapse,
    /// 対象が退化（ゼロ長線分・全頂点が同一のポリライン）で法線が定義不能。
    Degenerate,
    /// オフセットを定義できない対象（点）。
    Unsupported,
}

/// 距離が「正の有限値」か。オフセット距離の共通ガード（距離ゼロ・負・非有限を弾く）。
#[inline]
fn valid_offset_distance(distance: f64) -> bool {
    distance > 0.0 && distance.is_finite()
}

/// 無限直線どうしの交点。`p1` を通り方向 `d1` の直線と、`p2` を通り方向 `d2` の直線。
///
/// 2 方向がほぼ平行（`|d1 × d2| <= EPS·|d1|·|d2|`、= なす角の sin が EPS 未満）だと
/// 交点が無限遠へ飛び数値的に不安定なため `None` を返す（平行判定は他の幾何述語と
/// 同じ相対 EPS 基準。[`crate::EPS`] の doc 参照）。ポリラインのオフセットで、平行移動
/// した隣接セグメントのマイター交点を求めるのに使う。
#[must_use]
fn line_intersection(p1: Point2, d1: Vec2, p2: Point2, d2: Vec2) -> Option<Point2> {
    let denom = d1.cross(d2);
    if denom.abs() <= crate::EPS * d1.length() * d2.length() {
        return None;
    }
    // p1 + t·d1 = p2 + u·d2 を d2 との外積で解く（u を消去）。
    let t = (p2 - p1).cross(d2) / denom;
    Some(p1 + d1 * t)
}

/// ポリラインオフセットのマイター上限（頂点→マイター交点の距離 ÷ オフセット距離）。
///
/// 浅い角では平行判定（`sin(角) <= EPS`）をわずかに超えただけでもマイター交点がオフセット
/// 距離の数百万倍へ飛び、有限値なので検証を通過して確定され、fit で図面が見かけ上空になる。
/// これを防ぐため、マイター長が `MITER_LIMIT × distance` を超えたらベベルへフォールバックする。
/// 値 4.0 は SVG/Canvas の `stroke-miterlimit` 既定と同じ（内角およそ 29° 以下でベベル化）で、
/// 実用的な角では鋭いマイター角を保ちつつ、スパイクだけを抑える定番のしきい値。
const MITER_LIMIT: f64 = 4.0;

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
    /// 退化した（長さが端点の座標スケールに対する相対許容量以下の）線分では始点を返す
    /// （そのとき始点と終点は [`crate::point_tol`] の意味で同一点なので、どちらを返しても
    /// 許容量内で等価）。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        if seg_is_degenerate(self) {
            return self.a;
        }
        let d = self.direction();
        let t = ((p - self.a).dot(d) / d.length_squared()).clamp(0.0, 1.0);
        self.a + d * t
    }

    /// 変位 `delta` だけ平行移動した新しい線分。
    #[inline]
    #[must_use]
    pub fn translated(self, delta: Vec2) -> LineSeg {
        LineSeg::new(self.a + delta, self.b + delta)
    }

    /// `pivot` を中心に反時計回り（CCW）へ `angle` ラジアン回転した新しい線分。
    ///
    /// 両端点をそれぞれ `pivot + (p − pivot).rotated(angle)` へ写す。
    #[inline]
    #[must_use]
    pub fn rotated(self, pivot: Point2, angle: f64) -> LineSeg {
        LineSeg::new(
            rotate_point(self.a, pivot, angle),
            rotate_point(self.b, pivot, angle),
        )
    }

    /// `axis_a`→`axis_b` を通る直線に対して鏡映した新しい線分。
    ///
    /// 両端点をそれぞれ `axis_a + (p − axis_a).reflected(axis_b − axis_a)` へ写す。
    #[inline]
    #[must_use]
    pub fn mirrored(self, axis_a: Point2, axis_b: Point2) -> LineSeg {
        LineSeg::new(
            mirror_point(self.a, axis_a, axis_b),
            mirror_point(self.b, axis_a, axis_b),
        )
    }

    /// 距離 `distance`（正の有限値）だけ、参照点 `toward` のある側へ法線平行移動した線分。
    ///
    /// 側は「線分を含む無限直線に対して `toward` がどちら側にあるか」で決める（最近点
    /// → `toward` の向き。DESIGN.md M5 設計判断5）。両端点を同じ単位法線ベクトルで
    /// 平行移動するため、長さと向きは保存される。
    ///
    /// # Errors
    ///
    /// - [`OffsetError::NonPositiveDistance`] — `distance` が正の有限値でない。
    /// - [`OffsetError::Degenerate`] — 退化した線分（長さが [`crate::point_tol`] 以下）で
    ///   法線が定義できない。
    pub fn offset(self, distance: f64, toward: Point2) -> Result<LineSeg, OffsetError> {
        if !valid_offset_distance(distance) {
            return Err(OffsetError::NonPositiveDistance);
        }
        let Some(unit_dir) = seg_unit_direction(&self) else {
            return Err(OffsetError::Degenerate);
        };
        let normal = unit_dir.perp();
        // `toward` のある側を向く単位法線に符号を合わせる（perp は左法線）。
        let unit = if (toward - self.a).dot(normal) >= 0.0 {
            normal
        } else {
            -normal
        };
        let off = unit * distance;
        Ok(LineSeg::new(self.a + off, self.b + off))
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
    /// `p` が中心とほぼ一致する（座標スケールに対する相対許容量 [`crate::point_tol`]
    /// 以内）場合は方向が定まらないため、角度 0 の点（`center + (radius, 0)`）を返す。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        let dir = p - self.center;
        let len = dir.length();
        if len <= point_tol(p, self.center) {
            return self.center + Vec2::new(self.radius, 0.0);
        }
        self.center + dir * (self.radius / len)
    }

    /// 変位 `delta` だけ平行移動した新しい円（中心のみ動き、半径は不変）。
    #[inline]
    #[must_use]
    pub fn translated(self, delta: Vec2) -> Circle {
        Circle::new(self.center + delta, self.radius)
    }

    /// `pivot` を中心に CCW へ `angle` ラジアン回転した新しい円。
    ///
    /// 円の中心 `self.center` のみ回転し、半径は不変（回転は等長変換）。
    #[inline]
    #[must_use]
    pub fn rotated(self, pivot: Point2, angle: f64) -> Circle {
        Circle::new(rotate_point(self.center, pivot, angle), self.radius)
    }

    /// `axis_a`→`axis_b` を通る直線に対して鏡映した新しい円。
    ///
    /// 中心のみ鏡映し、半径は不変（鏡映は等長変換）。
    #[inline]
    #[must_use]
    pub fn mirrored(self, axis_a: Point2, axis_b: Point2) -> Circle {
        Circle::new(mirror_point(self.center, axis_a, axis_b), self.radius)
    }

    /// 距離 `distance`（正の有限値）だけ半径を増減したオフセット円。中心は不変。
    ///
    /// `toward` が中心より外側（中心からの距離 >= 半径）なら外側オフセット
    /// （`radius + distance`）、内側なら内側オフセット（`radius − distance`）とする
    /// （DESIGN.md M5 設計判断5）。
    ///
    /// # Errors
    ///
    /// - [`OffsetError::NonPositiveDistance`] — `distance` が正の有限値でない。
    /// - [`OffsetError::RadiusCollapse`] — 内側オフセットで `distance >= radius`（半径が消滅・反転する）。
    pub fn offset(self, distance: f64, toward: Point2) -> Result<Circle, OffsetError> {
        if !valid_offset_distance(distance) {
            return Err(OffsetError::NonPositiveDistance);
        }
        let outward = (toward - self.center).length() >= self.radius;
        let new_radius = if outward {
            self.radius + distance
        } else if distance >= self.radius {
            // 内側で半径以上のオフセットは半径が 0 以下になり退化するため拒否する。
            return Err(OffsetError::RadiusCollapse);
        } else {
            self.radius - distance
        };
        Ok(Circle::new(self.center, new_radius))
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
    ///
    /// 許容量は角度（ラジアン）空間なので [`crate::EPS`] を直接使う。角度は一様な
    /// 拡大縮小に対して不変で、半径 R の弧では弧長 `EPS * R` に相当する
    /// （＝もともと寸法に対して相対的）ため、[`crate::rel_tol`] による相対化は行わない。
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
    /// `p` が中心とほぼ一致する（相対許容量 [`crate::point_tol`] 以内）場合は始点を返す
    /// （弧上のどの点も等距離のため）。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        let dir = p - self.center;
        if dir.length() <= point_tol(p, self.center) {
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

    /// `pivot` を中心に CCW へ `angle` ラジアン回転した新しい円弧。
    ///
    /// 弧の中心を回転し、開始角・終了角にはともに `angle` を加える（回転は掃引の
    /// 向きを保つため、開始・終了の入れ替えは不要）。半径は不変。角度の正規化は
    /// 行わない（本型は開始/終了角を非正規化のまま保持する。掃引は
    /// [`Arc::sweep`] が `wrap_2pi` で解釈する）。
    #[inline]
    #[must_use]
    pub fn rotated(self, pivot: Point2, angle: f64) -> Arc {
        Arc::new(
            rotate_point(self.center, pivot, angle),
            self.radius,
            self.start_angle + angle,
            self.end_angle + angle,
        )
    }

    /// `axis_a`→`axis_b` を通る直線に対して鏡映した新しい円弧。半径は不変。
    ///
    /// # 掃引の向きの補正
    ///
    /// 鏡映は向きを反転させる等長変換なので、開始角・終了角を単純に個別へ反射する
    /// だけでは CCW 掃引の意味が壊れる。軸の方向角を `alpha = (axis_b − axis_a).angle()`
    /// とすると、軸を通る直線に対する角度の反射は `reflect_angle(θ) = 2·alpha − θ`
    /// で与えられる（角度の反射は方向のみに依存し、軸の位置 `axis_a` には依存しない）。
    /// 反射で周回方向が逆転する分を、開始角と終了角を入れ替えて帳尻を合わせる:
    /// `new_start = reflect_angle(end_angle)`、`new_end = reflect_angle(start_angle)`。
    /// これにより「元の弧が start→end へ CCW に掃引する」なら「鏡映後の弧も
    /// new_start→new_end へ CCW に掃引する」ことが保証される。
    ///
    /// # 退化軸のガード
    ///
    /// 軸 2 点がほぼ同一（`|axis_b − axis_a| <= point_tol(axis_a, axis_b)`）だと軸の向きが定まらず、
    /// `alpha = (axis_b − axis_a).angle()` が `atan2(0, 0) = 0` に化ける。すると
    /// 中心は [`mirror_point`]（内部で [`Vec2::reflected`] が退化軸を恒等写像として
    /// 扱う）により不変のまま、開始/終了角だけが「x 軸に対する反射」で反転してしまい、
    /// 中心と角度が食い違った不整合な弧になる。これを避けるため、退化軸では無変換で
    /// `self` のクローンをそのまま返す（他プリミティブの `mirrored` は
    /// [`Vec2::reflected`] のガードにより退化軸で自動的に恒等になるが、`Arc` は角度の
    /// 反射を別途行うため明示的にガードする。geom は tcad からも直接使われるので防御
    /// する。DESIGN.md M5 設計判断5）。
    #[inline]
    #[must_use]
    pub fn mirrored(self, axis_a: Point2, axis_b: Point2) -> Arc {
        // 退化軸（2 点がほぼ同一）は無変換で自身を返す（上記 doc 参照）。
        if (axis_b - axis_a).length() <= point_tol(axis_a, axis_b) {
            return self;
        }
        let alpha = (axis_b - axis_a).angle();
        let reflect_angle = |theta: f64| 2.0 * alpha - theta;
        Arc::new(
            mirror_point(self.center, axis_a, axis_b),
            self.radius,
            reflect_angle(self.end_angle),
            reflect_angle(self.start_angle),
        )
    }

    /// 距離 `distance`（正の有限値）だけ半径を増減したオフセット円弧。中心・開始/終了角は
    /// 不変（同心の弧）。
    ///
    /// 半径の増減は台円（[`Arc::circle`]）の [`Circle::offset`] に委譲する（外側／内側の
    /// 判定・退化ガードを共有する）。中心・角度が不変なので、掃引の向きや角度範囲は
    /// そのまま保存される（DESIGN.md M5 設計判断5）。
    ///
    /// # 通過点方式との関係
    ///
    /// 内外の側は `toward` の中心からの放射方向（`|toward − center|` と `radius` の大小）で
    /// 決まり、`toward` の角度は結果に影響しない。したがって通過点方式では距離も放射距離
    /// `| |toward − center| − radius |` を渡すこと（UI 側 `through_point_distance` が担当）。
    /// `toward` の角度が掃引範囲外でも同心弧を返すが、その場合は結果の弧が通過点を通らない
    /// （放射方向には正しく載るが弧の角度範囲外になる）。これは仕様として許容する。
    ///
    /// # Errors
    ///
    /// [`Circle::offset`] と同じ（[`OffsetError::NonPositiveDistance`] /
    /// [`OffsetError::RadiusCollapse`]）。
    pub fn offset(self, distance: f64, toward: Point2) -> Result<Arc, OffsetError> {
        let base = self.circle().offset(distance, toward)?;
        Ok(Arc::new(
            base.center,
            base.radius,
            self.start_angle,
            self.end_angle,
        ))
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

    /// 全頂点を `pivot` を中心に CCW へ `angle` ラジアン回転した新しいポリライン。
    ///
    /// `closed` フラグは不変。
    #[must_use]
    pub fn rotated(&self, pivot: Point2, angle: f64) -> Polyline {
        Polyline::new(
            self.vertices
                .iter()
                .map(|v| rotate_point(*v, pivot, angle))
                .collect(),
            self.closed,
        )
    }

    /// 全頂点を `axis_a`→`axis_b` を通る直線に対して鏡映した新しいポリライン。
    ///
    /// `closed` フラグは不変。
    #[must_use]
    pub fn mirrored(&self, axis_a: Point2, axis_b: Point2) -> Polyline {
        Polyline::new(
            self.vertices
                .iter()
                .map(|v| mirror_point(*v, axis_a, axis_b))
                .collect(),
            self.closed,
        )
    }

    /// 距離 `distance`（正の有限値）だけ、参照点 `toward` のある側へオフセットした
    /// ポリライン（DESIGN.md M5 設計判断3・設計判断5 の「単純方式」）。
    ///
    /// # アルゴリズム
    ///
    /// 1. **側の決定**: `toward` に最も近い非退化セグメントの左法線符号で、全体の
    ///    オフセット方向（符号付き距離 `s`）を1つ決める。ポリライン全体を一貫した
    ///    側へずらすため、側はセグメントごとではなく1回だけ決める。
    /// 2. **各セグメントの平行移動**: セグメント `i` を左法線 `perp(dir_i)` 方向へ
    ///    `s` だけ平行移動する（`s` は全セグメント共通なので、パスに沿って一貫した
    ///    側になる）。
    /// 3. **頂点の再構成（マイター接続）**: 各元頂点を、そこで交わる「平行移動後の
    ///    隣接2セグメントを含む無限直線」の交点へ置き換える（閉じている場合は末尾↔先頭も
    ///    接続する。開いている場合の両端頂点は、唯一隣接するセグメントの平行移動先を使う）。
    /// 4. **ベベルへのフォールバック**: 次のいずれかならマイターを諦めてベベルにする:
    ///    (a) 隣接セグメントがほぼ平行で交点が数値的に不安定（[`line_intersection`] が `None`）、
    ///    (b) マイター長（頂点→交点の距離）が `MITER_LIMIT × distance` を超える（浅い角の
    ///    スパイク防止）。**ベベルは平行移動後の2端点を両方出力する**（頂点は 1→2 個に増える。
    ///    「頂点数維持」は仕様から撤回）。直線継続（2端点がほぼ一致）のときのみ 1 頂点に畳む。
    ///    2端点の中点1点への畳み込みは、180°折り返しで法線が正反対になり中点が元頂点へ退化して
    ///    「オフセットでもベベルでもない」形状になるため採らない（設計判断5 の 2026-07-19 追記）。
    ///
    /// **出力頂点数は元と一致しない**（角の数だけベベルで増えうる）。自己交差は仕様として
    /// 許容する（設計判断3）。
    ///
    /// # Errors
    ///
    /// - [`OffsetError::NonPositiveDistance`] — `distance` が正の有限値でない。
    /// - [`OffsetError::Degenerate`] — 頂点が2未満、または全セグメントが退化
    ///   （長さが [`crate::point_tol`] 以下）で法線が定義できない。
    pub fn offset(&self, distance: f64, toward: Point2) -> Result<Polyline, OffsetError> {
        if !valid_offset_distance(distance) {
            return Err(OffsetError::NonPositiveDistance);
        }
        let n = self.vertices.len();
        if n < 2 {
            return Err(OffsetError::Degenerate);
        }
        let segs: Vec<LineSeg> = self.segments().collect();
        // 各セグメントの左法線単位ベクトル。退化セグメント（[`seg_is_degenerate`]）は `None`。
        let normals: Vec<Option<Vec2>> = segs
            .iter()
            .map(|s| seg_unit_direction(s).map(Vec2::perp))
            .collect();

        // 側の符号を、`toward` に最も近い非退化セグメントの左法線で決める。
        let mut best: Option<(f64, usize)> = None;
        for (i, s) in segs.iter().enumerate() {
            if normals[i].is_none() {
                continue;
            }
            let d2 = toward.distance_squared(s.closest_point(toward));
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, i));
            }
        }
        let Some((_, ci)) = best else {
            // 非退化セグメントが1本もない（全頂点が同一点）。
            return Err(OffsetError::Degenerate);
        };
        let closest_normal = normals[ci].expect("closest index is non-degenerate");
        let signed_distance = if (toward - segs[ci].a).dot(closest_normal) >= 0.0 {
            distance
        } else {
            -distance
        };

        // 各セグメントのオフセット変位（左法線 × 符号付き距離）。退化は `None`。
        let offs: Vec<Option<Vec2>> = normals
            .iter()
            .map(|nrm| nrm.map(|u| u * signed_distance))
            .collect();

        // 頂点 `j` に接続する「入る側／出る側」のセグメント index を返す。
        //   入る側 = 終点が v_j のセグメント、出る側 = 始点が v_j のセグメント。
        let seg_count = segs.len();
        let incident = |j: usize| -> (Option<usize>, Option<usize>) {
            if self.closed {
                // 閉: seg_count == n。v_j へ入るのは seg (j+n-1)%n、出るのは seg j。
                (Some((j + n - 1) % n), Some(j % seg_count))
            } else {
                // 開: seg_count == n-1。両端頂点は片側のみ。
                let inc = j.checked_sub(1).filter(|&i| i < seg_count);
                let out = (j < seg_count).then_some(j);
                (inc, out)
            }
        };

        // 平行移動後の「オフセット直線」を (直線上の1点, 方向) で表す。退化セグメントは None。
        let offset_line = |seg_idx: usize, vertex: Point2| -> Option<(Point2, Vec2)> {
            offs[seg_idx].map(|o| (vertex + o, segs[seg_idx].direction()))
        };

        // 出力頂点はベベルで増えうるので下限のみ確保する。
        let mut out_vertices = Vec::with_capacity(n);
        for j in 0..n {
            let v = self.vertices[j];
            let (inc, out) = incident(j);
            let inc_line = inc.and_then(|i| offset_line(i, v));
            let out_line = out.and_then(|i| offset_line(i, v));
            match (inc_line, out_line) {
                (Some((pi, di)), Some((po, dou))) => {
                    // マイター交点。ただし平行（交点なし）か、マイター長が上限超（浅い角の
                    // スパイク）ならベベルへフォールバックする。
                    let miter = line_intersection(pi, di, po, dou)
                        .filter(|q| v.distance(*q) <= MITER_LIMIT * distance);
                    match miter {
                        Some(q) => out_vertices.push(q),
                        None => {
                            // ベベル。平行移動後の端点 `pi`（入る側）・`po`（出る側）は、
                            // 直線継続なら同じ側でほぼ一致するので1頂点に畳む。折返し・鋭角
                            // （法線が逆向き）は中点だと元頂点へ退化するため2端点を両方残す。
                            // 同側判定は変位の内積符号で行う（`perp` は内積を保つので、変位の
                            // 内積符号 = 隣接セグメント方向の内積符号 = 折返しかどうか）。
                            if (pi - v).dot(po - v) >= 0.0 {
                                out_vertices.push(pi);
                            } else {
                                out_vertices.push(pi);
                                out_vertices.push(po);
                            }
                        }
                    }
                }
                // 片側のみ（開ポリラインの端点、または隣が退化）: その平行移動先を使う。
                (Some((pi, _)), None) => out_vertices.push(pi),
                (None, Some((po, _))) => out_vertices.push(po),
                // 両隣とも退化: 元頂点をそのまま残す（最善努力）。
                (None, None) => out_vertices.push(v),
            }
        }
        Ok(Polyline::new(out_vertices, self.closed))
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

    /// 形状全体を `pivot` を中心に CCW へ `angle` ラジアン回転した新しい形状を返す純関数。
    ///
    /// 選択エンティティの回転（Rotate）で、確定時に各エンティティの `Shape` を
    /// 回転した幾何（`Command::ModifyEntity { new_geom }` に載せる値）を作るのに使う。
    /// [`Shape::translated`] と同じく GUI 非依存の再利用可能な幾何演算として mcad-geom
    /// 側に置く（DESIGN M5 設計判断1: app 層での座標いじりを禁止する）。
    #[must_use]
    pub fn rotated(&self, pivot: Point2, angle: f64) -> Shape {
        match self {
            Shape::Point(p) => Shape::Point(rotate_point(*p, pivot, angle)),
            Shape::Line(s) => Shape::Line(s.rotated(pivot, angle)),
            Shape::Circle(c) => Shape::Circle(c.rotated(pivot, angle)),
            Shape::Arc(a) => Shape::Arc(a.rotated(pivot, angle)),
            Shape::Polyline(pl) => Shape::Polyline(pl.rotated(pivot, angle)),
        }
    }

    /// 形状全体を `axis_a`→`axis_b` を通る直線に対して鏡映した新しい形状を返す純関数。
    ///
    /// 選択エンティティの鏡映（Mirror）で、確定時に各エンティティの `Shape` を
    /// 鏡映した幾何（`Command::ModifyEntity { new_geom }` に載せる値）を作るのに使う。
    /// [`Shape::translated`] と同じく GUI 非依存の再利用可能な幾何演算として mcad-geom
    /// 側に置く（DESIGN M5 設計判断1: app 層での座標いじりを禁止する）。
    #[must_use]
    pub fn mirrored(&self, axis_a: Point2, axis_b: Point2) -> Shape {
        match self {
            Shape::Point(p) => Shape::Point(mirror_point(*p, axis_a, axis_b)),
            Shape::Line(s) => Shape::Line(s.mirrored(axis_a, axis_b)),
            Shape::Circle(c) => Shape::Circle(c.mirrored(axis_a, axis_b)),
            Shape::Arc(a) => Shape::Arc(a.mirrored(axis_a, axis_b)),
            Shape::Polyline(pl) => Shape::Polyline(pl.mirrored(axis_a, axis_b)),
        }
    }

    /// 形状全体を距離 `distance` だけ、参照点 `toward` のある側へオフセットした新しい
    /// 形状を返す純関数（DESIGN.md M5 設計判断5）。
    ///
    /// 各プリミティブの `offset` へ委譲する: 線分＝法線平行移動、円・円弧＝半径 ± d、
    /// ポリライン＝各セグメント平行移動＋隣接無限直線の交点接続。オフセット結果は
    /// 選択エンティティの複製（`Command::AddEntity`、レイヤー・スタイルは元を複製）に
    /// 載せる幾何として使う。[`Shape::translated`] と同じく GUI 非依存の再利用可能な
    /// 幾何演算として mcad-geom 側に置く（app 層での座標いじりを禁止する）。
    ///
    /// # Errors
    ///
    /// 対象・距離が退化して結果を作れない場合に [`OffsetError`]。[`Shape::Point`] は
    /// オフセットを定義できないため常に [`OffsetError::Unsupported`]。
    pub fn offset(&self, distance: f64, toward: Point2) -> Result<Shape, OffsetError> {
        match self {
            Shape::Point(_) => Err(OffsetError::Unsupported),
            Shape::Line(s) => s.offset(distance, toward).map(Shape::Line),
            Shape::Circle(c) => c.offset(distance, toward).map(Shape::Circle),
            Shape::Arc(a) => a.offset(distance, toward).map(Shape::Arc),
            Shape::Polyline(pl) => pl.offset(distance, toward).map(Shape::Polyline),
        }
    }

    /// 形状が幾何的に妥当か検証する。
    ///
    /// 条件: すべての座標・半径・角度が有限、半径は非負、ポリラインは頂点 1 つ以上。
    /// 半径 0 の円や長さ 0 の線分は退化しているが描画・計算を壊さないため許容する
    /// （作図ツールでも同一点クリックで作れるものをここだけ拒否しない）。
    ///
    /// mcad-io（ファイル入出力の境界）・mcad-core（`Document::apply` の
    /// `AddEntity`/`ModifyEntity`）の双方から呼ばれる、唯一のジオメトリ妥当性
    /// 判定（DESIGN.md M4 タスク15: 判定ロジックの重複を避けるため mcad-geom へ
    /// 引き上げた）。
    ///
    /// # Errors
    ///
    /// 不正な理由を人が読める `String` で返す。
    pub fn validate(&self) -> Result<(), String> {
        let finite = |p: Point2| p.x.is_finite() && p.y.is_finite();
        match self {
            Shape::Point(p) => {
                if !finite(*p) {
                    return Err("non-finite point coordinates".into());
                }
            }
            Shape::Line(l) => {
                if !finite(l.a) || !finite(l.b) {
                    return Err("non-finite line coordinates".into());
                }
            }
            Shape::Circle(c) => {
                if !finite(c.center) {
                    return Err("non-finite circle center".into());
                }
                if !c.radius.is_finite() || c.radius < 0.0 {
                    return Err(format!("invalid circle radius: {}", c.radius));
                }
            }
            Shape::Arc(a) => {
                if !finite(a.center) {
                    return Err("non-finite arc center".into());
                }
                if !a.radius.is_finite() || a.radius < 0.0 {
                    return Err(format!("invalid arc radius: {}", a.radius));
                }
                if !a.start_angle.is_finite() || !a.end_angle.is_finite() {
                    return Err("non-finite arc angles".into());
                }
            }
            Shape::Polyline(pl) => {
                if pl.vertices.is_empty() {
                    return Err("empty polyline".into());
                }
                if !pl.vertices.iter().all(|v| finite(*v)) {
                    return Err("non-finite polyline vertex".into());
                }
            }
        }
        Ok(())
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
    fn segment_shorter_than_point_tolerance_is_degenerate_far_from_origin() {
        // 原点から遠い座標では、点の一致許容量（`point_tol`）が座標の大きさに比例して
        // 広がる。その許容量より短い線分は「両端点が同一点」なので方向が定まらない。
        // 絶対 EPS 判定（`len <= 1e-9`）だと長さ 1e-8 を非退化と誤認していた。
        let a = Point2::new(1.0e6, 1.0e6);
        let b = Point2::new(1.0e6 + 1.0e-8, 1.0e6);
        let seg = LineSeg::new(a, b);
        assert!(
            seg.length() > crate::EPS,
            "旧・絶対 EPS では非退化だった長さ"
        );
        assert!(
            seg.length() < point_tol(a, b),
            "相対許容量では両端点が同一点"
        );

        assert_eq!(
            seg.offset(1.0, Point2::ORIGIN),
            Err(OffsetError::Degenerate)
        );
        assert_eq!(seg.closest_point(Point2::ORIGIN), a);
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

    #[test]
    fn rotated_turns_each_primitive_kind() {
        use std::f64::consts::FRAC_PI_2;
        let pivot = Point2::ORIGIN;

        // Point: (1,0) を原点まわり +90° → (0,1)
        assert!(approx(
            match Shape::Point(Point2::new(1.0, 0.0)).rotated(pivot, FRAC_PI_2) {
                Shape::Point(p) => p,
                _ => unreachable!(),
            },
            Point2::new(0.0, 1.0)
        ));

        // Line: (1,0)-(2,0) を +90° → (0,1)-(0,2)
        let line = LineSeg::new(Point2::new(1.0, 0.0), Point2::new(2.0, 0.0));
        let r = line.rotated(pivot, FRAC_PI_2);
        assert!(approx(r.a, Point2::new(0.0, 1.0)));
        assert!(approx(r.b, Point2::new(0.0, 2.0)));

        // Circle: 中心 (3,0) を +90° → (0,3)、半径は不変
        let circle = Circle::new(Point2::new(3.0, 0.0), 5.0);
        let rc = circle.rotated(pivot, FRAC_PI_2);
        assert!(approx(rc.center, Point2::new(0.0, 3.0)));
        assert!((rc.radius - 5.0).abs() < T);

        // Arc: 中心 (1,0)・start 0・end π/2 を +90° → 中心 (0,1)、両角に +π/2、半径不変
        let arc = Arc::new(Point2::new(1.0, 0.0), 2.0, 0.0, FRAC_PI_2);
        let ra = arc.rotated(pivot, FRAC_PI_2);
        assert!(approx(ra.center, Point2::new(0.0, 1.0)));
        assert!((ra.radius - 2.0).abs() < T);
        assert!((ra.start_angle - FRAC_PI_2).abs() < T);
        assert!((ra.end_angle - std::f64::consts::PI).abs() < T);

        // Polyline: 全頂点が回転し closed は不変
        let pl = Polyline::new(vec![Point2::new(1.0, 0.0), Point2::new(2.0, 0.0)], true);
        let rpl = pl.rotated(pivot, FRAC_PI_2);
        assert!(approx(rpl.vertices[0], Point2::new(0.0, 1.0)));
        assert!(approx(rpl.vertices[1], Point2::new(0.0, 2.0)));
        assert!(rpl.closed);
    }

    #[test]
    fn mirrored_reflects_each_primitive_kind() {
        // x 軸（原点→(1,0)）に対する鏡映。
        let axis_a = Point2::ORIGIN;
        let axis_b = Point2::new(1.0, 0.0);

        // Point: (1,2) → (1,-2)
        assert!(approx(
            match Shape::Point(Point2::new(1.0, 2.0)).mirrored(axis_a, axis_b) {
                Shape::Point(p) => p,
                _ => unreachable!(),
            },
            Point2::new(1.0, -2.0)
        ));

        // Line: (1,2)-(3,4) → (1,-2)-(3,-4)
        let line = LineSeg::new(Point2::new(1.0, 2.0), Point2::new(3.0, 4.0));
        let m = line.mirrored(axis_a, axis_b);
        assert!(approx(m.a, Point2::new(1.0, -2.0)));
        assert!(approx(m.b, Point2::new(3.0, -4.0)));

        // Circle: 中心 (2,3) → (2,-3)、半径は不変
        let circle = Circle::new(Point2::new(2.0, 3.0), 5.0);
        let mc = circle.mirrored(axis_a, axis_b);
        assert!(approx(mc.center, Point2::new(2.0, -3.0)));
        assert!((mc.radius - 5.0).abs() < T);

        // Polyline: 全頂点が鏡映し closed は不変
        let pl = Polyline::new(vec![Point2::new(1.0, 2.0), Point2::new(3.0, 4.0)], true);
        let mpl = pl.mirrored(axis_a, axis_b);
        assert!(approx(mpl.vertices[0], Point2::new(1.0, -2.0)));
        assert!(approx(mpl.vertices[1], Point2::new(3.0, -4.0)));
        assert!(mpl.closed);
    }

    #[test]
    fn arc_mirror_first_to_fourth_quadrant() {
        use std::f64::consts::FRAC_PI_2;
        // 中心原点・start 0 → end π/2（第 1 象限、sweep=π/2）の弧を x 軸で鏡映すると、
        // 第 4 象限の弧（start=-π/2, end=0, sweep=π/2）になる。
        let arc = Arc::new(Point2::ORIGIN, 3.0, 0.0, FRAC_PI_2);
        let m = arc.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0));
        assert!(approx(m.center, Point2::ORIGIN));
        assert!((m.radius - 3.0).abs() < T);
        // reflect_angle(θ)=-θ なので new_start=reflect(π/2)=-π/2, new_end=reflect(0)=0。
        assert!((m.start_angle - (-FRAC_PI_2)).abs() < T);
        assert!((m.end_angle - 0.0).abs() < T);
        // 掃引の大きさは保存される。
        assert!((m.sweep() - FRAC_PI_2).abs() < T);
        // 端点で確認: 元の始点(3,0)は鏡映後も(3,0)、元の終点(0,3)は(0,-3)。
        assert!(approx(m.start_point(), Point2::new(0.0, -3.0)));
        assert!(approx(m.end_point(), Point2::new(3.0, 0.0)));
    }

    #[test]
    fn rotated_by_zero_is_identity() {
        let arc = Shape::Arc(Arc::new(Point2::new(1.0, 2.0), 3.0, 0.5, 2.0));
        assert_eq!(arc.rotated(Point2::new(5.0, -1.0), 0.0), arc);
    }

    #[test]
    fn arc_mirror_degenerate_axis_returns_self() {
        // 軸 2 点がほぼ同一なら無変換で自身を返す（退化軸ガード、設計判断5）。
        let arc = Arc::new(Point2::new(1.0, 2.0), 3.0, 0.5, 2.0);
        let p = Point2::new(4.0, 4.0);
        assert_eq!(arc.mirrored(p, p), arc);
        // EPS 未満のわずかな差でも自身を返す。
        let q = Point2::new(4.0 + 1e-12, 4.0);
        assert_eq!(arc.mirrored(p, q), arc);
    }

    // --- オフセット（M5 設計判断5） ---

    #[test]
    fn line_offset_moves_by_normal_on_click_side() {
        let s = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        // 上側（y>0）をクリック → +2 平行移動。
        let up = s.offset(2.0, Point2::new(5.0, 5.0)).unwrap();
        assert!(approx(up.a, Point2::new(0.0, 2.0)));
        assert!(approx(up.b, Point2::new(10.0, 2.0)));
        // 下側（y<0）をクリック → −2 平行移動。
        let down = s.offset(2.0, Point2::new(5.0, -5.0)).unwrap();
        assert!(approx(down.a, Point2::new(0.0, -2.0)));
        assert!(approx(down.b, Point2::new(10.0, -2.0)));
    }

    #[test]
    fn line_offset_through_point_passes_through_it() {
        // 通過点方式: distance = 対象までの最短距離。結果は通過点を通る。
        let s = LineSeg::new(Point2::new(-3.0, 1.0), Point2::new(4.0, 6.0));
        let p = Point2::new(9.0, -2.0);
        let d = distance_to(&Shape::Line(s), p);
        let off = Shape::Line(s).offset(d, p).unwrap();
        assert!(distance_to(&off, p) < 1e-9);
    }

    #[test]
    fn line_offset_degenerate_and_zero_distance_rejected() {
        let zero = LineSeg::new(Point2::new(2.0, 2.0), Point2::new(2.0, 2.0));
        assert_eq!(
            zero.offset(1.0, Point2::new(5.0, 5.0)),
            Err(OffsetError::Degenerate)
        );
        let s = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        assert_eq!(
            s.offset(0.0, Point2::new(5.0, 5.0)),
            Err(OffsetError::NonPositiveDistance)
        );
        assert_eq!(
            s.offset(-1.0, Point2::new(5.0, 5.0)),
            Err(OffsetError::NonPositiveDistance)
        );
        assert_eq!(
            s.offset(f64::NAN, Point2::new(5.0, 5.0)),
            Err(OffsetError::NonPositiveDistance)
        );
    }

    #[test]
    fn circle_offset_outward_and_inward() {
        let c = Circle::new(Point2::ORIGIN, 5.0);
        // 外（中心より遠い点）→ r+d。
        let out = c.offset(2.0, Point2::new(10.0, 0.0)).unwrap();
        assert!((out.radius - 7.0).abs() < T);
        assert!(approx(out.center, Point2::ORIGIN));
        // 内（中心より近い点）→ r−d。
        let inn = c.offset(2.0, Point2::new(1.0, 0.0)).unwrap();
        assert!((inn.radius - 3.0).abs() < T);
    }

    #[test]
    fn circle_offset_inner_collapse_rejected() {
        let c = Circle::new(Point2::ORIGIN, 5.0);
        // 内側で d == r → 半径 0 になるため拒否。
        assert_eq!(
            c.offset(5.0, Point2::new(0.0, 0.0)),
            Err(OffsetError::RadiusCollapse)
        );
        // 内側で d > r も拒否。
        assert_eq!(
            c.offset(6.0, Point2::new(1.0, 0.0)),
            Err(OffsetError::RadiusCollapse)
        );
        // 外側は d > r でも問題ない。
        assert!(c.offset(6.0, Point2::new(10.0, 0.0)).is_ok());
    }

    #[test]
    fn arc_offset_preserves_center_and_angles() {
        use std::f64::consts::FRAC_PI_2;
        let a = Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        let out = a.offset(2.0, Point2::new(10.0, 10.0)).unwrap();
        assert!(approx(out.center, Point2::ORIGIN));
        assert!((out.radius - 7.0).abs() < T);
        assert!((out.start_angle - 0.0).abs() < T);
        assert!((out.end_angle - FRAC_PI_2).abs() < T);
        // 内側。
        let inn = a.offset(2.0, Point2::new(1.0, 1.0)).unwrap();
        assert!((inn.radius - 3.0).abs() < T);
    }

    #[test]
    fn polyline_offset_closed_square_inward_and_outward() {
        // CCW の正方形。左法線は内向き。
        let sq = Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(10.0, 10.0),
                Point2::new(0.0, 10.0),
            ],
            true,
        );
        // 内側（中心 (5,5)）をクリック → 各辺が内へ 2 → [2,8]^2 の正方形。
        let inn = sq.offset(2.0, Point2::new(5.0, 5.0)).unwrap();
        assert_eq!(inn.vertices.len(), 4);
        assert!(inn.closed);
        assert!(approx(inn.vertices[0], Point2::new(2.0, 2.0)));
        assert!(approx(inn.vertices[1], Point2::new(8.0, 2.0)));
        assert!(approx(inn.vertices[2], Point2::new(8.0, 8.0)));
        assert!(approx(inn.vertices[3], Point2::new(2.0, 8.0)));
        // 外側（下辺の外 (5,-5)）をクリック → 各辺が外へ 2 → [-2,12]^2。
        let out = sq.offset(2.0, Point2::new(5.0, -5.0)).unwrap();
        assert!(approx(out.vertices[0], Point2::new(-2.0, -2.0)));
        assert!(approx(out.vertices[1], Point2::new(12.0, -2.0)));
        assert!(approx(out.vertices[2], Point2::new(12.0, 12.0)));
        assert!(approx(out.vertices[3], Point2::new(-2.0, 12.0)));
    }

    #[test]
    fn polyline_offset_open_l_miters_interior_corner() {
        // L 字（開）。角 (10,0) を平行移動後の隣接直線の交点で接続する。
        let l = Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(10.0, 10.0),
            ],
            false,
        );
        let off = l.offset(2.0, Point2::new(5.0, 5.0)).unwrap();
        assert_eq!(off.vertices.len(), 3);
        assert!(!off.closed);
        // 端点は片側セグメントの平行移動先、中央はマイター交点 (8,2)。
        assert!(approx(off.vertices[0], Point2::new(0.0, 2.0)));
        assert!(approx(off.vertices[1], Point2::new(8.0, 2.0)));
        assert!(approx(off.vertices[2], Point2::new(8.0, 10.0)));
    }

    #[test]
    fn polyline_offset_collinear_stays_single_vertex() {
        // 一直線（中央頂点が共線）。隣接が平行なのでマイター交点は取れず、直線継続の
        // ベベル（2端点が一致）で1頂点に畳む。結果は素直に平行移動した3頂点になる。
        let straight = Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(5.0, 0.0),
                Point2::new(10.0, 0.0),
            ],
            false,
        );
        let off = straight.offset(2.0, Point2::new(5.0, 5.0)).unwrap();
        assert_eq!(off.vertices.len(), 3);
        assert!(approx(off.vertices[0], Point2::new(0.0, 2.0)));
        assert!(approx(off.vertices[1], Point2::new(5.0, 2.0)));
        assert!(approx(off.vertices[2], Point2::new(10.0, 2.0)));
    }

    #[test]
    fn polyline_offset_shallow_angle_bevels_instead_of_spiking() {
        // 浅い角（掃引が 180° 近く）。マイター交点はオフセット距離の何倍も遠くへ飛ぶが、
        // MITER_LIMIT でベベルへ切り替え、頂点は元頂点近傍（<= MITER_LIMIT × distance）に
        // 収まる。両回転方向（+y 側／−y 側の折れ）で確認する。
        let distance = 1.0;
        let apex = Point2::new(10.0, 0.0);
        for tip_y in [0.174_f64, -0.174_f64] {
            // seg1 の方向を +x から約 170°/190° へ向け、鋭いくさびを作る。
            let tip = Point2::new(9.015, tip_y);
            let wedge = Polyline::new(vec![Point2::new(0.0, 0.0), apex, tip], false);
            let off = wedge
                .offset(distance, Point2::new(5.0, 10.0 * tip_y.signum()))
                .unwrap();

            // マイターがそのまま採用されていれば頂点が遠方へ飛ぶ。ベベル化で 2 頂点に増え、
            // いずれも apex 近傍にとどまることを確認する。
            assert_eq!(off.vertices.len(), 4, "apex should bevel into 2 vertices");
            for v in &off.vertices {
                assert!(v.x.is_finite() && v.y.is_finite());
            }
            let apex_bevels = &off.vertices[1..3];
            for v in apex_bevels {
                assert!(
                    apex.distance(*v) <= MITER_LIMIT * distance + 1e-6,
                    "bevel vertex {v:?} spiked away from apex {apex:?}"
                );
            }
        }
    }

    #[test]
    fn polyline_offset_hairpin_fold_keeps_two_bevel_points() {
        // 180° 折返し（seg1 が seg0 と真逆）。中点1点に畳むと元頂点へ退化するため、
        // 平行移動後の2端点を両方残す。上側 (5,5) をクリック → +1 オフセット。
        let fold = Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(1.0, 0.0),
            ],
            false,
        );
        let off = fold.offset(1.0, Point2::new(5.0, 5.0)).unwrap();
        assert_eq!(off.vertices.len(), 4);
        // 端点。
        assert!(approx(off.vertices[0], Point2::new(0.0, 1.0)));
        assert!(approx(off.vertices[3], Point2::new(1.0, -1.0)));
        // 折返し頂点は (10,0) の中点退化 (10,0) ではなく、両側の (10,1)・(10,-1)。
        assert!(approx(off.vertices[1], Point2::new(10.0, 1.0)));
        assert!(approx(off.vertices[2], Point2::new(10.0, -1.0)));
    }

    #[test]
    fn polyline_offset_degenerate_rejected() {
        // 頂点1個。
        assert_eq!(
            Polyline::new(vec![Point2::new(1.0, 1.0)], false).offset(1.0, Point2::ORIGIN),
            Err(OffsetError::Degenerate)
        );
        // 全頂点が同一（全セグメントがゼロ長）。
        let same = Polyline::new(vec![Point2::new(1.0, 1.0), Point2::new(1.0, 1.0)], false);
        assert_eq!(
            same.offset(1.0, Point2::ORIGIN),
            Err(OffsetError::Degenerate)
        );
    }

    #[test]
    fn shape_offset_point_is_unsupported() {
        assert_eq!(
            Shape::Point(Point2::new(1.0, 1.0)).offset(1.0, Point2::ORIGIN),
            Err(OffsetError::Unsupported)
        );
    }

    #[test]
    fn validate_accepts_degenerate_but_finite_shapes() {
        // 半径 0 の円・長さ 0 の線分は退化しているが許容する。
        let cases = [
            Shape::Point(Point2::new(1.0, 2.0)),
            Shape::Line(LineSeg::new(Point2::new(2.0, 2.0), Point2::new(2.0, 2.0))),
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 0.0)),
            Shape::Arc(Arc::new(Point2::new(0.0, 0.0), 1.0, 0.0, 0.0)),
            Shape::Polyline(Polyline::new(vec![Point2::new(0.0, 0.0)], false)),
        ];
        for shape in cases {
            assert!(shape.validate().is_ok(), "should accept: {shape:?}");
        }
    }

    #[test]
    fn validate_rejects_non_finite_and_invalid_shapes() {
        let cases: Vec<Shape> = vec![
            Shape::Point(Point2::new(f64::NAN, 0.0)),
            Shape::Point(Point2::new(f64::INFINITY, 0.0)),
            Shape::Line(LineSeg::new(Point2::new(f64::NAN, 0.0), Point2::ORIGIN)),
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), -1.0)),
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), f64::INFINITY)),
            Shape::Circle(Circle::new(Point2::new(f64::NAN, 0.0), 1.0)),
            Shape::Arc(Arc::new(Point2::new(0.0, 0.0), 1.0, f64::NAN, 1.0)),
            Shape::Arc(Arc::new(Point2::new(0.0, 0.0), -1.0, 0.0, 1.0)),
            Shape::Arc(Arc::new(Point2::new(f64::NAN, 0.0), 1.0, 0.0, 1.0)),
            Shape::Polyline(Polyline::new(vec![], false)),
            Shape::Polyline(Polyline::new(vec![Point2::new(f64::NAN, 0.0)], false)),
        ];
        for shape in cases {
            assert!(shape.validate().is_err(), "should reject: {shape:?}");
        }
    }
}
