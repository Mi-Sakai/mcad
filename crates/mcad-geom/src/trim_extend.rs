//! トリム（[`trim`]）と延長（[`extend`]）。DESIGN.md M7 設計判断 2・3・3a。
//!
//! いずれも対象（target）は [`Shape::Line`] / [`Shape::Arc`] のみ、境界（boundary）は
//! [`Shape::Point`] 以外の全種を受け付ける。対象外の種別は
//! [`TrimExtendError::Unsupported`] を返す（[`Shape::offset`] と同じ薄いディスパッチ）。
//!
//! # 数値ロバスト性
//!
//! 交点計算は既存の [`crate::intersect`] をそのまま使い、新しい交点公式は追加しない。
//! 比較はすべてパラメータ空間（線分は無次元の `t`、弧は角度）で行うため
//! [`crate::EPS`] を直接使う（[`Arc::contains_angle`] と同じ流儀）。境界を「無限に
//! 伸ばした対象」で切る必要がある延長だけは、[`extend_reach`] で求めた
//! **幾何的に正当な上界** まで対象を一時的に伸ばした線分を作って [`crate::intersect`]
//! へ渡す。

use std::f64::consts::TAU;

use crate::primitives::wrap_2pi;
use crate::{Aabb, Arc, EPS, LineSeg, Point2, Shape, intersect};

/// トリム・延長が結果を作れない理由。
///
/// geom は GUI 非依存なので理由の列挙のみを返し、ユーザー向け文言は持たない
/// （UI 側 mcad-app が ASCII ステータスメッセージへ対応付ける）。
/// [`crate::OffsetError`] と同じパターン。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimExtendError {
    /// 対象と境界に（延長では「延長方向に」）交点がない。
    NoIntersection,
    /// トリム・延長を定義できない対象／境界の種別
    /// （対象が `Circle`/`Polyline`/`Point`、境界が `Point`）。
    Unsupported,
    /// 結果が退化して確定できない。ゼロ長線分・掃引 0 の弧といった退化した入力、
    /// 弧の延長で掃引が全周（2π）以上になる場合、およびトリムで残る断片が
    /// 1 つも無い場合（[`TrimResult`] の doc 参照）。
    Degenerate,
}

/// [`trim`] の結果。
///
/// 境界との交点で対象のパラメータ空間を分割し、クリックされた区間を取り除いた
/// **残りの断片** をすべて返す。`retained.len()` は 1（片側のみ残る）または
/// 2（クリック区間の両側に断片が残る）を取る。
///
/// **0 個は返さない**: 対象の両端点がちょうど境界上にある（交点が対象の下端と上端に
/// しか無い）配置で中間をクリックすると数学的に 0 断片になりうるが、その場合は
/// [`TrimExtendError::Degenerate`] を返して拒否する（呼び出し側が空 `Vec` を
/// 処理しなくて済むようにするための規約。DESIGN.md M7 設計判断3a の
/// 「通常入力では起こらない」を厳密化したもの）。
#[derive(Debug, Clone, PartialEq)]
pub struct TrimResult {
    /// 残る断片（1 個または 2 個）。パラメータの昇順。
    pub retained: Vec<Shape>,
}

/// 延長の探索用線分に与える相対マージン。
///
/// [`extend_reach`] は境界 AABB を覆う「ちょうどの」上界なので、最遠隅の上に交点が
/// あると探索線分の終端と一致して丸め次第で取りこぼす。それを避けるためだけの
/// 余裕であり、幾何的な意味を持つしきい値ではない（`EPS` より十分大きく、
/// 境界のスケールに対しては無視できる比率）。
const REACH_MARGIN: f64 = 1e-6;

/// 対象 `target` を境界 `boundary` で切り、`click` が示す区間を取り除く。
///
/// `click` は **捨てたい（除去したい）側** を示す点（AutoCAD TRIM と同じ慣習:
/// クリックした部分が消える）。境界との交点で `target` のパラメータ空間
/// （線分は `t ∈ [0, 1]`、弧は掃引角オフセット `d ∈ [0, sweep]`）を区間に分割し、
/// `click` の射影が属する区間だけを取り除いて残りを [`TrimResult`] で返す。
/// クリック区間の外側にある他の交点では **それ以上分割しない**
/// （過剰な細分化を避ける。DESIGN.md M7 設計判断3a）。
///
/// # Errors
///
/// - [`TrimExtendError::Unsupported`] — 対象が `Circle`/`Polyline`/`Point`、
///   または境界が `Point`。
/// - [`TrimExtendError::NoIntersection`] — 交点が 1 つも無い。
/// - [`TrimExtendError::Degenerate`] — 対象が退化している、または残る断片が 0 個。
pub fn trim(
    target: &Shape,
    boundary: &Shape,
    click: Point2,
) -> Result<TrimResult, TrimExtendError> {
    if matches!(boundary, Shape::Point(_)) {
        return Err(TrimExtendError::Unsupported);
    }
    match target {
        Shape::Line(seg) => trim_line(seg, boundary, click),
        Shape::Arc(arc) => trim_arc(arc, boundary, click),
        Shape::Point(_) | Shape::Circle(_) | Shape::Polyline(_) => {
            Err(TrimExtendError::Unsupported)
        }
    }
}

