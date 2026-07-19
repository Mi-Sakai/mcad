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
/// # クリックとドラッグ移動の区別
///
/// 「動かなければ選択、動けば移動/矩形」の閾値判定は **egui 組み込みのクリック/ドラッグ
/// 判定に委ねる**（`Response::clicked` と `drag_started`/`dragged`/`drag_stopped` は
/// egui 内部のドラッグ閾値で排他的に分岐する）。`McadApp` 側がこれらを対応する
/// メソッド呼び出しへ振り分けるため、本ツールにピクセル閾値を持たせる必要はない。
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
#[derive(Debug, Clone, Copy, PartialEq)]
enum DragState {
    /// 矩形選択中（`start` から `current` までのドラッグ矩形）。
    Rect { start: Point2, current: Point2 },
    /// 選択エンティティの移動中（`start` から `current` への変位ぶん動かす）。
    Move { start: Point2, current: Point2 },
}

/// ドラッグ中のプレビュー描画に必要な情報（`McadApp` の描画層向けの公開ビュー）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragPreview {
    /// 矩形選択のプレビュー（2 隅のワールド座標）。
    Rect { start: Point2, current: Point2 },
    /// 移動プレビュー（選択エンティティをこの変位ぶん動かした位置に仮表示する）。
    Move { delta: Vec2 },
}

/// 選択・編集ツール。単一選択（クリック）・矩形選択（ドラッグ）・移動（選択物上の
/// ドラッグ）・削除（Delete/Backspace）を担う。設計判断の詳細は [`DragState`] の
/// doc を参照。
#[derive(Debug, Default)]
pub struct SelectTool {
    /// 現在の選択集合（アプリ UI 状態。ドキュメント履歴には積まない）。
    selection: Vec<EntityId>,
    /// 進行中のドラッグ操作。無ければ `None`。
    drag: Option<DragState>,
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

    /// ドラッグ中のプレビュー情報。ドラッグしていなければ `None`。
    #[must_use]
    pub fn drag_preview(&self) -> Option<DragPreview> {
        match self.drag {
            Some(DragState::Rect { start, current }) => Some(DragPreview::Rect { start, current }),
            Some(DragState::Move { start, current }) => Some(DragPreview::Move {
                delta: current - start,
            }),
            None => None,
        }
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

    /// `world` が現在の選択エンティティのいずれかの許容量 `tol` 内にあるか。
    /// 移動ドラッグの開始判定に使う。
    fn hits_selected(&self, document: &Document, world: Point2, tol: f64) -> bool {
        self.selection.iter().any(|&id| {
            document
                .entity(id)
                .is_some_and(|e| distance_to(&e.geom, world) <= tol)
        })
    }

    /// 単一クリック（ドラッグを伴わない）。ヒットしたエンティティ 1 つを選択に置き換え、
    /// 何もヒットしなければ選択を空にする（＝空クリックでのクリア）。
    pub fn on_click(&mut self, document: &Document, world: Point2, tol: f64) {
        match self.pick(document, world, tol) {
            Some(id) => self.selection = vec![id],
            None => self.selection.clear(),
        }
    }

    /// ドラッグ開始。選択済みエンティティ上なら移動、そうでなければ矩形選択を始める。
    pub fn on_drag_start(&mut self, document: &Document, world: Point2, tol: f64) {
        self.drag = if self.hits_selected(document, world, tol) {
            Some(DragState::Move {
                start: world,
                current: world,
            })
        } else {
            Some(DragState::Rect {
                start: world,
                current: world,
            })
        };
    }

    /// ドラッグ中の現在位置を更新する（プレビュー用）。ドラッグしていなければ無視。
    pub fn on_drag(&mut self, world: Point2) {
        match &mut self.drag {
            Some(DragState::Rect { current, .. } | DragState::Move { current, .. }) => {
                *current = world;
            }
            None => {}
        }
    }

    /// ドラッグ確定。矩形選択なら選択集合を更新して `None` を返す。移動なら選択物全部を
    /// 1 つの [`Command::Batch`] にまとめて返す（呼び出し側が `Document::apply` する）。
    /// 変位が 0・選択が空などで確定すべき変更が無ければ `None`。
    #[must_use]
    pub fn on_drag_end(&mut self, document: &Document, world: Point2) -> Option<Command> {
        match self.drag.take()? {
            DragState::Rect { start, .. } => {
                let rect = Aabb::from_corners(start, world);
                // 完全内包（rect ⊇ entity.aabb）で選ぶ。基準の理由は DragState の doc を参照。
                self.selection = document
                    .entities()
                    .filter(|(_, e)| layer_visible(document, e))
                    .filter(|(_, e)| rect.contains(&e.geom.aabb()))
                    .map(|(id, _)| id)
                    .collect();
                None
            }
            DragState::Move { start, .. } => {
                let delta = world - start;
                if delta == Vec2::ZERO || self.selection.is_empty() {
                    return None;
                }
                // 選択物全部を 1 バッチにまとめて undo/redo を 1 単位にする。
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
    }

    /// 進行中のドラッグ（矩形選択・移動）を破棄する（Esc）。選択集合は変えない。
    /// 選択そのもののクリアは「空クリック」で行う（Esc とは別）。
    pub fn on_cancel(&mut self) {
        self.drag = None;
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
        let mut tool = SelectTool::default();

        // (0.5, 0.05) は a に非常に近く、b からは遠い。許容量 0.1 以内で a を選ぶ。
        tool.on_click(&doc, Point2::new(0.5, 0.05), 0.1);
        assert_eq!(tool.selection(), &[a]);

        // b の近くをクリックすれば選択が b に置き換わる（単一選択）。
        tool.on_click(&doc, Point2::new(10.5, 0.0), 0.1);
        assert_eq!(tool.selection(), &[b]);
    }

    #[test]
    fn click_beyond_tolerance_clears_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1);
        assert_eq!(tool.selection(), &[a]);

        // 何もない場所（許容量外）をクリックすると選択がクリアされる。
        tool.on_click(&doc, Point2::new(0.5, 100.0), 0.1);
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
        tool.on_click(&doc, Point2::new(0.5, 0.05), 1.0);
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
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1);
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
        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        tool.on_drag(Point2::new(5.0, 5.0));
        assert_eq!(tool.on_drag_end(&doc, Point2::new(5.0, 5.0)), None);
        assert_eq!(tool.selection(), &[inside]);
    }

