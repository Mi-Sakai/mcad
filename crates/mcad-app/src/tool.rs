//! Tool状態機械（DESIGN.md 3.4）と作図ツール（Point/Line/Circle/Arc/Polyline）。
//!
//! # 責務の分離
//!
//! [`Tool`] トレイトは「入力を受けて状態を進め、確定/継続/中断を返す」ことと
//! 「未確定の途中経過をプレビュー描画する」ことの 2 責務のみを持つ。
//! DESIGN.md 3.4 が示すシグネチャ
//!
//! ```rust,ignore
//! trait Tool {
//!     fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult;
//!     fn draw_preview(&self, painter: &Painter, vp: &Viewport);
//! }
//! ```
//!
//! を、実際の egui/mcad-app の型に合わせて次のように調整している:
//!
//! - `Painter` → `egui::Painter`
//! - `Viewport` → `crate::viewport::Viewport`
//! - `draw_preview` は `Viewport::world_to_screen` がスクリーン矩形を引数に取る
//!   （`Viewport` 自身はスクリーン矩形を保持しない設計、`viewport.rs` 参照）ため、
//!   `rect: egui::Rect` を追加引数として受け取る
//! - 入力 `InputEvent` はワールド座標（[`Point2`]）とキー操作のみからなる軽量な列挙型
//!   にし、egui の生入力（`egui::Event` 等）に依存しない。これにより GUI なしで
//!   ツールの状態遷移を単体テストできる（本ファイル末尾の `tests` 参照）
//!
//! 2 つの責務の分離自体（「入力→ Continue/Commit/Cancel」「プレビュー描画」）は
//! 設計方針どおり維持している。
//!
//! # プレビュー描画の再利用
//!
//! `draw_preview` は `main.rs` の `draw_shape`（`Shape` → `Painter` 変換）を
//! そのまま再利用する。`draw_shape` は crate ルート（`main.rs`）に定義された
//! プライベート関数だが、Rust の可視性規則上プライベート項目は「定義モジュールと
//! その子孫モジュール」から見えるため、`tool` モジュール（`main.rs` の子モジュール）
//! から `crate::draw_shape` として問題なく呼べる。

use egui::{Color32, Painter, Rect, Stroke};

use mcad_core::{Command, Document, Entity, EntityId, LayerId, Style};
use mcad_geom::{Aabb, Arc, LineSeg, Point2, Polyline, Shape, Vec2, circumcircle, distance_to};

use crate::draw_shape;
use crate::viewport::Viewport;

/// プレビュー線の色（確定済みエンティティと区別しやすい暖色）。
const PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 200, 60);
/// プレビュー線の太さ。
const PREVIEW_WIDTH: f32 = 1.5;

fn preview_stroke() -> Stroke {
    Stroke::new(PREVIEW_WIDTH, PREVIEW_COLOR)
}

/// ツールに渡す実行時コンテキスト。新規エンティティの所属レイヤーとスタイルを持つ。
#[derive(Debug, Clone, Copy)]
pub struct ToolCtx {
    /// 新規エンティティの所属先（呼び出し側は通常 `Document::current_layer()`）。
    pub layer: LayerId,
    /// 新規エンティティのスタイル（通常 `Style::inherited()`）。
    pub style: Style,
}

/// ツールへの入力。ワールド座標とキー操作のみからなる軽量な列挙型
/// （GUIなしで単体テストできるようにするための設計）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// マウス移動（クリックを伴わない）。プレビュー更新用。
    Move(Point2),
    /// 左クリック確定。
    Click(Point2),
    /// Enterキー（確定操作。主に Polyline がクリック列を確定するのに使う）。
    Confirm,
    /// Escキー（未確定の状態を破棄する）。
    Cancel,
}

/// [`Tool::on_input`] の結果。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResult {
    /// まだ確定しない。ツールは内部状態を進めただけ。
    Continue,
    /// 確定した。呼び出し側は `Document::apply` でコマンドを適用する。
    Commit(Command),
    /// キャンセルされた。未確定の状態は破棄済み。
    Cancel,
}

/// 作図ツールの状態機械（DESIGN.md 3.4）。
pub trait Tool {
    /// 入力を 1 つ受け取り、内部状態を進めて結果を返す。
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult;

    /// 未確定の途中経過（クリック済み点・マウス追従中の線など）をプレビュー描画する。
    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport);

    /// 作図中（未確定）の頂点列。スナップエンジン（`crate::snap::snap`）が端点
    /// （最優先）候補として扱う。まだ `Document` に存在しない自分自身の頂点へも
    /// スナップできるようにするための拡張で、既定では空（対象なし）。
    fn snap_points(&self) -> Vec<Point2> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------
// Point
// ---------------------------------------------------------------------

/// 点ツール。クリック1回で [`Shape::Point`] を確定する。
#[derive(Debug, Default)]
pub struct PointTool {
    cursor: Option<Point2>,
}

impl Tool for PointTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => ToolResult::Commit(Command::AddEntity(Entity::new(
                Shape::Point(p),
                ctx.layer,
                ctx.style,
            ))),
            InputEvent::Cancel => ToolResult::Cancel,
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if let Some(p) = self.cursor {
            draw_shape(painter, rect, viewport, &Shape::Point(p), preview_stroke());
        }
    }
}

// ---------------------------------------------------------------------
// Line
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
enum LineState {
    #[default]
    WaitingFirst,
    WaitingSecond(Point2),
}

/// 線分ツール（連続線分モード）。クリック2回（始点・終点）で1本目の
/// [`Shape::Line`] を確定した後は、その終点を次の線分の始点として引き継ぎ、
/// 以降はクリックのたびに独立した線分エンティティを1本ずつ確定し続ける
/// （AutoCAD の LINE コマンドと同様の挙動）。ポリラインと異なり、各辺は
/// 個別のエンティティとして選択・編集できる。Escで連続モードを終了する
/// （`WaitingFirst` へ戻り、次のクリックは新しい始点として扱われる）。
/// 1点目クリック後はマウス位置までのプレビュー線を表示する。
#[derive(Debug, Default)]
pub struct LineTool {
    state: LineState,
    cursor: Option<Point2>,
}

impl Tool for LineTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                LineState::WaitingFirst => {
                    self.state = LineState::WaitingSecond(p);
                    ToolResult::Continue
                }
                LineState::WaitingSecond(first) => {
                    let cmd = Command::AddEntity(Entity::new(
                        Shape::Line(LineSeg::new(first, p)),
                        ctx.layer,
                        ctx.style,
                    ));
                    // 連続線分モード: 今確定した終点を次の線分の始点として引き継ぐ。
                    self.state = LineState::WaitingSecond(p);
                    ToolResult::Commit(cmd)
                }
            },
            InputEvent::Cancel => {
                self.state = LineState::WaitingFirst;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if let (LineState::WaitingSecond(first), Some(cursor)) = (&self.state, self.cursor) {
            draw_shape(
                painter,
                rect,
                viewport,
                &Shape::Line(LineSeg::new(*first, cursor)),
                preview_stroke(),
            );
        }
    }

    fn snap_points(&self) -> Vec<Point2> {
        match self.state {
            LineState::WaitingFirst => Vec::new(),
            LineState::WaitingSecond(first) => vec![first],
        }
    }
}

// ---------------------------------------------------------------------
// Circle
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
enum CircleState {
    #[default]
    WaitingCenter,
    WaitingRadiusPoint(Point2),
}

/// 円ツール。クリック2回（中心・円周上の1点）で [`Shape::Circle`] を確定する。
/// プレビューは中心からマウス位置までの距離を半径とする円。
#[derive(Debug, Default)]
pub struct CircleTool {
    state: CircleState,
    cursor: Option<Point2>,
}

impl Tool for CircleTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                CircleState::WaitingCenter => {
                    self.state = CircleState::WaitingRadiusPoint(p);
                    ToolResult::Continue
                }
                CircleState::WaitingRadiusPoint(center) => {
                    let radius = center.distance(p);
                    let cmd = Command::AddEntity(Entity::new(
                        Shape::Circle(mcad_geom::Circle::new(center, radius)),
                        ctx.layer,
                        ctx.style,
                    ));
                    self.state = CircleState::WaitingCenter;
                    ToolResult::Commit(cmd)
                }
            },
            InputEvent::Cancel => {
                self.state = CircleState::WaitingCenter;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if let (CircleState::WaitingRadiusPoint(center), Some(cursor)) = (&self.state, self.cursor)
        {
            let radius = center.distance(cursor);
            draw_shape(
                painter,
                rect,
                viewport,
                &Shape::Circle(mcad_geom::Circle::new(*center, radius)),
                preview_stroke(),
            );
        }
    }

    fn snap_points(&self) -> Vec<Point2> {
        match self.state {
            CircleState::WaitingCenter => Vec::new(),
            CircleState::WaitingRadiusPoint(center) => vec![center],
        }
    }
}

