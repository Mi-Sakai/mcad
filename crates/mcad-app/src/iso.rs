//! アイソメ円（等測投影された円）の四心法近似（DESIGN.md 7章「随時対応」
//! アイソメ図・アクソメ図の作図補助、設計確定5・タスク アイソメ-3）。
//!
//! `Document`/`egui` に依存しない純関数だけを持つ（`ortho.rs`・`snap.rs` と同じ方針）。
//! 等測投影された円は本来楕円だが、mcad は真楕円エンティティを新設しない
//! （`EntityGeom` 追加と `.mcad` 版数上げを避ける設計判断。DESIGN.md 同項）。
//! 代わりに手描き製図の標準技法である **四心法**（four-center method）で
//! 4 本の [`Arc`] へ近似する。
//!
//! # 構成（定数ではなく幾何で組み立てる）
//!
//! 面の 2 軸方向 `u`・`v`（[`IsoFace`]）に沿う一辺 `d = 2r` の菱形（内角 60°/120°、
//! 中心は円の中心）を作り、
//!
//! - **接続点**: 菱形 4 辺の中点（＝中心から各軸方向へ `r` 離れた点。楕円の共役半径の端点）
//! - **大円弧の中心**: 鈍角（120°）の頂点 2 点（＝短対角線の両端）
//! - **小円弧の中心**: 鈍角頂点から対辺へ下ろした垂線（足はその辺の中点）同士の交点 2 点
//!   （＝長対角線上）
//!
//! とする。半径と中心角は構成の結果として決まる（大円弧 `R1 = (√3/2)·d` で中心角 60°、
//! 小円弧 `R2 = (√3/6)·d` で中心角 120°、`R1 = 3·R2`）が、実装はこれらの導出値を
//! 一切使わない。導出値との一致・接続点での接線連続・CCW 規約はテストが検算する
//! （DESIGN.md 同項「テストは定数を信用せず〜」）。
//!
//! # 接線連続が成り立つ理由
//!
//! 隣り合う 2 円弧の中心と接続点は同一直線上にある（小円弧の中心を「大円弧の中心と
//! 接続点を結ぶ直線」の交点として作るため、構成上そうなる）。共有点で両円の半径方向が
//! 一致する ＝ 接線も一致するので、4 円弧は滑らかにつながる。

use mcad_geom::{Arc, Point2, Vec2};

/// `√3 / 2`（30°・150° 軸の x 成分）。`f64::sqrt` は const 文脈で使えないため
/// 数値リテラルで持つ（`ortho.rs` の同名定数と同じ流儀）。
const SQRT3_OVER_2: f64 = 0.866_025_403_784_438_6;

/// 30°（右上がり）軸の単位方向ベクトル。
const AXIS_30: Vec2 = Vec2::new(SQRT3_OVER_2, 0.5);
/// 90°（鉛直）軸の単位方向ベクトル。
const AXIS_90: Vec2 = Vec2::new(0.0, 1.0);
/// 150°（左上がり）軸の単位方向ベクトル。
const AXIS_150: Vec2 = Vec2::new(-SQRT3_OVER_2, 0.5);

/// 等測投影の 3 面。アイソメ円ツールが `Tab` で循環する対象。
///
/// 面ごとの 2 軸方向は DESIGN.md 設計確定5 のとおり Top: 30°/150°、Right: 30°/90°、
/// Left: 150°/90°。等測の 3 軸そのものは直交モードの拘束軸（`ortho::ISO_AXES`）と
/// 同一で、両者が食い違わないことはテストで固定する。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum IsoFace {
    /// 上面（軸 30°/150°、長軸は水平）。
    #[default]
    Top,
    /// 左面（軸 150°/90°、長軸は 120°方向）。
    Left,
    /// 右面（軸 30°/90°、長軸は 60°方向）。
    Right,
}