/// 対象 `target` の自由端を、境界 `boundary` と交わるところまで伸ばす。
///
/// `click` は **延長したい自由端を選ぶための参考点**（`target` の 2 端点のうち
/// `click` にユークリッド距離で近い方を自由端とする）。選ばれた自由端から外側へ
/// 向かって、現在の範囲の外にある交点のうち最短で届くものまで到達させる。
/// 延長は対象を分裂させないため戻り値は [`Shape`] 1 個。
///
/// 線分は [`extend_reach`] で境界 AABB を覆う長さまで一時的に伸ばしてから
/// 既存の交点計算にかける。弧は台円（[`Arc::circle`]）との交点を使い、
/// **符号付き・非畳み込みの角度差** で候補を選ぶ（DESIGN.md M7 設計判断2）。
///
/// # Errors
///
/// - [`TrimExtendError::Unsupported`] — 対象が `Circle`/`Polyline`/`Point`、
///   または境界が `Point`。
/// - [`TrimExtendError::NoIntersection`] — 延長方向に交点が無い（届かない）。
/// - [`TrimExtendError::Degenerate`] — 対象が退化している、または弧の延長で
///   掃引が全周（2π）以上になる（[`Arc`] は全周を表現できない）。
pub fn extend(target: &Shape, boundary: &Shape, click: Point2) -> Result<Shape, TrimExtendError> {
    if matches!(boundary, Shape::Point(_)) {
        return Err(TrimExtendError::Unsupported);
    }
    match target {
        Shape::Line(seg) => extend_line(seg, boundary, click),
        Shape::Arc(arc) => extend_arc(arc, boundary, click),
        Shape::Point(_) | Shape::Circle(_) | Shape::Polyline(_) => {
            Err(TrimExtendError::Unsupported)
        }
    }
}

/// `free_end` から境界 AABB を確実に覆うのに必要な延長長さ（4 隅への距離の最大値）。
///
/// AABB は凸領域であり、凸領域内で任意の点から最も遠い点は必ず頂点になる。
/// したがって `free_end` から AABB **内の任意の点** までの距離はこの値を超えない
/// （`free_end` が AABB の内側にあっても成り立つ。証明が AABB の凸性のみに依存し、
/// `free_end` の位置関係を使わないため）。円は自身の AABB に内包され、ポリラインの
/// 各セグメント端点は全体の AABB に含まれるので、境界が円・ポリラインでも
/// 十分な上界になる（DESIGN.md M7 設計判断2）。
///
/// ヒューリスティックな「大きな定数」ではなく境界のスケールに比例した値なので、
/// 数値的に安定する。
#[must_use]
pub fn extend_reach(free_end: Point2, boundary_aabb: Aabb) -> f64 {
    let (lo, hi) = (boundary_aabb.min, boundary_aabb.max);
    [
        Point2::new(lo.x, lo.y),
        Point2::new(hi.x, lo.y),
        Point2::new(lo.x, hi.y),
        Point2::new(hi.x, hi.y),
    ]
    .iter()
    .map(|&c| free_end.distance(c))
    .fold(0.0, f64::max)
}

// ---------------------------------------------------------------------------
// トリム
// ---------------------------------------------------------------------------

/// 線分上の点 `p` の直線パラメータ（`a` が 0、`b` が 1）。退化線分は `None`。
fn line_param(seg: &LineSeg, p: Point2) -> Option<f64> {
    let d = seg.direction();
    let len2 = d.length_squared();
    if len2 <= EPS * EPS {
        return None;
    }
    Some((p - seg.a).dot(d) / len2)
}

/// 線分上のパラメータ `t` に対応する点。
fn line_point_at(seg: &LineSeg, t: f64) -> Point2 {
    seg.a + seg.direction() * t
}

/// `click` がいずれかの交点パラメータから `EPS` 以内にあるかどうか。
///
/// 一致していると `click_interval` の厳密比較（`t < click` / `t > click`）から
/// その交点自身が両側の候補から漏れ、さらに外側の交点が区間境界に選ばれて
/// 過大にトリムしてしまう。呼び出し側は本関数で事前に排除すること。
fn click_matches_any_param(sorted: &[f64], click: f64) -> bool {
    sorted.iter().any(|&t| (t - click).abs() <= EPS)
}

/// ソート済みの交点パラメータ列から、`click` が属する区間の両隣を求める。
///
/// 下側は `click` より小さい交点のうち最大（無ければ下端 `0`）、上側は `click` より
/// 大きい交点のうち最小（無ければ上端 `param_max`）。
///
/// # 前提条件
///
/// 呼び出し前に [`click_matches_any_param`] で `click` が交点と一致しないことを
/// 確認しておくこと。一致したまま渡すと、厳密比較（`<` / `>`）でその交点自身が
/// 両側の候補から漏れ、さらに外側の交点が区間境界として選ばれてしまう
/// （過大トリムのバグ。呼び出し元は `Degenerate` を返して拒否する）。
fn click_interval(sorted: &[f64], click: f64, param_max: f64) -> (f64, f64) {
    let left = sorted
        .iter()
        .copied()
        .filter(|&t| t < click)
        .fold(0.0, f64::max);
    let right = sorted
        .iter()
        .copied()
        .filter(|&t| t > click)
        .fold(param_max, f64::min);
    (left, right)
}