// ---------------------------------------------------------------------
// Arc（3点指定）
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
enum ArcState {
    #[default]
    WaitingP1,
    WaitingP2(Point2),
    WaitingP3(Point2, Point2),
}

/// 円弧ツール。クリック3回で [`Shape::Arc`] を確定する。
///
/// # 3点の解釈
///
/// DESIGN.md 3.4 は Arc ツールを「3点」方式とだけ定めている。ここでは
/// 「1点目=弧の始点、2点目=弧が通過する中間点（円弧の向き・張り出す側を決める）、
/// 3点目=弧の終点」という、3点円弧の一般的な解釈を採用する。
///
/// 3点から外接円（[`circumcircle`]）を求めた後、中心からの角度
/// `a1 = angle(p1)`, `a2 = angle(p2)`, `a3 = angle(p3)` を計算する。
/// `Arc` は開始角から終了角まで常に CCW（反時計回り）で掃引する型なので、
/// `start=a1, end=a3` の CCW 弧が 2 点目（`a2`）を含むかどうかで、実際に
/// 2 点目を通る側の弧を選ぶ（含まなければ `start`/`end` を入れ替えて反対側の
/// 弧を採用する。1つの円は2点の角度によって互いに補完し合う2つの弧に
/// 分割されるため、2点目を含まない側を捨てれば必ずもう一方が2点目を含む）。
///
/// 3点が同一直線上（に近い）場合は外接円が定まらないため、3点目のクリックは
/// 無視し（状態を変えず `Continue` を返す）、ユーザーに別の3点目を促す。
#[derive(Debug, Default)]
pub struct ArcTool {
    state: ArcState,
    cursor: Option<Point2>,
}

/// 3点 `p1`（始点）, `p2`（通過点）, `p3`（終点）から、`p2` を通る側の円弧を求める。
/// 3点が同一直線上に近く外接円が定まらない場合は `None`。
fn build_arc(p1: Point2, p2: Point2, p3: Point2) -> Option<Arc> {
    let circle = circumcircle(p1, p2, p3)?;
    let a1 = (p1 - circle.center).angle();
    let a2 = (p2 - circle.center).angle();
    let a3 = (p3 - circle.center).angle();
    let candidate = Arc::new(circle.center, circle.radius, a1, a3);
    if candidate.contains_angle(a2) {
        Some(candidate)
    } else {
        Some(Arc::new(circle.center, circle.radius, a3, a1))
    }
}

impl Tool for ArcTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                ArcState::WaitingP1 => {
                    self.state = ArcState::WaitingP2(p);
                    ToolResult::Continue
                }
                ArcState::WaitingP2(p1) => {
                    self.state = ArcState::WaitingP3(p1, p);
                    ToolResult::Continue
                }
                ArcState::WaitingP3(p1, p2) => match build_arc(p1, p2, p) {
                    Some(arc) => {
                        let cmd =
                            Command::AddEntity(Entity::new(Shape::Arc(arc), ctx.layer, ctx.style));
                        self.state = ArcState::WaitingP1;
                        ToolResult::Commit(cmd)
                    }
                    // 同一直線上の3点目は無視し、別の3点目のクリックを待つ。
                    None => ToolResult::Continue,
                },
            },
            InputEvent::Cancel => {
                self.state = ArcState::WaitingP1;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        match (&self.state, self.cursor) {
            (ArcState::WaitingP2(p1), Some(cursor)) => {
                draw_shape(
                    painter,
                    rect,
                    viewport,
                    &Shape::Line(LineSeg::new(*p1, cursor)),
                    preview_stroke(),
                );
            }
            (ArcState::WaitingP3(p1, p2), Some(cursor)) => {
                if let Some(arc) = build_arc(*p1, *p2, cursor) {
                    draw_shape(painter, rect, viewport, &Shape::Arc(arc), preview_stroke());
                }
            }
            _ => {}
        }
    }

    fn snap_points(&self) -> Vec<Point2> {
        match self.state {
            ArcState::WaitingP1 => Vec::new(),
            ArcState::WaitingP2(p1) => vec![p1],
            ArcState::WaitingP3(p1, p2) => vec![p1, p2],
        }
    }
}

// ---------------------------------------------------------------------
// Polyline
// ---------------------------------------------------------------------

/// ポリラインツール。クリックで頂点を追加し、Enterキーで確定する（2点以上必要）。
/// Escでキャンセル。ダブルクリックでの確定は実装しない（Enterのみ）。
///
/// # 始点クリックによる自動クローズ
///
/// 頂点が3つ以上ある状態で、クリック位置が最初の頂点とほぼ一致（距離 `<= 1e-9`）
/// すれば、その点を新しい頂点として追加せず、`closed = true` のポリラインとして
/// 即座に確定する（`Enter` を押さなくてよい）。頂点が2つ以下の場合は退化形状の
/// 自動確定を避けるため、通常どおり頂点として追加する。距離の判定はスナップで
/// 始点に吸着した場合は厳密一致になる前提なので、スナップ無効（F3 オフ）時は
/// クリックがちょうど始点に一致することは実質なく、自動クローズは発動しない
/// （これは仕様として許容する）。
#[derive(Debug, Default)]
pub struct PolylineTool {
    vertices: Vec<Point2>,
    cursor: Option<Point2>,
}

/// 自動クローズの一致判定に使う距離しきい値（ワールド単位）。スナップで始点に
/// 吸着した場合は厳密一致になるため、浮動小数点誤差を吸収できれば十分小さい値でよい。
const AUTO_CLOSE_EPSILON: f64 = 1e-9;

impl Tool for PolylineTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => {
                if self.vertices.len() >= 3 && self.vertices[0].distance(p) <= AUTO_CLOSE_EPSILON {
                    // 始点クリックで自動クローズ。始点の重複頂点は作らない。
                    let vertices = std::mem::take(&mut self.vertices);
                    return ToolResult::Commit(Command::AddEntity(Entity::new(
                        Shape::Polyline(Polyline::new(vertices, true)),
                        ctx.layer,
                        ctx.style,
                    )));
                }
                self.vertices.push(p);
                ToolResult::Continue
            }
            InputEvent::Confirm => {
                if self.vertices.len() >= 2 {
                    let vertices = std::mem::take(&mut self.vertices);
                    ToolResult::Commit(Command::AddEntity(Entity::new(
                        Shape::Polyline(Polyline::new(vertices, false)),
                        ctx.layer,
                        ctx.style,
                    )))
                } else {
                    // 2点未満では確定できない。Enter は無視して入力を続ける。
                    ToolResult::Continue
                }
            }
            InputEvent::Cancel => {
                self.vertices.clear();
                ToolResult::Cancel
            }
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if self.vertices.is_empty() {
            return;
        }
        let mut preview_vertices = self.vertices.clone();
        if let Some(cursor) = self.cursor {
            preview_vertices.push(cursor);
        }
        if preview_vertices.len() < 2 {
            return;
        }
        draw_shape(
            painter,
            rect,
            viewport,
            &Shape::Polyline(Polyline::new(preview_vertices, false)),
            preview_stroke(),
        );
    }

    fn snap_points(&self) -> Vec<Point2> {
        self.vertices.clone()
    }
}

// ---------------------------------------------------------------------
// Select / 編集（単一選択・矩形選択・移動・削除）
// ---------------------------------------------------------------------

