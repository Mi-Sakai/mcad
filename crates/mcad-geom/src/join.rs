//! 開いた点列（線分・開ポリライン）を 1 本の鎖へ結合する（[`join_chain`]）。
//! DESIGN.md M12 設計判断8。
//!
//! 対象は「端点どうしがつながって 1 本の鎖（分岐しない）になる」piece の集合だけ。
//! 各 piece は開いた点列（線分なら 2 点、開ポリラインなら頂点列）で渡す。閉じている
//! かどうかの判定・向きの調整はこの関数がすべて行い、呼び出し側（app 層）は
//! `Shape::Line` / 開いた `Shape::Polyline` から `Vec<Point2>` を取り出すだけでよい。
//!
//! # 端点の一致判定
//!
//! [`crate::point_tol`]（[`crate::EPS`] を両端点のスケールで相対化したもの）を使う。
//! [`crate::chamfer`] / [`crate::split`] / [`crate::trim_extend`] が端点近接判定に使っている
//! ものと同じ道具立て（絶対 `EPS` 直比較は座標スケールに弱いため採らない）。
//!
//! # 頂点順の決定性
//!
//! 出力の頂点順は **先頭 piece（`pieces[0]`）の向きをそのまま基準にする**。
//! `pieces[0]` の点列をまず結果へ置き、`pieces[0]` の終端から未使用 piece を
//! `pieces` の元の順で探して前へ伸ばし（見つからなくなるまで）、続けて `pieces[0]`
//! の始端から同様に後ろへ伸ばす。輪になる場合は終端側の伸長中に先頭点へ戻って閉じる
//! （この場合は始端側の伸長を行わない）。同じ入力なら常に同じ頂点列になる。
//!
//! 連結点の頂点は 1 つにまとめ、座標は **先に結果へ入った piece の端点**を採用する
//! （後から連結する piece 側の重複端点は捨てる）。共線の連続頂点はまとめない
//! （形状を変えないため）。

use crate::{Point2, Polyline, point_tol};

/// [`join_chain`] が結果を作れない理由。
///
/// geom は GUI 非依存なのでユーザー向け文言は持たない（[`crate::ChamferError`] と同じ
/// パターン）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinError {
    /// piece が 2 未満。
    TooFewPieces,
    /// 頂点が 2 未満、または実質長さ 0（[`crate::point_tol`] 以下）の piece がある。
    DegeneratePiece,
    /// 端点の次数が 3 以上（分岐）のノードがある。
    Branching,
    /// piece 全体が 1 つの連結成分にならない（離れている）。
    Disconnected,
    /// 結果が退化して確定できない（輪になったが頂点が 3 未満。例: 同じ 2 点を結ぶ
    /// 線分 2 本）。
    DegenerateResult,
}