impl IsoFace {
    /// 次の面（`Top` → `Left` → `Right` → `Top`）。
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            IsoFace::Top => IsoFace::Left,
            IsoFace::Left => IsoFace::Right,
            IsoFace::Right => IsoFace::Top,
        }
    }

    /// 上部パネル表示用のラベル（ASCII）。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            IsoFace::Top => "Top",
            IsoFace::Left => "Left",
            IsoFace::Right => "Right",
        }
    }

    /// この面の 2 軸方向（単位ベクトル）。菱形の辺の向きになる。
    #[must_use]
    fn axes(self) -> (Vec2, Vec2) {
        match self {
            IsoFace::Top => (AXIS_30, AXIS_150),
            IsoFace::Left => (AXIS_150, AXIS_90),
            IsoFace::Right => (AXIS_30, AXIS_90),
        }
    }
}

/// 2 直線 `p + t·dp`、`q + s·dq` の交点。平行（外積 0）・方向ベクトルが退化なら `None`。
///
/// 方向ベクトルは単位化してから外積を取る（`radius` が極端に大きいときに外積が
/// オーバーフローするのを避けるため。単位化後の外積は 2 直線のなす角の sin に等しい）。
fn line_intersection(p: Point2, dp: Vec2, q: Point2, dq: Vec2) -> Option<Point2> {
    let dp = dp.normalize()?;
    let dq = dq.normalize()?;
    let denom = dp.cross(dq);
    if denom == 0.0 {
        return None;
    }
    let t = (q - p).cross(dq) / denom;
    Some(p + dp * t)
}

/// 中心 `c` と、`c` から等距離にある 2 端点 `a`・`b` から円弧を作る。
///
/// [`Arc`] は開始角から **CCW** に掃引する型なので、掃引が `π` 未満になる向き
/// （＝短い方の弧）を選んで開始角・終了角を決める。四心法の 4 円弧はいずれも中心角が
/// 60°（大）・120°（小）で `π` 未満なので、この規則で向きが一意に定まる。凸閉曲線を
/// CCW に一周する弧片は、どれも自分の中心まわりに CCW になるため、4 円弧はこの規則
/// だけで首尾一貫した CCW の並びになる（菱形の向きは [`iso_circle_arcs`] 側で
/// 揃えてある）。
///
/// 半径は 2 端点までの距離の平均を採る（構成上は厳密に等しく、どちらか一方に
/// 寄せないための対称な取り方）。
fn arc_between(c: Point2, a: Point2, b: Point2) -> Option<Arc> {
    let va = a - c;
    let vb = b - c;
    let radius = (va.length() + vb.length()) / 2.0;
    if !(radius.is_finite() && radius > 0.0) {
        return None;
    }
    let (start, end) = (va.angle(), vb.angle());
    let sweep = (end - start).rem_euclid(std::f64::consts::TAU);
    Some(if sweep <= std::f64::consts::PI {
        Arc::new(c, radius, start, end)
    } else {
        Arc::new(c, radius, end, start)
    })
}