/// 選択エンティティのピック許容量以外に、[`SelectTool`] が扱う操作の設計判断。
///
/// # なぜ [`Tool`] トレイト（[`InputEvent`]）に載せないのか
///
/// 作図ツール（Point/Line/…）は「クリック列 → 新規 1 エンティティ」という純粋な
/// 状態機械で、[`Tool::on_input`] が受け取る [`InputEvent`]（`Move`/`Click`/
/// `Confirm`/`Cancel`）と `&ToolCtx`（レイヤー・スタイル）だけで完結する。ドキュメントを
/// 一切参照しない。
///
/// 一方、選択・編集ツールは本質的に異なる:
///
/// - ヒットテスト・矩形選択は **ドキュメントの既存エンティティを読む** 必要があり、
///   ピック許容量のためにビューポートのズームも要る。`Tool::on_input` の
///   シグネチャにはどちらも無い。
/// - 「選択集合の変化」は undo/redo される **ドキュメント状態ではなく、アプリの UI 状態**
///   であって、`ToolResult`（`Continue`/`Commit(Command)`/`Cancel`）では表現できない。
/// - ドラッグ（矩形選択・移動）は始点で開始し、途中はプレビュー、離した時点で確定という
///   操作で、単発 `Click` では表せない。
///
/// これらを `Tool`/`InputEvent` へ押し込むと、作図ツール側が使わない `Document` 引数や
/// ドラッグ用バリアントを常に無視することになり、純粋な状態機械という設計を汚す。そこで
/// **`Tool` トレイトと `InputEvent` は作図ツール専用のまま一切変更せず**（既存 4 ツールと
/// 13 テストも不変）、選択・編集ツールには `&Document` を受け取る専用 API を与える。
///
/// # 選択状態の所有者
///
/// 選択集合（[`SelectTool::selection`]）はこのツールが所有し、`McadApp` はツールを
/// 永続フィールドとして保持する。選択はアプリ UI 状態なのでドキュメント履歴には積まない。
/// ハイライト描画・Delete は `McadApp` がこのツールの選択集合を読んで行う。
///
/// # クリックとドラッグの区別
///
/// 「動かなければ選択、動けば矩形選択」の閾値判定は **egui 組み込みのクリック/ドラッグ
/// 判定に委ねる**（`Response::clicked` と `drag_started`/`dragged`/`drag_stopped` は
/// egui 内部のドラッグ閾値で排他的に分岐する）。`McadApp` 側がこれらを対応する
/// メソッド呼び出しへ振り分けるため、本ツールにピクセル閾値を持たせる必要はない。
/// ドラッグは常に矩形選択であり、選択物の上から始めても移動にはならない（移動は
/// [`PlacementKind::Move`] の2クリック配置へ移管した。[`Placement`] の doc を参照）。
///
/// # 矩形選択の判定基準（完全内包）
///
/// ドラッグ矩形に **AABB が完全に含まれる** エンティティを選ぶ（交差ではなく内包）。
/// 理由: AABB が矩形に内包されていれば実形状も必ず内包されるため、この基準は
/// AABB のみで **厳密**（偽陽性なし）。AABB 交差方式だと、大きな円の外接ボックスが
/// 矩形に触れるだけで実際には弧が矩形外、という過剰選択が起こりうる。予測しやすさと
/// 厳密性を優先して内包方式を採る（左→右/右→左の窓・交差の区別は MVP では設けない）。
///
/// # ロックレイヤーが選択に混ざった場合
///
/// ロックされたレイヤーのエンティティも（可視なら）選択自体は許す。移動・削除は
/// 選択集合全体を 1 つの [`Command::Batch`] にまとめて `Document::apply` へ渡すため、
/// 混在時は既存のバッチ原子性（1 つでも失敗＝全体ロールバック）に従い操作全体が
/// [`mcad_core::CoreError::LayerLocked`] で失敗し、何も動かない/消えない。これは
/// 「一部だけ動かす」より一貫性が高く、`Batch` の設計意図どおり。`McadApp` は
/// `apply` の `Err` をステータスバーへ表示する。
/// 進行中の矩形選択ドラッグ（`start` から `current` までのドラッグ矩形）。移動は
/// [`PlacementKind::Move`] の2クリック配置へ移管したため、ドラッグは矩形選択専用。
#[derive(Debug, Clone, Copy, PartialEq)]
struct DragState {
    start: Point2,
    current: Point2,
}

/// ドラッグ中のプレビュー描画に必要な情報（`McadApp` の描画層向けの公開ビュー）。
/// ドラッグは矩形選択専用なので矩形プレビューのみを表す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragPreview {
    /// 矩形選択のプレビュー（2 隅のワールド座標）。
    Rect { start: Point2, current: Point2 },
}

// ---------------------------------------------------------------------
// 配置モード（選択集合に対する「2段階クリック」操作）
// ---------------------------------------------------------------------

/// Select 中に動く「基準点→確定点」の2段階クリック操作の種別。
///
/// # なぜ [`DragState`] と別立てにするのか
///
/// 矩形選択は「ドラッグ 1 ストローク」で完結するが、複製配置（Ctrl+D）や移動（M）は
/// **クリック→カーソル追従プレビュー→クリック** という 2 発のクリックにまたがる。
/// ドラッグと同時には起きず、進行中は通常のクリック選択・矩形選択を止める
/// （入力ゲート）。この構造は M5 タスク19 の回転（基準点→角度参照点）・ミラー
/// （軸2点）とまったく同じなので、[`PlacementKind`] にバリアントを足すだけで
/// [`SelectTool::placement_click`] の分岐と本ステート機械をそのまま流用できる。
///
/// 移動（[`PlacementKind::Move`]）も複製と同じ2クリック配置に統一している。掴み判定の
/// 厳しいドラッグ移動をやめ、移動にも基準点・配置先の両クリックでスナップを効かせる
/// （2026-07-19、設計判断2 の追記）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementKind {
    /// 複製配置（Ctrl+D）。基準点→配置先の変位ぶん平行移動した複製を追加する。
    Duplicate,
    /// 移動配置（M）。基準点→配置先の変位ぶん、選択集合の幾何を平行移動する。
    Move,
}

/// 2段階クリックの進行段階。
#[derive(Debug, Clone, Copy, PartialEq)]
enum PlacementStage {
    /// 1クリック目（基準点）待ち。プレビューはまだ描かない。
    WaitingBase,
    /// 2クリック目（確定点）待ち。`base` は確定済み、`cursor` でプレビュー追従する。
    WaitingTarget { base: Point2, cursor: Point2 },
}

/// 進行中の配置モード（種別＋段階）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    kind: PlacementKind,
    stage: PlacementStage,
}

/// [`SelectTool::placement_click`] の結果。呼び出し側（`McadApp`）はこれを見て
/// ステータス表示・`Document::apply`・選択更新を行う。
#[derive(Debug, Clone, PartialEq)]
pub enum PlacementOutcome {
    /// まだ確定しない（基準点を確定した等）。呼び出し側は何もしない。
    Continue,
    /// 変位ゼロ等でモードをキャンセルした。ASCII メッセージを表示する。
    Cancelled(&'static str),
    /// 確定コマンド。呼び出し側が `Document::apply` したうえで、`kind` に応じた後処理を
    /// 行う（複製は返る `NewIds.entities` を [`SelectTool::set_selection`] で新選択にし、
    /// 移動は ID 不変なので選択をそのまま維持する）。`kind` はステータス文言の出し分けにも使う。
    Commit {
        /// どの配置操作の確定か（複製／移動）。
        kind: PlacementKind,
        /// `Document::apply` に渡す確定コマンド。
        cmd: Command,
    },
}

/// 配置モードのプレビュー描画情報（`McadApp` の描画層向けの公開ビュー）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlacementPreview {
    /// 複製プレビュー（選択集合をこの変位ぶん平行移動した位置に仮表示する）。
    Duplicate { delta: Vec2 },
    /// 移動プレビュー（選択集合をこの変位ぶん平行移動した先を仮表示する）。
    Move { delta: Vec2 },
}

/// 選択・編集ツール。単一選択（クリック）・矩形選択（ドラッグ）・移動（M、2クリック
/// 配置）・複製（Ctrl+D、2クリック配置）・削除（Delete/Backspace）を担う。設計判断の
/// 詳細は [`DragState`] と [`Placement`] の doc を参照。
#[derive(Debug, Default)]
pub struct SelectTool {
    /// 現在の選択集合（アプリ UI 状態。ドキュメント履歴には積まない）。
    selection: Vec<EntityId>,
    /// 進行中の矩形選択ドラッグ。無ければ `None`。
    drag: Option<DragState>,
    /// 進行中の配置モード（複製・移動など2段階クリック）。無ければ `None`。
    /// アクティブな間は通常のクリック選択・矩形選択より優先される（入力ゲート）。
    placement: Option<Placement>,
}

/// エンティティの所属レイヤーが可視か（非表示レイヤーは描画・ヒットテスト対象外）。
fn layer_visible(document: &Document, entity: &Entity) -> bool {
    document.layer(entity.layer).is_some_and(|l| l.visible)
}

impl SelectTool {
    /// 現在の選択集合。
    #[must_use]
    pub fn selection(&self) -> &[EntityId] {
        &self.selection
    }

    /// 選択を空にする。
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// ドキュメントに存在しなくなったエンティティを選択集合から取り除く。
    ///
    /// undo/redo はエンティティを削除・復活させるが、選択集合はアプリ UI 状態なので
    /// 履歴の巻き戻しに追随しない。死んだ ID が選択に残ると、削除
    /// （[`SelectTool::delete_command`]）が `EntityNotFound` でバッチごと原子的に
    /// 失敗し続けるため、undo/redo の直後に呼んで選択を浄化する。
    pub fn retain_alive(&mut self, document: &Document) {
        self.selection.retain(|&id| document.entity(id).is_some());
    }

