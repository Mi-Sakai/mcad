//! スナップエンジン（DESIGN.md 3.4、種別追加は M12 設計判断5）。
//!
//! カーソルのワールド座標とワールド単位の探索半径を受け取り、半径内のスナップ
//! 候補点を種別付きで列挙して **1 点だけ** 返す。GUI（egui）には一切依存せず、
//! [`Document`] と幾何プリミティブ（`mcad-geom`）だけで完結するため、候補列挙・
//! 優先度判定は headless で単体テストできる（本ファイル末尾の `tests` を参照）。
//!
//! # 優先度
//!
//! DESIGN.md M12 設計判断5 の優先順位に厳密に従う:
//!
//! > 端点 > 交点 > 中点 > 中心 = 四半円点 > 垂線 = 接線 > 仮想交点 > 近接点 > グリッド
//!
//! 中心と四半円点は同順位（[`SnapKind::priority`] が同じ値を返す）で、同順位内は
//! カーソルとの距離で決める。
//!
//! 半径内に複数種別の候補があれば **優先度が高い種別** を採用し、同種別内では
//! カーソルに **最も近い** ものを選ぶ（[`Best::consider`]）。半径内に候補が
//! 何も無ければ [`None`]（＝スナップなし。呼び出し側は素のカーソル位置を使う）。
//!
//! 垂線・接線（[`SnapKind::Perpendicular`]・[`SnapKind::Tangent`]、M12 タスク86）は
//! **基点が要る**（[`snap`] の `base` 引数）。作図ツールの2点目以降で
//! `Tool::ortho_origin()` が渡されたときだけ候補になり、基点の無い段階・ツールでは
//! 計算自体を行わない。
//!
//! # 種別ごとの ON/OFF
//!
//! [`SnapKinds`] が種別ごとの有効/無効を持つ（`config.json` に永続化。
//! [`crate::config::Config::snap_kinds`]）。無効な種別は [`Best::consider`] が
//! 弾くため候補にすら上がらない。**近接点（[`SnapKind::Nearest`]）は既定 OFF**
//! （常にどこかの図形に当たり、グリッドや他の候補を潰すため）。他は既定 ON。
//! `F3`（`snap_enabled`）は全体スイッチのままで、これとは別の階層。
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
//! 一方、端点・中点・中心・四半円点は 1 エンティティあたり定数個で列挙コストが
//! O(n) に収まるため、可視 AABB での事前絞り込みは行わず、可視レイヤーの全
//! エンティティを走査する（DESIGN.md が AABB 事前絞り込みを要求しているのは交点の
//! み）。半径内（数ピクセル相当）に入る端点・中点・中心・四半円点は、オンスクリーンの
//! カーソル近傍にある以上その所属エンティティも実質的に画面内なので、絞り込みの
//! 有無で結果は変わらない。
//!
//! 近接点（[`SnapKind::Nearest`]、M12 タスク85）は交点と **同じカーソル近傍 AABB**
//! （`near`）を再利用して絞り込む。無損失である理由も交点と同じ論法: カーソルから
//! 半径内に最近点を持つ形状があれば、その最近点自体が半径内（＝カーソル近傍 AABB
//! 内）にあり、かつ形状上の点は必ずその形状の AABB に含まれるため、形状の AABB は
//! カーソル近傍 AABB と交差する。
//!
//! 垂線・接線（M12 タスク86）も **同じ `near` を再利用する**。無損失の理由は近接点と
//! 全く同じ論法（候補点がカーソル半径内にあれば、その候補点を含む図形の AABB は
//! カーソル近傍 AABB と交わる）で、垂線の足・接点も形状上の点である以上この論法が
//! そのまま成り立つ。`near` を作る条件へ `(kinds.perpendicular || kinds.tangent) &&
//! base.is_some()` を足し、無効な種別・基点の無い呼び出しでは集合の構築自体を省く
//! （M12 タスク85 Codex adversarial review の「無効な種別は計算自体を省く」方針を
//! そのまま踏襲）。
//!
//! # 仮想交点の絞り込み（M12 タスク87）
//!
//! 仮想交点（[`SnapKind::VirtualIntersection`]）は線分・ポリラインの辺の **延長どうし**、
//! および延長と円・円弧の交点を候補にする。交点は延長線上（両エンティティの AABB の
//! 外）にありうるため、交点用の `near`（カーソル近傍 AABB との交差）は使えない
//! （延長線上の交点を持つ図形の AABB がカーソル近傍 AABB と交差する保証がない）。
//!
//! 代わりに、可視 Shape を全走査して次のどちらかに当たるものだけを集める:
//!
//! - 線要素（[`Shape::Line`]・[`Shape::Polyline`] の各辺）: **無限直線とカーソルの
//!   距離が探索半径以内**。
//! - 円要素（[`Shape::Circle`]・[`Shape::Arc`] を台円とみなしたもの）:
//!   **`|カーソル-中心間距離 − 半径| が探索半径以内**。
//!
//! この絞り込みは無損失: 交点 `p` がカーソルから半径 `r` 以内にあるとき、
//! - 直線については、点と直線の距離は `p` を含む直線上の任意の点までの距離以下なので、
//!   直線とカーソルの距離は `|cursor - p| ≤ r` 以下になる。
//! - 円については、三角不等式より `|cursor-center| ≤ |cursor-p| + |p-center| ≤ r + 半径`、
//!   かつ `|cursor-center| ≥ |p-center| - |cursor-p| ≥ 半径 - r` なので、
//!   `| |cursor-center| - 半径 | ≤ r` が成り立つ。
//!
//! 集めた線要素・円要素どうしの組（線×線、線×円のみ。円×円は設計判断5の対象外）を
//! 総当たりし、[`mcad_geom::line_line_intersection`]・[`mcad_geom::line_circle_intersection`]
//! （どちらも無限直線とみなす）で交点を求める。交点が両方の実体上（線分の区間内、
//! かつ円上／円弧の掃引範囲内）にある場合は**真の交点**として [`SnapKind::Intersection`]
//! の担当に譲り、仮想交点の候補にはしない（重複は優先度で解ける）。少なくとも片方が
//! 延長側なら仮想交点の候補にし、延長側にだけ [`SnapGuide`] を添える。
//! `kinds.virtual_intersection` が `false` なら、この走査自体を行わない
//! （M12 タスク85 Codex adversarial review の「無効な種別は計算自体を省く」方針を
//! そのまま踏襲）。
//!
//! ## 同一直線上の線要素のグループ化と上限（Codex adversarial review [high]、2026-09-26）
//!
//! 上記の絞り込みだけでは、「カーソルの行・列上に同じ無限直線の線分が数千本ある図面」
//! （例: 同じ y=0 上に短い線分が5000本並ぶ）で総当たりが本数の2乗に爆発し、既定 ON の
//! まま毎マウス移動で固まる。これを避けるため、収集した線要素を
//! [`group_collinear_lines`] で「同一の無限直線とみなせるもの」ごとに1つの
//! [`LineGroup`]（支持直線）へまとめてから総当たりする（交点計算はグループの代表
//! 線分どうし・代表と円で行い、実体上の判定〔[`group_contains_point`]〕・ガイドの
//! `from`〔[`nearest_group_endpoint`]〕はグループの**全メンバー**を見る）。これにより
//! 「同じ無限直線上に大量の線分」というケースの計算量は本数ではなくグループ数（≒
//! 図面中の異なる直線の本数）に依存するようになる。
//!
//! 一方、グループ化しても「放射状に多数の非平行な直線がカーソル近傍を通る」病的な
//! ケースは残る。**当初はグループ化後の線グループ数・円要素数を本数で切り詰めていたが
//! （`VIRTUAL_INTERSECTION_MAX_LINES`・`VIRTUAL_INTERSECTION_MAX_CIRCLES`）、
//! Codex adversarial review [medium] の再指摘（2026-09-26）でこの方式の欠陥が判明した**:
//! 本数の切り詰めは「交わるかどうか」を見ないため、カーソル半径内に異なる平行線
//! （ハッチング等）が上限本数以上あると、枠が平行線だけで埋まってしまい、少し遠い
//! 交差線（実際に仮想交点を作る相手）が切り捨てられて交点そのものが消える。
//!
//! 対応として、**本数の上限を「実際に計算する組の数」の予算へ置き換え**、あわせて
//! **平行なグループどうしは組を作らない**ようにした（[`group_collinear_lines`] の
//! 段階1で採番する [`LineGroup::angle_cluster`] が同じグループどうしはスキップする。
//! ハッチングのような平行線がいくら多くても組の数を増やさない）。線グループ・円要素は
//! どちらもカーソルとの距離で昇順に並べておき、組は「距離順に並べた添字 `i` の昇順、
//! 各 `i` に対し `j < i` の昇順」で処理する（先に処理される組ほど両方がカーソルに
//! 近い）。**線×線（[`VIRTUAL_INTERSECTION_MAX_LINE_PAIRS`]）と線×円
//! （[`VIRTUAL_INTERSECTION_MAX_LINE_CIRCLE_PAIRS`]）は独立した予算**を持つ（単一の
//! 共有予算だと、線×線を先に処理する実装順序の都合で、非平行な線グループが予算本数
//! 近くカーソル近傍にあるだけで線×円が1組も調べられなくなるため。Codex adversarial
//! review [medium] 再々指摘、2026-09-26）。**既知の制約**（打ち切りが無損失でなくなる
//! 唯一の例外）は DESIGN.md タスク87の実装時追記を参照。
//!
//! 副作用として、**同一直線上の別の線分の延長が真の交点上で仮想交点として出ることは
//! なくなった**: グループ化前は同一直線上の線分ごとに独立して真偽を判定していたため、
//! ある線分では延長（虚）でも同じグループの別の線分では区間内（実）という場合に、
//! 前者を仮想交点として提示してしまっていた。グループ化後は「グループ内のいずれかの
//! メンバー上にあれば実体上」という判定になり、この不整合が解消された。

use mcad_core::{Document, Entity, EntityGeom, EntityId};
use mcad_geom::{
    Aabb, Arc, Circle, LineSeg, Point2, Shape, Vec2, closest_point, intersect,
    line_circle_intersection, line_line_intersection, point_tol,
};
use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use crate::config::GridMode;

/// 四半円点（[`SnapKind::Quadrant`]）の候補角（ラジアン。0°/90°/180°/270°、+X から CCW）。
/// [`mcad_geom::Circle::quadrant_points`] と対応する。
const QUADRANT_ANGLES: [f64; 4] = [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2];

/// スナップ候補の種別。優先度は端点 > 交点 > 中点 > 中心 = 四半円点 > 垂線 = 接線 >
/// 仮想交点 > 近接点 > グリッド（DESIGN.md M12 設計判断5）。数値優先度は
/// [`SnapKind::priority`] が返す。
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
    /// 四半円点（円・円弧のワールド軸方向 0°/90°/180°/270° の点。円弧は弧の範囲内のみ。
    /// M12 タスク85）。
    Quadrant,
    /// 垂線の足（基点から線分・ポリラインの辺への垂線の足。線分上のみ、延長上は
    /// 出さない。基点が要る。M12 タスク86）。
    Perpendicular,
    /// 接線の接点（基点から円・円弧への接線の接点。円弧は弧の範囲内のみ。基点が要る。
    /// M12 タスク86）。
    Tangent,
    /// 仮想交点（線分・ポリラインの辺の延長どうし、および延長と円・円弧の交点。
    /// 実体上にある交点（真の交点）は [`SnapKind::Intersection`] の担当なので候補にしない。
    /// M12 タスク87）。
    VirtualIntersection,
    /// 近接点（可視 Shape 上でカーソルに最も近い点。既定 OFF。M12 タスク85）。
    Nearest,
    /// グリッド交点。
    Grid,
}

impl SnapKind {
    /// 数値優先度（小さいほど高優先）。同順位内はカーソルとの距離で決める。
    ///
    /// DESIGN.md M12 設計判断5: 端点 > 交点 > 中点 > 中心 = 四半円点 > 垂線 = 接線 >
    /// 仮想交点 > 近接点 > グリッド。
    #[must_use]
    fn priority(self) -> u8 {
        match self {
            SnapKind::Endpoint => 0,
            SnapKind::Intersection => 1,
            SnapKind::Midpoint => 2,
            SnapKind::Center => 3,
            SnapKind::Quadrant => 3,
            SnapKind::Perpendicular => 4,
            SnapKind::Tangent => 4,
            SnapKind::VirtualIntersection => 5,
            SnapKind::Nearest => 6,
            SnapKind::Grid => 7,
        }
    }
}

/// スナップ種別ごとの ON/OFF（`config.json` に永続化。[`crate::config::Config::snap_kinds`]）。
///
/// `#[serde(default)]` によりフィールド単位で前方互換にする（新種別が増えても旧
/// config.json には存在しないため、そのフィールドの [`Default`] 値へ補う）。**近接点
/// （`nearest`）だけ既定 OFF**（常にどこかの図形に当たり、グリッドや他の候補を潰す
/// ため）で、他は既定 ON（DESIGN.md M12 設計判断5）。`F3`（`snap_enabled`）は全体
/// スイッチのままで、これとは別の階層。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapKinds {
    /// 端点。既定 ON。
    #[serde(default = "default_true")]
    pub endpoint: bool,
    /// 交点。既定 ON。
    #[serde(default = "default_true")]
    pub intersection: bool,
    /// 中点。既定 ON。
    #[serde(default = "default_true")]
    pub midpoint: bool,
    /// 中心。既定 ON。
    #[serde(default = "default_true")]
    pub center: bool,
    /// 四半円点。既定 ON。
    #[serde(default = "default_true")]
    pub quadrant: bool,
    /// 垂線の足。既定 ON。M12 タスク86。
    #[serde(default = "default_true")]
    pub perpendicular: bool,
    /// 接線の接点。既定 ON。M12 タスク86。
    #[serde(default = "default_true")]
    pub tangent: bool,
    /// 仮想交点。既定 ON。M12 タスク87。
    #[serde(default = "default_true")]
    pub virtual_intersection: bool,
    /// 近接点。**既定 OFF**。
    #[serde(default)]
    pub nearest: bool,
    /// グリッド。既定 ON。
    #[serde(default = "default_true")]
    pub grid: bool,
}

/// `#[serde(default = "default_true")]` 用のヘルパー（`bool` の `Default` は `false`
/// なので、既定 ON のフィールドはこちらを使う）。
fn default_true() -> bool {
    true
}

impl Default for SnapKinds {
    fn default() -> Self {
        Self {
            endpoint: true,
            intersection: true,
            midpoint: true,
            center: true,
            quadrant: true,
            perpendicular: true,
            tangent: true,
            virtual_intersection: true,
            nearest: false,
            grid: true,
        }
    }
}