    #[test]
    fn rectangle_partially_covering_does_not_select() {
        let mut doc = Document::new();
        let _line = add_hline(&mut doc, 0.0); // AABB [0,0]-[1,0]
        let mut tool = SelectTool::default();

        // 矩形 (-1,-1)→(0.5,1) は線分の右半分を覆うが完全には内包しない → 非選択。
        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(0.5, 1.0)), None);
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn drag_on_selected_entity_starts_move_and_commits_batch() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();

        // まず矩形で 2 本とも選択する（両方を完全内包する矩形）。
        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(12.0, 1.0)), None);
        assert_eq!(tool.selection().len(), 2);

        // 選択済みエンティティ a の上（0.5,0）からドラッグ → 移動。変位 (0,5)。
        tool.on_drag_start(&doc, Point2::new(0.5, 0.0), 0.1);
        tool.on_drag(Point2::new(0.5, 5.0));
        let cmd = tool
            .on_drag_end(&doc, Point2::new(0.5, 5.0))
            .expect("移動は Batch コマンドを返す");

        // 2 本ぶんの ModifyEntity を含む Batch。各 new_geom は元の幾何を (0,5) 平行移動したもの。
        match cmd {
            Command::Batch(subs) => {
                assert_eq!(subs.len(), 2);
                let expect = |id: mcad_core::EntityId| {
                    let g = doc.entity(id).unwrap().geom.translated(Vec2::new(0.0, 5.0));
                    Command::ModifyEntity { id, new_geom: g }
                };
                assert!(subs.contains(&expect(a)));
                assert!(subs.contains(&expect(b)));
            }
            other => panic!("expected Batch, got {other:?}"),
        }
        // 移動の確定コマンドを返しただけでは選択は変わらない（呼び出し側が apply）。
        assert_eq!(tool.selection().len(), 2);
    }

    #[test]
    fn drag_on_empty_space_starts_rectangle_even_with_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let _b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();

        // a を選択済みにしておく。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1);
        assert_eq!(tool.selection(), &[a]);

        // 何もない場所 (50,50) からドラッグ開始 → 移動ではなく矩形選択。
        tool.on_drag_start(&doc, Point2::new(50.0, 50.0), 0.1);
        // 矩形が何も内包しなければ選択は空になる（矩形選択は集合を置き換える）。
        let cmd = tool.on_drag_end(&doc, Point2::new(60.0, 60.0));
        assert_eq!(cmd, None);
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn move_with_zero_delta_commits_nothing() {
        let mut doc = Document::new();
        let _a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1);

        // 選択物上でドラッグ開始したが同じ点で離した → 変位 0 → コマンドなし。
        tool.on_drag_start(&doc, Point2::new(0.5, 0.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(0.5, 0.0)), None);
    }

    #[test]
    fn escape_cancels_in_progress_drag_without_changing_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1);
        assert_eq!(tool.selection(), &[a]);

        // 移動ドラッグを始めてから Esc（on_cancel）。
        tool.on_drag_start(&doc, Point2::new(0.5, 0.0), 0.1);
        tool.on_drag(Point2::new(0.5, 9.0));
        assert!(tool.drag_preview().is_some());
        tool.on_cancel();

        // ドラッグは破棄され、選択は変わらない。
        assert!(tool.drag_preview().is_none());
        assert_eq!(tool.selection(), &[a]);
        // キャンセル後に離しても確定コマンドは生じない。
        assert_eq!(tool.on_drag_end(&doc, Point2::new(0.5, 9.0)), None);
    }

    #[test]
    fn delete_builds_batch_remove_of_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let mut tool = SelectTool::default();

        // 空選択なら削除コマンドは無い。
        assert_eq!(tool.delete_command(), None);

        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(12.0, 1.0)), None);
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
        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(12.0, 1.0)), None);
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
        // 検収シナリオ: 矩形選択 → 移動 → undo 1 回で全戻り。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let before_b = doc.entity(b).unwrap().geom.clone();

        let mut tool = SelectTool::default();
        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(12.0, 1.0)), None);

        tool.on_drag_start(&doc, Point2::new(0.5, 0.0), 0.1);
        let cmd = tool.on_drag_end(&doc, Point2::new(0.5, 7.0)).unwrap();
        doc.apply(cmd).unwrap();

        let delta = Vec2::new(0.0, 7.0);
        assert_eq!(doc.entity(a).unwrap().geom, before_a.translated(delta));
        assert_eq!(doc.entity(b).unwrap().geom, before_b.translated(delta));

        // 1 バッチなので undo 1 回で両方元へ戻る。
        assert!(doc.undo());
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
        assert_eq!(doc.entity(b).unwrap().geom, before_b);
    }

    #[test]
    fn move_touching_locked_layer_fails_atomically() {
        // ロックされたレイヤーのエンティティが混ざると、Batch 原子性により
        // 移動全体が失敗し、どのエンティティも動かない。
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
        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(2.0, 1.0)), None);
        assert_eq!(tool.selection().len(), 2);

        tool.on_drag_start(&doc, Point2::new(0.5, 0.0), 0.1);
        let cmd = tool.on_drag_end(&doc, Point2::new(0.5, 5.0)).unwrap();
        // Batch 原子性で全体が失敗する。
        assert!(doc.apply(cmd).is_err());
        // どちらも動いていない。
        assert_eq!(doc.entity(unlocked).unwrap().geom, before_unlocked);
        assert_eq!(doc.entity(locked_entity).unwrap().geom, before_locked);
    }

    #[test]
    fn retain_alive_drops_dead_ids_and_unblocks_delete() {
        // undo でエンティティが消えた後、選択に死んだ ID が残ると削除バッチが
        // EntityNotFound で失敗し続ける。retain_alive で浄化すれば残りを削除できる。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);

        let mut tool = SelectTool::default();
        tool.on_drag_start(&doc, Point2::new(-1.0, -1.0), 0.1);
        assert_eq!(tool.on_drag_end(&doc, Point2::new(12.0, 1.0)), None);
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
}