    /// ドラッグ中のプレビュー情報（矩形選択枠）。ドラッグしていなければ `None`。
    #[must_use]
    pub fn drag_preview(&self) -> Option<DragPreview> {
        self.drag
            .map(|DragState { start, current }| DragPreview::Rect { start, current })
    }

    /// クリック位置 `world` から許容量 `tol`（ワールド単位）以内で最も近い可視
    /// エンティティを返す。該当なしなら `None`。
    fn pick(&self, document: &Document, world: Point2, tol: f64) -> Option<EntityId> {
        let mut best: Option<(f64, EntityId)> = None;
        for (id, entity) in document.entities() {
            if !layer_visible(document, entity) {
                continue;
            }
            let d = distance_to(&entity.geom, world);
            if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// 単一クリック（ドラッグを伴わない）。累積方式（AutoCAD 流）:
    ///
    /// - ヒットあり・`shift` なし: 選択に**追加**する（既に選択済みなら何もしない。
    ///   重複 ID を作らない）。
    /// - ヒットあり・`shift` あり: そのエンティティだけ選択から**除去**する
    ///   （未選択なら何もしない）。
    /// - ヒットなし・`shift` なし: **何もしない**（誤クリックで大きな選択集合を
    ///   一瞬で失わないため）。全解除したいときは `shift` を押すか `Esc`
    ///   （[`SelectTool::on_cancel`]）を使う。
    /// - ヒットなし・`shift` あり: 選択を**全解除**する。
    pub fn on_click(&mut self, document: &Document, world: Point2, tol: f64, shift: bool) {
        match (self.pick(document, world, tol), shift) {
            (Some(id), false) => {
                if !self.selection.contains(&id) {
                    self.selection.push(id);
                }
            }
            (Some(id), true) => self.selection.retain(|&sel| sel != id),
            (None, false) => {}
            (None, true) => self.selection.clear(),
        }
    }

    /// 矩形選択のドラッグを開始する。選択物の上から始めても常に矩形選択になる
    /// （移動は [`SelectTool::start_move`] の2クリック配置へ移管した）。
    pub fn on_drag_start(&mut self, world: Point2) {
        self.drag = Some(DragState {
            start: world,
            current: world,
        });
    }

    /// ドラッグ中の現在位置を更新する（プレビュー用）。ドラッグしていなければ無視。
    pub fn on_drag(&mut self, world: Point2) {
        if let Some(DragState { current, .. }) = &mut self.drag {
            *current = world;
        }
    }

    /// 矩形選択ドラッグの確定。ドラッグ矩形に **完全内包** される可視エンティティを
    /// 新しい選択集合にする（内包判定の理由は [`DragState`] の doc を参照）。ドラッグ中で
    /// なければ何もしない。矩形選択は選択集合を書き換えるだけで Document は変更しない。
    pub fn on_drag_end(&mut self, document: &Document, world: Point2) {
        let Some(DragState { start, .. }) = self.drag.take() else {
            return;
        };
        let rect = Aabb::from_corners(start, world);
        self.selection = document
            .entities()
            .filter(|(_, e)| layer_visible(document, e))
            .filter(|(_, e)| rect.contains(&e.geom.aabb()))
            .map(|(id, _)| id)
            .collect();
    }

    /// Esc の2段階挙動: 進行中の矩形選択ドラッグがあればそれだけを破棄し
    /// （選択集合は変えない）、ドラッグが無ければ選択を全解除する。
    ///
    /// 「ドラッグ中断」と「選択解除」を1つのショートカットに重ねているのは、
    /// ドラッグ中の Esc がそのまま選択まで消してしまうと事故になりやすい一方、
    /// 何もしていない状態の Esc は「選択を諦める」操作として自然に使われるため。
    /// 配置モード（複製・移動）中の Esc は呼び出し側で別経路として先に処理され、
    /// ここには来ない。
    pub fn on_cancel(&mut self) {
        if self.drag.is_some() {
            self.drag = None;
        } else {
            self.selection.clear();
        }
    }

    /// 現在の選択を削除する [`Command`]。選択が空なら `None`。
    ///
    /// 単一選択でも一貫して [`Command::Batch`] にまとめる（undo/redo が常に 1 単位に
    /// なり、複数削除と挙動が揃う）。
    #[must_use]
    pub fn delete_command(&self) -> Option<Command> {
        if self.selection.is_empty() {
            return None;
        }
        Some(Command::Batch(
            self.selection
                .iter()
                .map(|&id| Command::RemoveEntity(id))
                .collect(),
        ))
    }

    /// 選択集合を丸ごと置き換える。複製確定後に `NewIds.entities`（コマンド順）を
    /// 新しい選択にする用途。イテレータ比較で ID を推測せず、コア側が返した ID を使う。
    pub fn set_selection(&mut self, ids: Vec<EntityId>) {
        self.selection = ids;
    }

    // --- 配置モード（2段階クリック） ---

    /// 配置モードが進行中か。`McadApp` はこれで通常のクリック選択・矩形選択を
    /// ゲートし、配置モード中は配置用の入力経路だけを通す。
    #[must_use]
    pub fn is_placing(&self) -> bool {
        self.placement.is_some()
    }

    /// `kind` の配置モードを開始する共通処理。選択集合が空なら何もせず `false`
    /// （呼び出し側は ASCII ステータスメッセージを出す）。非空なら基準点クリック待ちに
    /// 入って `true`。進行中のドラッグは配置と両立しないため破棄する。
    fn start_placement(&mut self, kind: PlacementKind) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.drag = None;
        self.placement = Some(Placement {
            kind,
            stage: PlacementStage::WaitingBase,
        });
        true
    }

    /// Ctrl+D: 複製配置モードを開始する（[`SelectTool::start_placement`] 参照）。
    pub fn start_duplicate(&mut self) -> bool {
        self.start_placement(PlacementKind::Duplicate)
    }

    /// M: 移動配置モードを開始する（[`SelectTool::start_placement`] 参照）。基準点→配置先の
    /// 2クリックで選択集合を平行移動する。ドラッグ移動を置き換え、移動にもスナップが効く。
    pub fn start_move(&mut self) -> bool {
        self.start_placement(PlacementKind::Move)
    }

    /// 配置モードのカーソル位置を更新する（プレビュー追従用）。基準点クリック前
    /// （`WaitingBase`）や非配置中は何もしない。
    pub fn placement_move(&mut self, world: Point2) {
        if let Some(Placement {
            stage: PlacementStage::WaitingTarget { cursor, .. },
            ..
        }) = &mut self.placement
        {
            *cursor = world;
        }
    }

    /// 配置モードでのクリック確定を1つ進める。
    ///
    /// - 1クリック目（`WaitingBase`）: `world` を基準点として確定し、配置先待ちへ。
    /// - 2クリック目（`WaitingTarget`）: `delta = world - base` を変位として確定する。
    ///   基準点と配置先がほぼ同一（`|delta| <= tol`、ピック許容量基準）なら、見えない
    ///   重複コピーを防ぐためモードをキャンセルする。
    ///
    /// 確定コマンドは種別による:
    /// - 複製（[`PlacementKind::Duplicate`]）: 選択中の各エンティティのレイヤー・スタイルを
    ///   複製し、幾何のみ `translated(delta)` に置き換えた `AddEntity` の [`Command::Batch`]。
    /// - 移動（[`PlacementKind::Move`]）: 選択中の各エンティティの幾何を `translated(delta)`
    ///   に置き換える `ModifyEntity` の [`Command::Batch`]（ID は不変）。
    ///
    /// 呼び出し側が `Document::apply` する（レイヤーロック時は Batch 原子性で全体が失敗し、
    /// 位置も選択も変わらない）。
    #[must_use]
    pub fn placement_click(
        &mut self,
        document: &Document,
        world: Point2,
        tol: f64,
    ) -> PlacementOutcome {
        // Copy 型なので取り出しても self を占有しない（下で self を書き換えられる）。
        let Some(placement) = self.placement else {
            return PlacementOutcome::Continue;
        };
        match placement.stage {
            PlacementStage::WaitingBase => {
                self.placement = Some(Placement {
                    kind: placement.kind,
                    stage: PlacementStage::WaitingTarget {
                        base: world,
                        cursor: world,
                    },
                });
                PlacementOutcome::Continue
            }
            PlacementStage::WaitingTarget { base, .. } => {
                let delta = world - base;
                if delta.length() <= tol {
                    self.placement = None;
                    return PlacementOutcome::Cancelled(match placement.kind {
                        PlacementKind::Duplicate => "Zero displacement - duplicate cancelled",
                        PlacementKind::Move => "Zero displacement - move cancelled",
                    });
                }
                let cmd = match placement.kind {
                    PlacementKind::Duplicate => self.build_duplicate_command(document, delta),
                    PlacementKind::Move => self.build_move_command(document, delta),
                };
                self.placement = None;
                match cmd {
                    Some(cmd) => PlacementOutcome::Commit {
                        kind: placement.kind,
                        cmd,
                    },
                    // 選択が全て死んだ ID だった等でコマンドが空。静かにキャンセルする。
                    None => PlacementOutcome::Cancelled(match placement.kind {
                        PlacementKind::Duplicate => "Nothing to duplicate",
                        PlacementKind::Move => "Nothing to move",
                    }),
                }
            }
        }
    }

    /// 配置モードを解除する（Esc・ツール切替・ファイル操作・モーダル表示から呼ぶ）。
    /// Document は一切変更しない。選択集合も変えない。
    pub fn cancel_placement(&mut self) {
        self.placement = None;
    }

    /// 配置モードのプレビュー情報。配置先待ちでなければ `None`。
    #[must_use]
    pub fn placement_preview(&self) -> Option<PlacementPreview> {
        let Some(Placement {
            kind,
            stage: PlacementStage::WaitingTarget { base, cursor },
        }) = self.placement
        else {
            return None;
        };
        let delta = cursor - base;
        Some(match kind {
            PlacementKind::Duplicate => PlacementPreview::Duplicate { delta },
            PlacementKind::Move => PlacementPreview::Move { delta },
        })
    }

    /// 選択中の各エンティティを `delta` 平行移動した複製の `AddEntity` を 1 バッチに
    /// まとめる。レイヤー・スタイルは元エンティティを丸ごとクローンして保持し、幾何のみ
    /// 差し替える。生存している選択物が無ければ `None`。
    fn build_duplicate_command(&self, document: &Document, delta: Vec2) -> Option<Command> {
        let subs: Vec<Command> = self
            .selection
            .iter()
            .filter_map(|&id| {
                document.entity(id).map(|e| {
                    let mut copy = e.clone();
                    copy.geom = copy.geom.translated(delta);
                    Command::AddEntity(copy)
                })
            })
            .collect();
        if subs.is_empty() {
            None
        } else {
            Some(Command::Batch(subs))
        }
    }

    /// 選択中の各エンティティの幾何を `delta` 平行移動する `ModifyEntity` を 1 バッチに
    /// まとめる（ID は不変）。生存している選択物が無ければ `None`。複製の
    /// [`SelectTool::build_duplicate_command`] と対になる移動版。
    fn build_move_command(&self, document: &Document, delta: Vec2) -> Option<Command> {
        let subs: Vec<Command> = self
            .selection
            .iter()
            .filter_map(|&id| {
                document.entity(id).map(|e| Command::ModifyEntity {
                    id,
                    new_geom: e.geom.translated(delta),
                })
            })
            .collect();
        if subs.is_empty() {
            None
        } else {
            Some(Command::Batch(subs))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::Document;

    fn ctx() -> (Document, ToolCtx) {
        let doc = Document::new();
        let ctx = ToolCtx {
            layer: doc.current_layer(),
            style: Style::inherited(),
        };
        (doc, ctx)
    }

    fn shape_of(result: ToolResult) -> Shape {
        match result {
            ToolResult::Commit(Command::AddEntity(entity)) => entity.geom,
            other => panic!("expected Commit(AddEntity(..)), got {other:?}"),
        }
    }

    // --- Point ---

    #[test]
    fn point_tool_commits_on_single_click() {
        let (_doc, ctx) = ctx();
        let mut tool = PointTool::default();
        let result = tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 2.0)));
        assert_eq!(shape_of(result), Shape::Point(Point2::new(1.0, 2.0)));
    }