impl SnapKinds {
    /// `kind` が有効か。
    #[must_use]
    pub fn enabled(&self, kind: SnapKind) -> bool {
        match kind {
            SnapKind::Endpoint => self.endpoint,
            SnapKind::Intersection => self.intersection,
            SnapKind::Midpoint => self.midpoint,
            SnapKind::Center => self.center,
            SnapKind::Quadrant => self.quadrant,
            SnapKind::Perpendicular => self.perpendicular,
            SnapKind::Tangent => self.tangent,
            SnapKind::VirtualIntersection => self.virtual_intersection,
            SnapKind::Nearest => self.nearest,
            SnapKind::Grid => self.grid,
        }
    }
}

/// 仮想交点（[`SnapKind::VirtualIntersection`]）の延長部分を示すガイド（点線で描く）。
///
/// 実体上（線分の区間内・円上・円弧の掃引範囲内）にある側にはガイドが無い
/// （[`SnapResult::guides`] が `None` になる）。`Copy` を保つため配列 2 要素で持つ
/// （組は線×線・線×円の 2 者間なので最大 2 本）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapGuide {
    /// 線要素（線分・ポリラインの辺）の延長部分。`from` は実体側の近い方の端点、
    /// `to` は仮想交点。
    Segment {
        /// ガイドの起点（実体側の近い方の端点）。
        from: Point2,
        /// ガイドの終点（仮想交点）。
        to: Point2,
    },
    /// 円弧の掃引範囲外（延長側）の円周に沿ったガイド。近い方の弧端点
    /// （`start_angle`）から仮想交点まで、弧の外側（掃引範囲の無い側）を回る向きに
    /// `sweep` ラジアン進む（負なら時計回り）。完全な円（[`Shape::Circle`]）は
    /// 掃引の概念が無くこのバリアントを使わない（常にガイド無し）。
    Arc {
        /// 台円の中心。
        center: Point2,
        /// 台円の半径。
        radius: f64,
        /// ガイドの開始角（ラジアン。近い方の弧端点の角度）。
        start_angle: f64,
        /// ガイドの掃引角（ラジアン、符号付き。正が CCW、負が CW）。
        sweep: f64,
    },
}

/// スナップ結果。採用した候補の種別と、スナップ先のワールド座標。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapResult {
    /// 採用した候補の種別（マーカー描画で見た目を変えるのに使う）。
    pub kind: SnapKind,
    /// スナップ先のワールド座標。
    pub point: Point2,
    /// 仮想交点（[`SnapKind::VirtualIntersection`]）の延長部分ガイド。組んだ 2
    /// エンティティぶん（線×線・線×円）。実体上にある側・他の種別は `None`。
    pub guides: [Option<SnapGuide>; 2],
}

/// グリッドスナップのパラメータ（刻み幅＋モード）。DESIGN.md 7章「随時対応」
/// アイソメ図・アクソメ図の作図補助 設計確定3。
///
/// 従来の `grid_step: f64` 引数を「刻み＋モード」へ一般化したもの。`step` が
/// `0` 以下・非有限ならモードによらずグリッド候補なし（[`snap`] 参照）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpec {
    /// グリッド間隔（ワールド単位）。
    pub step: f64,
    /// グリッド種別（矩形/等測）。
    pub mode: GridMode,
}

impl GridSpec {
    /// `GridSpec` を作る。
    #[must_use]
    pub fn new(step: f64, mode: GridMode) -> Self {
        Self { step, mode }
    }
}

/// 等測グリッドの格子点スナップ（純関数）。
///
/// 格子ベクトル `u = step·(cos30°, sin30°)`、`v = step·(cos150°, sin150°)` に対し、
/// 斜交座標 `cursor = a·u + b·v` を解き、`a`・`b` それぞれの floor/ceil の組合せ
/// 4 点のうち**ユークリッド距離が最小**の格子点を返す（DESIGN.md 7章「随時対応」
/// アイソメ図・アクソメ図の作図補助 設計確定3）。`step` が非有限または `0` 以下なら `None`。
///
/// `a`・`b` を独立に `round` してはいけない: 基底が 120° で交わる斜交座標では成分ごとの
/// 丸めが最近点にならず（例: `(a, b) = (0.49, −0.49)` は原点〔距離 0.849·step〕より
/// 格子点 `u`〔距離 0.500·step〕が近い）、描画された線群の交点とスナップ先が食い違う
/// （Codex adversarial review 2026-09-06 指摘）。三角格子の Voronoi 胞は隣接 4 候補の
/// いずれかに必ず含まれる。
#[must_use]
pub fn iso_grid_snap(cursor: Point2, step: f64) -> Option<Point2> {
    if !step.is_finite() || step <= 0.0 {
        return None;
    }
    // u = step·(cos30°, sin30°) = step·(√3/2, 1/2)
    // v = step·(cos150°, sin150°) = step·(-√3/2, 1/2)
    let ux = step * 3f64.sqrt() / 2.0;
    let uy = step * 0.5;
    let vx = -ux;
    let vy = uy;

    // [ux vx][a]   [cursor.x]
    // [uy vy][b] = [cursor.y]
    let det = ux * vy - vx * uy;
    if !det.is_finite() || det.abs() < f64::EPSILON {
        return None;
    }
    let a = (cursor.x * vy - vx * cursor.y) / det;
    let b = (ux * cursor.y - cursor.x * uy) / det;
    let lattice = |a: f64, b: f64| Point2::new(a * ux + b * vx, a * uy + b * vy);
    let (a0, b0) = (a.floor(), b.floor());
    let mut best: Option<(f64, Point2)> = None;
    for (da, db) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let p = lattice(a0 + da, b0 + db);
        let d2 = (p.x - cursor.x).powi(2) + (p.y - cursor.y).powi(2);
        if best.is_none_or(|(bd, _)| d2 < bd) {
            best = Some((d2, p));
        }
    }
    best.map(|(_, p)| p)
}