fn trim_line(
    seg: &LineSeg,
    boundary: &Shape,
    click: Point2,
) -> Result<TrimResult, TrimExtendError> {
    let Some(t_click) = line_param(seg, click) else {
        return Err(TrimExtendError::Degenerate);
    };
    let t_click = t_click.clamp(0.0, 1.0);

    let hits = intersect(&Shape::Line(*seg), boundary);
    if hits.is_empty() {
        return Err(TrimExtendError::NoIntersection);
    }
    let mut params: Vec<f64> = hits
        .iter()
        .filter_map(|&p| line_param(seg, p))
        .map(|t| t.clamp(0.0, 1.0))
        .collect();
    params.sort_by(f64::total_cmp);

    if click_matches_any_param(&params, t_click) {
        // クリックが交点ちょうどでは、どちらの区間を消すか一意に決められない。
        return Err(TrimExtendError::Degenerate);
    }
    let (left, right) = click_interval(&params, t_click, 1.0);

    let mut retained = Vec::new();
    if left > EPS {
        retained.push(Shape::Line(LineSeg::new(seg.a, line_point_at(seg, left))));
    }
    if 1.0 - right > EPS {
        retained.push(Shape::Line(LineSeg::new(line_point_at(seg, right), seg.b)));
    }
    if retained.is_empty() {
        return Err(TrimExtendError::Degenerate);
    }
    Ok(TrimResult { retained })
}

/// 弧上の角度 `theta` を掃引オフセット `[0, sweep]` へ写す。
///
/// `start_angle` の直前（`wrap_2pi` が `TAU` 付近を返す）は始点として 0 に丸め、
/// 終点をわずかに超える丸め誤差は `sweep` にクランプする。
fn arc_param(arc: &Arc, theta: f64) -> f64 {
    let sweep = arc.sweep();
    let d = wrap_2pi(theta - arc.start_angle);
    if d >= TAU - EPS { 0.0 } else { d.min(sweep) }
}

/// `click` を弧へ射影した掃引オフセット（[`Arc::closest_point`] と同じ規則:
/// 範囲内なら射影角、範囲外なら近い方の端点）。
fn arc_click_param(arc: &Arc, click: Point2) -> f64 {
    let dir = click - arc.center;
    if dir.length() <= EPS {
        // 中心上のクリックは弧上のどの点とも等距離。始点扱いにする。
        return 0.0;
    }
    let theta = dir.angle();
    if arc.contains_angle(theta) {
        arc_param(arc, theta)
    } else if click.distance_squared(arc.start_point()) <= click.distance_squared(arc.end_point()) {
        0.0
    } else {
        arc.sweep()
    }
}

fn trim_arc(arc: &Arc, boundary: &Shape, click: Point2) -> Result<TrimResult, TrimExtendError> {
    if !arc.radius.is_finite() || arc.radius <= EPS {
        return Err(TrimExtendError::Degenerate);
    }
    let sweep = arc.sweep();
    if sweep <= EPS {
        return Err(TrimExtendError::Degenerate);
    }

    let hits = intersect(&Shape::Arc(*arc), boundary);
    if hits.is_empty() {
        return Err(TrimExtendError::NoIntersection);
    }
    let mut params: Vec<f64> = hits
        .iter()
        .map(|&p| arc_param(arc, (p - arc.center).angle()))
        .collect();
    params.sort_by(f64::total_cmp);

    let click_param = arc_click_param(arc, click);
    if click_matches_any_param(&params, click_param) {
        // クリックが交点ちょうどでは、どちらの区間を消すか一意に決められない。
        return Err(TrimExtendError::Degenerate);
    }
    let (left, right) = click_interval(&params, click_param, sweep);

    let mut retained = Vec::new();
    if left > EPS {
        retained.push(Shape::Arc(Arc::new(
            arc.center,
            arc.radius,
            arc.start_angle,
            arc.start_angle + left,
        )));
    }
    if sweep - right > EPS {
        retained.push(Shape::Arc(Arc::new(
            arc.center,
            arc.radius,
            arc.start_angle + right,
            arc.end_angle,
        )));
    }
    if retained.is_empty() {
        return Err(TrimExtendError::Degenerate);
    }
    Ok(TrimResult { retained })
}

// ---------------------------------------------------------------------------
// 延長
// ---------------------------------------------------------------------------