    #[test]
    fn point_tool_cancel_returns_cancel() {
        let (_doc, ctx) = ctx();
        let mut tool = PointTool::default();
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
    }

    // --- Line ---

    #[test]
    fn line_tool_needs_two_clicks_to_commit() {
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(3.0, 4.0);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(a)),
            ToolResult::Continue
        );
        let result = tool.on_input(&ctx, InputEvent::Click(b));
        assert_eq!(shape_of(result), Shape::Line(LineSeg::new(a, b)));
    }

    #[test]
    fn line_tool_chains_next_segment_from_previous_endpoint() {
        // 連続線分モード: 1本目確定後、追加クリックなしで次のクリックが即座に
        // 前の終点始まりの線分として確定する（2本目に2クリック不要）。
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(5.0, 5.0);
        tool.on_input(&ctx, InputEvent::Click(a));
        let result_ab = tool.on_input(&ctx, InputEvent::Click(b));
        assert_eq!(shape_of(result_ab), Shape::Line(LineSeg::new(a, b)));

        // 追加クリックなしで C をクリックすると B→C の線分が即座に確定する。
        let result_bc = tool.on_input(&ctx, InputEvent::Click(c));
        assert_eq!(shape_of(result_bc), Shape::Line(LineSeg::new(b, c)));
    }

    #[test]
    fn line_tool_cancel_discards_first_point() {
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        // キャンセル後は新しい始点として扱われる（2クリック必要）。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0))),
            ToolResult::Continue
        );
    }

    #[test]
    fn line_tool_cancel_ends_continuous_mode() {
        // 連続線分モード中に Esc を押すと Cancel が返り、状態が WaitingFirst に
        // 戻る（次のクリックは新しい始点として扱われ、2クリック必要になる）。
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0))),
            ToolResult::Continue
        );
        let result = tool.on_input(&ctx, InputEvent::Click(Point2::new(10.0, 9.0)));
        assert_eq!(
            shape_of(result),
            Shape::Line(LineSeg::new(Point2::new(9.0, 9.0), Point2::new(10.0, 9.0)))
        );
    }

    // --- Circle ---

    #[test]
    fn circle_tool_commits_with_center_and_radius_point() {
        let (_doc, ctx) = ctx();
        let mut tool = CircleTool::default();
        let center = Point2::new(0.0, 0.0);
        let edge = Point2::new(3.0, 4.0);
        tool.on_input(&ctx, InputEvent::Click(center));
        let result = tool.on_input(&ctx, InputEvent::Click(edge));
        match shape_of(result) {
            Shape::Circle(c) => {
                assert_eq!(c.center, center);
                assert!((c.radius - 5.0).abs() < 1e-9);
            }
            other => panic!("expected Circle, got {other:?}"),
        }
    }

    // --- Arc ---

    #[test]
    fn arc_tool_commits_after_three_clicks_on_unit_circle() {
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        // 単位円上の3点: 角度 0, π/2, π。始点(1,0)→中間点(0,1)→終点(-1,0)。
        let p1 = Point2::new(1.0, 0.0);
        let p2 = Point2::new(0.0, 1.0);
        let p3 = Point2::new(-1.0, 0.0);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(p1)),
            ToolResult::Continue
        );
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(p2)),
            ToolResult::Continue
        );
        let result = tool.on_input(&ctx, InputEvent::Click(p3));
        match shape_of(result) {
            Shape::Arc(arc) => {
                assert!(arc.center.distance(Point2::ORIGIN) < 1e-9);
                assert!((arc.radius - 1.0).abs() < 1e-9);
                // 中間点(0,1)、すなわち角度 π/2 は弧の範囲内でなければならない。
                assert!(arc.contains_angle(std::f64::consts::FRAC_PI_2));
                // 始点・終点も弧上（角度が範囲内）。
                assert!(arc.contains_angle(0.0));
                assert!(arc.contains_angle(std::f64::consts::PI));
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }

    #[test]
    fn arc_tool_picks_the_side_containing_midpoint() {
        // 中間点を反対側（下半分, 角度 -π/2 = 3π/2）にすると、そちら側を通る弧を選ぶはず。
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        let p1 = Point2::new(1.0, 0.0);
        let p2 = Point2::new(0.0, -1.0);
        let p3 = Point2::new(-1.0, 0.0);
        tool.on_input(&ctx, InputEvent::Click(p1));
        tool.on_input(&ctx, InputEvent::Click(p2));
        let result = tool.on_input(&ctx, InputEvent::Click(p3));
        match shape_of(result) {
            Shape::Arc(arc) => {
                assert!(arc.contains_angle(3.0 * std::f64::consts::FRAC_PI_2));
                // 反対側（上半分, π/2）は含まれないはず。
                assert!(!arc.contains_angle(std::f64::consts::FRAC_PI_2));
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }

    #[test]
    fn arc_tool_ignores_collinear_third_point() {
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        // 同一直線上の3点目は無視され、Continue のまま3点目待ちが継続する。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(2.0, 0.0))),
            ToolResult::Continue
        );
        // 有効な3点目を与えれば、そこで確定できる。
        let result = tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 1.0)));
        assert!(matches!(result, ToolResult::Commit(_)));
    }

    #[test]
    fn arc_tool_cancel_discards_progress() {
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        // キャンセル後は最初の点として扱われる（3クリック必要）。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0))),
            ToolResult::Continue
        );
    }

    // --- Polyline ---

    #[test]
    fn polyline_tool_requires_enter_to_commit() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        let pts = [
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
        ];
        for p in pts {
            assert_eq!(
                tool.on_input(&ctx, InputEvent::Click(p)),
                ToolResult::Continue
            );
        }
        let result = tool.on_input(&ctx, InputEvent::Confirm);
        assert_eq!(
            shape_of(result),
            Shape::Polyline(Polyline::new(pts.to_vec(), false))
        );
    }

    #[test]
    fn polyline_tool_confirm_with_single_point_is_ignored() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        // 1点だけでは Enter しても確定しない（2点以上必要）。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Confirm),
            ToolResult::Continue
        );
    }

    #[test]
    fn polyline_tool_cancel_discards_all_vertices() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        // キャンセル後は新しいポリラインとして扱われる（1点だけでは確定不可）。
        tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0)));
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Confirm),
            ToolResult::Continue
        );
    }

    #[test]
    fn polyline_tool_click_on_start_point_with_three_vertices_auto_closes() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(1.0, 1.0);
        tool.on_input(&ctx, InputEvent::Click(a));
        tool.on_input(&ctx, InputEvent::Click(b));
        tool.on_input(&ctx, InputEvent::Click(c));
        // 4回目のクリックが始点 a とちょうど同座標（スナップで吸着した想定）。
        let result = tool.on_input(&ctx, InputEvent::Click(a));
        match shape_of(result) {
            Shape::Polyline(pl) => {
                assert!(pl.closed, "始点クリックで自動クローズするはず");
                // 始点の重複頂点は作らない（頂点数は3のまま）。
                assert_eq!(pl.vertices, vec![a, b, c]);
            }
            other => panic!("expected Polyline, got {other:?}"),
        }
    }

    #[test]
    fn polyline_tool_click_on_start_point_with_two_vertices_adds_vertex() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        tool.on_input(&ctx, InputEvent::Click(a));
        tool.on_input(&ctx, InputEvent::Click(b));
        // 頂点2つ（3未満）では自動クローズせず、始点と同座標でも頂点として追加する。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(a)),
            ToolResult::Continue
        );
        assert_eq!(tool.snap_points(), vec![a, b, a]);
    }

    #[test]
    fn polyline_tool_snap_points_reflects_clicked_vertices() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        assert_eq!(tool.snap_points(), Vec::<Point2>::new());
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        tool.on_input(&ctx, InputEvent::Click(a));
        assert_eq!(tool.snap_points(), vec![a]);
        tool.on_input(&ctx, InputEvent::Click(b));
        assert_eq!(tool.snap_points(), vec![a, b]);
    }

    #[test]
    fn line_tool_snap_points_reflects_first_click() {
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        assert_eq!(tool.snap_points(), Vec::<Point2>::new());
        let a = Point2::new(2.0, 3.0);
        tool.on_input(&ctx, InputEvent::Click(a));
        assert_eq!(tool.snap_points(), vec![a]);
    }

    // --- Select / 編集 ---

    /// カレントレイヤー上に線分 `(x,0)-(x+1,0)`（x 軸に平行な水平線分）を追加し、
    /// その [`EntityId`] を返す。
    fn add_hline(doc: &mut Document, x: f64) -> mcad_core::EntityId {
        let layer = doc.current_layer();
        let entity = Entity::new(
            Shape::Line(LineSeg::new(Point2::new(x, 0.0), Point2::new(x + 1.0, 0.0))),
            layer,
            Style::inherited(),
        );
        let new_ids = doc.apply(Command::AddEntity(entity)).unwrap();
        new_ids.entities[0]
    }

    #[test]
    fn click_selects_nearest_within_tolerance() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0); // 線分 (0,0)-(1,0)
        let b = add_hline(&mut doc, 10.0); // 線分 (10,0)-(11,0)

        // (0.5, 0.05) は a に非常に近く、b からは遠い。許容量 0.1 以内で a を選ぶ。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.05), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // b の近くをクリックすれば（別インスタンスで）b を選ぶ。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(10.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[b]);
    }

    #[test]
    fn click_accumulates_selection_without_duplicates() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();

        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // 別のエンティティをクリックすると選択に追加される（置き換わらない）。
        tool.on_click(&doc, Point2::new(10.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a, b]);

        // 既に選択済みの a を再クリックしても重複せず選択は変わらない。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a, b]);
    }

    #[test]
    fn shift_click_deselects_only_target() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        tool.on_click(&doc, Point2::new(10.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a, b]);

        // Shift+クリックで a だけ選択から外れ、b は残る。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, true);
        assert_eq!(tool.selection(), &[b]);
    }

    #[test]
    fn shift_click_on_unselected_entity_does_nothing() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let _b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // b は未選択なので Shift+クリックしても何も変わらない。
        tool.on_click(&doc, Point2::new(10.5, 0.0), 0.1, true);
        assert_eq!(tool.selection(), &[a]);
    }

    #[test]
    fn empty_click_without_shift_keeps_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // 何もない場所（許容量外）を Shift なしでクリックしても選択は変わらない
        // （誤クリックで大きな選択集合を一瞬で失わないため）。
        tool.on_click(&doc, Point2::new(0.5, 100.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);
    }

    #[test]
    fn shift_empty_click_clears_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // 何もない場所を Shift+クリックすると選択が全解除される。
        tool.on_click(&doc, Point2::new(0.5, 100.0), 0.1, true);
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn click_picks_the_closest_of_multiple_candidates() {
        let mut doc = Document::new();
        let near = add_hline(&mut doc, 0.0); // (0,0)-(1,0)
        let _far = add_hline(&mut doc, 0.0); // 同座標の別線分…
        // near のほうがクリック点に近くなるよう、2 本目を少し上へずらして作り直す。
        let layer = doc.current_layer();
        let above = doc
            .apply(Command::AddEntity(Entity::new(
                Shape::Line(LineSeg::new(Point2::new(0.0, 0.3), Point2::new(1.0, 0.3))),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];

        let mut tool = SelectTool::default();
        // (0.5, 0.05): near（y=0）まで 0.05、above（y=0.3）まで 0.25。許容量 1.0 で near。
        tool.on_click(&doc, Point2::new(0.5, 0.05), 1.0, false);
        assert_eq!(tool.selection(), &[near]);
        assert_ne!(tool.selection(), &[above]);
    }

    #[test]
    fn hidden_layer_entities_are_not_selectable() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        // カレント（デフォルト）レイヤーを非表示にする。
        let layer_id = doc.current_layer();
        let mut props = doc.layer(layer_id).unwrap().clone();
        props.visible = false;
        doc.apply(Command::SetLayerProps {
            id: layer_id,
            props,
        })
        .unwrap();

        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.selection().is_empty(), "非表示レイヤーは選択できない");
        let _ = a;
    }

    #[test]
    fn rectangle_selection_uses_full_containment() {
        let mut doc = Document::new();
        let inside = add_hline(&mut doc, 0.0); // AABB [0,0]-[1,0]
        let _outside = add_hline(&mut doc, 100.0); // AABB [100,0]-[101,0]
        let mut tool = SelectTool::default();

        // ドラッグ矩形 (-1,-1)→(5,5) は inside を完全内包し outside は含まない。
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag(Point2::new(5.0, 5.0));
        tool.on_drag_end(&doc, Point2::new(5.0, 5.0));
        assert_eq!(tool.selection(), &[inside]);
    }

    #[test]
    fn rectangle_partially_covering_does_not_select() {
        let mut doc = Document::new();
        let _line = add_hline(&mut doc, 0.0); // AABB [0,0]-[1,0]
        let mut tool = SelectTool::default();

        // 矩形 (-1,-1)→(0.5,1) は線分の右半分を覆うが完全には内包しない → 非選択。
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(0.5, 1.0));
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn drag_on_selected_entity_still_starts_rectangle() {
        // 移動はドラッグではなく M の2クリック配置へ移管したので、選択物の上から
        // ドラッグしても移動にはならず、常に矩形選択になる（選択集合を置き換える）。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();

        // まず a を単一選択しておく。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // 選択済みの a の上（0.5,0）からドラッグ開始しても矩形選択になる。
        tool.on_drag_start(Point2::new(0.5, 0.0));
        // プレビューは矩形選択（移動プレビューではない）。
        assert!(matches!(
            tool.drag_preview(),
            Some(DragPreview::Rect { .. })
        ));
        tool.on_drag(Point2::new(12.0, 1.0));
        // 矩形 (0.5,0)→(12,1) は b を内包するが a は内包しない（a は x=0..1）。
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));

        // 矩形選択として働き、選択は a から b へ置き換わる。a は一切動いていない。
        assert_eq!(tool.selection(), &[b]);
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
        assert_eq!(doc.entity_count(), 2);
    }

    #[test]
    fn drag_on_empty_space_starts_rectangle_even_with_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let _b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();

        // a を選択済みにしておく。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // 何もない場所 (50,50) からドラッグ開始 → 矩形選択。
        tool.on_drag_start(Point2::new(50.0, 50.0));
        // 矩形が何も内包しなければ選択は空になる（矩形選択は集合を置き換える）。
        tool.on_drag_end(&doc, Point2::new(60.0, 60.0));
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn escape_cancels_in_progress_drag_without_changing_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // 矩形選択ドラッグを始めてから Esc（on_cancel）。
        tool.on_drag_start(Point2::new(0.5, 0.0));
        tool.on_drag(Point2::new(0.5, 9.0));
        assert!(tool.drag_preview().is_some());
        tool.on_cancel();

        // ドラッグは破棄され、選択は変わらない。
        assert!(tool.drag_preview().is_none());
        assert_eq!(tool.selection(), &[a]);
        // キャンセル後に離しても選択は変わらない（ドラッグ状態は無い）。
        tool.on_drag_end(&doc, Point2::new(0.5, 9.0));
        assert_eq!(tool.selection(), &[a]);
    }

    #[test]
    fn escape_without_drag_clears_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        // ドラッグが進行中でないときの Esc は選択を全解除する（2段階挙動）。
        assert!(tool.drag_preview().is_none());
        tool.on_cancel();
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn delete_builds_batch_remove_of_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();

        // 空選択なら削除コマンドは無い。
        assert_eq!(tool.delete_command(), None);

        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));
        assert_eq!(tool.selection().len(), 2);

        match tool
            .delete_command()
            .expect("選択があれば削除コマンドがある")
        {
            Command::Batch(subs) => {
                assert_eq!(subs.len(), 2);
                assert!(subs.contains(&Command::RemoveEntity(a)));
                assert!(subs.contains(&Command::RemoveEntity(b)));
            }
            other => panic!("expected Batch, got {other:?}"),
        }
    }

    #[test]
    fn delete_then_apply_and_undo_roundtrip() {
        // 検収シナリオ: 選択 → 削除 → undo で 1 単位復元。
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));
        assert_eq!(tool.selection().len(), 2);

        let cmd = tool.delete_command().unwrap();
        doc.apply(cmd).unwrap();
        assert_eq!(doc.entity_count(), 0);

        // Batch なので undo 1 回で両方復元。
        assert!(doc.undo());
        assert_eq!(doc.entity_count(), 2);
    }

    #[test]
    fn move_then_apply_moves_all_and_undo_is_single_unit() {
        // 検収シナリオ: 矩形選択 → M の2クリック配置で移動 → undo 1 回で全戻り。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let before_b = doc.entity(b).unwrap().geom.clone();

        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));
        assert_eq!(tool.selection().len(), 2);
        let selection_before = tool.selection().to_vec();

        assert!(tool.start_move());
        // 1クリック目=基準点。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        // 2クリック目=配置先。基準点から (0,7) の変位で ModifyEntity の Batch を返す。
        let outcome = tool.placement_click(&doc, Point2::new(0.0, 7.0), 0.1);
        let PlacementOutcome::Commit { kind, cmd } = outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        assert_eq!(kind, PlacementKind::Move);
        // Batch は各選択物の ModifyEntity のみ（ID 不変、new_geom は translated）。
        match &cmd {
            Command::Batch(subs) => {
                assert_eq!(subs.len(), 2);
                let delta = Vec2::new(0.0, 7.0);
                let expect = |id: mcad_core::EntityId| Command::ModifyEntity {
                    id,
                    new_geom: doc.entity(id).unwrap().geom.translated(delta),
                };
                assert!(subs.contains(&expect(a)));
                assert!(subs.contains(&expect(b)));
            }
            other => panic!("expected Batch, got {other:?}"),
        }
        doc.apply(cmd).unwrap();

        let delta = Vec2::new(0.0, 7.0);
        assert_eq!(doc.entity(a).unwrap().geom, before_a.translated(delta));
        assert_eq!(doc.entity(b).unwrap().geom, before_b.translated(delta));
        // 移動は ID 不変なので選択は維持される（呼び出し側も再選択しない）。
        assert_eq!(tool.selection(), selection_before.as_slice());

        // 1 バッチなので undo 1 回で両方元へ戻る。
        assert!(doc.undo());
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
        assert_eq!(doc.entity(b).unwrap().geom, before_b);
    }

    #[test]
    fn move_touching_locked_layer_fails_atomically() {
        // ロックされたレイヤーのエンティティが混ざると、Batch 原子性により
        // 移動全体が失敗し、どのエンティティも動かない。選択も不変。
        let mut doc = Document::new();
        let unlocked = add_hline(&mut doc, 0.0);

        // 別レイヤーを作り、そこに 1 本追加してからロックする。
        let locked_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "locked",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let locked_entity = doc
            .apply(Command::AddEntity(Entity::new(
                Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
                locked_layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let mut props = doc.layer(locked_layer).unwrap().clone();
        props.locked = true;
        doc.apply(Command::SetLayerProps {
            id: locked_layer,
            props,
        })
        .unwrap();

        let before_unlocked = doc.entity(unlocked).unwrap().geom.clone();
        let before_locked = doc.entity(locked_entity).unwrap().geom.clone();

        // 両方を選択（同座標に重ねてあり、どちらも可視なので矩形で 2 本とも拾える）。
        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(2.0, 1.0));
        assert_eq!(tool.selection().len(), 2);
        let selection_before = tool.selection().to_vec();

        // M の2クリック配置で移動を確定する。
        assert!(tool.start_move());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        let outcome = tool.placement_click(&doc, Point2::new(5.0, 5.0), 0.1);
        let PlacementOutcome::Commit { cmd, .. } = outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        // Batch 原子性で全体が失敗する。
        assert!(doc.apply(cmd).is_err());
        // どちらも動いていない。
        assert_eq!(doc.entity(unlocked).unwrap().geom, before_unlocked);
        assert_eq!(doc.entity(locked_entity).unwrap().geom, before_locked);
        // 失敗時は選択も変えない。
        assert_eq!(tool.selection(), selection_before.as_slice());
    }

    #[test]
    fn retain_alive_drops_dead_ids_and_unblocks_delete() {
        // undo でエンティティが消えた後、選択に死んだ ID が残ると削除バッチが
        // EntityNotFound で失敗し続ける。retain_alive で浄化すれば残りを削除できる。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);

        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));
        assert_eq!(tool.selection().len(), 2);

        // 直近の AddEntity(b) を undo → b は存在しなくなる。
        assert!(doc.undo());
        assert!(doc.entity(b).is_none());

        // 浄化しないままの削除バッチは RemoveEntity(b) を含み、原子的に失敗する。
        let stale_cmd = tool.delete_command().unwrap();
        assert!(doc.apply(stale_cmd).is_err());
        assert!(doc.entity(a).is_some(), "失敗時は a も消えない（原子性）");

        // 浄化後は a だけの削除バッチになり、成功する。
        tool.retain_alive(&doc);
        assert_eq!(tool.selection(), &[a]);
        doc.apply(tool.delete_command().unwrap()).unwrap();
        assert!(doc.entity(a).is_none());
    }

    // --- 配置モード（Ctrl+D 複製） ---

    /// `PlacementOutcome::Commit { cmd: Batch(..), .. }` から複製 `Entity` 群を取り出す。
    /// 想定外の形なら panic する（テスト専用）。
    fn dup_entities(outcome: &PlacementOutcome) -> Vec<Entity> {
        match outcome {
            PlacementOutcome::Commit {
                cmd: Command::Batch(subs),
                ..
            } => subs
                .iter()
                .map(|c| match c {
                    Command::AddEntity(e) => e.clone(),
                    other => panic!("expected AddEntity, got {other:?}"),
                })
                .collect(),
            other => panic!("expected Commit(Batch), got {other:?}"),
        }
    }

    #[test]
    fn start_duplicate_requires_nonempty_selection() {
        let mut doc = Document::new();
        let _a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();

        // 空選択では配置モードに入らない（呼び出し側が案内メッセージを出す）。
        assert!(!tool.start_duplicate());
        assert!(!tool.is_placing());

        // 選択してから開始すると配置モードに入る。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.start_duplicate());
        assert!(tool.is_placing());
    }

    #[test]
    fn duplicate_two_click_flow_commits_translated_copies() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let before_b = doc.entity(b).unwrap().geom.clone();
        let layer = doc.entity(a).unwrap().layer;
        let style = doc.entity(a).unwrap().style;

        let mut tool = SelectTool::default();
        // 矩形で両方選択する。
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));
        assert_eq!(tool.selection().len(), 2);

        assert!(tool.start_duplicate());
        // 1クリック目=基準点。まだ確定せず、モードは継続。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        assert!(tool.is_placing());

        // カーソル追従でプレビュー変位が更新される。
        tool.placement_move(Point2::new(3.0, 4.0));
        assert_eq!(
            tool.placement_preview(),
            Some(PlacementPreview::Duplicate {
                delta: Vec2::new(3.0, 4.0),
            })
        );

        // 2クリック目=配置先。delta=(3,4) の複製 Batch を返し、モードは畳まれる。
        let delta = Vec2::new(3.0, 4.0);
        let outcome = tool.placement_click(&doc, Point2::new(3.0, 4.0), 0.1);
        assert!(!tool.is_placing());
        assert!(tool.placement_preview().is_none());

        let copies = dup_entities(&outcome);
        assert_eq!(copies.len(), 2);
        // レイヤー・スタイルを保持し、幾何のみ translated した複製であること。
        let expect_a = Entity::new(before_a.translated(delta), layer, style);
        let expect_b = Entity::new(before_b.translated(delta), layer, style);
        assert!(copies.contains(&expect_a));
        assert!(copies.contains(&expect_b));

        // 元エンティティは変更されない（複製は AddEntity のみ）。
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
        assert_eq!(doc.entity(b).unwrap().geom, before_b);
    }

    #[test]
    fn duplicate_apply_and_undo_is_single_unit() {
        // 検収シナリオ: 選択 → Ctrl+D → 2クリック配置 → apply、undo 1 回で複製全体が消える。
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        add_hline(&mut doc, 10.0);

        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));
        assert_eq!(tool.selection().len(), 2);

        assert!(tool.start_duplicate());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        let outcome = tool.placement_click(&doc, Point2::new(5.0, 5.0), 0.1);
        let PlacementOutcome::Commit { cmd, .. } = outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        let new_ids = doc.apply(cmd).unwrap();
        assert_eq!(new_ids.entities.len(), 2);
        assert_eq!(doc.entity_count(), 4);

        // 呼び出し側の流儀どおり、新しい ID を選択にする。
        tool.set_selection(new_ids.entities.clone());
        assert_eq!(tool.selection(), new_ids.entities.as_slice());

        // Batch なので undo 1 回で複製 2 本がまとめて消える。
        assert!(doc.undo());
        assert_eq!(doc.entity_count(), 2);
    }

    #[test]
    fn duplicate_zero_displacement_cancels() {
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.start_duplicate());

        // 基準点。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(5.0, 5.0), 0.1),
            PlacementOutcome::Continue
        );
        // 配置先が基準点とほぼ同一（tol=0.1 以内）なら見えない重複を避けてキャンセル。
        let outcome = tool.placement_click(&doc, Point2::new(5.05, 5.0), 0.1);
        assert_eq!(
            outcome,
            PlacementOutcome::Cancelled("Zero displacement - duplicate cancelled")
        );
        assert!(!tool.is_placing());
        // Document は変わらない。
        assert_eq!(doc.entity_count(), 1);
    }

    #[test]
    fn cancel_placement_exits_mode_and_keeps_selection() {
        // Esc・ツール切替・ファイル操作からの解除経路（`cancel_placement`）。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        assert!(tool.start_duplicate());
        // 基準点まで進めてから解除する。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        assert!(tool.is_placing());

        tool.cancel_placement();
        assert!(!tool.is_placing());
        assert!(tool.placement_preview().is_none());
        // 解除は Document も選択も変えない。
        assert_eq!(tool.selection(), &[a]);
        assert_eq!(doc.entity_count(), 1);
    }

    #[test]
    fn duplicate_touching_locked_layer_fails_atomically() {
        // ロックレイヤーのエンティティが混ざると、複製先が同レイヤーになるため
        // AddEntity が拒否され、Batch 原子性で複製全体が失敗する。何も追加されず選択も不変。
        let mut doc = Document::new();
        let _unlocked = add_hline(&mut doc, 0.0);

        let locked_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "locked",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        doc.apply(Command::AddEntity(Entity::new(
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
            locked_layer,
            Style::inherited(),
        )))
        .unwrap();
        let mut props = doc.layer(locked_layer).unwrap().clone();
        props.locked = true;
        doc.apply(Command::SetLayerProps {
            id: locked_layer,
            props,
        })
        .unwrap();

        let before_count = doc.entity_count();

        // 両方を選択（同座標に重ねてあり、どちらも可視なので矩形で 2 本とも拾える）。
        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(2.0, 1.0));
        assert_eq!(tool.selection().len(), 2);
        let selection_before = tool.selection().to_vec();

        assert!(tool.start_duplicate());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        let outcome = tool.placement_click(&doc, Point2::new(5.0, 5.0), 0.1);
        let PlacementOutcome::Commit { cmd, .. } = outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        // Batch 原子性で全体が失敗し、1 本も複製されない。
        assert!(doc.apply(cmd).is_err());
        assert_eq!(doc.entity_count(), before_count);
        // 失敗時は選択も変えない（新 ID による置換は成功時のみ）。
        assert_eq!(tool.selection(), selection_before.as_slice());
    }

    #[test]
    fn placement_click_without_active_mode_is_noop() {
        // 入力ゲートの前提が崩れても安全側で無視する（配置中でなければ Continue）。
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        assert!(!tool.is_placing());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(1.0, 1.0), 0.1),
            PlacementOutcome::Continue
        );
        assert_eq!(doc.entity_count(), 1);
    }

    #[test]
    fn start_duplicate_discards_in_progress_drag() {
        // 配置モードは進行中のドラッグと両立しない。開始時にドラッグを畳む。
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        // 矩形選択ドラッグを開始してからそのまま Ctrl+D。
        tool.on_drag_start(Point2::new(0.5, 0.0));
        assert!(tool.drag_preview().is_some());
        assert!(tool.start_duplicate());
        assert!(tool.is_placing());
        // ドラッグは破棄されている（配置プレビューが優先）。
        assert!(tool.drag_preview().is_none());
    }

    // --- 配置モード（M 移動） ---

    #[test]
    fn start_move_requires_nonempty_selection() {
        let mut doc = Document::new();
        let _a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();

        // 空選択では移動配置モードに入らない（呼び出し側が案内メッセージを出す）。
        assert!(!tool.start_move());
        assert!(!tool.is_placing());

        // 選択してから開始すると配置モードに入る。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.start_move());
        assert!(tool.is_placing());
    }

    #[test]
    fn move_preview_follows_cursor() {
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.start_move());

        // 基準点確定前はプレビューなし。
        assert!(tool.placement_preview().is_none());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        // カーソル追従で移動プレビューの変位が更新される（複製ではなく Move）。
        tool.placement_move(Point2::new(3.0, 4.0));
        assert_eq!(
            tool.placement_preview(),
            Some(PlacementPreview::Move {
                delta: Vec2::new(3.0, 4.0),
            })
        );
    }

    #[test]
    fn move_zero_displacement_cancels() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.start_move());

        // 基準点。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(5.0, 5.0), 0.1),
            PlacementOutcome::Continue
        );
        // 配置先が基準点とほぼ同一（tol=0.1 以内）なら移動をキャンセルする。
        let outcome = tool.placement_click(&doc, Point2::new(5.05, 5.0), 0.1);
        assert_eq!(
            outcome,
            PlacementOutcome::Cancelled("Zero displacement - move cancelled")
        );
        assert!(!tool.is_placing());
        // Document は変わらない。
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
    }

    #[test]
    fn cancel_placement_during_move_keeps_selection_and_document() {
        // Esc・ツール切替・ファイル操作からの解除経路を移動でも確認する。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);

        assert!(tool.start_move());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        assert!(tool.is_placing());

        tool.cancel_placement();
        assert!(!tool.is_placing());
        assert!(tool.placement_preview().is_none());
        // 解除は Document も選択も変えない。
        assert_eq!(tool.selection(), &[a]);
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
    }

    #[test]
    fn start_move_discards_in_progress_drag() {
        // 移動配置モードも進行中のドラッグと両立しない。開始時にドラッグを畳む。
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        tool.on_drag_start(Point2::new(0.5, 0.0));
        assert!(tool.drag_preview().is_some());
        assert!(tool.start_move());
        assert!(tool.is_placing());
        assert!(tool.drag_preview().is_none());
    }
}
