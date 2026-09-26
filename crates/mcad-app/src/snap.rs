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
//! カーソルとの距離で決める。垂線・接線（M12 タスク86）・仮想交点（M12 タスク87）は
//! 本タスク（85）の時点では未実装だが、優先度の数値は先に空けてある
//! （[`SnapKind::priority`] のコメント参照）。
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

use mcad_core::{Document, Entity, EntityGeom, EntityId};
use mcad_geom::{Aabb, Point2, Shape, closest_point, intersect, point_tol};
use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_2, PI};

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
    /// 近接点（可視 Shape 上でカーソルに最も近い点。既定 OFF。M12 タスク85）。
    Nearest,
    /// グリッド交点。
    Grid,
}

impl SnapKind {
    /// 数値優先度（小さいほど高優先）。同順位内はカーソルとの距離で決める。
    ///
    /// `5` は M12 タスク87（仮想交点）用に空けてある（DESIGN.md M12 設計判断5:
    /// 端点 > 交点 > 中点 > 中心 = 四半円点 > 垂線 = 接線 > 仮想交点 > 近接点 > グリッド）。
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
            // 5 は仮想交点（タスク87）用に予約。
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
            SnapKind::Nearest => self.nearest,
            SnapKind::Grid => self.grid,
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
    fn consider(&mut self, kind: SnapKind, point: Point2, cursor: Point2, r2: f64) {
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