/// 面 `face` の等測円（呼び径 `2 * radius`）を四心法で近似した 4 円弧を、
/// 接続順（各円弧の終点が次の円弧の始点）に返す。
///
/// `radius` が正の有限値でない、または `center` が非有限なら `None`
/// （退化入力。ツール側は `Rejected` として扱う）。
#[must_use]
pub fn iso_circle_arcs(face: IsoFace, center: Point2, radius: f64) -> Option<[Arc; 4]> {
    if !(radius.is_finite() && radius > 0.0 && center.x.is_finite() && center.y.is_finite()) {
        return None;
    }

    let (u, v) = face.axes();
    // 菱形は ±u・±v の 4 辺からなり、v の符号を反転しても同じ図形になる。以降の
    // 「鈍角頂点 = c ± r(u+v)」を面によらず成り立たせるため、u と v のなす角が
    // 鈍角（内積が負）になる代表を選ぶ。
    let v = if u.dot(v) > 0.0 { -v } else { v };
    // さらに (u, v) を右手系（外積が正）に揃える。菱形の形は u と v の入れ替えで
    // 変わらないが、下で組む 4 円弧の並び順が CCW になるかどうかはこの向きで決まる。
    let (u, v) = if u.cross(v) < 0.0 { (v, u) } else { (u, v) };

    let r = radius;
    // 菱形 4 辺の中点（＝円弧の接続点）。菱形の一辺は d = 2r で、中点は中心から
    // 各軸方向へ r。
    let m_u_pos = center + u * r;
    let m_u_neg = center - u * r;
    let m_v_pos = center + v * r;
    let m_v_neg = center - v * r;
    // 鈍角（120°）の頂点＝短対角線の両端。ここが大円弧の中心になる。
    let obtuse_pos = center + (u + v) * r;
    let obtuse_neg = center - (u + v) * r;
    // 小円弧の中心＝鈍角頂点から対辺へ下ろした垂線同士の交点（長対角線上に来る）。
    // 垂線の足は対辺の中点なので、鈍角頂点と対辺中点を結ぶ直線をそのまま使う。
    let small_pos = line_intersection(
        obtuse_pos,
        m_v_neg - obtuse_pos,
        obtuse_neg,
        m_u_pos - obtuse_neg,
    )?;
    let small_neg = line_intersection(
        obtuse_pos,
        m_u_neg - obtuse_pos,
        obtuse_neg,
        m_v_pos - obtuse_neg,
    )?;

    Some([
        arc_between(obtuse_pos, m_u_neg, m_v_neg)?,
        arc_between(small_pos, m_v_neg, m_u_pos)?,
        arc_between(obtuse_neg, m_u_pos, m_v_pos)?,
        arc_between(small_neg, m_v_pos, m_u_neg)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_3, TAU};

    /// 検算に使う面の一覧。
    const FACES: [IsoFace; 3] = [IsoFace::Top, IsoFace::Left, IsoFace::Right];

    /// 原点からずらした中心・キリの悪い半径で試す（対称性で偶然通るのを防ぐ）。
    const CENTER: Point2 = Point2::new(-12.5, 7.25);
    const RADIUS: f64 = 3.7;

    fn assert_near(actual: f64, expected: f64, tol: f64, what: &str) {
        assert!(
            (actual - expected).abs() <= tol,
            "{what}: actual={actual} expected={expected} diff={}",
            (actual - expected).abs()
        );
    }

    /// 方向を「向きを区別しない角度」（度、`[0, 180)`）へ正規化する。
    fn axis_degrees(v: Vec2) -> f64 {
        v.angle().to_degrees().rem_euclid(180.0)
    }

    #[test]
    fn next_cycles_top_left_right() {
        assert_eq!(IsoFace::default(), IsoFace::Top);
        assert_eq!(IsoFace::Top.next(), IsoFace::Left);
        assert_eq!(IsoFace::Left.next(), IsoFace::Right);
        assert_eq!(IsoFace::Right.next(), IsoFace::Top);
    }

    /// (a) 4 円弧の端点が順に一致し、閉じた輪になる。
    #[test]
    fn arcs_form_a_closed_chain() {
        for face in FACES {
            let arcs = iso_circle_arcs(face, CENTER, RADIUS).expect("valid input");
            for i in 0..4 {
                let joint = arcs[i].end_point();
                let next = arcs[(i + 1) % 4].start_point();
                assert!(
                    joint.distance(next) < 1e-12,
                    "{face:?}: arc{i}.end={joint:?} != arc{}.start={next:?}",
                    (i + 1) % 4
                );
            }
        }
    }

    /// (b) 各接続点で、隣り合う 2 円弧の中心と接続点が同一直線上にある（接線連続）。
    #[test]
    fn arc_joints_are_tangent_continuous() {
        for face in FACES {
            let arcs = iso_circle_arcs(face, CENTER, RADIUS).expect("valid input");
            for i in 0..4 {
                let next = (i + 1) % 4;
                let joint = arcs[i].end_point();
                // 接続点から見た 2 つの中心の方向が平行（外積 ≒ 0）なら同一直線上。
                // 単位ベクトルどうしの外積なので、値はそのまま「なす角の sin」。
                let a = (arcs[i].center - joint)
                    .normalize()
                    .expect("non-zero radius");
                let b = (arcs[next].center - joint)
                    .normalize()
                    .expect("non-zero radius");
                assert!(
                    a.cross(b).abs() < 1e-12,
                    "{face:?}: joint {i} not tangent-continuous (sin={})",
                    a.cross(b).abs()
                );
                // 大小の円弧は接続点で同じ側にあり（内接）、逆向きにはならない。
                assert!(a.dot(b) > 0.0, "{face:?}: joint {i} centers on both sides");
            }
        }
    }

    /// (c) 各円弧が CCW 規約に従い、掃引は大 60°・小 120°（合計 360°）。
    #[test]
    fn arc_sweeps_are_60_and_120_degrees() {
        for face in FACES {
            let arcs = iso_circle_arcs(face, CENTER, RADIUS).expect("valid input");
            let expected = [FRAC_PI_3, 2.0 * FRAC_PI_3, FRAC_PI_3, 2.0 * FRAC_PI_3];
            let mut total = 0.0;
            for (i, arc) in arcs.iter().enumerate() {
                // `Arc::sweep()` は wrap_2pi(end - start)。CCW 規約どおりなら
                // 期待する中心角そのものになる（反対向きなら 360° - 期待値になる）。
                assert_near(arc.sweep(), expected[i], 1e-12, &format!("{face:?} arc{i}"));
                total += arc.sweep();
            }
            assert_near(total, TAU, 1e-12, &format!("{face:?} total sweep"));
        }
    }

    /// (d) 半径が四心法の導出値 `R1 = (√3/2)d`・`R2 = (√3/6)d` に一致する（`R1 = 3·R2`）。
    #[test]
    fn arc_radii_match_four_center_derivation() {
        let d = 2.0 * RADIUS;
        let r1 = 3.0_f64.sqrt() / 2.0 * d;
        let r2 = 3.0_f64.sqrt() / 6.0 * d;
        for face in FACES {
            let arcs = iso_circle_arcs(face, CENTER, RADIUS).expect("valid input");
            assert_near(arcs[0].radius, r1, 1e-12, &format!("{face:?} arc0 R1"));
            assert_near(arcs[1].radius, r2, 1e-12, &format!("{face:?} arc1 R2"));
            assert_near(arcs[2].radius, r1, 1e-12, &format!("{face:?} arc2 R1"));
            assert_near(arcs[3].radius, r2, 1e-12, &format!("{face:?} arc3 R2"));
            assert_near(r1, 3.0 * r2, 1e-12, "R1 = 3 R2");
        }
    }

    /// (e) 3 面それぞれで、接続点（＝菱形 4 辺の中点）が設計どおりの軸方向・距離 `r` にある。
    #[test]
    fn joints_lie_on_the_designed_face_axes() {
        for (face, expected) in [
            (IsoFace::Top, [30.0, 150.0]),
            (IsoFace::Left, [90.0, 150.0]),
            (IsoFace::Right, [30.0, 90.0]),
        ] {
            let arcs = iso_circle_arcs(face, CENTER, RADIUS).expect("valid input");
            let mut degrees: Vec<f64> = Vec::new();
            for arc in &arcs {
                let joint = arc.end_point();
                // 接続点は中心から半径 r（楕円の共役半径の端点）。
                assert_near(
                    joint.distance(CENTER),
                    RADIUS,
                    1e-12,
                    &format!("{face:?} joint radius"),
                );
                degrees.push(axis_degrees(joint - CENTER));
            }
            degrees.sort_by(f64::total_cmp);
            degrees.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            assert_eq!(
                degrees.len(),
                2,
                "{face:?}: 軸は 2 方向のはず ({degrees:?})"
            );
            assert_near(degrees[0], expected[0], 1e-9, &format!("{face:?} axis0"));
            assert_near(degrees[1], expected[1], 1e-9, &format!("{face:?} axis1"));
        }
    }

    /// 面の軸が直交モードの等測拘束軸（`ortho::ISO_AXES`、30°/90°/150°）と同じ 3 方向で
    /// あることを固定する。片方だけ変えると、等測拘束した線とアイソメ円がずれる。
    #[test]
    fn face_axes_match_ortho_iso_axes() {
        let mut ortho: Vec<f64> = crate::ortho::ISO_AXES
            .iter()
            .map(|&(x, y)| axis_degrees(Vec2::new(x, y)))
            .collect();
        ortho.sort_by(f64::total_cmp);

        let mut faces: Vec<f64> = FACES
            .iter()
            .flat_map(|face| {
                let (u, v) = face.axes();
                [axis_degrees(u), axis_degrees(v)]
            })
            .collect();
        faces.sort_by(f64::total_cmp);
        faces.dedup_by(|a, b| (*a - *b).abs() < 1e-9);

        assert_eq!(faces.len(), ortho.len());
        for (f, o) in faces.iter().zip(ortho.iter()) {
            assert_near(*f, *o, 1e-9, "iso axis vs ortho::ISO_AXES");
        }
    }

    /// (f) 退化入力は `None`。
    #[test]
    fn degenerate_input_returns_none() {
        for radius in [0.0, -1.0, f64::NAN, f64::INFINITY, -f64::INFINITY] {
            assert!(
                iso_circle_arcs(IsoFace::Top, CENTER, radius).is_none(),
                "radius={radius} should be rejected"
            );
        }
        assert!(iso_circle_arcs(IsoFace::Top, Point2::new(f64::NAN, 0.0), 1.0).is_none());
        assert!(iso_circle_arcs(IsoFace::Top, Point2::new(0.0, f64::INFINITY), 1.0).is_none());
    }

    /// 相似変換（平行移動・拡大）で形が保たれる（半径が比例し、面の向きは変わらない）。
    #[test]
    fn arcs_scale_with_radius() {
        let unit = iso_circle_arcs(IsoFace::Right, Point2::ORIGIN, 1.0).expect("valid input");
        let scaled = iso_circle_arcs(IsoFace::Right, CENTER, 100.0).expect("valid input");
        for (a, b) in unit.iter().zip(scaled.iter()) {
            assert_near(b.radius, a.radius * 100.0, 1e-9, "radius scales");
            assert_near(b.sweep(), a.sweep(), 1e-12, "sweep is scale invariant");
            let expected_center = CENTER + (a.center - Point2::ORIGIN) * 100.0;
            assert!(b.center.distance(expected_center) < 1e-9);
        }
    }

    /// 円弧上の点が、等測投影された円（＝共役半径 `r`・`r`、軸方向 `u`・`v` の楕円）の
    /// 近くに載っている。
    ///
    /// 四心法は近似であり、楕円とは接続点（4 辺の中点）でしか一致しない。ずれの最大は
    /// 長軸端（小円弧の中点）の `0.0572·r`（呼び径 `d = 2r` の約 2.9%）で、これは構成
    /// から決まる既知の値。ここでは上限を `d` の 3% に採り、面の取り違え・軸のずれ・
    /// 半径の取り違えのような**粗い誤り**を捕らえる（近似精度そのものの主張ではない）。
    #[test]
    fn arcs_approximate_the_projected_ellipse() {
        for face in FACES {
            let (u, v) = face.axes();
            let arcs = iso_circle_arcs(face, CENTER, RADIUS).expect("valid input");
            // 楕円上の点は c + r(cos t · u + sin t · v)。点 p の楕円座標は
            // 斜交座標 (a, b) を解いて a² + b² = r² かどうかで測る。
            let det = u.cross(v);
            for arc in &arcs {
                for k in 0..=8 {
                    let t = arc.start_angle + arc.sweep() * f64::from(k) / 8.0;
                    let p = arc.circle().point_at_angle(t);
                    let w = p - CENTER;
                    let a = w.cross(v) / det;
                    let b = u.cross(w) / det;
                    let err = (a * a + b * b).sqrt() - RADIUS;
                    assert!(
                        err.abs() < 0.03 * 2.0 * RADIUS,
                        "{face:?}: 楕円からのずれ {err} が大きすぎる"
                    );
                }
            }
        }
    }
}