/// 開いた点列の集合 `pieces` を 1 本の鎖へ結合する。
///
/// 各 piece は端点（`piece[0]` と `piece[piece.len()-1]`）でのみ他の piece とつながる。
/// 全 piece の端点が作るグラフが「分岐せず、1 つに連結した鎖」であるときだけ成功する。
/// 全端点の次数がちょうど 2 なら輪（`closed = true`、重複する終端頂点は落とす）。
/// それ以外で連結・非分岐なら開いた鎖（`closed = false`）。
///
/// # Errors
///
/// モジュール doc の [`JoinError`] を参照。
pub fn join_chain(pieces: &[Vec<Point2>]) -> Result<Polyline, JoinError> {
    if pieces.len() < 2 {
        return Err(JoinError::TooFewPieces);
    }

    for p in pieces {
        if p.len() < 2 {
            return Err(JoinError::DegeneratePiece);
        }
        let total_len: f64 = p.windows(2).map(|w| w[0].distance(w[1])).sum();
        let first = p[0];
        let last = *p.last().expect("checked len >= 2");
        if total_len <= point_tol(first, last) {
            return Err(JoinError::DegeneratePiece);
        }
    }

    // 端点をクラスタリングしてノード化する（O(pieces^2)、選択集合はごく小規模な前提）。
    let mut nodes: Vec<Point2> = Vec::new();
    let node_of = |point: Point2, nodes: &mut Vec<Point2>| -> usize {
        for (i, n) in nodes.iter().enumerate() {
            if n.distance(point) <= point_tol(*n, point) {
                return i;
            }
        }
        nodes.push(point);
        nodes.len() - 1
    };

    // 各 piece の (始端ノード, 終端ノード)。
    let piece_nodes: Vec<(usize, usize)> = pieces
        .iter()
        .map(|p| {
            let s = node_of(p[0], &mut nodes);
            let e = node_of(*p.last().expect("checked len >= 2"), &mut nodes);
            (s, e)
        })
        .collect();

    let mut degree = vec![0usize; nodes.len()];
    for &(s, e) in &piece_nodes {
        degree[s] += 1;
        degree[e] += 1;
    }
    if degree.iter().any(|&d| d > 2) {
        return Err(JoinError::Branching);
    }

    // 連結性（union-find）。
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        if parent[x] != x {
            parent[x] = find(parent, parent[x]);
        }
        parent[x]
    }
    for &(s, e) in &piece_nodes {
        let rs = find(&mut parent, s);
        let re = find(&mut parent, e);
        if rs != re {
            parent[rs] = re;
        }
    }
    let root0 = find(&mut parent, 0);
    if (0..nodes.len()).any(|i| find(&mut parent, i) != root0) {
        return Err(JoinError::Disconnected);
    }

    let mut used = vec![false; pieces.len()];
    used[0] = true;
    let mut result: Vec<Point2> = pieces[0].clone();
    let mut closed = false;

    // 終端側（pieces[0] の終端ノード）から前へ伸ばす。
    let mut current = piece_nodes[0].1;
    while let Some(j) = (0..pieces.len())
        .find(|&i| !used[i] && (piece_nodes[i].0 == current || piece_nodes[i].1 == current))
    {
        used[j] = true;
        let (s, e) = piece_nodes[j];
        // seg_points の先頭が current と一致する向きへ揃える。
        let (seg_points, next): (Vec<Point2>, usize) = if s == current {
            (pieces[j].clone(), e)
        } else {
            (pieces[j].iter().rev().copied().collect(), s)
        };
        result.extend_from_slice(&seg_points[1..]);
        current = next;

        let head = result[0];
        let tail = *result.last().expect("result is never empty");
        if tail.distance(head) <= point_tol(head, tail) {
            result.pop();
            closed = true;
            break;
        }
    }

    if !closed {
        // 始端側（pieces[0] の始端ノード）から後ろへ伸ばす。
        let mut current = piece_nodes[0].0;
        while let Some(j) = (0..pieces.len())
            .find(|&i| !used[i] && (piece_nodes[i].0 == current || piece_nodes[i].1 == current))
        {
            used[j] = true;
            let (s, e) = piece_nodes[j];
            // seg_points の末尾が current と一致する向きへ揃え、末尾（current と重複）を
            // 落として前へ連結する。
            let (seg_points, next): (Vec<Point2>, usize) = if e == current {
                (pieces[j].clone(), s)
            } else {
                (pieces[j].iter().rev().copied().collect(), e)
            };
            let mut new_result = seg_points[..seg_points.len() - 1].to_vec();
            new_result.extend_from_slice(&result);
            result = new_result;
            current = next;
        }
    }

    // 次数 <= 2 かつ単一連結成分なら、グラフは単純な道か単純な輪のどちらかにしかならず、
    // 両方向の伸長で全 piece を使い切るはず。数値誤差等の想定外に備えた安全網。
    if used.iter().any(|&u| !u) {
        return Err(JoinError::Disconnected);
    }

    if closed && result.len() < 3 {
        return Err(JoinError::DegenerateResult);
    }

    Ok(Polyline::new(result, closed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn approx_eq(a: &Polyline, vertices: &[Point2], closed: bool) {
        assert_eq!(a.closed, closed);
        assert_eq!(a.vertices.len(), vertices.len(), "{:?}", a.vertices);
        for (v, w) in a.vertices.iter().zip(vertices) {
            assert!(v.distance(*w) < 1e-9, "{v:?} != {w:?}");
        }
    }

    /// 2 本の線分が端点で連結（既に順向き）。
    #[test]
    fn two_lines_in_order() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        let b = vec![pt(10.0, 0.0), pt(10.0, 10.0)];
        let res = join_chain(&[a, b]).unwrap();
        approx_eq(&res, &[pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0)], false);
    }

    /// 順不同・向き不同でも同じ鎖になる（結果の頂点集合・閉フラグは変わらない）。
    #[test]
    fn two_lines_reversed_and_swapped_order() {
        let a = vec![pt(10.0, 0.0), pt(0.0, 0.0)]; // 向き反転
        let b = vec![pt(10.0, 10.0), pt(10.0, 0.0)]; // 向き反転
        // 順番も入れ替え: b が先頭。
        let res = join_chain(&[b, a]).unwrap();
        // 先頭 piece (b, 向き [10,10]->[10,0]) を基準に伸びるので b の並びで始まる。
        approx_eq(&res, &[pt(10.0, 10.0), pt(10.0, 0.0), pt(0.0, 0.0)], false);
    }

    /// 線分 + 開ポリラインの混在。
    #[test]
    fn line_and_open_polyline_mixed() {
        let line = vec![pt(0.0, 0.0), pt(5.0, 0.0)];
        let poly = vec![pt(5.0, 0.0), pt(5.0, 5.0), pt(10.0, 5.0)];
        let res = join_chain(&[line, poly]).unwrap();
        approx_eq(
            &res,
            &[pt(0.0, 0.0), pt(5.0, 0.0), pt(5.0, 5.0), pt(10.0, 5.0)],
            false,
        );
    }

    /// 3 本以上が中間の piece を挟んでも正しく結合する（挿入順は piece0 基準）。
    #[test]
    fn three_pieces_form_a_path_regardless_of_order() {
        let a = vec![pt(0.0, 0.0), pt(1.0, 0.0)];
        let b = vec![pt(1.0, 0.0), pt(2.0, 0.0)];
        let c = vec![pt(2.0, 0.0), pt(3.0, 0.0)];
        // 先頭を b にして、a・c を逆順に渡す。
        let res = join_chain(&[b, c.clone(), a.clone()]).unwrap();
        approx_eq(
            &res,
            &[pt(0.0, 0.0), pt(1.0, 0.0), pt(2.0, 0.0), pt(3.0, 0.0)],
            false,
        );
    }

    /// 4 本の線分が四角形の輪になる。
    #[test]
    fn four_lines_form_a_closed_loop() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        let b = vec![pt(10.0, 0.0), pt(10.0, 10.0)];
        let c = vec![pt(10.0, 10.0), pt(0.0, 10.0)];
        let d = vec![pt(0.0, 10.0), pt(0.0, 0.0)];
        let res = join_chain(&[a, b, c, d]).unwrap();
        approx_eq(
            &res,
            &[pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)],
            true,
        );
    }

    /// 決定性: 同じ入力なら常に同じ頂点列。
    #[test]
    fn deterministic_for_same_input() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        let b = vec![pt(10.0, 0.0), pt(10.0, 10.0)];
        let r1 = join_chain(&[a.clone(), b.clone()]).unwrap();
        let r2 = join_chain(&[a, b]).unwrap();
        assert_eq!(r1, r2);
    }

    /// 分岐（T 字）は拒否する。
    #[test]
    fn branching_is_rejected() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        let b = vec![pt(10.0, 0.0), pt(10.0, 10.0)];
        let c = vec![pt(10.0, 0.0), pt(20.0, 0.0)];
        assert_eq!(join_chain(&[a, b, c]), Err(JoinError::Branching));
    }

    /// 離れた図形は拒否する。
    #[test]
    fn disconnected_pieces_are_rejected() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        let b = vec![pt(100.0, 100.0), pt(110.0, 100.0)];
        assert_eq!(join_chain(&[a, b]), Err(JoinError::Disconnected));
    }

    /// piece が 2 未満は拒否する。
    #[test]
    fn too_few_pieces_is_rejected() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        assert_eq!(join_chain(&[a]), Err(JoinError::TooFewPieces));
        assert_eq!(join_chain(&[]), Err(JoinError::TooFewPieces));
    }

    /// 頂点 1 個以下、または実質長さ 0 の piece は拒否する。
    #[test]
    fn degenerate_piece_is_rejected() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        let zero_len = vec![pt(10.0, 0.0), pt(10.0, 0.0)];
        assert_eq!(
            join_chain(&[a.clone(), zero_len]),
            Err(JoinError::DegeneratePiece)
        );
        let single_point = vec![pt(10.0, 0.0)];
        assert_eq!(
            join_chain(&[a, single_point]),
            Err(JoinError::DegeneratePiece)
        );
    }

    /// 同じ 2 点を結ぶ線分 2 本は輪として頂点 2 個にしかならないため拒否する。
    #[test]
    fn two_segments_between_same_points_form_degenerate_loop() {
        let a = vec![pt(0.0, 0.0), pt(10.0, 0.0)];
        let b = vec![pt(10.0, 0.0), pt(0.0, 0.0)];
        assert_eq!(join_chain(&[a, b]), Err(JoinError::DegenerateResult));
    }
}