fn extend_line(seg: &LineSeg, boundary: &Shape, click: Point2) -> Result<Shape, TrimExtendError> {
    let Some(dir) = seg.direction().normalize() else {
        return Err(TrimExtendError::Degenerate);
    };
    // 自由端はクリックに近い方の端点。`u` はその外向き単位方向。
    let extend_b = click.distance_squared(seg.b) <= click.distance_squared(seg.a);
    let (fixed, free, u) = if extend_b {
        (seg.a, seg.b, dir)
    } else {
        (seg.b, seg.a, -dir)
    };

    // 境界 AABB を確実に覆う長さまで一時的に伸ばした線分で既存の交点計算にかける。
    let reach = extend_reach(free, boundary.aabb());
    let probe = LineSeg::new(
        fixed,
        free + u * (reach * (1.0 + REACH_MARGIN) + REACH_MARGIN),
    );

    // 自由端から外向きへの符号付き距離が正（＝現在の範囲の外）の候補のみ採る。
    // 許容量は座標スケールに対する相対値（`intersect` の点一致判定と同じ形）。
    let tol = EPS * (1.0 + free.to_vec2().length() + seg.length());
    let best = intersect(&Shape::Line(probe), boundary)
        .into_iter()
        .map(|p| ((p - free).dot(u), p))
        .filter(|&(s, _)| s > tol)
        .min_by(|a, b| a.0.total_cmp(&b.0));

    let Some((_, hit)) = best else {
        return Err(TrimExtendError::NoIntersection);
    };
    Ok(Shape::Line(if extend_b {
        LineSeg::new(fixed, hit)
    } else {
        LineSeg::new(hit, fixed)
    }))
}