/// カーソル位置 `cursor` から半径 `radius`（ワールド単位）内のスナップ候補を
/// 列挙し、優先度・距離規則で 1 点を選んで返す。候補が無ければ `None`。
///
/// - `document`: 候補の元となるエンティティ（可視レイヤーのみ対象）。
/// - `cursor`: カーソルのワールド座標。
/// - `radius`: 探索半径（ワールド単位。呼び出し側で `半径px / zoom` に換算済み）。
/// - `grid`: グリッド間隔＋モード（[`GridSpec`]）。`step` が `0` 以下・非有限なら
///   モードによらずグリッド候補なし。
/// - `kinds`: 種別ごとの ON/OFF（[`SnapKinds`]）。無効な種別は候補にならない
///   （`extra_points` も [`SnapKind::Endpoint`] として扱われるため、`kinds.endpoint`
///   が `false` なら出なくなる）。
/// - `extra_points`: 作図中（未確定）ツールの頂点列（[`crate::tool::Tool::snap_points`]）。
///   `Document` にまだ存在しない点だが、[`SnapKind::Endpoint`]（最優先）候補として
///   他の端点と同じ優先度・最近傍規則で扱う。ポリライン作図中に自分自身の始点へ
///   スナップして自動クローズできるようにするための拡張。
/// - `base`: 垂線・接線（[`SnapKind::Perpendicular`]・[`SnapKind::Tangent`]、M12
///   タスク86）の基点。`None` なら垂線・接線の候補を一切出さない（計算自体を省く）。
///   作図ツールの経路では `Tool::ortho_origin()` を渡す（選択モードの配置・グリップ
///   ドラッグ等、基点を持たない呼び出しは `None`）。
#[must_use]
pub fn snap(
    document: &Document,
    cursor: Point2,
    radius: f64,
    grid: GridSpec,
    kinds: SnapKinds,
    extra_points: &[Point2],
    base: Option<Point2>,
) -> Option<SnapResult> {
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let r2 = radius * radius;
    let mut best = Best::new(kinds);

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
            // Text はアンカー点のみをスナップ源にする（端点扱い。DESIGN.md M6 設計判断1）。
            EntityGeom::Text(text) => best.consider(SnapKind::Endpoint, text.anchor, cursor, r2),
            // 寸法はスナップ源にしない（DESIGN.md M6 設計判断1）。**表も同じく
            // スナップ源にしない**（M10 タスク58）: 罫線の交点・端点をスナップ源に
            // すると表 1 つで数十〜数百の候補点が増え、近傍探索が実質役に立たなく
            // なるうえ、表は寸法を測る対象ではなく注記だから。表の位置合わせは
            // 「表題欄の上に配置」ボタン（M10 タスク60）が担う。`EntityGeom` は
            // `#[non_exhaustive]` なので、未知の幾何も同じくスナップ源にしない。
            _ => {}
        }
    }

    // 交点: カーソル近傍 AABB（カーソル±半径のボックス）と交差するエンティティのみを
    // 事前絞り込みし、その集合内の全ペアで交点を求める。絞り込みが無損失である理由は
    // モジュール doc を参照。交点・近接点がともに無効なら集合自体を作らず、交点が無効なら
    // O(k^2) の総当たりを走らせない（`Best::consider` で捨てるだけだとマウス移動ごとの
    // 計算が残る。M12 タスク85 Codex adversarial review 指摘）。
    let cursor_box = Aabb::from_point(cursor).expanded(radius);
    let wants_perp_or_tangent = base.is_some() && (kinds.perpendicular || kinds.tangent);
    let near: Vec<&Shape> = if kinds.intersection || kinds.nearest || wants_perp_or_tangent {
        document
            .entities()
            .filter(|(_, e)| layer_visible(document, e))
            .filter(|(_, e)| e.geom.aabb().intersects(&cursor_box))
            // M6: 交点計算は Shape 系のみを対象にする（寸法・テキストは交点源にしない）。
            .filter_map(|(_, e)| e.geom.as_shape())
            .collect()
    } else {
        Vec::new()
    };
    if kinds.intersection {
        for i in 0..near.len() {
            for j in (i + 1)..near.len() {
                for p in intersect(near[i], near[j]) {
                    best.consider(SnapKind::Intersection, p, cursor, r2);
                }
            }
        }
    }

    // 垂線・接線: 基点が要る（M12 タスク86、DESIGN.md M12 設計判断5）。基点が無い、
    // または種別が両方とも無効なら `near` 自体が空（構築を省いている）なのでこの
    // ループは何もしない。基点があるときだけ `near` 上の Line/Polyline（垂線）・
    // Circle/Arc（接線）を走査する。
    if let Some(origin) = base {
        for shape in &near {
            if kinds.perpendicular {
                match shape {
                    Shape::Line(s) => {
                        consider_perpendicular_foot(s, origin, cursor, r2, &mut best);
                    }
                    Shape::Polyline(pl) => {
                        for seg in pl.segments() {
                            consider_perpendicular_foot(&seg, origin, cursor, r2, &mut best);
                        }
                    }
                    _ => {}
                }
            }
            if kinds.tangent {
                match shape {
                    Shape::Circle(c) => {
                        if let Some(pts) = c.tangent_points(origin) {
                            for p in pts {
                                best.consider(SnapKind::Tangent, p, cursor, r2);
                            }
                        }
                    }
                    Shape::Arc(a) => {
                        let circle = a.circle();
                        if let Some(pts) = circle.tangent_points(origin) {
                            for p in pts {
                                let theta = (p.y - circle.center.y).atan2(p.x - circle.center.x);
                                if a.contains_angle(theta) {
                                    best.consider(SnapKind::Tangent, p, cursor, r2);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // 仮想交点: 交点用の `near`（カーソル近傍 AABB で絞り込んだ Shape 集合）は延長線上の
    // 交点を取りこぼすため使えない。専用の絞り込み（無限直線・円とカーソルの距離。
    // 無損失である理由はモジュール doc 参照）で線要素・円要素を集め、組（線×線・線×円）を
    // 総当たりする。種別が無効なら走査自体を行わない。
    if kinds.virtual_intersection {
        collect_virtual_intersections(document, cursor, radius, &mut best, r2);
    }

    // 近接点: 既定 OFF（モジュール doc）なので、無効なら計算自体を省く。有効なら
    // 交点と同じ `near`（カーソル近傍 AABB で絞り込んだ Shape 集合。無損失である
    // 理由はモジュール doc 参照）上でカーソルに最も近い点を候補にする。Point は
    // 端点が既に拾うので対象外にする（DESIGN.md M12 設計判断5 の対象形状: Line・
    // Circle・Arc・Polyline）。
    if kinds.nearest {
        for shape in &near {
            if matches!(shape, Shape::Point(_)) {
                continue;
            }
            let p = closest_point(shape, cursor);
            best.consider(SnapKind::Nearest, p, cursor, r2);
        }
    }

    // グリッド: カーソルに最も近いグリッド交点。最低優先度なので他候補が無いときの
    // フォールバックになる。矩形は直交丸め、等測は斜交座標の丸め（[`iso_grid_snap`]）。
    if grid.step.is_finite() && grid.step > 0.0 {
        let candidate = match grid.mode {
            GridMode::Rectangular => {
                let gx = (cursor.x / grid.step).round() * grid.step;
                let gy = (cursor.y / grid.step).round() * grid.step;
                Some(Point2::new(gx, gy))
            }
            GridMode::Isometric => iso_grid_snap(cursor, grid.step),
        };
        if let Some(p) = candidate {
            best.consider(SnapKind::Grid, p, cursor, r2);
        }
    }

    best.finish()
}

/// 分割ツール専用のスナップ候補列挙（DESIGN.md 7章「分割ツールのスナップ対応」論点(1)）。
///
/// 図面全体を対象にする [`snap`] と異なり、候補を **対象エンティティ `target` 上に
/// 限定** する。対象上に無い点（別エンティティの端点・[`SnapKind::Center`]・
/// [`SnapKind::Grid`]）へ吸着すると、マーカー位置と `mcad_geom::split` が実際に使う
/// 分割位置が乖離してマーカーが嘘をつくため、この2種別は候補にしない
/// （DESIGN.md 同節・論点(1)）。
///
/// 候補は以下の3種類のみ:
/// - [`SnapKind::Midpoint`]: [`Shape::Line`] の中点・[`Shape::Polyline`] 各辺の中点。
/// - [`SnapKind::Endpoint`]: [`Shape::Polyline`] の **中間頂点のみ**。先頭・末尾頂点、
///   および [`Shape::Line`]／[`Shape::Arc`] の両端点は除外する。`mcad_geom::split` は
///   これらを常に `SplitError::TooCloseToEndpoint` で拒否するため、マーカーを出すと
///   「表示→クリック→拒否」の無意味な誘導になる（中間頂点での分割は頂点重複の無い
///   正当な操作なので候補にする）。
/// - [`SnapKind::Intersection`]: `target`（`target_id`）と他の可視 [`Shape`] エンティティ
///   （`target_id` 自身は除く）との交点。[`snap`] と同じカーソル近傍 AABB カリングを
///   踏襲する（絞り込みが無損失である理由は本モジュール doc を参照）。**target の全体端点
///   （Line/Arc の両端、開いた Polyline の先頭・末尾頂点）の近傍にある交点は除外する**
///   （端点候補と同じ理由 — `split` がそこを常に `TooCloseToEndpoint` で拒否するため）。
///
/// `target` が `mcad_geom::split` の `SplitError::Unsupported` を返す形状
/// （[`Shape::Circle`]・閉じた [`Shape::Polyline`]・[`Shape::Point`]）の場合は候補列挙
/// 自体を行わず `None` を返す（拒否確定の対象にマーカーを出さない）。
///
/// `kinds`（種別ごとの ON/OFF）は候補種別 Midpoint/Endpoint/Intersection にそのまま
/// 従う（M12 タスク85）。四半円点・近接点は分割スナップには足さない
/// （DESIGN.md M12 設計判断5: 分割スナップは対象上の点に限定する既存の設計を保つ）。
// 「分割スナップ-2」で `SplitTool`（`Tool::snaps_shape_pick`）/`handle_tool_input`
// （main.rs）へ配線済み（DESIGN.md 7章「分割ツールのスナップ対応」タスク分割）。
#[must_use]
pub fn snap_split_position(
    document: &Document,
    target_id: EntityId,
    target: &Shape,
    cursor: Point2,
    radius: f64,
    kinds: SnapKinds,
) -> Option<SnapResult> {
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    // split が Unsupported を返す対象は候補列挙自体を行わない。
    if matches!(target, Shape::Point(_) | Shape::Circle(_))
        || matches!(target, Shape::Polyline(pl) if pl.closed)
    {
        return None;
    }

    let r2 = radius * radius;
    let mut best = Best::new(kinds);

    match target {
        Shape::Line(s) => {
            best.consider(SnapKind::Midpoint, s.a.midpoint(s.b), cursor, r2);
        }
        Shape::Polyline(pl) => {
            // 中間頂点のみ（先頭・末尾は split が TooCloseToEndpoint で拒否するため除外）。
            if pl.vertices.len() > 2 {
                for &v in &pl.vertices[1..pl.vertices.len() - 1] {
                    best.consider(SnapKind::Endpoint, v, cursor, r2);
                }
            }
            for seg in pl.segments() {
                best.consider(SnapKind::Midpoint, seg.a.midpoint(seg.b), cursor, r2);
            }
        }
        // Arc は端点(始終点)が除外対象なので中点・端点候補を持たない（交点のみ）。
        // Point・閉じた Polyline・Circle はここへ来ない（上の Unsupported ガードで弾く）。
        Shape::Arc(_) | Shape::Point(_) | Shape::Circle(_) => {}
    }

    // target の全体端点（Line/Arc は両端、開いた Polyline は先頭・末尾頂点）。
    // split はこれらの近傍を必ず TooCloseToEndpoint で拒否するため、他エンティティとの
    // 交点がたまたまここに載っても候補にしない（中点・端点候補と同じ除外規約。
    // Codex レビュー指摘: 除外していないと「マーカー→クリック→拒否」が交点経由で再発する）。
    let overall_endpoints: &[Point2] = match target {
        Shape::Line(s) => &[s.a, s.b],
        Shape::Arc(a) => &[a.start_point(), a.end_point()],
        Shape::Polyline(pl) if pl.vertices.len() >= 2 => {
            &[pl.vertices[0], pl.vertices[pl.vertices.len() - 1]]
        }
        _ => &[],
    };
    let near_overall_endpoint = |p: Point2| {
        overall_endpoints
            .iter()
            .any(|&e| p.distance(e) <= point_tol(p, e))
    };

    // 交点: target とカーソル近傍 AABB が交差する他エンティティのみを事前絞り込みし、
    // target_id 自身は除外する。絞り込みが無損失である理由は本モジュール doc を参照。
    let cursor_box = Aabb::from_point(cursor).expanded(radius);
    if kinds.intersection && target.aabb().intersects(&cursor_box) {
        for (id, entity) in document.entities() {
            if id == target_id || !layer_visible(document, entity) {
                continue;
            }
            let Some(other) = entity.geom.as_shape() else {
                continue;
            };
            if !other.aabb().intersects(&cursor_box) {
                continue;
            }
            for p in intersect(target, other) {
                if near_overall_endpoint(p) {
                    continue;
                }
                best.consider(SnapKind::Intersection, p, cursor, r2);
            }
        }
    }

    best.finish()
}

/// 1 エンティティの端点・中点・中心・四半円点の候補を [`Best`] に投入する。
///
/// 各種別の割り当て:
/// - 点 [`Shape::Point`] は点そのものを端点として扱う。
/// - 線分 [`Shape::Line`] は両端が端点、中点が中点。
/// - 円 [`Shape::Circle`] は中心に加え、[`mcad_geom::Circle::quadrant_points`] の
///   ワールド軸方向 4 点が四半円点（端点・中点は持たない）。
/// - 円弧 [`Shape::Arc`] は始点・終点が端点、中心が中心。四半円点は 4 候補角
///   （[`QUADRANT_ANGLES`]）のうち [`mcad_geom::Arc::contains_angle`] が弧の範囲内と
///   判定したものだけ（弧の中点は候補にしない）。
/// - ポリライン [`Shape::Polyline`] は各頂点が端点、各辺（閉じている場合は閉じ辺も）の中点が中点。
fn enumerate_features(shape: &Shape, cursor: Point2, r2: f64, best: &mut Best) {
    match shape {
        Shape::Point(p) => best.consider(SnapKind::Endpoint, *p, cursor, r2),
        Shape::Line(s) => {
            best.consider(SnapKind::Endpoint, s.a, cursor, r2);
            best.consider(SnapKind::Endpoint, s.b, cursor, r2);
            best.consider(SnapKind::Midpoint, s.a.midpoint(s.b), cursor, r2);
        }
        Shape::Circle(c) => {
            best.consider(SnapKind::Center, c.center, cursor, r2);
            for p in c.quadrant_points() {
                best.consider(SnapKind::Quadrant, p, cursor, r2);
            }
        }
        Shape::Arc(a) => {
            best.consider(SnapKind::Endpoint, a.start_point(), cursor, r2);
            best.consider(SnapKind::Endpoint, a.end_point(), cursor, r2);
            best.consider(SnapKind::Center, a.center, cursor, r2);
            let circle = a.circle();
            for &theta in &QUADRANT_ANGLES {
                if a.contains_angle(theta) {
                    best.consider(SnapKind::Quadrant, circle.point_at_angle(theta), cursor, r2);
                }
            }
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

/// 基点 `origin` から線分 `seg` への垂線の足を候補に投入する（M12 タスク86）。
///
/// 足が線分上に無ければ（延長上）[`LineSeg::foot_of_perpendicular`] が `None` を
/// 返すので何もしない。足が `origin` そのものと一致する（距離が [`point_tol`] 未満）
/// 場合も投入しない: `origin` は基点であり、そこへ「基点自身への垂線」を候補にすると
/// 長さ 0 の線になって意味がないため（DESIGN.md M12 設計判断5 の実装時追記）。
fn consider_perpendicular_foot(
    seg: &mcad_geom::LineSeg,
    origin: Point2,
    cursor: Point2,
    r2: f64,
    best: &mut Best,
) {
    if let Some(foot) = seg.foot_of_perpendicular(origin)
        && foot.distance(origin) >= point_tol(foot, origin)
    {
        best.consider(SnapKind::Perpendicular, foot, cursor, r2);
    }
}

/// エンティティの所属レイヤーが可視か（非表示レイヤーはスナップ対象外。
/// `SelectTool` のヒットテストが非表示レイヤーを除外するのと同じ方針）。
fn layer_visible(document: &Document, entity: &Entity) -> bool {
    document.layer(entity.layer).is_some_and(|l| l.visible)
}

/// 仮想交点（[`SnapKind::VirtualIntersection`]、M12 タスク87）用の円要素。
/// 完全な円は `arc` が `None`、円弧は `Some`（掃引範囲の判定に使う）。
struct CircleCandidate {
    circle: Circle,
    arc: Option<Arc>,
}

/// 点 `p` から無限直線 `seg`（`LineSeg` を通る直線とみなす）への距離。
/// `seg` が退化（長さが [`point_tol`] 以下）していれば `f64::INFINITY`
/// （距離が定まらないので絶対に絞り込みへ入らないようにする）。
fn distance_to_infinite_line(seg: &LineSeg, p: Point2) -> f64 {
    let d = seg.direction();
    let len = d.length();
    if len <= point_tol(seg.a, seg.b) {
        return f64::INFINITY;
    }
    (p - seg.a).cross(d).abs() / len
}

/// 線分 `seg` 上での点 `p` の無次元パラメータ `t`（`p` は `seg` の乗る直線上にある前提）。
/// `t ∈ [0, 1]` なら線分の区間内（境界は [`crate::EPS`] 込み）。
fn line_param(seg: &LineSeg, p: Point2) -> f64 {
    let d = seg.direction();
    (p - seg.a).dot(d) / d.length_squared()
}

/// `line_param` が返す `t` が線分の区間内（境界含む）か。
fn t_on_segment(t: f64) -> bool {
    (-mcad_geom::EPS..=1.0 + mcad_geom::EPS).contains(&t)
}

/// 仮想交点（M12 タスク87）の候補列挙で実際に計算する「線×線」の組の数の予算。
/// 平行なグループどうしは組にしない絞り込み（[`LineGroup::angle_cluster`]）のあとも、
/// 非平行な直線が数十本以上カーソル近傍を通る病的な図面では組合せが爆発しうるため、
/// 線グループの**本数**ではなく実際に**計算する組の数**に上限を設ける（Codex
/// adversarial review [medium] 再指摘、2026-09-26。モジュール doc「同一直線上の線要素の
/// グループ化と上限」・DESIGN.md タスク87の実装時追記「既知の制約」参照）。カーソルに
/// 近いものから順に組を処理するため、打ち切りが起きても近傍の組は先に調べ終えている。
///
/// **[`VIRTUAL_INTERSECTION_MAX_LINE_CIRCLE_PAIRS`] とは独立の予算**（Codex
/// adversarial review [medium] 再々指摘、2026-09-26）: 単一の共有予算だと、線×線を
/// 先に処理する実装順序の都合で、非平行な線グループが予算本数近くカーソル近傍にあると
/// 線×線だけで予算を使い切り、線×円の組が1つも調べられなくなる（有効な線×円の仮想
/// 交点が消える）。
const VIRTUAL_INTERSECTION_MAX_LINE_PAIRS: usize = 4096;
/// 仮想交点（M12 タスク87）の候補列挙で実際に計算する「線×円」の組の数の予算。
/// 理由・打ち切りの性質は [`VIRTUAL_INTERSECTION_MAX_LINE_PAIRS`] と同じだが、
/// **独立したカウンタ**を持つ（同上）。
const VIRTUAL_INTERSECTION_MAX_LINE_CIRCLE_PAIRS: usize = 4096;

/// 仮想交点（M12 タスク87）の同一直線グループ（「支持直線」）。
///
/// `representative` は交点計算に使う無限直線（メンバーのうち最初の1本）。`members`
/// はグループに属する線要素そのもので、実体上の判定（[`group_contains_point`]）・
/// ガイドの最近端点探索（[`nearest_group_endpoint`]）で全メンバーを見る。`angle_cluster`
/// は [`group_collinear_lines`] の段階1（角度クラスタ化）で採番したクラスタ番号で、
/// 同じ番号どうし（＝平行なグループどうし）は [`collect_virtual_intersections`] が
/// 組を作らない（Codex adversarial review [medium]、2026-09-26。モジュール doc
/// 「同一直線上の線要素のグループ化と上限」参照）。
struct LineGroup {
    representative: LineSeg,
    members: Vec<LineSeg>,
    angle_cluster: usize,
}

/// 線要素 `lines` を、同一の無限直線とみなせるもの同士でグループ化する（純関数。
/// カーソル位置に依存しない。モジュール doc「同一直線上の線要素のグループ化と上限」
/// 参照）。
///
/// **2段階**でまとめる（一次レビュー後の Codex adversarial review [high] 再指摘への
/// 対応、2026-09-26）:
///
/// 1. **角度クラスタ化**: 各線分を「方向角（`y > 0`、または `y == 0` かつ `x > 0` に
///    なるよう正規化した向きの角度、範囲 `[0, π)`）」だけでソートし、グループの
///    最初のメンバー（代表）を基準に [`mcad_geom::EPS`] 以内なら同じ角度クラスタへ
///    まとめる（ラジアンは無次元なので直接比較する、`mcad-geom` の許容量ポリシー）。
///    **`0` と `π` の境界は同じ角度として扱う**: 回転・鏡映を経た図形は `y` 成分の
///    符号がわずかな浮動小数の揺れ（例: `1e-17`）で反転しうるため、正規化後の角度が
///    `0` 付近と `π` 付近に分かれてしまうことがある（同一直線上の線分どうしでも）。
///    ソート後の最初のクラスタ（`0` 付近）と最後のクラスタ（`π` 付近）の角度差が
///    `EPS` 以内なら、この2クラスタを1つに統合する。
/// 2. **距離クラスタ化**: 各角度クラスタの中で、原点からの符号付き距離でソートし、
///    やはり代表基準で [`mcad_geom::rel_tol`]（[`mcad_geom::line_line_intersection`]
///    の平行判定〔`EPS * |r| * |s|`〕と矛盾しない、座標スケールに対する相対許容量）
///    以内なら同じ距離クラスタ（＝最終的な [`LineGroup`]）へまとめる。**距離は
///    角度クラスタの代表（最初のメンバー）の法線を使って全メンバー分を計算し直す**
///    （メンバーごとに個別計算した法線のわずかな違いに左右されないようにするため。
///    段階1で `0`/`π` 境界をまたいで統合したクラスタは、メンバーによって正規化後の
///    向きが逆転しているため、これをしないと距離の符号がメンバーごとにばらつく）。
///
/// どちらの段階も比較は常にクラスタの最初のメンバー（代表）を基準にする（直前の
/// 要素との連鎖比較だと、閾値以下の差が積み重なって実際には閾値を超える2線分が
/// 同じクラスタへ入りかねない）。
fn group_collinear_lines(lines: &[LineSeg]) -> Vec<LineGroup> {
    /// 正規化した向き（`y > 0`、または `y == 0` かつ `x > 0`）とその角度。
    fn canonical_direction_and_angle(seg: &LineSeg) -> (Vec2, f64) {
        let d = seg.direction();
        let canonical = if d.y > 0.0 || (d.y == 0.0 && d.x > 0.0) {
            d
        } else {
            Vec2::new(-d.x, -d.y)
        };
        (canonical, canonical.angle())
    }

    // 段階1: 角度だけでソートしてクラスタ化。
    struct AngleKeyed {
        seg: LineSeg,
        angle: f64,
    }
    let mut angle_keyed: Vec<AngleKeyed> = lines
        .iter()
        .map(|&seg| {
            let (_, angle) = canonical_direction_and_angle(&seg);
            AngleKeyed { seg, angle }
        })
        .collect();
    angle_keyed.sort_by(|a, b| {
        a.angle
            .partial_cmp(&b.angle)
            .expect("角度は非有限にならない（degenerate は呼び出し側で除外済み）")
    });

    struct AngleCluster {
        repr_angle: f64,
        members: Vec<LineSeg>,
    }
    let mut angle_clusters: Vec<AngleCluster> = Vec::new();
    for k in angle_keyed {
        if let Some(last) = angle_clusters.last_mut()
            && (k.angle - last.repr_angle).abs() <= mcad_geom::EPS
        {
            last.members.push(k.seg);
            continue;
        }
        angle_clusters.push(AngleCluster {
            repr_angle: k.angle,
            members: vec![k.seg],
        });
    }

    // `0` と `π` の境界: 最初（`0` 付近）と最後（`π` 付近）のクラスタが実質同じ角度
    // （`π` だけ向きがずれているだけ）なら1つに統合する。統合後の代表（先頭）は常に
    // 「`0` 付近」クラスタの先頭メンバーのままなので、段階2で使う法線は向きの揺れに
    // 左右されない。
    if angle_clusters.len() >= 2 {
        let first_angle = angle_clusters[0].repr_angle;
        let last_angle = angle_clusters[angle_clusters.len() - 1].repr_angle;
        if (PI - last_angle) + first_angle <= mcad_geom::EPS {
            let wrapped = angle_clusters.pop().expect("len >= 2 を確認済み");
            angle_clusters[0].members.extend(wrapped.members);
        }
    }

    // 段階2: 各角度クラスタの中で距離クラスタ化（クラスタ代表の法線で距離を再計算）。
    // 角度クラスタ番号（`angle_cluster`）はこの段階の添字でそのまま採番する
    // （0/π 境界統合後の最終的な角度クラスタ一覧に対する番号）。
    let mut groups: Vec<LineGroup> = Vec::new();
    for (angle_cluster, cluster) in angle_clusters.into_iter().enumerate() {
        let (repr_dir, _) = canonical_direction_and_angle(&cluster.members[0]);
        let normal = (repr_dir / repr_dir.length()).perp();

        struct DistKeyed {
            seg: LineSeg,
            dist: f64,
        }
        let mut dist_keyed: Vec<DistKeyed> = cluster
            .members
            .iter()
            .map(|&seg| DistKeyed {
                seg,
                dist: seg.a.to_vec2().dot(normal),
            })
            .collect();
        dist_keyed.sort_by(|a, b| a.dist.partial_cmp(&b.dist).expect("距離は非有限にならない"));

        let mut local: Vec<(f64, LineGroup)> = Vec::new();
        for k in dist_keyed {
            if let Some((ref_dist, group)) = local.last_mut() {
                let dist_close = (k.dist - *ref_dist).abs()
                    <= mcad_geom::rel_tol(ref_dist.abs().max(k.dist.abs()));
                if dist_close {
                    group.members.push(k.seg);
                    continue;
                }
            }
            local.push((
                k.dist,
                LineGroup {
                    representative: k.seg,
                    members: vec![k.seg],
                    angle_cluster,
                },
            ));
        }
        groups.extend(local.into_iter().map(|(_, g)| g));
    }

    groups
}

/// グループ `group` のいずれかのメンバー線分の区間内（[`t_on_segment`]、`p` は
/// グループの無限直線上にある前提）に点 `p` があるか。1本でも区間内なら実体上とみなす
/// （同一直線上の他の線分が延長側でも、このグループ全体としては「実体上」になる）。
fn group_contains_point(group: &LineGroup, p: Point2) -> bool {
    group
        .members
        .iter()
        .any(|seg| t_on_segment(line_param(seg, p)))
}

/// グループ `group` の全メンバーの端点のうち、`p`（仮想交点）に最も近いもの。
/// ガイド（[`SnapGuide::Segment`]）の `from` に使う。
fn nearest_group_endpoint(group: &LineGroup, p: Point2) -> Point2 {
    group
        .members
        .iter()
        .flat_map(|seg| [seg.a, seg.b])
        .min_by(|x, y| {
            p.distance_squared(*x)
                .partial_cmp(&p.distance_squared(*y))
                .expect("距離の2乗は非有限にならない")
        })
        .expect("group_collinear_lines は空グループを作らない")
}

/// `[0, TAU)` へ正規化する（`Arc::contains_angle` 等と同じ規約）。
fn wrap_2pi(theta: f64) -> f64 {
    let r = theta % TAU;
    if r < 0.0 { r + TAU } else { r }
}

/// 円弧 `a` の掃引範囲外にある角度 `theta` の点への [`SnapGuide::Arc`] を作る。
/// 近い方の弧端点（始点 or 終点）から `theta` まで、掃引範囲の無い側（外側）を
/// 回る向きに符号付き `sweep` で表す（DESIGN.md M12 設計判断5 の実装時追記）。
fn arc_guide_for(a: &Arc, theta: f64) -> SnapGuide {
    let dist_from_end = wrap_2pi(theta - a.end_angle);
    let dist_from_start = wrap_2pi(a.start_angle - theta);
    if dist_from_end <= dist_from_start {
        SnapGuide::Arc {
            center: a.center,
            radius: a.radius,
            start_angle: a.end_angle,
            sweep: dist_from_end,
        }
    } else {
        SnapGuide::Arc {
            center: a.center,
            radius: a.radius,
            start_angle: a.start_angle,
            sweep: -dist_from_start,
        }
    }
}

/// 仮想交点（[`SnapKind::VirtualIntersection`]、M12 タスク87）の候補を列挙する。
///
/// 絞り込みが無損失である理由はモジュール doc（「仮想交点の絞り込み」節）を参照。
/// 組は (線要素, 線要素)・(線要素, 円要素) のみ（円要素どうしは設計判断5の対象外）。
/// 真の交点（両実体上）は [`SnapKind::Intersection`] の担当なので候補にしない。
fn collect_virtual_intersections(
    document: &Document,
    cursor: Point2,
    radius: f64,
    best: &mut Best,
    r2: f64,
) {
    let mut lines: Vec<LineSeg> = Vec::new();
    // (|カーソル-中心間距離 − 半径|, 候補) の組で集め、後で上限まで切り詰める。
    let mut circles: Vec<(f64, CircleCandidate)> = Vec::new();

    for (_, entity) in document.entities() {
        if !layer_visible(document, entity) {
            continue;
        }
        let EntityGeom::Shape(shape) = &entity.geom else {
            continue;
        };
        match shape {
            Shape::Line(s) => {
                if distance_to_infinite_line(s, cursor) <= radius {
                    lines.push(*s);
                }
            }
            Shape::Polyline(pl) => {
                for seg in pl.segments() {
                    if distance_to_infinite_line(&seg, cursor) <= radius {
                        lines.push(seg);
                    }
                }
            }
            Shape::Circle(c) => {
                let d = (cursor.distance(c.center) - c.radius).abs();
                if d <= radius {
                    circles.push((
                        d,
                        CircleCandidate {
                            circle: *c,
                            arc: None,
                        },
                    ));
                }
            }
            Shape::Arc(a) => {
                let circle = a.circle();
                let d = (cursor.distance(circle.center) - circle.radius).abs();
                if d <= radius {
                    circles.push((
                        d,
                        CircleCandidate {
                            circle,
                            arc: Some(*a),
                        },
                    ));
                }
            }
            Shape::Point(_) => {}
        }
    }

    // 同一直線上の線要素を1本の支持直線へまとめ、O(本数^2) の爆発を避ける
    // （Codex adversarial review [high]、2026-09-26。モジュール doc参照）。
    // 線グループ・円要素とも本数そのものは切り詰めず、カーソルに近い順に並べておく
    // （近い順に組を処理するため。Codex adversarial review [medium] 再指摘、
    // 2026-09-26: 本数で切り詰めると「平行線ばかりが近くにあり、少し遠い交差相手が
    // 切り捨てられる」ケースで仮想交点が消えるため、本数の上限はやめた）。
    let mut groups = group_collinear_lines(&lines);
    groups.sort_by(|a, b| {
        let da = distance_to_infinite_line(&a.representative, cursor);
        let db = distance_to_infinite_line(&b.representative, cursor);
        da.partial_cmp(&db)
            .expect("距離は非有限にならない（near 収集条件で保証済み）")
    });

    circles.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .expect("距離は非有限にならない（near 収集条件で保証済み）")
    });
    let circles: Vec<CircleCandidate> = circles.into_iter().map(|(_, c)| c).collect();

    // 実際に計算する組の数を、線×線・線×円それぞれ独立の予算で管理する
    // （[`VIRTUAL_INTERSECTION_MAX_LINE_PAIRS`]・[`VIRTUAL_INTERSECTION_MAX_LINE_CIRCLE_PAIRS`]。
    // 共有の単一予算だと線×線を先に処理する都合上、非平行な線グループが予算本数近く
    // カーソル近傍にあるだけで線×円が1組も調べられなくなる。Codex adversarial review
    // [medium] 再々指摘、2026-09-26）。
    let mut line_pairs_computed: usize = 0;

    // 線 × 線（グループの代表直線どうし）。距離順に並んだ添字 i の昇順、各 i に対し
    // j < i の昇順で組を作る（先に処理される組ほど両方がカーソルに近い）。
    // 同じ角度クラスタ（平行なグループ）どうしは組にしない（ハッチングのような平行線が
    // いくら多くても組を増やさない）。予算に達したら打ち切る。
    'line_pairs: for i in 0..groups.len() {
        for j in 0..i {
            if groups[i].angle_cluster == groups[j].angle_cluster {
                continue; // 平行（同一角度クラスタ）は組にしない。
            }
            if line_pairs_computed >= VIRTUAL_INTERSECTION_MAX_LINE_PAIRS {
                break 'line_pairs;
            }
            line_pairs_computed += 1;

            // ガイド配列の並びは添字の昇順（`j < i`）に揃える（走査順序自体は
            // カーソルに近い方を先に処理するため i の昇順・j<i だが、結果の
            // `guides` の意味は「小さい添字側→大きい添字側」で一貫させる）。
            let Some(p) =
                line_line_intersection(&groups[j].representative, &groups[i].representative)
            else {
                continue;
            };
            let on_lo = group_contains_point(&groups[j], p);
            let on_hi = group_contains_point(&groups[i], p);
            if on_lo && on_hi {
                continue; // 真の交点（両グループのいずれかのメンバー上）: Intersection の担当。
            }
            let g_lo = (!on_lo).then(|| SnapGuide::Segment {
                from: nearest_group_endpoint(&groups[j], p),
                to: p,
            });
            let g_hi = (!on_hi).then(|| SnapGuide::Segment {
                from: nearest_group_endpoint(&groups[i], p),
                to: p,
            });
            best.consider_guided(SnapKind::VirtualIntersection, p, [g_lo, g_hi], cursor, r2);
        }
    }

    // 線 × 円（グループの代表直線 × 円）。円は距離順、各円に対し線も距離順に処理する
    // （どちらも近いものから優先）。**線×線とは別の独立した予算**を持つ（線×線の
    // 消費状況に関わらず、線×円の組は必ず一定数まで調べられる）。
    let mut circle_pairs_computed: usize = 0;
    'circle_pairs: for cand in &circles {
        for group in &groups {
            if circle_pairs_computed >= VIRTUAL_INTERSECTION_MAX_LINE_CIRCLE_PAIRS {
                break 'circle_pairs;
            }
            circle_pairs_computed += 1;

            for p in line_circle_intersection(&group.representative, &cand.circle) {
                let on_line = group_contains_point(group, p);
                let theta = (p - cand.circle.center).angle();
                let on_circle = cand.arc.is_none_or(|a| a.contains_angle(theta));
                if on_line && on_circle {
                    continue; // 真の交点: Intersection の担当。
                }
                let line_guide = (!on_line).then(|| SnapGuide::Segment {
                    from: nearest_group_endpoint(group, p),
                    to: p,
                });
                let circle_guide = if on_circle {
                    None
                } else {
                    cand.arc.as_ref().map(|a| arc_guide_for(a, theta))
                };
                best.consider_guided(
                    SnapKind::VirtualIntersection,
                    p,
                    [line_guide, circle_guide],
                    cursor,
                    r2,
                );
            }
        }
    }
}

/// これまでに見た候補のうち「最良」を保持するアキュムレータ。
///
/// 最良の定義（DESIGN.md M12 設計判断5 の優先順位）:
/// 1. 優先度（[`SnapKind::priority`]）が高い（数値が小さい）ものを優先。
/// 2. 優先度が同じなら、カーソルとの距離（の 2 乗）が小さいものを優先。
///
/// 半径外（`d2 > r2`）の候補は無視する。無効な種別（[`SnapKinds::enabled`] が
/// `false`）の候補も同様に無視する（M12 タスク85）。
struct Best {
    /// 種別ごとの ON/OFF。無効な種別は `consider` が弾く。
    kinds: SnapKinds,
    /// `(優先度, カーソルとの距離^2, 結果)`。未確定なら `None`。
    found: Option<(u8, f64, SnapResult)>,
}

impl Best {
    /// `kinds` を種別フィルタに使う空のアキュムレータを作る。
    fn new(kinds: SnapKinds) -> Self {
        Self { kinds, found: None }
    }

    /// 候補 1 点を検討する。有効な種別かつ半径内かつ現状より良ければ採用する。
    /// ガイドを持たない種別用（[`SnapResult::guides`] は `[None, None]`）。
    fn consider(&mut self, kind: SnapKind, point: Point2, cursor: Point2, r2: f64) {
        self.consider_guided(kind, point, [None, None], cursor, r2);
    }

    /// [`consider`](Self::consider) のガイド指定版（仮想交点用、M12 タスク87）。
    fn consider_guided(
        &mut self,
        kind: SnapKind,
        point: Point2,
        guides: [Option<SnapGuide>; 2],
        cursor: Point2,
        r2: f64,
    ) {
        if !self.kinds.enabled(kind) {
            return;
        }
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
            self.found = Some((
                rank,
                d2,
                SnapResult {
                    kind,
                    point,
                    guides,
                },
            ));
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
    use mcad_geom::{Circle, LineSeg, Polyline, Shape};

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

    /// カレントレイヤーに `shape` を追加し、発行された [`EntityId`] を返す
    /// （分割スナップのテストで target_id が要る）。
    fn add_id(doc: &mut Document, shape: Shape) -> EntityId {
        let layer = doc.current_layer();
        let new_ids = doc
            .apply(Command::AddEntity(Entity::new(
                shape,
                layer,
                Style::inherited(),
            )))
            .expect("current layer must accept entity");
        new_ids.entities[0]
    }

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Shape {
        Shape::Line(LineSeg::new(Point2::new(ax, ay), Point2::new(bx, by)))
    }

    /// グリッド無効（`grid_step = 0`）・`extra_points` 無し・基点無し・全種別 ON で
    /// スナップする短縮版。
    fn snap_no_grid(doc: &Document, cursor: Point2, radius: f64) -> Option<SnapResult> {
        snap_no_grid_with_kinds(doc, cursor, radius, SnapKinds::default())
    }

    /// [`snap_no_grid`] の種別指定版（種別 ON/OFF のテスト用）。基点は無し。
    fn snap_no_grid_with_kinds(
        doc: &Document,
        cursor: Point2,
        radius: f64,
        kinds: SnapKinds,
    ) -> Option<SnapResult> {
        snap_no_grid_with_kinds_and_base(doc, cursor, radius, kinds, None)
    }

    /// [`snap_no_grid_with_kinds`] の基点指定版（垂線・接線のテスト用、M12 タスク86）。
    fn snap_no_grid_with_kinds_and_base(
        doc: &Document,
        cursor: Point2,
        radius: f64,
        kinds: SnapKinds,
        base: Option<Point2>,
    ) -> Option<SnapResult> {
        snap(
            doc,
            cursor,
            radius,
            GridSpec::new(0.0, GridMode::Rectangular),
            kinds,
            &[],
            base,
        )
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
        let r = snap(
            &doc,
            Point2::new(0.1, 0.1),
            0.5,
            GridSpec::new(1.0, GridMode::Rectangular),
            SnapKinds::default(),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Center);
        assert_eq!(r.point, Point2::new(0.3, 0.3));
    }

    #[test]
    fn grid_is_fallback_when_nothing_else_in_range() {
        // エンティティのない空ドキュメントでもグリッドにはスナップする。
        let doc = Document::new();
        let r = snap(
            &doc,
            Point2::new(0.2, -0.1),
            0.5,
            GridSpec::new(1.0, GridMode::Rectangular),
            SnapKinds::default(),
            &[],
            None,
        )
        .unwrap();
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
        let r = snap(
            &doc,
            cursor,
            3.0,
            GridSpec::new(0.0, GridMode::Rectangular),
            SnapKinds::default(),
            &[],
            None,
        )
        .unwrap();
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
        assert_eq!(
            snap(
                &doc,
                Point2::new(0.0, 0.0),
                3.0,
                GridSpec::new(0.0, GridMode::Rectangular),
                SnapKinds::default(),
                &[],
                None
            ),
            None
        );
    }

    #[test]
    fn extra_point_within_radius_snaps_as_endpoint() {
        // Document は空でグリッドのみ候補になる状況で、extra_points（作図中ツールの
        // 未確定頂点）を渡すと、より優先度の高い端点として選ばれる。
        let doc = Document::new();
        let extra = [Point2::new(0.05, 0.0)];
        let r = snap(
            &doc,
            Point2::new(0.0, 0.0),
            0.5,
            GridSpec::new(1.0, GridMode::Rectangular),
            SnapKinds::default(),
            &extra,
            None,
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(0.05, 0.0));
    }

    #[test]
    fn text_anchor_snaps_as_endpoint() {
        use mcad_core::TextGeom;
        // Text エンティティのアンカーだけが端点候補になる（DESIGN.md M6 設計判断1）。
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
        let r = snap(
            &doc,
            Point2::new(0.1, -0.1),
            0.5,
            GridSpec::new(1.0, GridMode::Rectangular),
            SnapKinds::default(),
            &extra,
            None,
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Grid);
        assert_eq!(r.point, Point2::new(0.0, 0.0));
    }

    // --- snap_split_position (分割ツール専用スナップ) ---

    #[test]
    fn split_line_midpoint_is_candidate() {
        let mut doc = Document::new();
        let shape = line(0.0, 0.0, 4.0, 0.0);
        let id = add_id(&mut doc, shape.clone());

        let r = snap_split_position(
            &doc,
            id,
            &shape,
            Point2::new(2.1, 0.0),
            0.5,
            SnapKinds::default(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Midpoint);
        assert_eq!(r.point, Point2::new(2.0, 0.0));
    }

    #[test]
    fn split_polyline_edge_midpoint_is_candidate() {
        let mut doc = Document::new();
        let shape = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(4.0, 0.0),
                Point2::new(4.0, 4.0),
            ],
            false,
        ));
        let id = add_id(&mut doc, shape.clone());

        // 2 本目の辺 (4,0)-(4,4) の中点 (4,2)。
        let r = snap_split_position(
            &doc,
            id,
            &shape,
            Point2::new(4.1, 2.0),
            0.5,
            SnapKinds::default(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Midpoint);
        assert_eq!(r.point, Point2::new(4.0, 2.0));
    }

    #[test]
    fn split_polyline_middle_vertex_is_candidate_and_beats_midpoint() {
        let mut doc = Document::new();
        // 中間頂点 (4,0) の近くには辺の中点は無い(端点 (2,0) と (4,2) の中点はそれぞれ遠い)。
        let shape = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(4.0, 0.0),
                Point2::new(4.0, 4.0),
            ],
            false,
        ));
        let id = add_id(&mut doc, shape.clone());

        let r = snap_split_position(
            &doc,
            id,
            &shape,
            Point2::new(4.1, 0.1),
            0.5,
            SnapKinds::default(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(4.0, 0.0));
    }

    #[test]
    fn split_line_x_intersection_is_candidate() {
        let mut doc = Document::new();
        let target = line(0.0, 0.0, 10.0, 0.0);
        let id = add_id(&mut doc, target.clone());
        add(&mut doc, line(5.0, -5.0, 5.0, 5.0));

        // 交点 (5,0)。target の中点も (5,0) だが、優先度で交点(=端点相当の高優先度)が勝つ。
        let r = snap_split_position(
            &doc,
            id,
            &target,
            Point2::new(5.1, 0.0),
            0.5,
            SnapKinds::default(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Intersection);
        assert_eq!(r.point, Point2::new(5.0, 0.0));
    }

    #[test]
    fn split_t_contact_intersection_is_candidate() {
        // T字接触: 他線分 B の端点 (5,0) が target A の内部に載る。
        let mut doc = Document::new();
        let target = line(0.0, 0.0, 10.0, 0.0);
        let id = add_id(&mut doc, target.clone());
        add(&mut doc, line(5.0, 0.0, 5.0, 5.0));

        let r = snap_split_position(
            &doc,
            id,
            &target,
            Point2::new(5.1, 0.0),
            0.5,
            SnapKinds::default(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Intersection);
        assert_eq!(r.point, Point2::new(5.0, 0.0));
    }

    #[test]
    fn split_intersection_at_target_overall_endpoint_is_not_a_candidate() {
        // 他線分が target の全体端点 (0,0) ちょうどを通る（T字接触が target の端点上で
        // 起きるケース）。この交点を候補にすると、split が必ず TooCloseToEndpoint で
        // 拒否する点にマーカーを出す「表示→クリック→拒否」が交点経由で再発する
        // （Codex レビュー指摘、2026-08-12）。
        let mut doc = Document::new();
        let target = line(0.0, 0.0, 10.0, 0.0);
        let id = add_id(&mut doc, target.clone());
        add(&mut doc, line(0.0, 0.0, 0.0, 5.0));

        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &target,
                Point2::new(0.1, 0.0),
                0.5,
                SnapKinds::default()
            ),
            None
        );
    }

    #[test]
    fn split_arc_intersection_at_overall_endpoint_is_not_a_candidate() {
        use mcad_geom::Arc;
        use std::f64::consts::FRAC_PI_2;

        // 他線分が弧の始点 (5,0) ちょうどを通る。
        let mut doc = Document::new();
        let arc = Arc::new(Point2::new(0.0, 0.0), 5.0, 0.0, FRAC_PI_2);
        let shape = Shape::Arc(arc);
        let id = add_id(&mut doc, shape.clone());
        add(&mut doc, line(5.0, 0.0, 5.0, 5.0));

        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &shape,
                Point2::new(4.9, 0.0),
                0.3,
                SnapKinds::default()
            ),
            None
        );
    }

    #[test]
    fn split_polyline_intersection_at_overall_endpoint_is_not_a_candidate() {
        // 他線分が Polyline の先頭頂点 (0,0) ちょうどを通る。
        let mut doc = Document::new();
        let shape = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(4.0, 0.0),
                Point2::new(4.0, 4.0),
            ],
            false,
        ));
        let id = add_id(&mut doc, shape.clone());
        add(&mut doc, line(0.0, 0.0, 0.0, 5.0));

        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &shape,
                Point2::new(0.1, 0.0),
                0.5,
                SnapKinds::default()
            ),
            None
        );
    }

    #[test]
    fn split_line_full_endpoints_are_not_candidates() {
        let mut doc = Document::new();
        let shape = line(0.0, 0.0, 4.0, 0.0);
        let id = add_id(&mut doc, shape.clone());

        // 端点 (0,0) の近傍。線分全体の端点は候補にならない。
        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &shape,
                Point2::new(0.1, 0.0),
                0.5,
                SnapKinds::default()
            ),
            None
        );
    }

    #[test]
    fn split_arc_full_endpoints_are_not_candidates() {
        use mcad_geom::Arc;
        use std::f64::consts::FRAC_PI_2;

        let mut doc = Document::new();
        let arc = Arc::new(Point2::new(0.0, 0.0), 5.0, 0.0, FRAC_PI_2);
        let shape = Shape::Arc(arc);
        let id = add_id(&mut doc, shape.clone());

        // 弧の始点 (5,0) 近傍には、端点・中点候補が無い(弧は候補を持たない)。
        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &shape,
                Point2::new(4.9, 0.0),
                0.3,
                SnapKinds::default()
            ),
            None
        );
    }

    #[test]
    fn split_center_is_not_a_candidate() {
        // Circle は target とは別エンティティで、target からも離れた位置にある
        // (交点も生じない)。円の中心近傍にカーソルを置いても、snap_split_position は
        // Center 種別を持たないため候補が無い(通常の snap() なら Center にスナップする
        // 状況)。グリッドは引数自体が無いため型シグネチャ上そもそも候補になり得ない。
        let mut doc = Document::new();
        let shape = line(0.0, 0.0, 4.0, 0.0);
        let id = add_id(&mut doc, shape.clone());
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(10.0, 10.0), 1.0)),
        );

        let r = snap_split_position(
            &doc,
            id,
            &shape,
            Point2::new(10.0, 10.0),
            0.5,
            SnapKinds::default(),
        );
        assert_eq!(r, None);
    }

    #[test]
    fn split_candidate_outside_radius_is_ignored() {
        let mut doc = Document::new();
        let shape = line(0.0, 0.0, 4.0, 0.0);
        let id = add_id(&mut doc, shape.clone());

        // 中点 (2,0) から半径 0.1 だけ離れた位置。半径 0.05 では届かない。
        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &shape,
                Point2::new(2.1, 0.0),
                0.05,
                SnapKinds::default()
            ),
            None
        );
    }

    #[test]
    fn split_hidden_layer_entity_is_not_intersection_source() {
        let mut doc = Document::new();
        let target = line(0.0, 0.0, 10.0, 0.0);
        let id = add_id(&mut doc, target.clone());

        // 交差する別レイヤーの線分を追加し、そのレイヤーを非表示にする。
        let hidden_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "hidden",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        doc.apply(Command::AddEntity(Entity::new(
            line(5.0, -5.0, 5.0, 5.0),
            hidden_layer,
            Style::inherited(),
        )))
        .unwrap();
        let mut props = doc.layer(hidden_layer).unwrap().clone();
        props.visible = false;
        doc.apply(Command::SetLayerProps {
            id: hidden_layer,
            props,
        })
        .unwrap();

        // 交点(5,0)は非表示レイヤー由来なので候補にならない。target の中点も(5,0)なので、
        // 交点として拾われていないことを確認するには半径をごく小さくして中点との
        // 距離差が出ない位置を選ぶ必要はなく、ここでは中点自体が候補として残る
        // (=Intersection でなく Midpoint になる、あるいは同一種別優先の中点が採用される)
        // ことを確認する。
        let r = snap_split_position(
            &doc,
            id,
            &target,
            Point2::new(5.0, 0.0),
            0.5,
            SnapKinds::default(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Midpoint);
        assert_eq!(r.point, Point2::new(5.0, 0.0));
    }

    #[test]
    fn split_target_itself_is_not_paired_for_intersection() {
        // target 自身との intersect() 呼び出しを避けていることを確認する。target_id と
        // 同じ id のエンティティがドキュメント内にあっても、それは除外される。
        let mut doc = Document::new();
        let target = line(0.0, 0.0, 10.0, 0.0);
        let id = add_id(&mut doc, target.clone());

        // 半径内には target 自身しかない(中点(5,0)のみが候補になるはず)。
        let r = snap_split_position(
            &doc,
            id,
            &target,
            Point2::new(5.0, 0.0),
            0.5,
            SnapKinds::default(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Midpoint);
        assert_eq!(r.point, Point2::new(5.0, 0.0));
    }

    #[test]
    fn split_unsupported_targets_return_none() {
        let mut doc = Document::new();

        let point_shape = Shape::Point(Point2::new(0.0, 0.0));
        let id = add_id(&mut doc, point_shape.clone());
        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &point_shape,
                Point2::new(0.0, 0.0),
                1.0,
                SnapKinds::default()
            ),
            None
        );

        let circle_shape = Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 1.0));
        let id = add_id(&mut doc, circle_shape.clone());
        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &circle_shape,
                Point2::new(1.0, 0.0),
                1.0,
                SnapKinds::default()
            ),
            None
        );

        let closed_polyline = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 0.0),
                Point2::new(1.0, 1.0),
            ],
            true,
        ));
        let id = add_id(&mut doc, closed_polyline.clone());
        assert_eq!(
            snap_split_position(
                &doc,
                id,
                &closed_polyline,
                Point2::new(0.5, 0.0),
                1.0,
                SnapKinds::default()
            ),
            None
        );
    }

    // --- iso_grid_snap（等測グリッドの格子点スナップ） ---

    #[test]
    fn iso_grid_snap_exact_lattice_point_is_unchanged() {
        // a=1, b=0 の格子点 u = (√3/2, 1/2)。
        let step = 1.0;
        let u = Point2::new(3f64.sqrt() / 2.0, 0.5);
        let snapped = iso_grid_snap(u, step).unwrap();
        assert!((snapped.x - u.x).abs() < 1e-9);
        assert!((snapped.y - u.y).abs() < 1e-9);
    }

    #[test]
    fn iso_grid_snap_rounds_midpoint_to_nearest_lattice_point() {
        // u と v のちょうど中間 ((u+v)/2 = (0, 0.5)) は a=0.5, b=0.5 で、
        // 四捨五入丸めにより a=1, b=0（あるいは a=0, b=1 のような近傍格子点）へ寄る。
        let step = 1.0;
        let u = Point2::new(3f64.sqrt() / 2.0, 0.5);
        let v = Point2::new(-3f64.sqrt() / 2.0, 0.5);
        let midpoint = Point2::new((u.x + v.x) / 2.0, (u.y + v.y) / 2.0);
        let snapped = iso_grid_snap(midpoint, step).unwrap();
        // 丸め先はいずれかの格子点（原点・u・v 等）であり、中間点そのものにはならない。
        let dist_to_midpoint =
            ((snapped.x - midpoint.x).powi(2) + (snapped.y - midpoint.y).powi(2)).sqrt();
        assert!(dist_to_midpoint > 1e-6);
    }

    #[test]
    fn iso_grid_snap_handles_negative_coordinates() {
        let step = 2.0;
        // a=-2, b=3 の格子点。
        let ux = step * 3f64.sqrt() / 2.0;
        let uy = step * 0.5;
        let vx = -ux;
        let vy = uy;
        let a = -2.0;
        let b = 3.0;
        let target = Point2::new(a * ux + b * vx, a * uy + b * vy);
        let snapped = iso_grid_snap(target, step).unwrap();
        assert!((snapped.x - target.x).abs() < 1e-9);
        assert!((snapped.y - target.y).abs() < 1e-9);
    }

    #[test]
    fn iso_grid_snap_picks_euclidean_nearest_not_componentwise_rounding() {
        // (a, b) = (0.49, -0.49) → ワールド (0.49·√3, 0) ≈ (0.849, 0)。成分ごとの丸めは
        // 原点（距離 0.849）を返すが、格子点 u = (√3/2, 1/2) は距離 0.500 で近い。
        let step = 1.0;
        let ux = 3f64.sqrt() / 2.0;
        let cursor = Point2::new(0.49 * ux - (-0.49) * ux, 0.49 * 0.5 + (-0.49) * 0.5);
        // u と −v = (√3/2, −1/2) はどちらも距離 0.500 で同値。どちらが返っても
        // 「最近点」であることだけを固定する（原点〔0.849〕を返してはいけない）。
        let p = iso_grid_snap(cursor, step).unwrap();
        let dist = ((p.x - cursor.x).powi(2) + (p.y - cursor.y).powi(2)).sqrt();
        let origin_dist = (cursor.x.powi(2) + cursor.y.powi(2)).sqrt();
        assert!(
            dist < 0.51 && dist < origin_dist,
            "got {p:?} at distance {dist}"
        );
        assert!((p.x - ux).abs() < 1e-9 && p.y.abs() > 0.4, "got {p:?}");
    }

    #[test]
    fn iso_grid_snap_non_finite_or_non_positive_step_returns_none() {
        let cursor = Point2::new(1.0, 1.0);
        assert_eq!(iso_grid_snap(cursor, f64::NAN), None);
        assert_eq!(iso_grid_snap(cursor, f64::INFINITY), None);
        assert_eq!(iso_grid_snap(cursor, 0.0), None);
        assert_eq!(iso_grid_snap(cursor, -1.0), None);
    }

    #[test]
    fn snap_uses_isometric_grid_when_mode_is_isometric() {
        // 空ドキュメント: グリッドのみが候補になる。等測格子点 (u = (√3/2, 0.5)) の
        // すぐ近くにカーソルを置くと、矩形丸め ((1,1) など) ではなく等測格子点へ
        // スナップすることを確認する。
        let doc = Document::new();
        let step = 1.0;
        let u = Point2::new(3f64.sqrt() / 2.0, 0.5);
        let cursor = Point2::new(u.x + 0.02, u.y - 0.01);
        let r = snap(
            &doc,
            cursor,
            0.5,
            GridSpec::new(step, GridMode::Isometric),
            SnapKinds::default(),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Grid);
        assert!((r.point.x - u.x).abs() < 1e-9);
        assert!((r.point.y - u.y).abs() < 1e-9);
    }

    // --- 四半円点（SnapKind::Quadrant、M12 タスク85） ---

    #[test]
    fn circle_quadrant_point_snaps_even_though_center_is_farther() {
        // 円 中心 (0,0) 半径 5: 四半円点は (5,0),(0,5),(-5,0),(0,-5)。カーソルを
        // (5,0) の近くに置くと、中心（距離 5.05）よりずっと近い四半円点（距離 0.05）が
        // 半径内に入り、そちらへスナップする。
        let mut doc = Document::new();
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0)),
        );

        let r = snap_no_grid(&doc, Point2::new(5.05, 0.0), 0.5).unwrap();
        assert_eq!(r.kind, SnapKind::Quadrant);
        assert_eq!(r.point, Point2::new(5.0, 0.0));
    }

    #[test]
    fn arc_quadrant_point_outside_sweep_is_not_a_candidate() {
        // 円弧 中心 (0,0) 半径 5、掃引角 0°→90°（第1象限のみ）。180° の四半円点
        // (-5,0) は弧の範囲外なので候補にならない。中心は遠いので他の候補もない。
        use mcad_geom::Arc;
        let mut doc = Document::new();
        let arc = Arc::new(Point2::new(0.0, 0.0), 5.0, 0.0, FRAC_PI_2);
        add(&mut doc, Shape::Arc(arc));

        assert_eq!(snap_no_grid(&doc, Point2::new(-5.0, 0.0), 0.5), None);
    }

    #[test]
    fn arc_quadrant_point_inside_sweep_is_a_candidate() {
        // 弧の始終点を四半円角そのものからずらす（-10°→100°）ことで、0° の四半円点
        // (5,0) が「掃引範囲内だが端点ではない」候補になることを確認する。
        use mcad_geom::Arc;
        let mut doc = Document::new();
        let arc = Arc::new(
            Point2::new(0.0, 0.0),
            5.0,
            -10f64.to_radians(),
            100f64.to_radians(),
        );
        add(&mut doc, Shape::Arc(arc));

        let r = snap_no_grid(&doc, Point2::new(5.05, 0.0), 0.5).unwrap();
        assert_eq!(r.kind, SnapKind::Quadrant);
        assert!(r.point.distance(Point2::new(5.0, 0.0)) < 1e-9);
    }

    #[test]
    fn center_and_quadrant_are_same_priority_nearer_one_wins() {
        // 円 中心 (0,0) 半径 5。中心と四半円点 (5,0) は同順位（3）なので、
        // カーソルに近い方が勝つ。
        let mut doc = Document::new();
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0)),
        );

        // カーソル (2.4,0): 中心まで 2.4、四半円点(5,0)まで 2.6 → 中心が勝つ。
        let r = snap_no_grid(&doc, Point2::new(2.4, 0.0), 3.0).unwrap();
        assert_eq!(r.kind, SnapKind::Center);
        assert_eq!(r.point, Point2::new(0.0, 0.0));

        // カーソル (2.6,0): 中心まで 2.6、四半円点(5,0)まで 2.4 → 四半円点が勝つ。
        let r = snap_no_grid(&doc, Point2::new(2.6, 0.0), 3.0).unwrap();
        assert_eq!(r.kind, SnapKind::Quadrant);
        assert_eq!(r.point, Point2::new(5.0, 0.0));
    }

    // --- 垂線・接線（SnapKind::Perpendicular / SnapKind::Tangent、M12 タスク86） ---

    /// 垂線のみを有効にした [`SnapKinds`]。
    fn perpendicular_only() -> SnapKinds {
        SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: true,
            tangent: false,
            virtual_intersection: false,
            nearest: false,
            grid: false,
        }
    }

    /// 接線のみを有効にした [`SnapKinds`]。
    fn tangent_only() -> SnapKinds {
        SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: false,
            tangent: true,
            virtual_intersection: false,
            nearest: false,
            grid: false,
        }
    }

    #[test]
    fn perpendicular_foot_on_line_is_a_candidate() {
        // 線分 (0,0)-(10,0)、基点 (3,5) → 足 (3,0)。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));

        let r = snap_no_grid_with_kinds_and_base(
            &doc,
            Point2::new(3.05, 0.0),
            0.5,
            perpendicular_only(),
            Some(Point2::new(3.0, 5.0)),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Perpendicular);
        assert_eq!(r.point, Point2::new(3.0, 0.0));
    }

    #[test]
    fn perpendicular_foot_on_extension_is_not_a_candidate() {
        // 線分 (0,0)-(10,0)、基点 (15,5) → 足は t=1.5（延長上）なので候補にならない。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));

        assert_eq!(
            snap_no_grid_with_kinds_and_base(
                &doc,
                Point2::new(15.0, 0.0),
                1.0,
                perpendicular_only(),
                Some(Point2::new(15.0, 5.0)),
            ),
            None
        );
    }

    #[test]
    fn perpendicular_and_tangent_disabled_kinds_hide_candidates_even_with_base() {
        // base があっても種別が OFF なら候補にならない（垂線・接線それぞれ）。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(20.0, 0.0), 5.0)),
        );
        let base = Some(Point2::new(3.0, 5.0));

        // 垂線を無効化: 線分への垂線候補(3,0)近傍でも出ない。
        let kinds = SnapKinds {
            perpendicular: false,
            ..perpendicular_only()
        };
        assert_eq!(
            snap_no_grid_with_kinds_and_base(&doc, Point2::new(3.05, 0.0), 0.5, kinds, base),
            None
        );

        // 接線を無効化: 円への接線候補近傍でも出ない。
        let tangent_base = Some(Point2::new(28.0, 0.0));
        let kinds = SnapKinds {
            tangent: false,
            ..tangent_only()
        };
        assert_eq!(
            snap_no_grid_with_kinds_and_base(
                &doc,
                Point2::new(20.0, 4.0),
                3.0,
                kinds,
                tangent_base
            ),
            None
        );
    }

    #[test]
    fn perpendicular_without_base_is_not_a_candidate() {
        // base が None なら、同じ状況（線分と近傍カーソル）でも垂線候補は出ない。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));

        assert_eq!(
            snap_no_grid_with_kinds(&doc, Point2::new(3.05, 0.0), 0.5, perpendicular_only()),
            None
        );
    }

    #[test]
    fn circle_tangent_points_are_candidates_nearest_cursor_wins() {
        // 円 中心(0,0) 半径5、基点(13,0)（5-12-13 の直角三角形）。接点は
        // (25/13, 60/13) と (25/13, -60/13)。カーソルを上側の接点近くに置くと
        // そちらが選ばれる。
        let mut doc = Document::new();
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0)),
        );

        let expected = Point2::new(25.0 / 13.0, 60.0 / 13.0);
        let r = snap_no_grid_with_kinds_and_base(
            &doc,
            Point2::new(expected.x, expected.y + 0.05),
            0.5,
            tangent_only(),
            Some(Point2::new(13.0, 0.0)),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Tangent);
        assert!(r.point.distance(expected) < 1e-9);
    }

    #[test]
    fn arc_tangent_point_outside_sweep_is_not_a_candidate() {
        // 円弧 中心(0,0) 半径5、掃引 0°→90°。基点(13,0) の接点のうち下側
        // (25/13,-60/13) は弧の範囲外なので候補にならない。
        use mcad_geom::Arc;
        let mut doc = Document::new();
        let arc = Arc::new(Point2::new(0.0, 0.0), 5.0, 0.0, FRAC_PI_2);
        add(&mut doc, Shape::Arc(arc));

        let below = Point2::new(25.0 / 13.0, -60.0 / 13.0);
        assert_eq!(
            snap_no_grid_with_kinds_and_base(
                &doc,
                Point2::new(below.x, below.y - 0.05),
                0.5,
                tangent_only(),
                Some(Point2::new(13.0, 0.0)),
            ),
            None
        );
    }

    #[test]
    fn tangent_point_when_base_inside_circle_is_not_a_candidate() {
        // 基点(1,0)は円 中心(0,0) 半径5 の内部なので接線が存在しない。
        let mut doc = Document::new();
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0)),
        );

        assert_eq!(
            snap_no_grid_with_kinds_and_base(
                &doc,
                Point2::new(5.0, 0.0),
                1.0,
                tangent_only(),
                Some(Point2::new(1.0, 0.0)),
            ),
            None
        );
    }

    #[test]
    fn perpendicular_priority_is_below_center_and_above_nearest() {
        // 円 中心(0,3) 半径0.05（実質、中心のごく近くにしか候補が無い曲線）と
        // 線分 (0,0)-(10,0) を同じ場に置く。基点(5,5) からの垂線の足は (5,0)。
        // カーソル (5,0.05) 付近では中心(0,3)は遠いため候補外だが、垂線と近接点は
        // ともに射程内。中心・四半円点(優先度3)は垂線(優先度4)より高優先、
        // 近接点(優先度6)は垂線より低優先であることを確認する。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        let base = Some(Point2::new(5.0, 5.0));

        // 垂線と近接点をともに有効にすると、垂線(優先度4)が近接点(優先度6)に勝つ。
        let kinds = SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: true,
            tangent: false,
            virtual_intersection: false,
            nearest: true,
            grid: false,
        };
        let r = snap_no_grid_with_kinds_and_base(&doc, Point2::new(5.0, 0.05), 0.5, kinds, base)
            .unwrap();
        assert_eq!(r.kind, SnapKind::Perpendicular);
        assert_eq!(r.point, Point2::new(5.0, 0.0));

        // 中心を同じ半径内に置くと、中心(優先度3)が垂線(優先度4)に勝つ。
        let mut doc2 = Document::new();
        add(&mut doc2, line(0.0, 0.0, 10.0, 0.0));
        add(
            &mut doc2,
            Shape::Circle(Circle::new(Point2::new(5.2, 0.05), 0.01)),
        );
        let kinds2 = SnapKinds {
            center: true,
            ..perpendicular_only()
        };
        let r = snap_no_grid_with_kinds_and_base(&doc2, Point2::new(5.1, 0.05), 0.5, kinds2, base)
            .unwrap();
        assert_eq!(r.kind, SnapKind::Center);
    }

    // --- 近接点（SnapKind::Nearest、M12 タスク85） ---

    /// 近接点のみを有効にした [`SnapKinds`]（他の候補と混ざらないよう分離するテスト用）。
    fn nearest_only() -> SnapKinds {
        SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: false,
            tangent: false,
            virtual_intersection: false,
            nearest: true,
            grid: false,
        }
    }

    #[test]
    fn nearest_point_on_line_is_a_candidate() {
        // 線分 (0,0)-(10,0) の中間あたり、端点でも中点でもない位置に垂線の足が立つ。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));

        let r = snap_no_grid_with_kinds(&doc, Point2::new(3.0, 0.2), 0.5, nearest_only()).unwrap();
        assert_eq!(r.kind, SnapKind::Nearest);
        assert_eq!(r.point, Point2::new(3.0, 0.0));
    }

    #[test]
    fn nearest_point_on_circle_is_a_candidate() {
        // 円 中心 (0,0) 半径 5 の円周上、四半円点でも中心でもない角度の点。
        let mut doc = Document::new();
        add(
            &mut doc,
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0)),
        );

        let cursor = Point2::new(4.0, 3.2); // (4,3) は円周上（3-4-5）のすぐ外側。
        let r = snap_no_grid_with_kinds(&doc, cursor, 0.5, nearest_only()).unwrap();
        assert_eq!(r.kind, SnapKind::Nearest);
        assert!((r.point.distance(Point2::new(0.0, 0.0)) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn nearest_point_on_polyline_edge_is_a_candidate() {
        // ポリライン (0,0)-(4,0)-(4,4) の2本目の辺上、端点でも中点でもない位置。
        let mut doc = Document::new();
        add(
            &mut doc,
            Shape::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(4.0, 0.0),
                    Point2::new(4.0, 4.0),
                ],
                false,
            )),
        );

        let r = snap_no_grid_with_kinds(&doc, Point2::new(4.2, 3.0), 0.5, nearest_only()).unwrap();
        assert_eq!(r.kind, SnapKind::Nearest);
        assert_eq!(r.point, Point2::new(4.0, 3.0));
    }

    #[test]
    fn nearest_is_off_by_default_and_on_when_enabled() {
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        let cursor = Point2::new(3.0, 0.2);

        // 既定（SnapKinds::default）では近接点は出ない。他候補も無ければ None。
        assert_eq!(snap_no_grid(&doc, cursor, 0.5), None);

        // 明示的に有効化すると近接点が出る。
        let r = snap_no_grid_with_kinds(&doc, cursor, 0.5, nearest_only()).unwrap();
        assert_eq!(r.kind, SnapKind::Nearest);
    }

    #[test]
    fn endpoint_beats_nearest_when_both_enabled() {
        // 線分 (0,0)-(10,0)。カーソルを端点 (10,0) のすぐ内側に置くと、垂線の足
        // （近接点候補、線分上そのもの＝カーソルの直下）より端点が優先される。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        let kinds = SnapKinds {
            nearest: true,
            ..SnapKinds::default()
        };

        let r = snap_no_grid_with_kinds(&doc, Point2::new(9.9, 0.05), 0.5, kinds).unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(10.0, 0.0));
    }

    // --- 仮想交点（SnapKind::VirtualIntersection、M12 タスク87） ---

    /// 仮想交点のみを有効にした [`SnapKinds`]。
    fn virtual_intersection_only() -> SnapKinds {
        SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: false,
            tangent: false,
            virtual_intersection: true,
            nearest: false,
            grid: false,
        }
    }

    #[test]
    fn virtual_intersection_of_two_line_extensions_is_a_candidate() {
        // 線分 a (0,0)-(1,1)、線分 b (0,10)-(1,9)。どちらも延長した無限直線は
        // (5,5) で交わるが、両方とも区間の外（延長線上）。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 1.0, 1.0));
        add(&mut doc, line(0.0, 10.0, 1.0, 9.0));

        let r = snap_no_grid_with_kinds(
            &doc,
            Point2::new(5.0, 5.0),
            1.0,
            virtual_intersection_only(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!(r.point.distance(Point2::new(5.0, 5.0)) < 1e-9);

        // 両方延長側なので、ガイドは両方 Some。from は実体の近い方の端点
        // （a は (1,1)、b は (1,9)）。
        match r.guides[0] {
            Some(SnapGuide::Segment { from, to }) => {
                assert_eq!(from, Point2::new(1.0, 1.0));
                assert!(to.distance(Point2::new(5.0, 5.0)) < 1e-9);
            }
            other => panic!("線分 a 側は Segment ガイドを期待: {other:?}"),
        }
        match r.guides[1] {
            Some(SnapGuide::Segment { from, .. }) => {
                assert_eq!(from, Point2::new(1.0, 9.0));
            }
            other => panic!("線分 b 側は Segment ガイドを期待: {other:?}"),
        }
    }

    #[test]
    fn virtual_intersection_parallel_lines_is_not_a_candidate() {
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        add(&mut doc, line(0.0, 1.0, 10.0, 1.0));

        assert_eq!(
            snap_no_grid_with_kinds(
                &doc,
                Point2::new(5.0, 0.5),
                1.0,
                virtual_intersection_only()
            ),
            None
        );
    }

    #[test]
    fn virtual_intersection_excludes_real_intersections_even_when_intersection_disabled() {
        // 交点(5,0)は両線分の区間内(真の交点)。仮想交点のみ有効でも候補にしない
        // （SnapKind::Intersection の担当。交点スナップが OFF でも仮想交点として
        // 出てはいけない）。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        add(&mut doc, line(5.0, -5.0, 5.0, 5.0));

        assert_eq!(
            snap_no_grid_with_kinds(
                &doc,
                Point2::new(5.1, 0.0),
                1.0,
                virtual_intersection_only()
            ),
            None
        );
    }

    #[test]
    fn virtual_intersection_line_extension_with_circle() {
        // 線分区間 (-10,0)-(-8,0) は円から離れているが、延長した直線は円
        // 中心(0,0) 半径5 と (5,0)/(-5,0) で交わる。完全な円はガイド無し。
        let mut doc = Document::new();
        add(&mut doc, line(-10.0, 0.0, -8.0, 0.0));
        add(&mut doc, Shape::Circle(Circle::new(Point2::ORIGIN, 5.0)));

        let r = snap_no_grid_with_kinds(
            &doc,
            Point2::new(5.05, 0.0),
            0.5,
            virtual_intersection_only(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!(r.point.distance(Point2::new(5.0, 0.0)) < 1e-9);
        match r.guides[0] {
            Some(SnapGuide::Segment { from, .. }) => assert_eq!(from, Point2::new(-8.0, 0.0)),
            other => panic!("線分側は Segment ガイドを期待: {other:?}"),
        }
        assert_eq!(r.guides[1], None, "完全な円はガイド無し");
    }

    #[test]
    fn virtual_intersection_arc_extension_produces_arc_guide() {
        // 円弧 中心(0,0) 半径5、掃引 0°→90°。線分 (-10,0)-(-7,0) の延長は
        // 180°方向の (-5,0) で台円と交わるが、掃引範囲外（延長側）なので
        // 円弧側のガイドは Arc になる。
        use mcad_geom::Arc;
        let mut doc = Document::new();
        add(&mut doc, line(-10.0, 0.0, -7.0, 0.0));
        add(
            &mut doc,
            Shape::Arc(Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2)),
        );

        let r = snap_no_grid_with_kinds(
            &doc,
            Point2::new(-5.0, 0.0),
            0.5,
            virtual_intersection_only(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!(r.point.distance(Point2::new(-5.0, 0.0)) < 1e-9);
        match r.guides[1] {
            Some(SnapGuide::Arc {
                center,
                radius,
                start_angle,
                sweep,
            }) => {
                assert!(center.distance(Point2::ORIGIN) < 1e-9);
                assert!((radius - 5.0).abs() < 1e-9);
                assert!((start_angle - FRAC_PI_2).abs() < 1e-9);
                assert!((sweep - FRAC_PI_2).abs() < 1e-9);
            }
            other => panic!("円弧側は Arc ガイドを期待: {other:?}"),
        }
    }

    #[test]
    fn real_endpoint_beats_virtual_intersection_at_the_same_point() {
        // 線分 a・b の延長交点 (5,5) ちょうどに点エンティティを置くと、既定設定
        // （端点・仮想交点ともに ON）では端点(優先度0)が仮想交点(優先度5)に勝つ。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 1.0, 1.0));
        add(&mut doc, line(0.0, 10.0, 1.0, 9.0));
        add(&mut doc, Shape::Point(Point2::new(5.0, 5.0)));

        let r = snap_no_grid(&doc, Point2::new(5.0, 5.0), 1.0).unwrap();
        assert_eq!(r.kind, SnapKind::Endpoint);
        assert_eq!(r.point, Point2::new(5.0, 5.0));
    }

    #[test]
    fn virtual_intersection_disabled_kind_hides_candidate() {
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 1.0, 1.0));
        add(&mut doc, line(0.0, 10.0, 1.0, 9.0));
        let kinds = SnapKinds {
            virtual_intersection: false,
            ..SnapKinds::default()
        };

        // 他の候補（端点・中点等）は (5,5) から遠いので None になる。
        assert_eq!(
            snap_no_grid_with_kinds(&doc, Point2::new(5.0, 5.0), 1.0, kinds),
            None
        );
    }

    #[test]
    fn virtual_intersection_priority_is_below_perpendicular_and_above_nearest() {
        // 線分 a (0,0)-(1,1)・b (0,10)-(1,9)（延長交点 (5,5)）に加え、
        // 線分 Lp (0,5)-(10,5) を置く。Lp 上では (5,5) は区間内の実点なので、
        // 基点(5,10) からの垂線の足・カーソルの最近接点としても (5,5) が得られる。
        // 同じ点に複数種別の候補が重なる状況で、優先度だけで勝敗が決まることを確認する。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 1.0, 1.0));
        add(&mut doc, line(0.0, 10.0, 1.0, 9.0));
        add(&mut doc, line(0.0, 5.0, 10.0, 5.0));

        // 垂線(優先度4) vs 仮想交点(優先度5): 垂線が勝つ。
        let kinds = SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: true,
            tangent: false,
            virtual_intersection: true,
            nearest: false,
            grid: false,
        };
        let r = snap_no_grid_with_kinds_and_base(
            &doc,
            Point2::new(5.0, 4.99),
            0.5,
            kinds,
            Some(Point2::new(5.0, 10.0)),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::Perpendicular);
        assert_eq!(r.point, Point2::new(5.0, 5.0));

        // 仮想交点(優先度5) vs 近接点(優先度6): 仮想交点が勝つ。
        let kinds2 = SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: false,
            tangent: false,
            virtual_intersection: true,
            nearest: true,
            grid: false,
        };
        let r2 = snap_no_grid_with_kinds(&doc, Point2::new(5.0, 4.99), 0.5, kinds2).unwrap();
        assert_eq!(r2.kind, SnapKind::VirtualIntersection);
        assert!(r2.point.distance(Point2::new(5.0, 5.0)) < 1e-9);
    }

    // --- 同一直線グループ化と上限（Codex adversarial review [high]、2026-09-26） ---

    #[test]
    fn group_collinear_lines_merges_thousands_of_segments_into_two_groups() {
        // 同じ無限直線 y=0 上に5000本、x=10 上に5000本の短い線分を並べる。グループ化が
        // 無ければ総当たりが本数の2乗（=1000万ペア）に爆発する状況で、純関数
        // `group_collinear_lines` だけで2本の支持直線へ縮むことを確認する
        // （所要時間はアサーションしない）。
        let mut lines: Vec<LineSeg> = Vec::new();
        for k in 0..5000 {
            let x = f64::from(k) * 0.001;
            lines.push(LineSeg::new(
                Point2::new(x, 0.0),
                Point2::new(x + 0.0005, 0.0),
            ));
        }
        for k in 0..5000 {
            let y = f64::from(k) * 0.001;
            lines.push(LineSeg::new(
                Point2::new(10.0, y),
                Point2::new(10.0, y + 0.0005),
            ));
        }

        let groups = group_collinear_lines(&lines);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups.iter().map(|g| g.members.len()).sum::<usize>(),
            10_000
        );

        // グループ化後も `snap()` が正しく仮想交点を返すこと（実際にドキュメントへ
        // 1万エンティティを追加して確認する）。
        let mut doc = Document::new();
        for seg in &lines {
            add(&mut doc, Shape::Line(*seg));
        }
        let r = snap_no_grid_with_kinds(
            &doc,
            Point2::new(10.0, 0.0),
            1.0,
            virtual_intersection_only(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!(r.point.distance(Point2::new(10.0, 0.0)) < 1e-6);
    }

    #[test]
    fn group_collinear_lines_merges_angle_wobble_across_two_parallel_rows() {
        // 2本の平行な水平線（y=0 と y=5）を、それぞれ200本の短い線分で表現する。各線分の
        // 終点 y をわずかに ±1e-15 だけ揺らし、正規化後の方向角が 0 付近・π 付近に
        // 交互に振れるようにする（回転・鏡映を経た図形で実際に起こりうる揺れの再現。
        // 一次修正後の Codex adversarial review [high] 再指摘、2026-09-26）。この揺れが
        // あっても行ごとに正しく1グループへまとまり、2グループ（2行）に分かれることを
        // 確認する。
        let mut lines: Vec<LineSeg> = Vec::new();
        for k in 0..200 {
            let x = f64::from(k) * 0.01;
            let eps_a = if k % 2 == 0 { 1e-15 } else { -1e-15 };
            lines.push(LineSeg::new(
                Point2::new(x, 0.0),
                Point2::new(x + 0.005, eps_a),
            ));
            let eps_b = if k % 2 == 0 { -1e-15 } else { 1e-15 };
            lines.push(LineSeg::new(
                Point2::new(x, 5.0),
                Point2::new(x + 0.005, 5.0 + eps_b),
            ));
        }

        let groups = group_collinear_lines(&lines);
        assert_eq!(groups.len(), 2);
        let mut sizes: Vec<usize> = groups.iter().map(|g| g.members.len()).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![200, 200]);
    }

    #[test]
    fn group_collinear_lines_merges_boundary_direction_wobble() {
        // 方向 (1, -1e-17) は正規化で (-1, 1e-17) へ反転され、角度は π 付近になる。
        // 方向 (1, 0) は角度 0。どちらも同じ無限直線（y=0）上の向きの揺れにすぎないので
        // 同じグループにまとまるべき（0 と π の境界の統合、Codex adversarial review
        // [high] 再指摘、2026-09-26）。
        let a = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, -1e-17));
        let b = LineSeg::new(Point2::new(2.0, 0.0), Point2::new(3.0, 0.0));

        let groups = group_collinear_lines(&[a, b]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
    }

    #[test]
    fn virtual_intersection_survives_cap_when_wobbly_row_is_grouped_properly() {
        // 揺れのある行（5000本、上記と同じ ±1e-15 の交互パターン）を正しく1グループへ
        // まとめられないと、多数の小グループに分裂し、線×線の組の数が本来の
        // 「行×列」の数をはるかに超えて膨らむ（Codex adversarial review [high] 再指摘、
        // 2026-09-26）。グループ化が正しく機能していれば、行は少数のグループへ縮み、
        // 列との仮想交点を検出できる。
        let mut doc = Document::new();
        for k in 0..5000 {
            let x = f64::from(k) * 0.001;
            let eps = if k % 2 == 0 { 1e-15 } else { -1e-15 };
            add(&mut doc, line(x, 0.0, x + 0.0005, eps));
        }
        add(&mut doc, line(10.0, -1.0, 10.0, 1.0));

        let r = snap_no_grid_with_kinds(
            &doc,
            Point2::new(10.0, 0.0),
            1.0,
            virtual_intersection_only(),
        )
        .expect("行のグループ化が壊れていなければ列との仮想交点が見つかるはず");
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!(r.point.distance(Point2::new(10.0, 0.0)) < 1e-6);
    }

    #[test]
    fn virtual_intersection_group_containing_a_real_intersection_member_is_not_virtual() {
        // 同一直線 y=0 上の2線分: segA1 (0,0)-(10,0) は交点(5,0)を含む（実体上）。
        // segA2 (20,0)-(30,0) は同じ無限直線上だが交点(5,0)を含まない（延長）。
        // グループ化前は segA2 側を独立に「延長＝仮想」と誤判定しうるが、グループ化後は
        // 「グループ内のいずれかのメンバー上にあれば実体上」なので、この点は仮想交点に
        // ならない（SnapKind::Intersection の担当）。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 10.0, 0.0));
        add(&mut doc, line(20.0, 0.0, 30.0, 0.0));
        add(&mut doc, line(5.0, -5.0, 5.0, 5.0));

        assert_eq!(
            snap_no_grid_with_kinds(
                &doc,
                Point2::new(5.1, 0.0),
                1.0,
                virtual_intersection_only()
            ),
            None
        );
    }

    #[test]
    fn virtual_intersection_guide_uses_nearest_endpoint_across_the_whole_group() {
        // 同一直線 y=0 上の2線分: segA1 (0,0)-(1,0)・segA2 (100,0)-(101,0)。どちらも
        // 交点(5,0)を含まない（延長）。ガイドの from はグループ全体の端点のうち
        // 最も近いもの（(1,0)）であるべきで、たまたま最初に見つかったメンバーの端点
        // （(0,0)）ではない。
        let mut doc = Document::new();
        add(&mut doc, line(0.0, 0.0, 1.0, 0.0));
        add(&mut doc, line(100.0, 0.0, 101.0, 0.0));
        add(&mut doc, line(5.0, -5.0, 5.0, 5.0));

        let r = snap_no_grid_with_kinds(
            &doc,
            Point2::new(5.0, 0.5),
            1.0,
            virtual_intersection_only(),
        )
        .unwrap();
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        // 垂直線 B 側は交点(5,0)を区間内に含む（実体上）のでガイド無し。y=0 グループ側
        // だけがガイドを持つ。
        let guide = r
            .guides
            .into_iter()
            .flatten()
            .next()
            .expect("y=0 グループ側のガイドがあるはず");
        match guide {
            SnapGuide::Segment { from, to } => {
                assert_eq!(from, Point2::new(1.0, 0.0));
                assert!(to.distance(Point2::new(5.0, 0.0)) < 1e-9);
            }
            other => panic!("Segment ガイドを期待: {other:?}"),
        }
    }

    #[test]
    fn virtual_intersection_pair_budget_truncates_without_panicking_for_many_radiating_lines() {
        // 互いに平行でない直線を、線×線の組の予算（`VIRTUAL_INTERSECTION_MAX_LINE_PAIRS`
        // = 4096）を超える組合せ数になる本数（200本、角度がすべて異なる放射状）だけ
        // 配置しても panic せず、打ち切りが起きても最初に処理される（カーソルに最も
        // 近い）仮想交点は返ること（Codex adversarial review [medium] 再指摘、
        // 2026-09-26。角度がすべて異なるため平行グループのスキップは効かず、
        // 200*199/2=19,900 組が本来必要になるが、予算 4096 で打ち切られる）。
        let mut doc = Document::new();
        let n: usize = 200;
        for k in 0..n {
            let theta = (k as f64) * std::f64::consts::PI / (n as f64);
            let (s, c) = theta.sin_cos();
            add(&mut doc, line(5.0 * c, 5.0 * s, 6.0 * c, 6.0 * s));
        }

        let r = snap_no_grid_with_kinds(&doc, Point2::ORIGIN, 1.0, virtual_intersection_only())
            .expect("予算で打ち切られても仮想交点の候補が見つかるはず");
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!(r.point.distance(Point2::ORIGIN) < 1e-6);
    }

    #[test]
    fn virtual_intersection_parallel_lines_do_not_crowd_out_a_crossing_line() {
        // カーソル近傍に平行な水平線が100本（ハッチングのような状況）あっても、
        // 少し離れた交差する垂直線1本との仮想交点が失われないこと（Codex adversarial
        // review [medium] 再指摘の再現例、2026-09-26。本数の上限だった旧実装では、
        // カーソルとの距離で近い順に切り詰めると平行線ばかりが残り、垂直線が
        // 切り捨てられて交点が消えていた）。平行などうしは組にしないため、水平線が
        // 何本あっても垂直線との組合せ自体は変わらない。
        let mut doc = Document::new();
        for k in 0..100 {
            let y = f64::from(k) * 0.0001;
            add(&mut doc, line(0.0, y, 1.0, y));
        }
        // 垂直線はカーソルから見て水平線群よりわずかに遠い（x オフセット 0.5）。
        add(&mut doc, line(50.5, -1.0, 50.5, 1.0));

        let r = snap_no_grid_with_kinds(
            &doc,
            Point2::new(50.0, 0.0),
            0.52,
            virtual_intersection_only(),
        )
        .expect("平行な水平線に埋もれても垂直線との仮想交点が見つかるはず");
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!((r.point.x - 50.5).abs() < 1e-6);
    }

    #[test]
    fn virtual_intersection_finds_match_among_many_concentric_circles() {
        // 同心円50個（半径がわずかに異なるだけ）と交差する直線1本でも、半径内の交点が
        // 返ること（Codex adversarial review [medium] 再指摘、2026-09-26: 円要素の
        // 本数上限32も同じ欠陥を持っていたため、本数の上限自体をやめたことの確認）。
        let mut doc = Document::new();
        for k in 0..50 {
            let radius = 10.0 + f64::from(k) * 0.001;
            add(&mut doc, Shape::Circle(Circle::new(Point2::ORIGIN, radius)));
        }
        // 直線は円との交点（半径10.000〜10.049付近）から離れた区間にあるので、
        // 延長側になる。
        add(&mut doc, line(-5.0, 0.0, -4.0, 0.0));

        let cursor = Point2::new(10.049, 0.0);
        let r = snap_no_grid_with_kinds(&doc, cursor, 0.06, virtual_intersection_only())
            .expect("多数の同心円があっても交点が見つかるはず");
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!(r.point.distance(cursor) <= 0.06 + 1e-9);
    }

    #[test]
    fn virtual_intersection_line_circle_budget_is_independent_of_line_pair_budget() {
        // 原点を起点とする放射状の線分200本を配置する。互いに非平行なので角度クラスタの
        // スキップは効かず、線×線の組の予算（`VIRTUAL_INTERSECTION_MAX_LINE_PAIRS` =
        // 4096）を使い切る（200*199/2=19,900組）。各線分の始点が原点そのものなので、
        // どの2本の交点（＝原点）も両方の区間内（実交点）となり、Intersection の担当
        // としてスキップされる（＝線×線からは仮想交点候補が一切出ない）。
        //
        // 原点中心・半径0.5の円を1個置くと、各線分は無限直線として円と2点で交わる:
        // 線分の向きの側（パラメータ0.5、区間[0,1]内＝実交点）と、その逆側
        // （パラメータ-0.5、区間外＝仮想交点）。後者は200本すべての線分について
        // 仮想交点になる。線×円は**独立した予算**
        // （`VIRTUAL_INTERSECTION_MAX_LINE_CIRCLE_PAIRS`）を持つため、線×線の組の予算が
        // 尽きていてもこの仮想交点が見つかることを確認する（Codex adversarial review
        // [medium] 再々指摘、2026-09-26）。
        let mut doc = Document::new();
        let n: usize = 200;
        for k in 0..n {
            let theta = (k as f64) * std::f64::consts::PI / (n as f64);
            let (s, c) = theta.sin_cos();
            add(&mut doc, line(0.0, 0.0, c, s));
        }
        add(&mut doc, Shape::Circle(Circle::new(Point2::ORIGIN, 0.5)));

        let r = snap_no_grid_with_kinds(&doc, Point2::ORIGIN, 1.0, virtual_intersection_only())
            .expect("線×線の予算を使い切っても線×円の仮想交点は見つかるはず");
        assert_eq!(r.kind, SnapKind::VirtualIntersection);
        assert!((r.point.distance(Point2::ORIGIN) - 0.5).abs() < 1e-6);
    }

    // --- 種別ごとの ON/OFF（SnapKinds、M12 タスク85） ---

    #[test]
    fn disabled_endpoint_kind_hides_extra_points_too() {
        // extra_points（作図中ツールの未確定頂点）も Endpoint 種別として扱われるため、
        // endpoint を無効化すると出なくなる（他候補も無ければ None）。
        let doc = Document::new();
        let extra = [Point2::new(0.05, 0.0)];
        let kinds = SnapKinds {
            endpoint: false,
            ..SnapKinds::default()
        };
        let r = snap(
            &doc,
            Point2::new(0.0, 0.0),
            0.5,
            GridSpec::new(0.0, GridMode::Rectangular),
            kinds,
            &extra,
            None,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn disabled_grid_kind_removes_grid_fallback() {
        // グリッドを無効化すると、他に候補が無いときのフォールバックも働かない。
        let doc = Document::new();
        let kinds = SnapKinds {
            grid: false,
            ..SnapKinds::default()
        };
        let r = snap(
            &doc,
            Point2::new(0.2, -0.1),
            0.5,
            GridSpec::new(1.0, GridMode::Rectangular),
            kinds,
            &[],
            None,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn split_snap_obeys_disabled_kinds() {
        // 分割スナップ（Midpoint/Endpoint/Intersection）も設定に従う。中点を無効化
        // すると、中点候補だった位置はスナップしなくなる。
        let mut doc = Document::new();
        let shape = line(0.0, 0.0, 4.0, 0.0);
        let id = add_id(&mut doc, shape.clone());
        let kinds = SnapKinds {
            midpoint: false,
            ..SnapKinds::default()
        };

        assert_eq!(
            snap_split_position(&doc, id, &shape, Point2::new(2.1, 0.0), 0.5, kinds),
            None
        );

        // 全部無効化すれば、交点があっても拾わない。
        let mut doc2 = Document::new();
        let target = line(0.0, 0.0, 10.0, 0.0);
        let id2 = add_id(&mut doc2, target.clone());
        add(&mut doc2, line(5.0, -5.0, 5.0, 5.0));
        let all_off = SnapKinds {
            endpoint: false,
            intersection: false,
            midpoint: false,
            center: false,
            quadrant: false,
            perpendicular: false,
            tangent: false,
            virtual_intersection: false,
            nearest: false,
            grid: false,
        };
        assert_eq!(
            snap_split_position(&doc2, id2, &target, Point2::new(5.1, 0.0), 0.5, all_off),
            None
        );
    }

    // --- SnapKinds ---

    #[test]
    fn snap_kinds_default_has_only_nearest_off() {
        let kinds = SnapKinds::default();
        assert!(kinds.endpoint);
        assert!(kinds.intersection);
        assert!(kinds.midpoint);
        assert!(kinds.center);
        assert!(kinds.quadrant);
        assert!(kinds.perpendicular);
        assert!(kinds.tangent);
        assert!(kinds.virtual_intersection);
        assert!(!kinds.nearest);
        assert!(kinds.grid);
    }

    #[test]
    fn snap_kinds_enabled_matches_each_field() {
        let kinds = SnapKinds {
            endpoint: true,
            intersection: false,
            midpoint: true,
            center: false,
            quadrant: true,
            perpendicular: false,
            tangent: true,
            virtual_intersection: false,
            nearest: false,
            grid: true,
        };
        assert!(kinds.enabled(SnapKind::Endpoint));
        assert!(!kinds.enabled(SnapKind::Intersection));
        assert!(kinds.enabled(SnapKind::Midpoint));
        assert!(!kinds.enabled(SnapKind::Center));
        assert!(kinds.enabled(SnapKind::Quadrant));
        assert!(!kinds.enabled(SnapKind::Perpendicular));
        assert!(kinds.enabled(SnapKind::Tangent));
        assert!(!kinds.enabled(SnapKind::VirtualIntersection));
        assert!(!kinds.enabled(SnapKind::Nearest));
        assert!(kinds.enabled(SnapKind::Grid));
    }

    #[test]
    fn snap_kinds_serde_round_trip_and_missing_fields_default() {
        let kinds = SnapKinds {
            endpoint: false,
            intersection: true,
            midpoint: false,
            center: true,
            quadrant: false,
            perpendicular: true,
            tangent: false,
            virtual_intersection: true,
            nearest: true,
            grid: false,
        };
        let json = serde_json::to_string(&kinds).unwrap();
        let parsed: SnapKinds = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kinds);

        // 空オブジェクトは各フィールドの既定値（nearest だけ false）へ補完される。
        let empty: SnapKinds = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SnapKinds::default());
    }
}