fn extend_arc(arc: &Arc, boundary: &Shape, click: Point2) -> Result<Shape, TrimExtendError> {
    if !arc.radius.is_finite() || arc.radius <= EPS {
        return Err(TrimExtendError::Degenerate);
    }
    let sweep = arc.sweep();
    if sweep <= EPS {
        return Err(TrimExtendError::Degenerate);
    }
    // 自由端はクリックに近い方の端点。
    let extend_start =
        click.distance_squared(arc.start_point()) <= click.distance_squared(arc.end_point());

    // 台円は「無限に伸びた弧」そのものなので、既存の交点計算をそのまま使える。
    let mut best: Option<f64> = None;
    for p in intersect(&Shape::Circle(arc.circle()), boundary) {
        let theta = (p - arc.center).angle();
        // 既に弧の内部にある点は延長先の候補にしない（内部へ戻る・自己ループする解の排除）。
        let inside = wrap_2pi(theta - arc.start_angle);
        if inside > EPS && inside < sweep - EPS {
            continue;
        }
        // その側の端点から外側へ向かって最短でどれだけ回れば届くか（非負・非畳み込み）。
        let delta = if extend_start {
            wrap_2pi(arc.start_angle - theta)
        } else {
            wrap_2pi(theta - arc.end_angle)
        };
        if delta <= EPS {
            continue; // 自由端そのもの＝実質 no-op。
        }
        if best.is_none_or(|b| delta < b) {
            best = Some(delta);
        }
    }
    let Some(delta) = best else {
        return Err(TrimExtendError::NoIntersection);
    };
    // `Arc` を構築する前に、畳み込まれていない生の delta で全周を判定する
    // （構築後の `sweep()` は 2π で畳まれるため「ほぼ一周」と区別できない）。
    if sweep + delta >= TAU - EPS {
        return Err(TrimExtendError::Degenerate);
    }
    Ok(Shape::Arc(if extend_start {
        Arc::new(
            arc.center,
            arc.radius,
            arc.start_angle - delta,
            arc.end_angle,
        )
    } else {
        Arc::new(
            arc.center,
            arc.radius,
            arc.start_angle,
            arc.end_angle + delta,
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Circle, Polyline};
    use std::f64::consts::{FRAC_PI_2, PI};

    const TOL: f64 = 1e-9;

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Shape {
        Shape::Line(LineSeg::new(Point2::new(ax, ay), Point2::new(bx, by)))
    }

    fn pt(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn as_line(s: &Shape) -> LineSeg {
        match s {
            Shape::Line(l) => *l,
            other => panic!("expected line, got {other:?}"),
        }
    }

    fn as_arc(s: &Shape) -> Arc {
        match s {
            Shape::Arc(a) => *a,
            other => panic!("expected arc, got {other:?}"),
        }
    }

    fn approx_pt(a: Point2, b: Point2) {
        assert!(a.distance(b) < 1e-9, "{a:?} != {b:?}");
    }

    // --- トリム: 円を貫通する線分の 3 ケース（DESIGN.md M7 設計判断3a/8） ---

    /// 手前側（近い交点より外）をクリック → 1 断片（奥側が残る）。
    #[test]
    fn trim_line_through_circle_near_click_keeps_far_side() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let boundary = Shape::Circle(Circle::new(Point2::ORIGIN, 5.0));
        let r = trim(&target, &boundary, pt(-8.0, 0.0)).unwrap();
        assert_eq!(r.retained.len(), 1);
        let l = as_line(&r.retained[0]);
        approx_pt(l.a, pt(-5.0, 0.0));
        approx_pt(l.b, pt(10.0, 0.0));
    }

    /// 2 交点の中間をクリック → 2 断片（両側が残る）。
    #[test]
    fn trim_line_through_circle_middle_click_keeps_two_pieces() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let boundary = Shape::Circle(Circle::new(Point2::ORIGIN, 5.0));
        let r = trim(&target, &boundary, pt(0.0, 0.0)).unwrap();
        assert_eq!(r.retained.len(), 2);
        let first = as_line(&r.retained[0]);
        let second = as_line(&r.retained[1]);
        approx_pt(first.a, pt(-10.0, 0.0));
        approx_pt(first.b, pt(-5.0, 0.0));
        approx_pt(second.a, pt(5.0, 0.0));
        approx_pt(second.b, pt(10.0, 0.0));
    }

    /// 奥側（遠い交点より外）をクリック → 1 断片（手前側が残る）。
    #[test]
    fn trim_line_through_circle_far_click_keeps_near_side() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let boundary = Shape::Circle(Circle::new(Point2::ORIGIN, 5.0));
        let r = trim(&target, &boundary, pt(8.0, 0.0)).unwrap();
        assert_eq!(r.retained.len(), 1);
        let l = as_line(&r.retained[0]);
        approx_pt(l.a, pt(-10.0, 0.0));
        approx_pt(l.b, pt(5.0, 0.0));
    }

    /// 両側クリックで残る側が入れ替わる（`click` = 捨てたい側の確認）。
    #[test]
    fn trim_line_click_side_selects_removed_side() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let boundary = line(0.0, -5.0, 0.0, 5.0);
        let left = trim(&target, &boundary, pt(-5.0, 0.0)).unwrap();
        let right = trim(&target, &boundary, pt(5.0, 0.0)).unwrap();
        assert_eq!(left.retained.len(), 1);
        assert_eq!(right.retained.len(), 1);
        approx_pt(as_line(&left.retained[0]).a, pt(0.0, 0.0));
        approx_pt(as_line(&left.retained[0]).b, pt(10.0, 0.0));
        approx_pt(as_line(&right.retained[0]).a, pt(-10.0, 0.0));
        approx_pt(as_line(&right.retained[0]).b, pt(0.0, 0.0));
    }

    /// 境界ポリラインと 3 点で交わっても、クリック区間の両隣以外では分割しない。
    #[test]
    fn trim_line_polyline_boundary_splits_only_at_click_interval() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        // y=0 を x = -4, 0, 4 の 3 点で横切るジグザグ。
        let boundary = Shape::Polyline(Polyline::new(
            vec![
                pt(-4.0, -1.0),
                pt(-4.0, 1.0),
                pt(0.0, 1.0),
                pt(0.0, -1.0),
                pt(4.0, -1.0),
                pt(4.0, 1.0),
            ],
            false,
        ));
        let r = trim(&target, &boundary, pt(2.0, 0.0)).unwrap();
        assert_eq!(r.retained.len(), 2);
        // x = -4 の交点では分割されず、断片は [-10, 0] と [4, 10]。
        let first = as_line(&r.retained[0]);
        let second = as_line(&r.retained[1]);
        approx_pt(first.a, pt(-10.0, 0.0));
        approx_pt(first.b, pt(0.0, 0.0));
        approx_pt(second.a, pt(4.0, 0.0));
        approx_pt(second.b, pt(10.0, 0.0));
    }

    #[test]
    fn trim_without_intersection_is_rejected() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let boundary = line(-10.0, 5.0, 10.0, 5.0);
        assert_eq!(
            trim(&target, &boundary, pt(0.0, 0.0)),
            Err(TrimExtendError::NoIntersection)
        );
    }

    /// 交点が対象の両端にしか無い配置で中間をクリックすると 0 断片になるため拒否する
    /// （`TrimResult` の doc に記した規約。空 `Vec` は返さない）。
    #[test]
    fn trim_with_no_remaining_piece_is_degenerate() {
        let target = line(0.0, 0.0, 10.0, 0.0);
        // 端点 (0,0) と (10,0) のちょうど上を通る境界。
        let boundary = Shape::Polyline(Polyline::new(
            vec![pt(0.0, -1.0), pt(0.0, 1.0), pt(10.0, 1.0), pt(10.0, -1.0)],
            false,
        ));
        assert_eq!(
            trim(&target, &boundary, pt(5.0, 0.0)),
            Err(TrimExtendError::Degenerate)
        );
    }

    /// クリックが中間の交点ちょうどだと、どちらの区間を消すか一意に決められないため
    /// 拒否する（過大トリムのバグの再発防止。Codex M7 レビュー指摘）。
    #[test]
    fn trim_line_click_exactly_on_middle_intersection_is_degenerate() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        // y=0 を x = -4, 0, 4 の 3 点で横切るジグザグ（既存テストと同じ配置）。
        let boundary = Shape::Polyline(Polyline::new(
            vec![
                pt(-4.0, -1.0),
                pt(-4.0, 1.0),
                pt(0.0, 1.0),
                pt(0.0, -1.0),
                pt(4.0, -1.0),
                pt(4.0, 1.0),
            ],
            false,
        ));
        assert_eq!(
            trim(&target, &boundary, pt(0.0, 0.0)),
            Err(TrimExtendError::Degenerate)
        );
    }

    /// 中間の交点から `EPS` 以内の微小オフセットでも同様に拒否する。
    #[test]
    fn trim_line_click_within_eps_of_middle_intersection_is_degenerate() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let boundary = Shape::Polyline(Polyline::new(
            vec![
                pt(-4.0, -1.0),
                pt(-4.0, 1.0),
                pt(0.0, 1.0),
                pt(0.0, -1.0),
                pt(4.0, -1.0),
                pt(4.0, 1.0),
            ],
            false,
        ));
        // t = (x + 10) / 20 なので x のオフセット 1e-10 は t 空間で 5e-12（< EPS）。
        assert_eq!(
            trim(&target, &boundary, pt(1e-10, 0.0)),
            Err(TrimExtendError::Degenerate)
        );
    }

    /// 交点から十分離れた位置のクリックでは、従来どおり隣接区間だけが消える
    /// （過大トリムしない回帰確認）。
    #[test]
    fn trim_line_click_far_from_intersections_removes_only_adjacent_interval() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let boundary = Shape::Polyline(Polyline::new(
            vec![
                pt(-4.0, -1.0),
                pt(-4.0, 1.0),
                pt(0.0, 1.0),
                pt(0.0, -1.0),
                pt(4.0, -1.0),
                pt(4.0, 1.0),
            ],
            false,
        ));
        let r = trim(&target, &boundary, pt(-2.0, 0.0)).unwrap();
        assert_eq!(r.retained.len(), 2);
        let first = as_line(&r.retained[0]);
        let second = as_line(&r.retained[1]);
        approx_pt(first.a, pt(-10.0, 0.0));
        approx_pt(first.b, pt(-4.0, 0.0));
        approx_pt(second.a, pt(0.0, 0.0));
        approx_pt(second.b, pt(10.0, 0.0));
    }

    #[test]
    fn trim_unsupported_targets() {
        let boundary = line(-10.0, 0.0, 10.0, 0.0);
        let click = pt(0.0, 0.0);
        for target in [
            Shape::Circle(Circle::new(Point2::ORIGIN, 5.0)),
            Shape::Polyline(Polyline::new(vec![pt(-5.0, -5.0), pt(5.0, 5.0)], false)),
            Shape::Point(pt(0.0, 0.0)),
        ] {
            assert_eq!(
                trim(&target, &boundary, click),
                Err(TrimExtendError::Unsupported),
                "target {target:?}"
            );
        }
    }

    #[test]
    fn trim_point_boundary_is_unsupported() {
        let target = line(-10.0, 0.0, 10.0, 0.0);
        assert_eq!(
            trim(&target, &Shape::Point(pt(0.0, 0.0)), pt(5.0, 0.0)),
            Err(TrimExtendError::Unsupported)
        );
    }

    /// 弧のトリム: 上半円を x=0 の線で切り、クリック側だけが消える。
    #[test]
    fn trim_arc_one_piece() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let boundary = line(0.0, 0.0, 0.0, 10.0);
        // 角 π/4 付近（始点側）をクリック → 残るのは [π/2, π]。
        let r = trim(&Shape::Arc(arc), &boundary, pt(3.5355, 3.5355)).unwrap();
        assert_eq!(r.retained.len(), 1);
        let a = as_arc(&r.retained[0]);
        assert!((a.start_angle - FRAC_PI_2).abs() < 1e-6);
        assert!((a.end_angle - PI).abs() < TOL);
    }

    /// 弧のトリム: 弦で 2 点交わる位置の中間をクリック → 2 断片。
    #[test]
    fn trim_arc_two_pieces() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let boundary = line(-10.0, 3.0, 10.0, 3.0); // 交点 (±4, 3)。
        let r = trim(&Shape::Arc(arc), &boundary, pt(0.0, 5.0)).unwrap();
        assert_eq!(r.retained.len(), 2);
        let low = 3.0f64.atan2(4.0);
        let high = PI - low;
        let first = as_arc(&r.retained[0]);
        let second = as_arc(&r.retained[1]);
        assert!((first.start_angle - 0.0).abs() < TOL);
        assert!((first.end_angle - low).abs() < 1e-9);
        assert!((second.start_angle - high).abs() < 1e-9);
        assert!((second.end_angle - PI).abs() < TOL);
    }

    /// 弧版: 交点が3つある配置で、中間の交点ちょうどをクリックすると拒否する
    /// （線分版と同じ欠陥の弧側の再発防止。Codex M7 レビュー指摘）。
    #[test]
    fn trim_arc_click_exactly_on_middle_intersection_is_degenerate() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        // 角 PI/4, PI/2, 3*PI/4 で弧を横切る階段状の境界。
        let boundary = Shape::Polyline(Polyline::new(
            vec![
                pt(3.535_533_9, 0.0),
                pt(3.535_533_9, 10.0),
                pt(0.0, 10.0),
                pt(0.0, 0.0),
                pt(-3.535_533_9, 0.0),
                pt(-3.535_533_9, 10.0),
            ],
            false,
        ));
        // 中間の交点（角 PI/2、点 (0,5)）ちょうどをクリック。
        assert_eq!(
            trim(&Shape::Arc(arc), &boundary, pt(0.0, 5.0)),
            Err(TrimExtendError::Degenerate)
        );
    }

    /// 同じ配置で中間の交点から `EPS` 以内をクリックしても拒否する。
    #[test]
    fn trim_arc_click_within_eps_of_middle_intersection_is_degenerate() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let boundary = Shape::Polyline(Polyline::new(
            vec![
                pt(3.535_533_9, 0.0),
                pt(3.535_533_9, 10.0),
                pt(0.0, 10.0),
                pt(0.0, 0.0),
                pt(-3.535_533_9, 0.0),
                pt(-3.535_533_9, 10.0),
            ],
            false,
        ));
        // (1e-10, 5.0) の角は PI/2 から 2e-11 程度しかずれず EPS 以内。
        assert_eq!(
            trim(&Shape::Arc(arc), &boundary, pt(1e-10, 5.0)),
            Err(TrimExtendError::Degenerate)
        );
    }

    /// 同じ配置で交点から十分離れた位置をクリックすると、従来どおり隣接区間だけが
    /// 消える（過大トリムしない回帰確認）。
    #[test]
    fn trim_arc_click_far_from_intersections_removes_only_adjacent_interval() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let boundary = Shape::Polyline(Polyline::new(
            vec![
                pt(3.535_533_9, 0.0),
                pt(3.535_533_9, 10.0),
                pt(0.0, 10.0),
                pt(0.0, 0.0),
                pt(-3.535_533_9, 0.0),
                pt(-3.535_533_9, 10.0),
            ],
            false,
        ));
        // PI/4 と PI/2 の中間の角でクリック。
        let theta_click = (FRAC_PI_2 / 2.0 + FRAC_PI_2) / 2.0;
        let click = pt(5.0 * theta_click.cos(), 5.0 * theta_click.sin());
        let r = trim(&Shape::Arc(arc), &boundary, click).unwrap();
        assert_eq!(r.retained.len(), 2);
        let first = as_arc(&r.retained[0]);
        let second = as_arc(&r.retained[1]);
        assert!((first.start_angle - 0.0).abs() < TOL);
        assert!((first.end_angle - FRAC_PI_2 / 2.0).abs() < 1e-6);
        assert!((second.start_angle - FRAC_PI_2).abs() < 1e-6);
        assert!((second.end_angle - PI).abs() < TOL);
    }

    // --- 延長 ---

    #[test]
    fn extend_reach_matches_farthest_corner() {
        let bb = Aabb::from_corners(pt(1.0, 2.0), pt(4.0, 6.0));
        let free = pt(0.0, 0.0);
        // 最遠隅は (4, 6)。
        assert!((extend_reach(free, bb) - (16.0f64 + 36.0).sqrt()).abs() < TOL);
        // 内側の点でも 4 隅の最大距離。
        let inside = pt(2.0, 3.0);
        let expect = [pt(1.0, 2.0), pt(4.0, 2.0), pt(1.0, 6.0), pt(4.0, 6.0)]
            .iter()
            .map(|&c| inside.distance(c))
            .fold(0.0f64, f64::max);
        assert!((extend_reach(inside, bb) - expect).abs() < TOL);
    }

    #[test]
    fn extend_line_to_line_boundary() {
        let target = line(0.0, 0.0, 1.0, 0.0);
        let boundary = line(5.0, -1.0, 5.0, 1.0);
        let r = extend(&target, &boundary, pt(1.0, 0.0)).unwrap();
        let l = as_line(&r);
        approx_pt(l.a, pt(0.0, 0.0));
        approx_pt(l.b, pt(5.0, 0.0));
    }

    #[test]
    fn extend_line_to_circle_boundary_takes_nearest_hit() {
        let target = line(0.0, 0.0, 1.0, 0.0);
        let boundary = Shape::Circle(Circle::new(pt(10.0, 0.0), 2.0));
        let r = extend(&target, &boundary, pt(1.0, 0.0)).unwrap();
        let l = as_line(&r);
        approx_pt(l.a, pt(0.0, 0.0));
        approx_pt(l.b, pt(8.0, 0.0)); // (12,0) ではなく手前の (8,0)。
    }

    #[test]
    fn extend_line_to_arc_boundary() {
        let target = line(0.0, 0.0, 1.0, 0.0);
        // (10,0) 中心・半径 2 の左半円（(8,0) を含み (12,0) を含まない）。
        let boundary = Shape::Arc(Arc::new(pt(10.0, 0.0), 2.0, FRAC_PI_2, 3.0 * FRAC_PI_2));
        let r = extend(&target, &boundary, pt(1.0, 0.0)).unwrap();
        approx_pt(as_line(&r).b, pt(8.0, 0.0));
    }

    /// クリックが始点側なら始点側が伸びる。
    #[test]
    fn extend_line_picks_free_end_near_click() {
        let target = line(0.0, 0.0, 1.0, 0.0);
        let boundary = line(-5.0, -1.0, -5.0, 1.0);
        let r = extend(&target, &boundary, pt(0.0, 0.0)).unwrap();
        let l = as_line(&r);
        approx_pt(l.a, pt(-5.0, 0.0));
        approx_pt(l.b, pt(1.0, 0.0));
    }

    #[test]
    fn extend_line_out_of_reach_is_rejected() {
        let target = line(0.0, 0.0, 1.0, 0.0);
        // 延長方向（+x）の直線 y=0 とは交わらない位置にある境界。
        let boundary = line(5.0, 5.0, 5.0, 10.0);
        assert_eq!(
            extend(&target, &boundary, pt(1.0, 0.0)),
            Err(TrimExtendError::NoIntersection)
        );
    }

    #[test]
    fn extend_unsupported_targets() {
        let boundary = line(5.0, -1.0, 5.0, 1.0);
        for target in [
            Shape::Circle(Circle::new(Point2::ORIGIN, 1.0)),
            Shape::Polyline(Polyline::new(vec![pt(0.0, 0.0), pt(1.0, 0.0)], false)),
            Shape::Point(pt(0.0, 0.0)),
        ] {
            assert_eq!(
                extend(&target, &boundary, pt(1.0, 0.0)),
                Err(TrimExtendError::Unsupported),
                "target {target:?}"
            );
        }
    }

    /// 弧の end 側延長: 台円と 2 点で交わる境界のうち、外側で最短の候補が選ばれる。
    #[test]
    fn extend_arc_end_side_picks_nearest_outside_hit() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        let boundary = line(-3.0, -5.0, -3.0, 5.0); // 交点 (-3, ±4)。
        let r = extend(&Shape::Arc(arc), &boundary, pt(0.0, 5.0)).unwrap();
        let a = as_arc(&r);
        assert!((a.start_angle - 0.0).abs() < TOL);
        assert!((a.end_angle - 4.0f64.atan2(-3.0)).abs() < 1e-9);
    }

    /// 弧の start 側延長: 逆向きに最短で届く候補が選ばれる（角度は負のまま保持）。
    #[test]
    fn extend_arc_start_side_picks_nearest_outside_hit() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        let boundary = line(-3.0, -5.0, -3.0, 5.0);
        let r = extend(&Shape::Arc(arc), &boundary, pt(5.0, 0.0)).unwrap();
        let a = as_arc(&r);
        // (-3,-4) の角は wrap で 4.069。start 側 delta = TAU - 4.069 = 2.214。
        let delta = TAU - wrap_2pi((-4.0f64).atan2(-3.0));
        assert!((a.start_angle - (-delta)).abs() < 1e-9);
        assert!((a.end_angle - FRAC_PI_2).abs() < TOL);
    }

    /// 候補がすべて現在の弧の内部にある場合は延長先が無い（内部候補の除外の確認）。
    #[test]
    fn extend_arc_ignores_candidates_inside_current_sweep() {
        // 掃引 3π/2 の弧。x=-3 の交点（角 2.214 / 4.069）はどちらも弧の内部。
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, 3.0 * FRAC_PI_2);
        let boundary = line(-3.0, -5.0, -3.0, 5.0);
        assert_eq!(
            extend(&Shape::Arc(arc), &boundary, pt(5.0, 0.0)),
            Err(TrimExtendError::NoIntersection)
        );
        assert_eq!(
            extend(&Shape::Arc(arc), &boundary, pt(0.0, -5.0)),
            Err(TrimExtendError::NoIntersection)
        );
    }

    /// start 側を反対の端点まで回すと掃引がちょうど 2π になるため拒否する。
    ///
    /// `Arc` を構築してから `sweep()` を見ると `wrap_2pi(TAU) == 0` に畳まれ、掃引 0 の
    /// 退化弧と区別できない。畳み込み前の生の delta で判定していることの担保。
    #[test]
    fn extend_arc_full_circle_from_start_side_is_degenerate() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        // 端点 (0,5) だけで台円と交わる境界（もう一方の交点 (0,-5) は線分の外）。
        let boundary = line(0.0, 5.0, 0.0, 7.0);
        assert_eq!(
            extend(&Shape::Arc(arc), &boundary, pt(5.0, 0.0)),
            Err(TrimExtendError::Degenerate)
        );
        // 拒否しなかった場合に構築される弧は sweep() が 0 に畳まれて見分けられない。
        let folded = Arc::new(Point2::ORIGIN, 5.0, -3.0 * FRAC_PI_2, FRAC_PI_2);
        assert!(folded.sweep() < 1e-9);
    }

    /// end 側でも同様に、反対の端点まで回す候補は拒否される。
    #[test]
    fn extend_arc_full_circle_from_end_side_is_degenerate() {
        let arc = Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        // 端点 (5,0) だけで台円と交わる境界。
        let boundary = line(5.0, 0.0, 7.0, 0.0);
        assert_eq!(
            extend(&Shape::Arc(arc), &boundary, pt(0.0, 5.0)),
            Err(TrimExtendError::Degenerate)
        );
    }

    #[test]
    fn extend_degenerate_targets() {
        let boundary = line(5.0, -1.0, 5.0, 1.0);
        // ゼロ長線分は方向が定まらない。
        assert_eq!(
            extend(&line(0.0, 0.0, 0.0, 0.0), &boundary, pt(0.0, 0.0)),
            Err(TrimExtendError::Degenerate)
        );
        // 掃引 0 の弧。
        let arc = Shape::Arc(Arc::new(Point2::ORIGIN, 5.0, 1.0, 1.0));
        assert_eq!(
            extend(&arc, &boundary, pt(5.0, 0.0)),
            Err(TrimExtendError::Degenerate)
        );
    }
}
