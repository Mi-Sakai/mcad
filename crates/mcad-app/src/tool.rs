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

use mcad_core::{
    Command, DimLinear, DimRadial, Document, Entity, EntityGeom, EntityId, LayerId, Linetype,
    NewIds, Style,
};
use mcad_geom::{
    Aabb, Arc, FilletError, LineSeg, OffsetError, Point2, Polyline, Shape, SplitError,
    TrimExtendError, Vec2, circumcircle, closest_point, distance_to, extend, fillet_lines, split,
    trim,
};

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

/// 寸法の方向が定まるかを判定するスケール非依存の幾何許容値（ワールド単位）。
///
/// ピック許容量（`PICK_TOLERANCE_PX / zoom`、スクリーン基準でズーム依存）とは別物で、
/// 「線分・引出方向が数学的に成立するか」だけを見る絶対値。ズームアウト時に有効な短い
/// 寸法を誤って拒否しないよう、退化判定にはこちらを使う（[`PolylineTool`] の
/// `AUTO_CLOSE_EPSILON` と同じ考え方）。長さ寸法の p1≈p2、半径寸法の引出クリック＝中心の
/// 両方で共有する。
const DIM_DEGENERATE_EPSILON: f64 = 1e-9;

/// 半径寸法ツールの 1 クリック目で拾った円／円弧の採取値（中心・半径）。
///
/// 半径寸法は非関連（作成時に値を採取して独立する。DESIGN.md M6 設計判断2）なので、
/// ヒットした [`EntityId`] は保持せず中心・半径だけを写し取る。ヒットテスト自体は
/// [`Document`] を要するため app 層（[`pick_circle_or_arc`]）が行い、結果をこの型で
/// ツールへ渡す（[`Tool::on_circle_pick`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CirclePick {
    /// 円／円弧の中心。
    pub center: Point2,
    /// 円／円弧の半径。
    pub radius: f64,
}

/// [`pick_shape_entity`] が返す、任意形状エンティティのヒット結果（M7 タスク30）。
///
/// [`CirclePick`] と異なり、対象エンティティの [`EntityId`] とヒット時のクリック点
/// （ワールド座標）も保持する。M7 のトリム・延長・フィレット・分割（タスク31〜33）は
/// 既存エンティティを実際に書き換える（`ModifyEntity`）ため、値を写し取るだけでなく
/// 対象を ID で追跡する必要があり、演算によってはクリック位置自体（例: どちら側の端を
/// 残すか）も要るための設計。
#[derive(Debug, Clone, PartialEq)]
pub struct ShapePick {
    /// ヒットしたエンティティの ID。
    pub id: EntityId,
    /// ヒットしたエンティティの形状（`EntityGeom::as_shape()` の複製）。
    pub shape: Shape,
    /// ヒットテストに使ったクリック点（ワールド座標）。
    pub click: Point2,
    /// ヒットしたエンティティの所属レイヤー。
    ///
    /// トリムが 2 断片へ分かれた場合（[`TrimTool`]）や分割（タスク33）は、元エンティティを
    /// 削除して新規 2 件を追加するため、**元のレイヤー・スタイルを複製**する必要がある
    /// （M5 の複製・オフセットと同じ規約）。ツールは [`Document`] を参照しない設計なので、
    /// ヒットテストを行う app 層（[`pick_shape_entity`]）がここへ写し取って渡す。
    pub layer: LayerId,
    /// ヒットしたエンティティのスタイル（複製用。`layer` と同じ理由）。
    pub style: Style,
}

/// [`pick_circle_or_arc`] / [`pick_shape_entity`] / [`SelectTool::pick`] が共有する
/// 「可視エンティティを走査し、`tol` 以内で最も近いものを選ぶ」ヒットテストの骨格
/// （M7 タスク30、重複整理）。`score` はエンティティごとに距離と結果値の組
/// `(distance, T)` を返す（対象外なら `None`）。3 箇所とも距離の定義自体は異なる
/// （実形状への最近距離／Text の aabb 距離／寸法の展開線分距離など）ため、その部分だけ
/// 呼び出し側のクロージャに残し、走査・可視レイヤーフィルタ・`tol` 判定・最小距離選択の
/// 共通部分だけをここへ集約する。
fn pick_nearest<T>(
    document: &Document,
    tol: f64,
    mut score: impl FnMut(EntityId, &Entity) -> Option<(f64, T)>,
) -> Option<T> {
    let mut best: Option<(f64, T)> = None;
    for (id, entity) in document.entities() {
        if !layer_visible(document, entity) {
            continue;
        }
        let Some((d, value)) = score(id, entity) else {
            continue;
        };
        if d <= tol && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, value));
        }
    }
    best.map(|(_, value)| value)
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
    /// 入力を退化として拒否した。ツールの状態は進めておらず（据え置き）、コマンドも
    /// 作らない。呼び出し側は付随する ASCII 理由をステータスバーへ表示する。無反応に
    /// 見えないよう理由を伝えるための結果で、`Continue` と違い「クリックは効いたが確定
    /// できなかった」ことを app 層へ通知する（寸法ツールの退化クリックで使う）。
    Rejected(&'static str),
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

    /// テキストツールがアンカーを確定して文字入力待ち（パネル編集中）のとき、その
    /// アンカー座標を返す。それ以外のツール・状態では `None`（既定）。
    ///
    /// # なぜ Tool トレイトに置くのか
    ///
    /// テキストの文字列・高さの入力欄と確定（`AddEntity`）は app 層が担う
    /// （文字列は egui の `TextEdit`、確定は Enter 検出とレイヤーロック時のステータス表示を
    /// 伴い、[`Tool::on_input`] の `Move`/`Click`/`Confirm`/`Cancel` と `ToolResult` では
    /// 表現しきれないため。オフセットが `SelectTool` 側で独自の入力欄を持つのと同じ事情）。
    /// app 層は「テキストツールがアンカー確定済みか・どこか」だけ分かれば入力欄の表示・
    /// プレビュー描画・確定ができる。`Box<dyn Tool>` からその 1 点を取り出すための、
    /// [`Tool::snap_points`] と同様の既定実装つき拡張点。
    fn pending_text_anchor(&self) -> Option<Point2> {
        None
    }

    /// 半径寸法ツールが「次のクリックで円／円弧をヒットテストする」段階にあるとき `true`
    /// （1 クリック目）。app 層（`handle_tool_input`）はこのとき通常の
    /// [`Tool::on_input`]`(Click)` の代わりにドキュメントをヒットテストし、結果を
    /// [`Tool::on_circle_pick`] へ渡す。他ツール・他状態では `false`（既定）。
    ///
    /// # なぜ Tool トレイトに置くのか
    ///
    /// 作図ツールは [`Document`] を参照しない設計だが、半径寸法の 1 クリック目だけは既存の
    /// 円／円弧を当てる必要があり、ヒットテストには [`Document`] が要る。そこで [`snap_points`]
    /// / [`pending_text_anchor`] と同じ「既定実装つき拡張点」として、ヒットテストの実行だけを
    /// app 層へ委ね（[`InputEvent`] は Document 非依存のまま汚さない）、結果を型で受け取る。
    ///
    /// [`snap_points`]: Tool::snap_points
    /// [`pending_text_anchor`]: Tool::pending_text_anchor
    fn wants_circle_pick(&self) -> bool {
        false
    }

    /// [`Tool::wants_circle_pick`] が `true` のとき、app 層が当てた円／円弧を受け取り状態を
    /// 進める。既定は何もしない（他ツールでは呼ばれない）。外した場合は app 層が本メソッドを
    /// 呼ばずステータスへ拒否メッセージを出すため、状態は据え置きになる。
    fn on_circle_pick(&mut self, _hit: CirclePick) {}

    /// ツールが「次のクリックで任意形状のエンティティをヒットテストする」段階にあるとき
    /// `true`。app 層（`handle_tool_input`）はこのとき通常の [`Tool::on_input`]`(Click)` の
    /// 代わりにドキュメントをヒットテストし（[`pick_shape_entity`]）、結果を
    /// [`Tool::on_shape_pick`] へ渡す。他ツール・他状態では `false`（既定）。
    ///
    /// [`Tool::wants_circle_pick`] の一般化版（M7 タスク30）。半径寸法は円／円弧だけを対象に
    /// 中心・半径だけを採取すればよかったが、M7 のトリム・延長・フィレット・分割（タスク
    /// 31〜33）は対象形状が Line/Arc/Polyline と多様で、かつクリック位置そのもの（`click`）も
    /// 演算に必要（例: フィレットの「どちら側の端を残すか」判定）なため、別の拡張点として
    /// 用意する。既存の `wants_circle_pick`/`on_circle_pick` はそのまま残す（`DimRadialTool`
    /// のシグネチャ変更禁止）。
    fn wants_shape_pick(&self) -> bool {
        false
    }

    /// [`Tool::wants_shape_pick`] が `true` のとき、app 層が当てたエンティティを受け取り状態を
    /// 進める。既定は何もしない（他ツールでは呼ばれない）。外した場合は app 層が本メソッドを
    /// 呼ばずステータスへ拒否メッセージを出すため、状態は据え置きになる。
    ///
    /// [`Tool::on_circle_pick`] と違い [`ToolResult`] を返すのは、トリム・延長（タスク31）が
    /// **ピックそのもので確定する**（2 クリック目の対象ピックが即 `Commit`）ためで、
    /// `on_input` を経由しない確定・拒否経路が必要になる。半径寸法は 1 クリック目のピックが
    /// 状態を進めるだけで確定は 2 クリック目の `on_input` が担うため戻り値が要らなかった。
    fn on_shape_pick(&mut self, _hit: ShapePick) -> ToolResult {
        ToolResult::Continue
    }

    /// 直前の [`ToolResult::Commit`] が `Document` へ適用された直後に呼ばれ、選択集合を
    /// 置き換えたい場合にその ID 列を返す。`None`（既定）は「選択集合に触らない」。
    ///
    /// トポロジが変わる確定（トリムが 2 断片へ分かれたケース。DESIGN.md M7 設計判断3a）は、
    /// 元エンティティが墓標化して選択から自然に外れる代わりに、新規 2 件を選択集合へ
    /// 載せ替える規約になっている。その ID は `Document::apply` の戻り値 [`NewIds`] に
    /// しか無く、ツール側は確定時点では知り得ないため、適用後にこのフックで受け渡す。
    ///
    /// `&mut self` なのは「今回の確定だけ載せ替える」フラグを消費するため（次の確定で
    /// 意図せず選択が書き換わらないようにする）。
    fn take_commit_selection(&mut self, _new_ids: &NewIds) -> Option<Vec<EntityId>> {
        None
    }

    /// 直前の [`ToolResult::Commit`] を `Document::apply` へ渡した結果が `Err`
    /// （レイヤーロック等）だったときに呼ばれる後始末フック。既定は何もしない。
    ///
    /// M7 の4ツール（トリム・延長・フィレット・分割）は [`ToolKind::keeps_state_on_commit_failure`]
    /// が `true` を返すため、app 層は失敗時も `spawn()` でツールを作り直さず、状態
    /// （境界・1本目の線分など）を保ったまま再挑戦させる（DESIGN.md M7設計判断6）。
    /// 一方これらのツールは確定成功時、次の [`Tool::take_commit_selection`] 呼び出しで
    /// 選択集合を新規エンティティへ載せ替える片道フラグ（`select_new_entities`/
    /// `committed`/`committed_pair` 等）をコマンド作成と同時に立てる。作り直しを
    /// やめたことでこのフラグが「確定は失敗したのに立ったまま」残ってしまうと、次に
    /// 別の確定が成功したとき `take_commit_selection` がそれを誤って消費し、無関係の
    /// 確定の選択集合を空／不正な ID 列へ壊す。本フックはこの片道フラグを失敗時に
    /// クリアするために使う（Codex adversarial review 2026-07-26 指摘）。
    ///
    /// [`ToolKind::keeps_state_on_commit_failure`]: crate::ToolKind::keeps_state_on_commit_failure
    fn on_commit_failed(&mut self) {}

    /// 直交モード（ortho、v0.7.1）が拘束の基準にする「直前に確定した点」を返す。
    /// 既定は `None`（対象外。基準点が無い＝ortho を適用しない）。
    ///
    /// # なぜ `snap_points()` を流用しないのか
    ///
    /// [`Tool::snap_points`] は「スナップ候補として扱う頂点の集合」であり意味が違う
    /// （例: `ArcTool` の `WaitingP3` は `snap_points()` が `p1`・`p2` の2点を返すが、
    /// ortho の基準は直近の `p2` だけでよい）。誤って対象に入れないよう、ツールごとに
    /// 明示的にオプトインする専用の拡張点にする（`ortho.rs` のモジュール doc も参照）。
    ///
    /// override するのは [`LineTool`]・[`PolylineTool`]・[`ArcTool`] の3つのみ。
    /// Circle・Text・寸法系・M7の4ツールは「直前の点からの方向が結果に効かない」
    /// または「クリックの意味が用途ごとに変わる」ため既定のまま（対象外）。
    fn ortho_origin(&self) -> Option<Point2> {
        None
    }

    /// app 層が持つ数値入力欄（[`FilletTool`] の半径欄）の解析値をツールへ渡す
    /// （正の有限値のみ `Some`、空欄・不正値は `None`）。既定は何もしない。
    ///
    /// # なぜ Tool トレイトに置くのか
    ///
    /// 入力欄そのものは egui のウィジェットなので app 層（`main.rs` の上部パネル）が持つ
    /// のが既定路線（テキストの文字列・高さ欄、オフセットの距離欄と同じ）。一方フィレットは
    /// 「2 クリック目のヒットテストと同時に確定する」ため、確定判断を行うツール側が値を
    /// 知っている必要がある。[`Tool::pending_text_anchor`] が「ツールの状態を app 層へ
    /// 見せる」向きの拡張点なのに対し、こちらは「app 層の入力欄をツールへ流し込む」逆向きの
    /// 拡張点。app 層は毎フレーム呼ぶので、ツール側は最後に渡された値だけを保持すればよい。
    fn set_radius_input(&mut self, _radius: Option<f64>) {}
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
            draw_shape(
                painter,
                rect,
                viewport,
                &Shape::Point(p),
                preview_stroke(),
                Linetype::Continuous,
                1.0,
            );
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
                Linetype::Continuous,
                1.0,
            );
        }
    }

    fn snap_points(&self) -> Vec<Point2> {
        match self.state {
            LineState::WaitingFirst => Vec::new(),
            LineState::WaitingSecond(first) => vec![first],
        }
    }

    fn ortho_origin(&self) -> Option<Point2> {
        match self.state {
            LineState::WaitingFirst => None,
            LineState::WaitingSecond(first) => Some(first),
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
                Linetype::Continuous,
                1.0,
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
                    Linetype::Continuous,
                    1.0,
                );
            }
            (ArcState::WaitingP3(p1, p2), Some(cursor)) => {
                if let Some(arc) = build_arc(*p1, *p2, cursor) {
                    draw_shape(
                        painter,
                        rect,
                        viewport,
                        &Shape::Arc(arc),
                        preview_stroke(),
                        Linetype::Continuous,
                        1.0,
                    );
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

    fn ortho_origin(&self) -> Option<Point2> {
        match self.state {
            ArcState::WaitingP1 => None,
            ArcState::WaitingP2(p1) => Some(p1),
            // 3点目の基準は直近の p2（設計書 §3）。
            ArcState::WaitingP3(_p1, p2) => Some(p2),
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
            Linetype::Continuous,
            1.0,
        );
    }

    fn snap_points(&self) -> Vec<Point2> {
        self.vertices.clone()
    }

    fn ortho_origin(&self) -> Option<Point2> {
        self.vertices.last().copied()
    }
}

// ---------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------

/// テキストツールの進行段階。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum TextState {
    /// アンカー未確定。次のクリックでアンカーを置く。
    #[default]
    WaitingAnchor,
    /// アンカー確定済み・文字入力待ち。文字列／高さの入力欄は app 層のパネルが出し、
    /// Enter で `AddEntity` を確定、Esc で `WaitingAnchor` へ戻る。
    Editing(Point2),
}

/// テキストツール（`T`）。クリックでアンカー（ベースライン左端）を置き、app 層の
/// パネル入力欄で文字列と高さ（ワールド単位）を入力して Enter で確定する。角度は 0 で
/// 作成し、向きは既存の回転（`R`）で変える（DESIGN.md M6 設計判断6）。
///
/// # 確定を app 層に委ねる理由
///
/// 文字列（IME 入力可の `TextEdit`）・高さ欄・確定失敗のステータス表示は egui の UI と
/// 密接で、純粋な [`Tool`] 状態機械（[`InputEvent`]／[`ToolResult`]）には収まらない。
/// 本ツールはアンカーのクリック確定と、確定待ちアンカーの公開（[`Tool::pending_text_anchor`]）
/// だけを担い、入力欄と `AddEntity` は app 層が行う。オフセットが [`SelectTool`] 側で独自の
/// 入力欄を持つのと同じ役割分担。
#[derive(Debug, Default)]
pub struct TextTool {
    state: TextState,
    /// アンカー未確定時のカーソル位置（配置プレビュー用）。
    cursor: Option<Point2>,
}

impl Tool for TextTool {
    fn on_input(&mut self, _ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => {
                // アンカー未確定のクリックだけ受け付ける。入力待ち中のクリックは
                // （パネル編集を邪魔しないよう）アンカーを動かさず無視する。
                if matches!(self.state, TextState::WaitingAnchor) {
                    self.state = TextState::Editing(p);
                }
                ToolResult::Continue
            }
            // 確定（Enter による `AddEntity`）は app 層が担う。ここでは何もしない。
            InputEvent::Confirm => ToolResult::Continue,
            InputEvent::Cancel => match self.state {
                // 入力待ち中の Esc はアンカーを捨てて未確定へ戻る（ツールは維持）。
                TextState::Editing(_) => {
                    self.state = TextState::WaitingAnchor;
                    ToolResult::Continue
                }
                // 未確定の Esc は作図をやめる（app 層が Select へ戻す）。
                TextState::WaitingAnchor => ToolResult::Cancel,
            },
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        // アンカー確定後はその位置に小さな十字マーカーを描く（文字列プレビューは
        // 文字列・高さを持つ app 層が別途描く）。未確定時はカーソルにマーカーを描く。
        let mark = match self.state {
            TextState::Editing(anchor) => Some(anchor),
            TextState::WaitingAnchor => self.cursor,
        };
        if let Some(p) = mark {
            let c = viewport.world_to_screen(rect, p);
            let s = 5.0;
            let stroke = preview_stroke();
            painter.line_segment([c + egui::vec2(-s, 0.0), c + egui::vec2(s, 0.0)], stroke);
            painter.line_segment([c + egui::vec2(0.0, -s), c + egui::vec2(0.0, s)], stroke);
        }
    }

    fn pending_text_anchor(&self) -> Option<Point2> {
        match self.state {
            TextState::Editing(anchor) => Some(anchor),
            TextState::WaitingAnchor => None,
        }
    }
}

// ---------------------------------------------------------------------
// 長さ寸法（DimLinear）
// ---------------------------------------------------------------------

/// カーソル位置 `cursor` から計測線（`p1`→`p2`）への符号付きオフセットを求める
/// （寸法線の位置決め用）。法線 `n = dir.perp()` 方向の投影量。計測 2 点がほぼ同一で
/// 方向が定まらない場合は 0。
fn linear_offset(p1: Point2, p2: Point2, cursor: Point2) -> f64 {
    match (p2 - p1).normalize() {
        Some(dir) => (cursor - p1).dot(dir.perp()),
        None => 0.0,
    }
}

/// 長さ寸法ツール（`D`）の状態。3 クリック（計測点 p1 → p2 → 寸法線位置）。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum DimLinearState {
    /// 計測点 1（p1）待ち。
    #[default]
    WaitingP1,
    /// 計測点 2（p2）待ち。`p1` 確定済み。
    WaitingP2(Point2),
    /// 寸法線位置（offset）のクリック待ち。`p1`・`p2` 確定済み。
    WaitingLine(Point2, Point2),
}

/// 長さ寸法ツール（`D`）。計測点 p1 → p2 → 寸法線位置の 3 クリックで
/// [`EntityGeom::DimLinear`] を確定する（DESIGN.md M6 設計判断6）。ArcTool と同じ
/// 3 クリック状態機械。2 クリック目以降はカーソル追従プレビュー。p1≈p2（スケール非依存の
/// 幾何許容値 [`DIM_DEGENERATE_EPSILON`] 内）は `Rejected` で理由を返し、2 クリック目を
/// 待ち続ける（退化したゼロ長寸法を作らない）。
#[derive(Debug, Default)]
pub struct DimLinearTool {
    state: DimLinearState,
    cursor: Option<Point2>,
}

impl Tool for DimLinearTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                DimLinearState::WaitingP1 => {
                    self.state = DimLinearState::WaitingP2(p);
                    ToolResult::Continue
                }
                DimLinearState::WaitingP2(p1) => {
                    // p1≈p2 は寸法線の方向（法線）が定まらずゼロ長寸法になる。判定はズーム
                    // 非依存の幾何許容値で行い（ズームアウト時に有効な短い寸法まで拒否しない）、
                    // 拒否理由を app 層へ伝える。状態は WaitingP2 のまま据え置き、2 点目を待ち続ける。
                    if p1.distance(p) <= DIM_DEGENERATE_EPSILON {
                        ToolResult::Rejected("Linear dim: measure points coincide")
                    } else {
                        self.state = DimLinearState::WaitingLine(p1, p);
                        ToolResult::Continue
                    }
                }
                DimLinearState::WaitingLine(p1, p2) => {
                    let offset = linear_offset(p1, p2, p);
                    let cmd = Command::AddEntity(Entity::new(
                        EntityGeom::DimLinear(DimLinear { p1, p2, offset }),
                        ctx.layer,
                        ctx.style,
                    ));
                    self.state = DimLinearState::WaitingP1;
                    ToolResult::Commit(cmd)
                }
            },
            InputEvent::Cancel => {
                self.state = DimLinearState::WaitingP1;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        match (self.state, self.cursor) {
            // p2 待ち: 計測線の暫定（p1→カーソル）を細線で示す。
            (DimLinearState::WaitingP2(p1), Some(cursor)) => {
                draw_shape(
                    painter,
                    rect,
                    viewport,
                    &Shape::Line(LineSeg::new(p1, cursor)),
                    preview_stroke(),
                    Linetype::Continuous,
                    1.0,
                );
            }
            // 寸法線位置待ち: カーソルで決まる offset の寸法を丸ごとプレビューする。
            (DimLinearState::WaitingLine(p1, p2), Some(cursor)) => {
                let offset = linear_offset(p1, p2, cursor);
                let dim = DimLinear { p1, p2, offset };
                let (arrow_len, text_height) = crate::dim_sizes(viewport.zoom);
                let ex = crate::dimension::expand_linear(&dim, arrow_len, text_height);
                crate::draw_dim_expansion(painter, rect, viewport, &ex, preview_stroke());
            }
            _ => {}
        }
    }

    fn snap_points(&self) -> Vec<Point2> {
        match self.state {
            DimLinearState::WaitingP1 => Vec::new(),
            DimLinearState::WaitingP2(p1) => vec![p1],
            DimLinearState::WaitingLine(p1, p2) => vec![p1, p2],
        }
    }
}

// ---------------------------------------------------------------------
// 半径寸法（DimRadial）
// ---------------------------------------------------------------------

/// 半径寸法ツール（`Shift+D`）の状態。円／円弧のヒットテスト → 引出方向の 2 段。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum DimRadialState {
    /// 円／円弧のヒットテスト待ち（1 クリック目）。app 層が当てて [`Tool::on_circle_pick`]
    /// で結果を渡す。
    #[default]
    WaitingCircle,
    /// 引出方向のクリック待ち（2 クリック目）。中心・半径は採取済み。
    WaitingLeader { center: Point2, radius: f64 },
}

/// 半径寸法ツール（`Shift+D`）。円／円弧をヒットテストで拾い（それ以外は app 層が
/// ASCII メッセージで拒否）、引出方向のクリックで [`EntityGeom::DimRadial`] を確定する
/// （DESIGN.md M6 設計判断6）。非関連寸法なので中心・半径を採取して独立させる。
#[derive(Debug, Default)]
pub struct DimRadialTool {
    state: DimRadialState,
    cursor: Option<Point2>,
}

impl Tool for DimRadialTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                // 1 クリック目は円ヒットテスト（app 層が on_circle_pick 経由で処理する）。
                // ここへ来る Click は無い想定だが、来ても状態は変えない。
                DimRadialState::WaitingCircle => ToolResult::Continue,
                DimRadialState::WaitingLeader { center, radius } => {
                    // 引出クリックが中心とほぼ一致すると方向が定まらない（`.angle()` が実質 0 rad
                    // になり、ユーザーが指定していない右向き引出線が確定してしまう）。ズーム非依存の
                    // 幾何許容値で退化を弾き、状態を据え置いて理由を app 層へ伝える。
                    if p.distance(center) <= DIM_DEGENERATE_EPSILON {
                        ToolResult::Rejected("Radial dim: leader direction undefined at center")
                    } else {
                        let leader_angle = (p - center).angle();
                        let cmd = Command::AddEntity(Entity::new(
                            EntityGeom::DimRadial(DimRadial {
                                center,
                                radius,
                                leader_angle,
                            }),
                            ctx.layer,
                            ctx.style,
                        ));
                        self.state = DimRadialState::WaitingCircle;
                        ToolResult::Commit(cmd)
                    }
                }
            },
            InputEvent::Cancel => {
                self.state = DimRadialState::WaitingCircle;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if let (DimRadialState::WaitingLeader { center, radius }, Some(cursor)) =
            (self.state, self.cursor)
        {
            // カーソルが中心とほぼ一致する間は方向不定なのでプレビューを描かない
            // （確定時の退化拒否と同じ基準。誤った右向き引出線を見せない）。
            if cursor.distance(center) <= DIM_DEGENERATE_EPSILON {
                return;
            }
            let leader_angle = (cursor - center).angle();
            let dim = DimRadial {
                center,
                radius,
                leader_angle,
            };
            let (arrow_len, text_height) = crate::dim_sizes(viewport.zoom);
            let ex = crate::dimension::expand_radial(&dim, arrow_len, text_height);
            crate::draw_dim_expansion(painter, rect, viewport, &ex, preview_stroke());
        }
    }

    fn wants_circle_pick(&self) -> bool {
        matches!(self.state, DimRadialState::WaitingCircle)
    }

    fn on_circle_pick(&mut self, hit: CirclePick) {
        if matches!(self.state, DimRadialState::WaitingCircle) {
            self.state = DimRadialState::WaitingLeader {
                center: hit.center,
                radius: hit.radius,
            };
        }
    }
}

/// クリック点 `world` から許容量 `tol`（ワールド）以内で最も近い可視な円／円弧の
/// 中心・半径を返す。半径寸法ツールの 1 クリック目（[`Tool::wants_circle_pick`]）で app 層が
/// 使う。ヒットテストは [`Document`] を要するため Tool ではなくここ（app 層）に置く。
/// 判定は [`SelectTool::pick`] と同じく「実形状への最近距離 ≤ tol かつ最も近い」統一契約に従う。
#[must_use]
pub fn pick_circle_or_arc(document: &Document, world: Point2, tol: f64) -> Option<CirclePick> {
    pick_nearest(document, tol, |_id, entity| {
        let shape = entity.geom.as_shape()?;
        let pick = match shape {
            Shape::Circle(c) => CirclePick {
                center: c.center,
                radius: c.radius,
            },
            Shape::Arc(a) => CirclePick {
                center: a.center,
                radius: a.radius,
            },
            _ => return None,
        };
        let d = distance_to(shape, world);
        Some((d, pick))
    })
}

/// クリック点 `world` から許容量 `tol`（ワールド）以内で最も近い可視な**任意形状**の
/// エンティティを [`ShapePick`] で返す（M7 タスク30、app 層の汎用エンティティピック基盤）。
///
/// 契約は [`SelectTool::pick`] と同じ「実形状への最短距離 ≤ tol の中で最も近い、可視
/// レイヤーのエンティティのみ」。`EntityGeom::as_shape()` が `None` を返すもの（`Text`・
/// 寸法）は自然に除外される（[`pick_circle_or_arc`] が Circle/Arc 以外を除外するのと同じ
/// 仕組み）。M7 のトリム・延長・フィレット・分割（タスク31〜33）が対象エンティティを拾う
/// 際の共通入口として使う（タスク31 で [`TrimTool`] / [`ExtendTool`] が利用開始。
/// フィレット・分割はタスク32/33 で追加する）。
#[must_use]
pub fn pick_shape_entity(document: &Document, world: Point2, tol: f64) -> Option<ShapePick> {
    pick_nearest(document, tol, |id, entity| {
        let shape = entity.geom.as_shape()?;
        let d = distance_to(shape, world);
        Some((
            d,
            ShapePick {
                id,
                shape: shape.clone(),
                click: world,
                layer: entity.layer,
                style: entity.style,
            },
        ))
    })
}

// ---------------------------------------------------------------------
// トリム（X）／延長（E）
// ---------------------------------------------------------------------

/// トリム（`X`）と延長（`E`）が共有する 2 段階状態（DESIGN.md M7 設計判断6）。
///
/// 1 クリック目で境界を、2 クリック目で対象をヒットテストする。**確定後も
/// `WaitingTarget` に留まる**ので、同じ境界に対して対象を変えながら連続適用できる
/// （AutoCAD の TRIM/EXTEND と同じ操作感）。
#[derive(Debug, Default, Clone, PartialEq)]
enum BoundaryTargetState {
    /// 境界エンティティのヒットテスト待ち（1 クリック目）。
    #[default]
    WaitingBoundary,
    /// 対象エンティティのヒットテスト待ち（2 クリック目以降）。境界の形状は採取済み。
    ///
    /// 境界は「交点計算の相手」としてしか使わないので [`EntityId`] ではなく [`Shape`] の
    /// スナップショットを持つ（半径寸法が中心・半径だけを写し取るのと同じ非関連方式）。
    /// このため undo/redo で境界エンティティ自体が消えるとスナップショットが実体と
    /// 食い違いうるが、app 層が undo/redo 後にツールを作り直して初期状態へ戻すことで
    /// 古い境界を持ち越さないようにしている（`main.rs` の `after_history_change`）。
    WaitingTarget { boundary: Shape },
}

/// [`BoundaryTargetTool`] が行う演算の種別。状態機械は完全に共通で、確定時に呼ぶ
/// geom 関数と ASCII メッセージだけが異なる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrimExtendOp {
    Trim,
    Extend,
}

/// トリム・延長に共通の状態機械の実体。[`TrimTool`] / [`ExtendTool`] が薄く包む。
#[derive(Debug)]
struct BoundaryTargetTool {
    op: TrimExtendOp,
    state: BoundaryTargetState,
    /// 直前の `Commit` が「元 1 件を削除して新規 2 件を追加する」トリムだったか。
    /// [`Tool::take_commit_selection`] が消費し、適用後の選択集合を新規 2 件へ載せ替える。
    select_new_entities: bool,
}

impl BoundaryTargetTool {
    fn new(op: TrimExtendOp) -> Self {
        Self {
            op,
            state: BoundaryTargetState::WaitingBoundary,
            select_new_entities: false,
        }
    }

    /// geom のエラーを ASCII のステータス文言へ対応付ける（geom は GUI 非依存で文言を
    /// 持たないため。`OffsetError` の扱いと同じ）。
    fn reject(self_op: TrimExtendOp, err: TrimExtendError) -> ToolResult {
        ToolResult::Rejected(match (self_op, err) {
            (TrimExtendOp::Trim, TrimExtendError::Unsupported) => {
                "Trim: target must be a line or an arc"
            }
            (TrimExtendOp::Trim, TrimExtendError::NoIntersection) => {
                "Trim: target does not cross the boundary"
            }
            (TrimExtendOp::Trim, TrimExtendError::Degenerate) => {
                "Trim: ambiguous click, pick a point away from the intersections"
            }
            (TrimExtendOp::Extend, TrimExtendError::Unsupported) => {
                "Extend: target must be a line or an arc"
            }
            (TrimExtendOp::Extend, TrimExtendError::NoIntersection) => {
                "Extend: boundary is not reachable in that direction"
            }
            (TrimExtendOp::Extend, TrimExtendError::Degenerate) => {
                "Extend: result would be degenerate"
            }
        })
    }

    /// 対象ピックを演算へ通し、確定コマンドまたは拒否を返す。状態は
    /// `WaitingTarget { boundary }` のまま据え置く（成功・失敗どちらでも）。
    fn apply_to_target(&mut self, boundary: &Shape, hit: ShapePick) -> ToolResult {
        match self.op {
            TrimExtendOp::Trim => match trim(&hit.shape, boundary, hit.click) {
                Ok(result) => self.commit_trim(&hit, result.retained),
                Err(err) => Self::reject(self.op, err),
            },
            TrimExtendOp::Extend => match extend(&hit.shape, boundary, hit.click) {
                Ok(shape) => ToolResult::Commit(Command::ModifyEntity {
                    id: hit.id,
                    new_geom: EntityGeom::from(shape),
                }),
                Err(err) => Self::reject(self.op, err),
            },
        }
    }

    /// トリム結果の断片数で確定コマンドを分岐する（DESIGN.md M7 設計判断3a）。
    fn commit_trim(&mut self, hit: &ShapePick, mut retained: Vec<Shape>) -> ToolResult {
        match retained.len() {
            // 1 断片: 「同じ物体が短くなっただけ」なので ID を維持する。
            1 => ToolResult::Commit(Command::ModifyEntity {
                id: hit.id,
                new_geom: EntityGeom::from(retained.remove(0)),
            }),
            // 2 断片: 1 エンティティが 2 つへ分かれてトポロジが変わるので、分割
            // （タスク33）と同じ「削除 + 追加 ×2」で表す。新規 2 件は元のレイヤー・
            // スタイルを複製し、確定後の選択集合を新規 2 件へ載せ替える。
            2 => {
                let second = retained.remove(1);
                let first = retained.remove(0);
                self.select_new_entities = true;
                ToolResult::Commit(Command::Batch(vec![
                    Command::RemoveEntity(hit.id),
                    Command::AddEntity(Entity::new(first, hit.layer, hit.style)),
                    Command::AddEntity(Entity::new(second, hit.layer, hit.style)),
                ]))
            }
            // 0 個・3 個以上は `TrimResult` の不変条件（1 か 2）に反する。geom 側が
            // `Degenerate` で弾いている想定なので通常は到達しないが、コマンドを
            // 作らず拒否する防御的分岐（設計判断3a）。
            _ => ToolResult::Rejected("Trim: nothing left to keep"),
        }
    }

    fn on_input(&mut self, ev: InputEvent) -> ToolResult {
        match ev {
            // Move はプレビューに使わない（ヒットテスト結果が無いと対象形状が分からず、
            // `Move` は Document を持たないため。DESIGN.md M7 設計判断6 の
            // 「ホバー中のライブプレビューは行わない」）。
            InputEvent::Move(_) | InputEvent::Confirm => ToolResult::Continue,
            InputEvent::Cancel => {
                self.state = BoundaryTargetState::WaitingBoundary;
                ToolResult::Cancel
            }
            // クリックは常にヒットテスト経路（`wants_shape_pick`）を通るのでここへは
            // 来ない想定。来ても状態は変えない（`DimRadialTool` と同じ扱い）。
            InputEvent::Click(_) => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        // 選択済みの境界だけをハイライトして「今どちらを選んだか」を示す。カーソル追従の
        // ライブプレビューは行わない（上記のとおり）。
        if let BoundaryTargetState::WaitingTarget { boundary } = &self.state {
            draw_shape(
                painter,
                rect,
                viewport,
                boundary,
                preview_stroke(),
                Linetype::Continuous,
                1.0,
            );
        }
    }

    fn on_shape_pick(&mut self, hit: ShapePick) -> ToolResult {
        match self.state.clone() {
            BoundaryTargetState::WaitingBoundary => {
                self.state = BoundaryTargetState::WaitingTarget {
                    boundary: hit.shape,
                };
                ToolResult::Continue
            }
            BoundaryTargetState::WaitingTarget { boundary } => self.apply_to_target(&boundary, hit),
        }
    }

    fn take_commit_selection(&mut self, new_ids: &NewIds) -> Option<Vec<EntityId>> {
        if std::mem::take(&mut self.select_new_entities) {
            Some(new_ids.entities.clone())
        } else {
            None
        }
    }

    /// [`Tool::on_commit_failed`] の実体。2断片トリムがコマンド作成時に立てた
    /// `select_new_entities` フラグを、`Document::apply` の失敗でクリアする
    /// （そのままだと次に成功した無関係の確定の選択集合を誤って載せ替えてしまう）。
    fn on_commit_failed(&mut self) {
        self.select_new_entities = false;
    }
}

/// トリムツール（`X`）。境界 → 対象の順にクリックし、**対象のクリックした側**を境界との
/// 交点まで切り落とす（DESIGN.md M7 設計判断3・3a・6）。
///
/// 確定後も同じ境界のまま対象待ちに留まるので、境界を選び直さずに連続してトリムできる。
/// 失敗（対象が Line/Arc 以外・交点なし・交点直上のクリック・レイヤーロック）は
/// ステータスへ ASCII で理由を出し、`Document` は変更しない。
#[derive(Debug)]
pub struct TrimTool(BoundaryTargetTool);

impl Default for TrimTool {
    fn default() -> Self {
        Self(BoundaryTargetTool::new(TrimExtendOp::Trim))
    }
}

/// 延長ツール（`E`）。境界 → 対象の順にクリックし、**クリックに近い側の自由端**を境界まで
/// 伸ばす（DESIGN.md M7 設計判断2・6）。状態機械はトリムと共通で、連続適用も同じ。
#[derive(Debug)]
pub struct ExtendTool(BoundaryTargetTool);

impl Default for ExtendTool {
    fn default() -> Self {
        Self(BoundaryTargetTool::new(TrimExtendOp::Extend))
    }
}

/// [`TrimTool`] / [`ExtendTool`] の [`Tool`] 実装は内側の共通状態機械へ丸ごと委譲する
/// （2 ツールの違いは [`TrimExtendOp`] だけ）。
macro_rules! impl_tool_for_boundary_target {
    ($ty:ty) => {
        impl Tool for $ty {
            fn on_input(&mut self, _ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
                self.0.on_input(ev)
            }

            fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
                self.0.draw_preview(painter, rect, viewport);
            }

            fn wants_shape_pick(&self) -> bool {
                true
            }

            fn on_shape_pick(&mut self, hit: ShapePick) -> ToolResult {
                self.0.on_shape_pick(hit)
            }

            fn take_commit_selection(&mut self, new_ids: &NewIds) -> Option<Vec<EntityId>> {
                self.0.take_commit_selection(new_ids)
            }

            fn on_commit_failed(&mut self) {
                self.0.on_commit_failed();
            }
        }
    };
}

impl_tool_for_boundary_target!(TrimTool);
impl_tool_for_boundary_target!(ExtendTool);

// ---------------------------------------------------------------------
// フィレット（F）
// ---------------------------------------------------------------------

/// [`FilletTool`] の状態（DESIGN.md M7 設計判断6 のフィレットの項）。
#[derive(Debug, Default, Clone, PartialEq)]
enum FilletState {
    /// 1 本目の線分のヒットテスト待ち。
    #[default]
    WaitingFirstLine,
    /// 2 本目の線分のヒットテスト待ち。1 本目は採取済み。
    ///
    /// 1 本目は幾何（`line`）だけでなく [`ShapePick`] 全体を保持する: `id` は確定コマンド
    /// （`ModifyEntity` の対象）と確定後の選択集合に、`click` は `fillet_lines` の `near_a`
    /// （残したい側の指示）に、`layer`/`style` は新しい弧の継承元（設計判断4）に要るため。
    /// 形状のスナップショットを抱えるので、undo/redo・ファイル操作の後は app 層が
    /// ツールを作り直して初期状態へ戻す（[`crate::ToolKind::caches_picked_shapes`]）。
    WaitingSecondLine { first: ShapePick, line: LineSeg },
}

/// フィレットツール（`F`）。2 本の線分を順にクリックし、半径入力欄の値で角を丸める
/// （DESIGN.md M7 設計判断4・6）。
///
/// 1 本目・2 本目とも「フィレット後に**残したい側**」をクリックする（クリック点が
/// `fillet_lines` の `near_a`/`near_b` になり、中心の分岐選択と残る側の両方を決める）。
/// 確定は `Command::Batch([ModifyEntity(a), ModifyEntity(b), AddEntity(arc)])` の 1 発で、
/// 3 つのうちどれか 1 つでも失敗（レイヤーロック等）すれば全体がロールバックされる。
/// 新しい弧のレイヤー・スタイルは **1 本目**のものを継承する（設計判断4）。
///
/// 確定後は `WaitingFirstLine` へ戻る単発仕様（角ごとに半径が異なりうるため、トリム・延長の
/// ような連続適用にはしない）。失敗は `Rejected` で状態を `WaitingSecondLine` に据え置き、
/// 1 本目を選び直さずに 2 本目だけ試し直せるようにする。
#[derive(Debug, Default)]
pub struct FilletTool {
    state: FilletState,
    /// app 層の半径入力欄の解析値（[`Tool::set_radius_input`] が毎フレーム更新する）。
    radius: Option<f64>,
    /// 直前の `Commit` で変更した 2 本の ID。[`Tool::take_commit_selection`] が消費し、
    /// 新規の弧（`NewIds.entities`）と合わせて確定後の選択集合にする。
    committed_pair: Option<(EntityId, EntityId)>,
    /// `apply_second_line` が確定コマンドを組み立てて `state` を `WaitingFirstLine` へ
    /// 進めた際、直前の `WaitingSecondLine { first, line }` のスナップショットをここへ
    /// 退避する。確定コマンドを作った時点では `Document::apply` が実際に成功するかは
    /// まだ分からない（それは main.rs が後で行う）ため、[`Tool::on_commit_failed`] が
    /// 呼ばれたらここから状態を巻き戻し、1本目を選び直さず2本目だけ試し直せるようにする
    /// （DESIGN.md M7設計判断6）。`committed_pair` と対で `take`/クリアする。
    pre_commit_state: Option<FilletState>,
}

impl FilletTool {
    /// 対象が線分ならその [`LineSeg`] を返す。フィレットは線分同士のみ対応
    /// （円弧×直線・円弧×円弧は M7 対象外。設計判断1）。
    fn as_line(shape: &Shape) -> Option<LineSeg> {
        match shape {
            Shape::Line(seg) => Some(*seg),
            _ => None,
        }
    }

    /// geom のエラーを ASCII のステータス文言へ対応付ける（`TrimExtendError` と同じパターン）。
    fn reject(err: FilletError) -> ToolResult {
        ToolResult::Rejected(match err {
            FilletError::Parallel => "Fillet: the two lines are parallel",
            FilletError::NonPositiveRadius => "Fillet: radius must be a positive number",
            FilletError::RadiusTooLarge => "Fillet: radius is too large for these lines",
            FilletError::Degenerate => "Fillet: cannot fillet there, click away from the corner",
        })
    }

    /// 2 本目のピックを受けて確定コマンドを組み立てる。成功時のみ状態を
    /// `WaitingFirstLine` へ戻す（失敗時は据え置き）。
    fn apply_second_line(&mut self, first: &ShapePick, a: LineSeg, hit: &ShapePick) -> ToolResult {
        let Some(b) = Self::as_line(&hit.shape) else {
            return ToolResult::Rejected("Fillet: second pick must be a line");
        };
        if hit.id == first.id {
            return ToolResult::Rejected("Fillet: pick two different lines");
        }
        let Some(radius) = self.radius else {
            return ToolResult::Rejected("Fillet: enter a radius");
        };
        let result = match fillet_lines(a, b, radius, first.click, hit.click) {
            Ok(result) => result,
            Err(err) => return Self::reject(err),
        };

        // 確定コマンドを作った時点ではまだ Document::apply の成否は分からないので、
        // 巻き戻し用に直前の状態を退避してから WaitingFirstLine へ進める
        // （`pre_commit_state` の doc / `Tool::on_commit_failed` 参照）。
        self.pre_commit_state = Some(FilletState::WaitingSecondLine {
            first: first.clone(),
            line: a,
        });
        self.state = FilletState::WaitingFirstLine;
        self.committed_pair = Some((first.id, hit.id));
        ToolResult::Commit(Command::Batch(vec![
            Command::ModifyEntity {
                id: first.id,
                new_geom: EntityGeom::from(Shape::Line(result.trimmed_a)),
            },
            Command::ModifyEntity {
                id: hit.id,
                new_geom: EntityGeom::from(Shape::Line(result.trimmed_b)),
            },
            // 新しい弧は 1 本目のレイヤー・スタイルを継承する（設計判断4）。
            Command::AddEntity(Entity::new(
                Shape::Arc(result.arc),
                first.layer,
                first.style,
            )),
        ]))
    }
}

impl Tool for FilletTool {
    fn on_input(&mut self, _ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            // トリム・延長と同じ理由でホバー中のライブプレビューは行わない
            // （`Move` は Document を持たず、ヒットするまで対象形状が分からない）。
            InputEvent::Move(_) | InputEvent::Confirm => ToolResult::Continue,
            InputEvent::Cancel => {
                self.state = FilletState::WaitingFirstLine;
                ToolResult::Cancel
            }
            // クリックは常にヒットテスト経路（`wants_shape_pick`）を通るのでここへは
            // 来ない想定。来ても状態は変えない。
            InputEvent::Click(_) => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        // 選んだ 1 本目だけをハイライトする（トリム・延長が境界を示すのと同じ流儀）。
        if let FilletState::WaitingSecondLine { first, .. } = &self.state {
            draw_shape(
                painter,
                rect,
                viewport,
                &first.shape,
                preview_stroke(),
                Linetype::Continuous,
                1.0,
            );
        }
    }

    fn wants_shape_pick(&self) -> bool {
        true
    }

    fn on_shape_pick(&mut self, hit: ShapePick) -> ToolResult {
        match self.state.clone() {
            FilletState::WaitingFirstLine => match Self::as_line(&hit.shape) {
                Some(line) => {
                    self.state = FilletState::WaitingSecondLine { first: hit, line };
                    ToolResult::Continue
                }
                None => ToolResult::Rejected("Fillet: first pick must be a line"),
            },
            FilletState::WaitingSecondLine { first, line } => {
                self.apply_second_line(&first, line, &hit)
            }
        }
    }

    fn take_commit_selection(&mut self, new_ids: &NewIds) -> Option<Vec<EntityId>> {
        let (id_a, id_b) = self.committed_pair.take()?;
        // 確定は成功したと確定したので、巻き戻し用スナップショットはもう不要。
        self.pre_commit_state = None;
        // 「変更された 2 本 + 新規の弧」の 3 件へ載せ替える（設計判断6）。弧の ID は
        // 確定時点では分からず `Document::apply` の戻り値にしか無いのでここで足す。
        let mut ids = vec![id_a, id_b];
        ids.extend(new_ids.entities.iter().copied());
        Some(ids)
    }

    fn set_radius_input(&mut self, radius: Option<f64>) {
        self.radius = radius;
    }

    /// `Document::apply` の失敗を受けて後始末する。2本目のピック確定時にセットした
    /// `committed_pair`（そのままだと次に成功した無関係の確定の選択集合を誤って
    /// 「変更された2本 + 新規の弧」へ載せ替えてしまう）をクリアし、`apply_second_line`
    /// が先取りで進めていた状態を `pre_commit_state` から `WaitingSecondLine` へ巻き戻す
    /// （DESIGN.md M7設計判断6: 失敗時は1本目を選び直さず2本目だけ試し直せる）。
    fn on_commit_failed(&mut self) {
        self.committed_pair = None;
        if let Some(previous) = self.pre_commit_state.take() {
            self.state = previous;
        }
    }
}

// ---------------------------------------------------------------------
// 分割（B）
// ---------------------------------------------------------------------

/// 分割ツール（`B`）。クリックで対象エンティティをヒットテストし、
/// `mcad_geom::closest_point` で対象上へ射影した点を分割点として 2 つに分ける
/// （DESIGN.md M7 設計判断6 の分割の項）。
///
/// トリム・延長・フィレットと違い「境界」「1本目」のような非対称な役割を持つ対象が
/// なく、状態は `WaitingTarget` 相当の単一状態のみ（`ShapePick` のスナップショットを
/// 跨いで保持しないため専用の enum は持たない）。確定後もそのまま同じ状態に留まり、
/// 続けて別のエンティティを分割できる。失敗（`SplitError::TooCloseToEndpoint`/
/// `SplitError::Unsupported`）は `Rejected` とし、状態は据え置く（単一状態なので
/// 実質何も変わらないが、他ツールと同じ「拒否時は確定せず据え置き」規約に合わせる）。
#[derive(Debug, Default)]
pub struct SplitTool {
    /// 直前の `Commit` が新規 2 件への選択切替を要求したか。
    /// [`Tool::take_commit_selection`] が消費し、次の確定へ持ち越さない。
    committed: bool,
}

impl SplitTool {
    /// geom のエラーを ASCII のステータス文言へ対応付ける（`TrimExtendError`/`FilletError`
    /// と同じパターン）。
    fn reject(err: SplitError) -> ToolResult {
        ToolResult::Rejected(match err {
            SplitError::TooCloseToEndpoint => "Split: too close to an endpoint",
            SplitError::Unsupported => "Split: target must be a line, arc, or open polyline",
        })
    }
}

impl Tool for SplitTool {
    fn on_input(&mut self, _ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            // トリム・延長・フィレットと同じ理由でホバー中のライブプレビューは行わない
            // （`Move` は Document を持たず、ヒットするまで対象形状が分からない）。
            InputEvent::Move(_) | InputEvent::Confirm => ToolResult::Continue,
            InputEvent::Cancel => ToolResult::Cancel,
            // クリックは常にヒットテスト経路（`wants_shape_pick`）を通るのでここへは
            // 来ない想定。来ても状態は変えない。
            InputEvent::Click(_) => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, _painter: &Painter, _rect: Rect, _viewport: &Viewport) {
        // 単一状態でハイライトすべき「選択済みの一部」が無いため、他ツールと違い
        // プレビュー描画自体を持たない。
    }

    fn wants_shape_pick(&self) -> bool {
        true
    }

    fn on_shape_pick(&mut self, hit: ShapePick) -> ToolResult {
        let projected = closest_point(&hit.shape, hit.click);
        match split(&hit.shape, projected) {
            Ok((piece1, piece2)) => {
                self.committed = true;
                ToolResult::Commit(Command::Batch(vec![
                    Command::RemoveEntity(hit.id),
                    Command::AddEntity(Entity::new(piece1, hit.layer, hit.style)),
                    Command::AddEntity(Entity::new(piece2, hit.layer, hit.style)),
                ]))
            }
            Err(err) => Self::reject(err),
        }
    }

    fn take_commit_selection(&mut self, new_ids: &NewIds) -> Option<Vec<EntityId>> {
        if std::mem::take(&mut self.committed) {
            Some(new_ids.entities.clone())
        } else {
            None
        }
    }

    /// 確定コマンド作成時にセットした `committed` フラグを、`Document::apply` の
    /// 失敗でクリアする（そのままだと次に成功した無関係の確定の選択集合を誤って
    /// 載せ替えてしまう）。
    fn on_commit_failed(&mut self) {
        self.committed = false;
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
///
/// 回転（[`PlacementKind::Rotate`]）・鏡映（[`PlacementKind::Mirror`]）も同じ機構を共有する
/// （M5 タスク19、設計判断4）。回転は pivot→基準点→回転先の3クリック、鏡映は軸2点の
/// 2クリックで、いずれも確定は選択集合の [`Command::ModifyEntity`] を 1 バッチにまとめる
/// （移動と同じく ID・選択は不変）。クリック数が種別で異なる（回転のみ3クリック）ため、
/// 進行段階は [`PlacementStage`] が最大3点まで表せるよう一般化してある。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementKind {
    /// 複製配置（Ctrl+D）。基準点→配置先の変位ぶん平行移動した複製を追加する（2クリック）。
    Duplicate,
    /// 移動配置（M）。基準点→配置先の変位ぶん、選択集合の幾何を平行移動する（2クリック）。
    Move,
    /// 回転（R）。pivot（回転中心）→基準点→回転先の3クリック。回転角は
    /// `angle(pivot→回転先) − angle(pivot→基準点)` の相対角（設計判断4）。
    Rotate,
    /// 鏡映（Shift+M）。軸点A→軸点B の2クリックで、その2点を通る直線に対して鏡映する。
    Mirror,
}

/// 2段階／3段階クリックの進行段階。点は種別ごとに意味が変わる:
///
/// - 複製・移動: `p1`=基準点、`p2`=配置先（`WaitingP2` で確定、2クリック）。
/// - 鏡映: `p1`=軸点A、`p2`=軸点B（`WaitingP2` で確定、2クリック）。
/// - 回転: `p1`=pivot、`p2`=基準点、`p3`=回転先（`WaitingP3` で確定、3クリック）。
///
/// `cursor` は最後に確定した点の次に来るクリック候補で、プレビュー追従に使う。
#[derive(Debug, Clone, Copy, PartialEq)]
enum PlacementStage {
    /// 1クリック目（`p1`）待ち。プレビューはまだ描かない。
    WaitingP1,
    /// 2クリック目（`p2`）待ち。`p1` は確定済み、`cursor` でプレビュー追従する。
    WaitingP2 { p1: Point2, cursor: Point2 },
    /// 3クリック目（`p3`）待ち（回転のみ）。`p1`・`p2` は確定済み、`cursor` で
    /// プレビュー追従する。
    WaitingP3 {
        p1: Point2,
        p2: Point2,
        cursor: Point2,
    },
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
    /// 移動・回転・鏡映は ID 不変なので選択をそのまま維持する）。`kind` はステータス文言の
    /// 出し分けにも使う。
    Commit {
        /// どの配置操作の確定か（複製／移動／回転／鏡映）。
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
    /// 回転プレビュー（選択集合を `pivot` 中心に `angle` ラジアン回転した先を仮表示する）。
    Rotate { pivot: Point2, angle: f64 },
    /// 鏡映プレビュー（選択集合を `axis_a`→`axis_b` の直線で鏡映した先を仮表示する）。
    Mirror { axis_a: Point2, axis_b: Point2 },
}

/// 回転の相対角がこの値（ラジアン）未満なら no-op として確定を拒否するしきい値。
///
/// pivot からの基準方向と回転先方向のなす角が実質ゼロのとき、確定しても見た目は
/// 変わらないのに ModifyEntity バッチと undo 履歴だけが増える。それを防ぐための下限。
/// `1e-6 rad ≈ 5.7e-5 度` は、実用上のどのズーム・半径でも人が知覚できない微小回転で、
/// かつ f64 の角度計算誤差より十分大きい（安全に「効果なし」と見なせる）。同一 ray 上の
/// 半径ちがい（外積ゼロ → 相対角ちょうど 0）も確実にこのゲートで弾ける。
const ROTATE_MIN_ANGLE: f64 = 1e-6;

/// `pivot` を中心に、`from` 方向から `to` 方向へ回すのに必要な符号付き相対角を返す。
///
/// `atan2(cross, dot)` で求めるため、結果は正規化済みの `(-π, π]`。生の
/// `angle(to−pivot) − angle(from−pivot)` と違い ±π 境界をまたいでも安定し、各ベクトルの
/// 長さ（pivot からの距離）に依存せず**方向のみ**で決まる。回転先が同一 ray 上（外積ゼロ、
/// 内積正）なら 0 を、真反対（内積負、外積ゼロ）なら π を返す。
fn relative_angle(pivot: Point2, from: Point2, to: Point2) -> f64 {
    let a = from - pivot;
    let b = to - pivot;
    a.cross(b).atan2(a.dot(b))
}

// ---------------------------------------------------------------------
// オフセットモード（単一エンティティに対する1クリック操作）
// ---------------------------------------------------------------------

/// 進行中のオフセットモード（M5 タスク20、設計判断5）。
///
/// # なぜ [`Placement`] とは別機構にするのか
///
/// 複製・移動・回転・鏡映（[`PlacementKind`]）は「選択集合全体を 2〜3 クリックの
/// 変位／角度／軸で変換する」操作で、[`PlacementStage`] の `delta`/`angle` 意味論を
/// 共有できる。一方オフセットは本質的に異なる:
///
/// - **対象は単一エンティティ**（起動時にちょうど1個だったもの）で、選択集合の一括
///   変換ではない。
/// - **1クリックで確定**する（基準点→配置先のような多段クリックではない）。
/// - **距離入力欄**（アプリUI側の文字列）と連動し、値があれば距離固定＋クリックは側の
///   決定のみ、空なら通過点方式、という分岐を持つ。
/// - **確定は元を変えず結果を `AddEntity`** で追加し、選択は元エンティティのまま維持する
///   （複製の「新IDを選択」とも、移動などの「同一IDを変換」とも違う）。
/// - 退化拒否がオフセット固有（半径消滅・ゼロ長・距離ゼロ）で、幾何は
///   [`Shape::offset`] が [`OffsetError`] で返す。
///
/// これらを [`PlacementKind`] に押し込むと、クリック数・対象数・後処理・入力欄連携の
/// すべてに種別分岐が必要になり、せっかく揃った配置ステート機械の意味論を汚す。そこで
/// オフセットは独立した小さなステート（本型 1 個）として持ち、[`SelectTool::is_placing`]
/// と同じ入力ゲート思想（[`SelectTool::is_offsetting`]）だけを踏襲する。
#[derive(Debug, Clone, Copy, PartialEq)]
struct OffsetState {
    /// オフセット対象。起動時にちょうど1個だった選択エンティティの ID。
    target: EntityId,
    /// カーソル位置（プレビュー追従用）。起動直後は原点で、最初の [`SelectTool::offset_move`]
    /// で実カーソルへ更新される。
    cursor: Point2,
}

/// [`SelectTool::offset_click`] の結果。呼び出し側（`McadApp`）が確定・キャンセルを処理する。
#[derive(Debug, Clone, PartialEq)]
pub enum OffsetOutcome {
    /// 退化入力でモードをキャンセルした。ASCII メッセージを表示する（undo 履歴は作らない）。
    Cancelled(&'static str),
    /// 確定コマンド（オフセット結果の `AddEntity`）。呼び出し側が `Document::apply` する。
    /// 元エンティティは変更せず、レイヤー・スタイルを複製して幾何のみ差し替えた複製を追加する。
    Commit(Command),
}

/// 通過点方式での、通過点 `reference` から対象 `geom` までのオフセット距離（設計判断5）。
///
/// # 円・円弧は放射方向で定義する
///
/// 円・円弧は距離を `| |reference − center| − r |`（放射距離）、側を中心との内外で定義する。
/// これは [`Shape::offset`]（[`mcad_geom::Circle::offset`] / [`mcad_geom::Arc::offset`]）が
/// 「中心・角度を保った同心円/同心弧へ半径を ± d する」規則と一致するため、通過点が確実に
/// 結果上へ載る。有限弧への最近点距離（[`distance_to`]）を使うと、クリック角が掃引範囲外の
/// ときに最近点が端点になって接線方向成分が距離へ混入し、「通過点を通る」契約と矛盾する。
/// なお掃引範囲外をクリックした場合、結果は同心弧なので通過点（の角度）は結果の弧には
/// 載らない（放射距離は正しく取れるが弧の角度範囲外）。これは仕様として許容する。
///
/// 線分・ポリラインは最近点までの垂線距離（[`distance_to`]）をそのまま使う。
fn through_point_distance(geom: &Shape, reference: Point2) -> f64 {
    match geom {
        Shape::Circle(c) => (reference.distance(c.center) - c.radius).abs(),
        Shape::Arc(a) => (reference.distance(a.center) - a.radius).abs(),
        _ => distance_to(geom, reference),
    }
}

/// オフセットの距離と側の参照点を、距離入力欄の状態から決める（設計判断5）。
///
/// - `fixed_distance` が `Some(d)`（正の有限値）: 距離は `d`、側は `reference`（クリック／
///   カーソル）で決める（数値入力方式）。
/// - `None`: `reference` を通過点とみなし、距離 = [`through_point_distance`]（円・円弧は
///   放射距離、それ以外は最短距離）、側 = `reference`（通過点方式）。
fn offset_params(geom: &Shape, reference: Point2, fixed_distance: Option<f64>) -> (f64, Point2) {
    match fixed_distance {
        Some(d) => (d, reference),
        None => (through_point_distance(geom, reference), reference),
    }
}

/// [`OffsetError`] を ASCII ステータスメッセージへ対応付ける（egui 既定フォントは CJK 非対応
/// のため可視文字列は ASCII 限定）。文言は geom 側に持たせず UI 側で決める。
fn offset_error_message(err: OffsetError) -> &'static str {
    match err {
        OffsetError::NonPositiveDistance => "Zero offset distance - offset cancelled",
        OffsetError::RadiusCollapse => "Distance too large for inner offset - cancelled",
        OffsetError::Degenerate => "Zero-length target - offset cancelled",
        OffsetError::Unsupported => "Cannot offset a point",
    }
}

/// 選択・編集ツール。単一選択（クリック）・矩形選択（ドラッグ）・移動（M、2クリック
/// 配置）・複製（Ctrl+D、2クリック配置）・回転（R、3クリック）・鏡映（Shift+M、軸2点）・
/// 削除（Delete/Backspace）を担う。設計判断の詳細は [`DragState`] と [`Placement`] の doc を参照。
#[derive(Debug, Default)]
pub struct SelectTool {
    /// 現在の選択集合（アプリ UI 状態。ドキュメント履歴には積まない）。
    selection: Vec<EntityId>,
    /// 進行中の矩形選択ドラッグ。無ければ `None`。
    drag: Option<DragState>,
    /// 進行中の配置モード（複製・移動など2段階クリック）。無ければ `None`。
    /// アクティブな間は通常のクリック選択・矩形選択より優先される（入力ゲート）。
    placement: Option<Placement>,
    /// 進行中のオフセットモード（`O`、単一エンティティ対象の1クリック操作）。無ければ `None`。
    /// 配置モードと同様、アクティブな間は通常の選択入力より優先される（入力ゲート）。
    offset: Option<OffsetState>,
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
        // ヒット距離: Shape は実形状への最近距離、Text は近似 aabb への符号なし距離
        // （DESIGN.md M6 L429 の割り切り。内部なら 0、外部なら境界までのユークリッド距離。
        // これにより tol 内なら枠のすぐ外側も拾えるし、枠内の空白よりも実形状が近ければ
        // そちらを優先できる）。寸法は展開線分（寸法線・補助線・引出線）への最近距離
        // （`dimension` の純関数）。いずれも「tol 以内で最も近いものを拾う」統一比較に
        // 素直に載る連続距離で、2 値判定（線の上/外）にはしない（M6 タスク23 の教訓）。
        pick_nearest(document, tol, |id, entity| {
            let d = match &entity.geom {
                EntityGeom::Shape(shape) => distance_to(shape, world),
                EntityGeom::Text(_) => entity.geom.aabb().distance_to_point(world),
                EntityGeom::DimLinear(dim) => crate::dimension::linear_distance(dim, world),
                EntityGeom::DimRadial(dim) => crate::dimension::radial_distance(dim, world),
                // `EntityGeom` は `#[non_exhaustive]`。未知の幾何は近似 aabb への
                // 距離で拾う（ピック対象から黙って消えるより穏当）。
                _ => entity.geom.aabb().distance_to_point(world),
            };
            Some((d, id))
        })
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
        // オフセットモードとは排他（同時には進行しない）。
        self.offset = None;
        self.placement = Some(Placement {
            kind,
            stage: PlacementStage::WaitingP1,
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

    /// R: 回転モードを開始する（[`SelectTool::start_placement`] 参照）。pivot→基準点→回転先の
    /// 3クリックで、選択集合を pivot 中心に相対角ぶん回転する（設計判断4）。
    pub fn start_rotate(&mut self) -> bool {
        self.start_placement(PlacementKind::Rotate)
    }

    /// Shift+M: 鏡映モードを開始する（[`SelectTool::start_placement`] 参照）。軸点A→軸点B の
    /// 2クリックで、その2点を通る直線に対して選択集合を鏡映する（設計判断4）。
    pub fn start_mirror(&mut self) -> bool {
        self.start_placement(PlacementKind::Mirror)
    }

    /// 配置モードのカーソル位置を更新する（プレビュー追従用）。1クリック目待ち
    /// （`WaitingP1`）や非配置中は何もしない。
    pub fn placement_move(&mut self, world: Point2) {
        match &mut self.placement {
            Some(Placement {
                stage: PlacementStage::WaitingP2 { cursor, .. },
                ..
            })
            | Some(Placement {
                stage: PlacementStage::WaitingP3 { cursor, .. },
                ..
            }) => *cursor = world,
            _ => {}
        }
    }

    /// 配置モードでのクリック確定を1つ進める。
    ///
    /// - 1クリック目（`WaitingP1`）: `world` を `p1` として確定し、次のクリック待ちへ。
    /// - 回転の2クリック目（`WaitingP2`）: `world` を基準点として確定し、回転先待ち
    ///   （`WaitingP3`）へ進む。基準点が pivot とほぼ同一（`<= tol`）なら基準方向が
    ///   定まらないためモードをキャンセルする（絶対角に化けるのを防ぐ）。
    /// - 複製・移動の2クリック目（`WaitingP2`）: `delta = world - p1` を変位として確定する。
    ///   基準点と配置先がほぼ同一（`<= tol`）なら見えない結果を防ぐためキャンセルする。
    /// - 鏡映の2クリック目（`WaitingP2`）: `p1`→`world` を軸として確定する。軸2点が
    ///   ほぼ同一（`<= tol`）なら軸が定まらないためキャンセルする。
    /// - 回転の3クリック目（`WaitingP3`）: 回転先を確定し、`angle(pivot→回転先) −
    ///   angle(pivot→基準点)` を回転角とする。回転先が基準点とほぼ同一（`<= tol`、回転角
    ///   実質ゼロ）ならキャンセルする。
    ///
    /// 確定コマンドは種別による:
    /// - 複製（[`PlacementKind::Duplicate`]）: 選択中の各エンティティのレイヤー・スタイルを
    ///   複製し、幾何のみ `translated(delta)` に置き換えた `AddEntity` の [`Command::Batch`]。
    /// - 移動・回転・鏡映: 選択中の各エンティティの幾何を `translated`／`rotated`／`mirrored`
    ///   に置き換える `ModifyEntity` の [`Command::Batch`]（ID は不変）。
    ///
    /// 呼び出し側が `Document::apply` する（レイヤーロック時は Batch 原子性で全体が失敗し、
    /// 位置も選択も変わらない）。同一点入力でのキャンセルは Document を変更せず、undo 履歴も
    /// 作らない（設計判断4）。
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
            PlacementStage::WaitingP1 => {
                self.placement = Some(Placement {
                    kind: placement.kind,
                    stage: PlacementStage::WaitingP2 {
                        p1: world,
                        cursor: world,
                    },
                });
                PlacementOutcome::Continue
            }
            PlacementStage::WaitingP2 { p1, .. } => {
                // 回転は基準点を確定して3クリック目へ進むだけ（まだ確定しない）。
                if placement.kind == PlacementKind::Rotate {
                    if p1.distance(world) <= tol {
                        self.placement = None;
                        return PlacementOutcome::Cancelled(
                            "Reference equals pivot - rotate cancelled",
                        );
                    }
                    self.placement = Some(Placement {
                        kind: placement.kind,
                        stage: PlacementStage::WaitingP3 {
                            p1,
                            p2: world,
                            cursor: world,
                        },
                    });
                    return PlacementOutcome::Continue;
                }
                // 複製・移動・鏡映は2クリック目で確定する。
                if p1.distance(world) <= tol {
                    self.placement = None;
                    return PlacementOutcome::Cancelled(match placement.kind {
                        PlacementKind::Duplicate => "Zero displacement - duplicate cancelled",
                        PlacementKind::Move => "Zero displacement - move cancelled",
                        PlacementKind::Mirror => "Zero-length axis - mirror cancelled",
                        PlacementKind::Rotate => unreachable!("rotate handled above"),
                    });
                }
                let delta = world - p1;
                let cmd = match placement.kind {
                    PlacementKind::Duplicate => self.build_duplicate_command(document, delta),
                    PlacementKind::Move => {
                        self.build_modify_command(document, |g| g.translated(delta))
                    }
                    PlacementKind::Mirror => {
                        self.build_modify_command(document, |g| g.mirrored(p1, world))
                    }
                    PlacementKind::Rotate => unreachable!("rotate handled above"),
                };
                self.finish_commit(placement.kind, cmd)
            }
            PlacementStage::WaitingP3 { p1, p2, .. } => {
                // 回転3クリック目: pivot=p1, 基準点=p2, 回転先=world。
                //
                // ゲートは「回転として意味を持つか」で判定し、距離ではなく方向・角度で見る:
                // 1. 回転先 ≈ pivot なら pivot からの方向が定義不能（ゼロベクトルの角度で
                //    任意の角度を作ってしまう）ため拒否する。
                // 2. 正規化相対角（`relative_angle`）がほぼゼロなら、no-op の ModifyEntity
                //    バッチと空の undo 履歴を作らないよう拒否する。同一 ray・半径ちがい
                //    （距離は大きいが実角度ゼロ）もこれで確実に弾ける。
                if p1.distance(world) <= tol {
                    self.placement = None;
                    return PlacementOutcome::Cancelled("Target equals pivot - rotate cancelled");
                }
                let angle = relative_angle(p1, p2, world);
                if angle.abs() < ROTATE_MIN_ANGLE {
                    self.placement = None;
                    return PlacementOutcome::Cancelled("Zero rotation angle - rotate cancelled");
                }
                let cmd = self.build_modify_command(document, |g| g.rotated(p1, angle));
                self.finish_commit(placement.kind, cmd)
            }
        }
    }

    /// 生成した確定コマンドを畳んで [`PlacementOutcome`] にする共通処理。配置モードを解除し、
    /// コマンドがあれば `Commit`、選択が全て死んだ ID 等で空なら `Cancelled` を返す。
    fn finish_commit(&mut self, kind: PlacementKind, cmd: Option<Command>) -> PlacementOutcome {
        self.placement = None;
        match cmd {
            Some(cmd) => PlacementOutcome::Commit { kind, cmd },
            // 選択が全て死んだ ID だった等でコマンドが空。静かにキャンセルする。
            None => PlacementOutcome::Cancelled(match kind {
                PlacementKind::Duplicate => "Nothing to duplicate",
                PlacementKind::Move => "Nothing to move",
                PlacementKind::Rotate => "Nothing to rotate",
                PlacementKind::Mirror => "Nothing to mirror",
            }),
        }
    }

    /// 配置モードを解除する（Esc・ツール切替・ファイル操作・モーダル表示から呼ぶ）。
    /// Document は一切変更しない。選択集合も変えない。
    pub fn cancel_placement(&mut self) {
        self.placement = None;
    }

    // --- オフセットモード（O、単一エンティティ1クリック。設計判断5） ---

    /// `O`: オフセットモードを開始する。選択が**ちょうど1個**のときだけ起動して `true`。
    /// 空・複数のときは何もせず `false`（呼び出し側が ASCII ステータスメッセージを出す）。
    ///
    /// 通過点1点に対して複数対象の距離が一意に定まらないため、対象は単一に限定する
    /// （AutoCAD も1対象ずつ）。進行中のドラッグ・配置モードは両立しないため破棄する。
    pub fn start_offset(&mut self) -> bool {
        if self.selection.len() != 1 {
            return false;
        }
        self.drag = None;
        self.placement = None;
        self.offset = Some(OffsetState {
            target: self.selection[0],
            cursor: Point2::ORIGIN,
        });
        true
    }

    /// オフセットモードが進行中か。`McadApp` はこれで通常のクリック選択・矩形選択を
    /// ゲートし、オフセット用の入力経路だけを通す（[`SelectTool::is_placing`] と同じ思想）。
    #[must_use]
    pub fn is_offsetting(&self) -> bool {
        self.offset.is_some()
    }

    /// オフセットのプレビュー用カーソル位置を更新する。非オフセット中は何もしない。
    pub fn offset_move(&mut self, world: Point2) {
        if let Some(state) = &mut self.offset {
            state.cursor = world;
        }
    }

    /// オフセットモードを解除する（Esc・ツール切替・ファイル操作・モーダル表示から呼ぶ）。
    /// Document は一切変更しない。選択集合も変えない。
    pub fn cancel_offset(&mut self) {
        self.offset = None;
    }

    /// オフセットのプレビュー形状（いま確定した場合の結果ゴースト）。退化して結果を
    /// 作れない・対象が消えている・オフセット中でない場合は `None`（ゴーストを描かない）。
    ///
    /// `fixed_distance` は距離入力欄の解析値（`Some(正の有限値)` なら固定距離＋カーソル側、
    /// `None` ならカーソルを通過点とみなす通過点方式。[`offset_params`] 参照）。
    #[must_use]
    pub fn offset_preview(
        &self,
        document: &Document,
        fixed_distance: Option<f64>,
    ) -> Option<Shape> {
        let state = self.offset?;
        let entity = document.entity(state.target)?;
        // M6: オフセット対象は Shape 系のみ（テキスト・寸法はオフセット対象外）。
        let shape = entity.geom.as_shape()?;
        let (distance, toward) = offset_params(shape, state.cursor, fixed_distance);
        shape.offset(distance, toward).ok()
    }

    /// オフセットモードでのクリック確定。単一クリックで結果を確定するかキャンセルする
    /// （多段クリックではないので `Continue` は返さない）。確定・キャンセルいずれでも
    /// オフセットモードは解除する。選択集合は元エンティティのまま維持する（設計判断5:
    /// 同一元からの等間隔連続オフセットを `O` 再押下で繰り返しやすくするため）。
    ///
    /// - `fixed_distance` が `None`（通過点方式）で通過点が対象上（ピック許容量 `tol` 内）
    ///   なら、距離ゼロの no-op を防ぐためキャンセルする。
    /// - それ以外は [`Shape::offset`] を呼び、`Ok` なら元のレイヤー・スタイルを複製して
    ///   幾何のみ差し替えた `AddEntity` を返し、`Err`（半径消滅・ゼロ長など）は
    ///   ASCII メッセージでキャンセルする。
    #[must_use]
    pub fn offset_click(
        &mut self,
        document: &Document,
        world: Point2,
        tol: f64,
        fixed_distance: Option<f64>,
    ) -> OffsetOutcome {
        let Some(state) = self.offset else {
            return OffsetOutcome::Cancelled("No offset in progress");
        };
        // 対象が（undo 等で）消えていれば静かに終了する。
        let Some(entity) = document.entity(state.target) else {
            self.offset = None;
            return OffsetOutcome::Cancelled("Offset target missing");
        };
        // M6: オフセット対象は Shape 系のみ。テキスト・寸法は非対応としてキャンセルする。
        let Some(shape) = entity.geom.as_shape() else {
            self.offset = None;
            return OffsetOutcome::Cancelled("Offset not supported for this entity");
        };
        // 通過点方式で通過点が対象上（ピック許容量内）なら距離ゼロの no-op を拒否する。
        // 距離は側の判定と同じ [`through_point_distance`]（円・円弧は放射距離）で測るので、
        // 弧の半径上を掃引外にクリックした場合（放射距離ほぼゼロ）もここで確実に弾ける。
        // geom の EPS(1e-9) より粗いピック許容量で先に弾く（設計判断5）。
        if fixed_distance.is_none() && through_point_distance(shape, world) <= tol {
            self.offset = None;
            return OffsetOutcome::Cancelled("Zero offset distance - offset cancelled");
        }
        let (distance, toward) = offset_params(shape, world, fixed_distance);
        let result = shape.offset(distance, toward);
        // クリックで必ずモードを抜ける（成功でもキャンセルでも）。
        self.offset = None;
        match result {
            Ok(geom) => {
                // 元エンティティのレイヤー・スタイルを丸ごと複製し、幾何のみ差し替える。
                let mut copy = entity.clone();
                copy.geom = EntityGeom::Shape(geom);
                OffsetOutcome::Commit(Command::AddEntity(copy))
            }
            Err(err) => OffsetOutcome::Cancelled(offset_error_message(err)),
        }
    }

    /// 配置モードのプレビュー情報。プレビュー対象の段階でなければ `None`。
    ///
    /// - 複製・移動・鏡映: 1点確定後（`WaitingP2`）からカーソル追従でプレビューする。
    /// - 回転: 基準点まで確定した後（`WaitingP3`）からプレビューする（設計判断4:
    ///   「2クリック目以降」＝基準点確定後に相対角ゴーストを描く）。基準点選択中
    ///   （`WaitingP2`）はまだ回転角が定まらないためゴーストを描かない。
    #[must_use]
    pub fn placement_preview(&self) -> Option<PlacementPreview> {
        match self.placement? {
            Placement {
                kind: PlacementKind::Duplicate,
                stage: PlacementStage::WaitingP2 { p1, cursor },
            } => Some(PlacementPreview::Duplicate { delta: cursor - p1 }),
            Placement {
                kind: PlacementKind::Move,
                stage: PlacementStage::WaitingP2 { p1, cursor },
            } => Some(PlacementPreview::Move { delta: cursor - p1 }),
            Placement {
                kind: PlacementKind::Mirror,
                stage: PlacementStage::WaitingP2 { p1, cursor },
            } => Some(PlacementPreview::Mirror {
                axis_a: p1,
                axis_b: cursor,
            }),
            Placement {
                kind: PlacementKind::Rotate,
                stage: PlacementStage::WaitingP3 { p1, p2, cursor },
            } => Some(PlacementPreview::Rotate {
                pivot: p1,
                // 確定と同じ正規化相対角でゴーストを回し、±π 境界での飛びをなくす。
                angle: relative_angle(p1, p2, cursor),
            }),
            _ => None,
        }
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

    /// 選択中の各エンティティの幾何を `transform` で変換する `ModifyEntity` を 1 バッチに
    /// まとめる（ID は不変）。生存している選択物が無ければ `None`。移動（`translated`）・
    /// 回転（`rotated`）・鏡映（`mirrored`）が共有する、複製（`AddEntity`）と対になる編集版。
    fn build_modify_command(
        &self,
        document: &Document,
        transform: impl Fn(&EntityGeom) -> EntityGeom,
    ) -> Option<Command> {
        let subs: Vec<Command> = self
            .selection
            .iter()
            .filter_map(|&id| {
                document.entity(id).map(|e| Command::ModifyEntity {
                    id,
                    new_geom: transform(&e.geom),
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
    use mcad_core::{Document, TextGeom};

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
            ToolResult::Commit(Command::AddEntity(entity)) => match entity.geom {
                EntityGeom::Shape(shape) => shape,
                other => panic!("expected Shape geom, got {other:?}"),
            },
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

    // --- 配置モード（R 回転） ---

    /// `PlacementOutcome::Commit { cmd: Batch(..), .. }` から `ModifyEntity` の `(id, new_geom)`
    /// 群を取り出す（テスト専用）。想定外の形なら panic する。
    fn modify_pairs(outcome: &PlacementOutcome) -> Vec<(EntityId, EntityGeom)> {
        match outcome {
            PlacementOutcome::Commit {
                cmd: Command::Batch(subs),
                ..
            } => subs
                .iter()
                .map(|c| match c {
                    Command::ModifyEntity { id, new_geom } => (*id, new_geom.clone()),
                    other => panic!("expected ModifyEntity, got {other:?}"),
                })
                .collect(),
            other => panic!("expected Commit(Batch), got {other:?}"),
        }
    }

    #[test]
    fn start_rotate_requires_nonempty_selection() {
        let mut doc = Document::new();
        let _a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();

        // 空選択では回転モードに入らない（呼び出し側が案内メッセージを出す）。
        assert!(!tool.start_rotate());
        assert!(!tool.is_placing());

        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.start_rotate());
        assert!(tool.is_placing());
    }

    #[test]
    fn rotate_three_click_flow_commits_rotated_geometry() {
        // pivot=(0,0), 基準点=(1,0)（角度 0）, 回転先=(0,1)（角度 π/2）→ 相対角 +π/2。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0); // 線分 (0,0)-(1,0)
        let before_a = doc.entity(a).unwrap().geom.clone();

        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert_eq!(tool.selection(), &[a]);
        let selection_before = tool.selection().to_vec();

        assert!(tool.start_rotate());
        // 1クリック目=pivot。プレビューはまだ無い（角度未定）。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        assert!(tool.placement_preview().is_none());
        // 2クリック目=基準点。ここで初めて回転先待ちへ進む。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        // 基準点確定後はカーソル追従で相対角ゴーストが出る。
        tool.placement_move(Point2::new(0.0, 1.0));
        match tool.placement_preview() {
            Some(PlacementPreview::Rotate { pivot, angle }) => {
                assert_eq!(pivot, Point2::new(0.0, 0.0));
                assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
            }
            other => panic!("expected Rotate preview, got {other:?}"),
        }

        // 3クリック目=回転先(0,1)。相対角 +π/2 の ModifyEntity バッチを返す。
        let angle = std::f64::consts::FRAC_PI_2;
        let outcome = tool.placement_click(&doc, Point2::new(0.0, 1.0), 0.1);
        let PlacementOutcome::Commit { kind, .. } = &outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        assert_eq!(*kind, PlacementKind::Rotate);
        let pairs = modify_pairs(&outcome);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, a);
        assert_eq!(pairs[0].1, before_a.rotated(Point2::new(0.0, 0.0), angle));

        // ID 不変なので選択は維持される。
        assert!(!tool.is_placing());
        assert_eq!(tool.selection(), selection_before.as_slice());
    }

    #[test]
    fn rotate_apply_and_undo_is_single_unit() {
        // 検収シナリオ: 選択 → R → 3クリック → apply、undo 1 回で回転が全て戻る。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 10.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let before_b = doc.entity(b).unwrap().geom.clone();

        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(12.0, 1.0));
        assert_eq!(tool.selection().len(), 2);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1); // pivot
        let _ = tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1); // 基準点
        let outcome = tool.placement_click(&doc, Point2::new(0.0, 1.0), 0.1); // 回転先
        let PlacementOutcome::Commit { cmd, .. } = outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        doc.apply(cmd).unwrap();

        let angle = std::f64::consts::FRAC_PI_2;
        let pivot = Point2::new(0.0, 0.0);
        assert_eq!(doc.entity(a).unwrap().geom, before_a.rotated(pivot, angle));
        assert_eq!(doc.entity(b).unwrap().geom, before_b.rotated(pivot, angle));

        // 1 バッチなので undo 1 回で両方元へ戻る。
        assert!(doc.undo());
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
        assert_eq!(doc.entity(b).unwrap().geom, before_b);
    }

    #[test]
    fn rotate_reference_equal_to_pivot_cancels() {
        // 基準点が pivot とほぼ同一だと基準方向が定まらない（絶対角に化ける）ため拒否する。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        assert!(tool.start_rotate());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(5.0, 5.0), 0.1),
            PlacementOutcome::Continue
        );
        // 基準点が pivot(5,5) と tol=0.1 以内。
        let outcome = tool.placement_click(&doc, Point2::new(5.05, 5.0), 0.1);
        assert_eq!(
            outcome,
            PlacementOutcome::Cancelled("Reference equals pivot - rotate cancelled")
        );
        assert!(!tool.is_placing());
        // Document は変わらない（履歴も作らない）。
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
    }

    #[test]
    fn rotate_zero_angle_cancels() {
        // 回転先が基準点とほぼ同一 → 回転角ゼロ。見えない結果を防いで拒否する。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1); // pivot
        assert_eq!(
            tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1), // 基準点
            PlacementOutcome::Continue
        );
        // 回転先が基準点(1,0) と tol=0.1 以内。
        let outcome = tool.placement_click(&doc, Point2::new(1.05, 0.0), 0.1);
        assert_eq!(
            outcome,
            PlacementOutcome::Cancelled("Zero rotation angle - rotate cancelled")
        );
        assert!(!tool.is_placing());
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
    }

    #[test]
    fn rotate_target_equal_to_pivot_cancels() {
        // 回転先が pivot とほぼ同一だと pivot からの方向が定義不能。基準点が pivot から
        // 遠くてもゼロベクトルの角度で任意の角を作らないよう、方向定義不能として拒否する。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let gen_before = doc.generation();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1); // pivot
        // 基準点は pivot から遠い（(2,0)）ので基準点ゲートは通過する。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(2.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        // 回転先が pivot(0,0) と tol=0.1 以内。
        let outcome = tool.placement_click(&doc, Point2::new(0.05, 0.0), 0.1);
        assert_eq!(
            outcome,
            PlacementOutcome::Cancelled("Target equals pivot - rotate cancelled")
        );
        assert!(!tool.is_placing());
        // Document も undo 履歴（世代）も変化しない。
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
        assert_eq!(doc.generation(), gen_before);
    }

    #[test]
    fn rotate_same_ray_different_radius_cancels() {
        // 回転先が基準点と同一 ray（同方向）で半径だけ違うと、2点間距離は tol を大きく
        // 超えるが実相対角はゼロ。距離ではなく正規化相対角で判定するので no-op を弾ける。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let gen_before = doc.generation();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1); // pivot
        assert_eq!(
            tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1), // 基準点（+x ray, 半径1）
            PlacementOutcome::Continue
        );
        // 回転先は同じ +x ray 上の半径5（距離4、tol を大きく超える）だが相対角はゼロ。
        let outcome = tool.placement_click(&doc, Point2::new(5.0, 0.0), 0.1);
        assert_eq!(
            outcome,
            PlacementOutcome::Cancelled("Zero rotation angle - rotate cancelled")
        );
        assert!(!tool.is_placing());
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
        assert_eq!(doc.generation(), gen_before);
    }

    #[test]
    fn rotate_pi_boundary_commits() {
        // 相対角 π ちょうど（真反対の ray）でも生の角度差ではなく atan2 で安定に求まり、
        // ±π 境界で破綻せず確定する。線分 (0,0)-(1,0) を原点まわり π 回転 → (0,0)-(-1,0)。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0); // (0,0)-(1,0)
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1); // pivot
        let _ = tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1); // 基準点（+x）
        // 回転先は真反対の -x ray。相対角は +π。
        let outcome = tool.placement_click(&doc, Point2::new(-1.0, 0.0), 0.1);
        let pairs = modify_pairs(&outcome);
        assert_eq!(pairs.len(), 1);
        assert_eq!(
            pairs[0].1,
            before_a.rotated(Point2::new(0.0, 0.0), std::f64::consts::PI)
        );
    }

    #[test]
    fn rotate_near_pivot_large_angle_commits() {
        // pivot 近傍（小半径）だと基準点と回転先の距離は tol 以内になりうるが、角度差は
        // 大きい。距離ゲートなら誤って拒否するところを、角度ゲートは正しく確定させる。
        // tol=0.1、半径 0.11（>tol）で基準点(0.11,0) と回転先を約 50 度離す。
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        let pivot = Point2::new(0.0, 0.0);
        let reference = Point2::new(0.11, 0.0);
        // 半径 0.11・角度 50 度の点。基準点との弦長 ≈ 0.093 < tol=0.1（距離ゲールなら誤拒否）。
        let (s, c) = (50.0_f64).to_radians().sin_cos();
        let target = Point2::new(0.11 * c, 0.11 * s);
        // 前提の確認: 両点とも pivot から tol 超、かつ2点間は tol 以内。
        assert!(pivot.distance(reference) > 0.1);
        assert!(pivot.distance(target) > 0.1);
        assert!(reference.distance(target) <= 0.1);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, pivot, 0.1);
        assert_eq!(
            tool.placement_click(&doc, reference, 0.1),
            PlacementOutcome::Continue
        );
        let outcome = tool.placement_click(&doc, target, 0.1);
        // 誤拒否されず確定し、実角度 ≈ 50 度の回転になる。
        let pairs = modify_pairs(&outcome);
        assert_eq!(pairs.len(), 1);
        // (0,0)-(1,0) を原点まわり 50 度回転 → (0,0)-(cos50, sin50)。1ulp の角度差に
        // 依存しないよう端点を近似比較する。
        match pairs[0].1.as_shape().expect("rotate produces a Shape") {
            Shape::Line(l) => {
                assert!(l.a.distance(Point2::ORIGIN) < 1e-9);
                assert!(l.b.distance(Point2::new(c, s)) < 1e-9);
            }
            other => panic!("expected Line, got {other:?}"),
        }
    }

    #[test]
    fn rotate_touching_locked_layer_fails_atomically() {
        // ロックレイヤーのエンティティが混ざると Batch 原子性で回転全体が失敗し、何も動かない。
        let mut doc = Document::new();
        let unlocked = add_hline(&mut doc, 0.0);
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

        let mut tool = SelectTool::default();
        tool.on_drag_start(Point2::new(-1.0, -1.0));
        tool.on_drag_end(&doc, Point2::new(2.0, 1.0));
        assert_eq!(tool.selection().len(), 2);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1);
        let _ = tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1);
        let outcome = tool.placement_click(&doc, Point2::new(0.0, 1.0), 0.1);
        let PlacementOutcome::Commit { cmd, .. } = outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        assert!(doc.apply(cmd).is_err());
        assert_eq!(doc.entity(unlocked).unwrap().geom, before_unlocked);
        assert_eq!(doc.entity(locked_entity).unwrap().geom, before_locked);
    }

    // --- 配置モード（Shift+M 鏡映） ---

    #[test]
    fn start_mirror_requires_nonempty_selection() {
        let mut doc = Document::new();
        let _a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();

        assert!(!tool.start_mirror());
        assert!(!tool.is_placing());

        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);
        assert!(tool.start_mirror());
        assert!(tool.is_placing());
    }

    #[test]
    fn mirror_two_click_flow_commits_mirrored_geometry() {
        // 軸を x 軸（(0,0)→(1,0)）にすると、y 座標が反転する。
        let mut doc = Document::new();
        // 水平線分ではなく y!=0 の線分を使い、鏡映の効果が見えるようにする。
        let layer = doc.current_layer();
        let a = doc
            .apply(Command::AddEntity(Entity::new(
                Shape::Line(LineSeg::new(Point2::new(0.0, 2.0), Point2::new(1.0, 3.0))),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let before_a = doc.entity(a).unwrap().geom.clone();

        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 2.5), 0.2, false);
        assert_eq!(tool.selection(), &[a]);
        let selection_before = tool.selection().to_vec();

        assert!(tool.start_mirror());
        // 1クリック目=軸点A。以降カーソル追従で鏡映プレビュー。
        assert_eq!(
            tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        tool.placement_move(Point2::new(1.0, 0.0));
        match tool.placement_preview() {
            Some(PlacementPreview::Mirror { axis_a, axis_b }) => {
                assert_eq!(axis_a, Point2::new(0.0, 0.0));
                assert_eq!(axis_b, Point2::new(1.0, 0.0));
            }
            other => panic!("expected Mirror preview, got {other:?}"),
        }

        // 2クリック目=軸点B(1,0)。x 軸に対する鏡映の ModifyEntity バッチを返す。
        let outcome = tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1);
        let PlacementOutcome::Commit { kind, .. } = &outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        assert_eq!(*kind, PlacementKind::Mirror);
        let pairs = modify_pairs(&outcome);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, a);
        assert_eq!(
            pairs[0].1,
            before_a.mirrored(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))
        );

        assert!(!tool.is_placing());
        assert_eq!(tool.selection(), selection_before.as_slice());
    }

    #[test]
    fn mirror_apply_and_undo_is_single_unit() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let a = doc
            .apply(Command::AddEntity(Entity::new(
                Shape::Line(LineSeg::new(Point2::new(0.0, 2.0), Point2::new(1.0, 3.0))),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let before_a = doc.entity(a).unwrap().geom.clone();

        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 2.5), 0.2, false);

        assert!(tool.start_mirror());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1);
        let outcome = tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1);
        let PlacementOutcome::Commit { cmd, .. } = outcome else {
            panic!("expected Commit, got {outcome:?}");
        };
        doc.apply(cmd).unwrap();
        assert_eq!(
            doc.entity(a).unwrap().geom,
            before_a.mirrored(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))
        );

        assert!(doc.undo());
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
    }

    #[test]
    fn mirror_zero_length_axis_cancels() {
        // 軸2点がほぼ同一だと軸が定まらないため拒否する。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        assert!(tool.start_mirror());
        assert_eq!(
            tool.placement_click(&doc, Point2::new(5.0, 5.0), 0.1),
            PlacementOutcome::Continue
        );
        let outcome = tool.placement_click(&doc, Point2::new(5.05, 5.0), 0.1);
        assert_eq!(
            outcome,
            PlacementOutcome::Cancelled("Zero-length axis - mirror cancelled")
        );
        assert!(!tool.is_placing());
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
    }

    #[test]
    fn cancel_placement_during_rotate_keeps_selection_and_document() {
        // Esc・ツール切替・ファイル操作からの解除経路を回転（3クリックの途中）でも確認する。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before_a = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.1, false);

        assert!(tool.start_rotate());
        let _ = tool.placement_click(&doc, Point2::new(0.0, 0.0), 0.1); // pivot
        let _ = tool.placement_click(&doc, Point2::new(1.0, 0.0), 0.1); // 基準点（回転先待ち）
        assert!(tool.is_placing());

        tool.cancel_placement();
        assert!(!tool.is_placing());
        assert!(tool.placement_preview().is_none());
        assert_eq!(tool.selection(), &[a]);
        assert_eq!(doc.entity(a).unwrap().geom, before_a);
    }

    // --- オフセットモード（O、単一エンティティ。設計判断5） ---

    /// カレントレイヤー上に円を追加して [`EntityId`] を返すヘルパー。
    fn add_circle(doc: &mut Document, center: Point2, radius: f64) -> EntityId {
        let layer = doc.current_layer();
        let entity = Entity::new(
            Shape::Circle(mcad_geom::Circle::new(center, radius)),
            layer,
            Style::inherited(),
        );
        doc.apply(Command::AddEntity(entity)).unwrap().entities[0]
    }

    /// カレントレイヤー上に円弧を追加して [`EntityId`] を返すヘルパー。
    fn add_arc(doc: &mut Document, center: Point2, radius: f64, start: f64, end: f64) -> EntityId {
        let layer = doc.current_layer();
        let entity = Entity::new(
            Shape::Arc(Arc::new(center, radius, start, end)),
            layer,
            Style::inherited(),
        );
        doc.apply(Command::AddEntity(entity)).unwrap().entities[0]
    }

    #[test]
    fn offset_arc_through_point_uses_radial_distance_within_sweep() {
        use std::f64::consts::FRAC_PI_2;
        // 中心原点・半径5・第1象限(0〜90°)の弧。掃引内の点 (10,10)(角度45°)を通過点に。
        let mut doc = Document::new();
        let arc = add_arc(&mut doc, Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![arc]);
        assert!(tool.start_offset());

        let p = Point2::new(10.0, 10.0); // 半径 sqrt(200)≈14.142, 放射距離≈9.142, 外側
        let outcome = tool.offset_click(&doc, p, 0.1, None);
        let OffsetOutcome::Commit(Command::AddEntity(entity)) = outcome else {
            panic!("expected Commit(AddEntity), got {outcome:?}");
        };
        let shape = entity.geom.as_shape().expect("offset produces a Shape");
        match shape {
            Shape::Arc(a) => {
                // 放射距離ぶん外へ → 新半径 = 元の |p−center|。角度・中心は不変。
                assert!((a.radius - p.distance(Point2::ORIGIN)).abs() < 1e-9);
                assert!(a.center.distance(Point2::ORIGIN) < 1e-9);
                assert!((a.start_angle - 0.0).abs() < 1e-9);
                assert!((a.end_angle - FRAC_PI_2).abs() < 1e-9);
                // 掃引内なので結果は通過点を通る。
                assert!(mcad_geom::distance_to(shape, p) < 1e-6);
            }
            other => panic!("expected offset arc, got {other:?}"),
        }
    }

    #[test]
    fn offset_arc_through_point_out_of_sweep_uses_radial_not_endpoint() {
        use std::f64::consts::FRAC_PI_2;
        // 第1象限の弧。掃引範囲外（第2象限、角度180°）の点 (-8,0) を通過点に。
        // 放射距離 = |8−5| = 3(外側)。距離が有限弧の最近点(端点)距離ではなく放射距離で
        // 決まることを確認する（distance_to だと端点までで ~9.4 になり別の半径になる）。
        let mut doc = Document::new();
        let arc = add_arc(&mut doc, Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![arc]);
        assert!(tool.start_offset());

        let outcome = tool.offset_click(&doc, Point2::new(-8.0, 0.0), 0.1, None);
        let OffsetOutcome::Commit(Command::AddEntity(entity)) = outcome else {
            panic!("expected Commit(AddEntity), got {outcome:?}");
        };
        match entity.geom.as_shape().expect("offset produces a Shape") {
            Shape::Arc(a) => {
                assert!((a.radius - 8.0).abs() < 1e-9, "radius {} != 8", a.radius);
            }
            other => panic!("expected offset arc, got {other:?}"),
        }
    }

    #[test]
    fn offset_arc_through_point_on_radius_out_of_sweep_is_zero_distance() {
        use std::f64::consts::FRAC_PI_2;
        // 掃引外だが弧の半径上（(-5,0)、放射距離0）→ 距離ゼロ no-op として拒否する。
        // 有限弧の最近点距離（端点まで ~7）を使っていたら誤って確定してしまうケース。
        let mut doc = Document::new();
        let arc = add_arc(&mut doc, Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![arc]);
        assert!(tool.start_offset());

        let outcome = tool.offset_click(&doc, Point2::new(-5.0, 0.0), 0.1, None);
        assert_eq!(
            outcome,
            OffsetOutcome::Cancelled("Zero offset distance - offset cancelled")
        );
        assert!(!tool.is_offsetting());
    }

    #[test]
    fn start_offset_requires_exactly_one_selection() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let b = add_hline(&mut doc, 5.0);
        let mut tool = SelectTool::default();

        // 空 → 起動しない。
        assert!(!tool.start_offset());
        assert!(!tool.is_offsetting());

        // 単一 → 起動する。
        tool.set_selection(vec![a]);
        assert!(tool.start_offset());
        assert!(tool.is_offsetting());
        tool.cancel_offset();

        // 複数 → 起動しない。
        tool.set_selection(vec![a, b]);
        assert!(!tool.start_offset());
        assert!(!tool.is_offsetting());
    }

    #[test]
    fn offset_through_point_commits_and_keeps_original_selection() {
        // 水平線分 (0,0)-(1,0) を、上方 (0.5,5) を通過点にオフセット → y=5 の線分。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![a]);
        assert!(tool.start_offset());

        // 通過点方式（距離入力なし）。
        let outcome = tool.offset_click(&doc, Point2::new(0.5, 5.0), 0.1, None);
        let OffsetOutcome::Commit(Command::AddEntity(entity)) = outcome else {
            panic!("expected Commit(AddEntity), got {outcome:?}");
        };
        match entity.geom.as_shape().expect("offset produces a Shape") {
            Shape::Line(l) => {
                assert!(l.a.distance(Point2::new(0.0, 5.0)) < 1e-9);
                assert!(l.b.distance(Point2::new(1.0, 5.0)) < 1e-9);
            }
            other => panic!("expected offset line, got {other:?}"),
        }
        // 確定後: モード解除、選択は元エンティティのまま維持（設計判断5）。
        assert!(!tool.is_offsetting());
        assert_eq!(tool.selection(), &[a]);
    }

    #[test]
    fn offset_fixed_distance_uses_input_and_click_side() {
        // 距離入力欄に 2 が入っている場合、クリックは側の決定のみに使う。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![a]);
        assert!(tool.start_offset());

        // 下側 (0.5,-5) をクリック、固定距離 2 → y=-2 の線分。
        let outcome = tool.offset_click(&doc, Point2::new(0.5, -5.0), 0.1, Some(2.0));
        let OffsetOutcome::Commit(Command::AddEntity(entity)) = outcome else {
            panic!("expected Commit(AddEntity), got {outcome:?}");
        };
        match entity.geom.as_shape().expect("offset produces a Shape") {
            Shape::Line(l) => {
                assert!(l.a.distance(Point2::new(0.0, -2.0)) < 1e-9);
                assert!(l.b.distance(Point2::new(1.0, -2.0)) < 1e-9);
            }
            other => panic!("expected offset line, got {other:?}"),
        }
    }

    #[test]
    fn offset_through_point_on_target_is_cancelled() {
        // 通過点が対象上（ピック許容量内）→ 距離ゼロの no-op を拒否。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![a]);
        assert!(tool.start_offset());

        let outcome = tool.offset_click(&doc, Point2::new(0.5, 0.0), 0.1, None);
        assert_eq!(
            outcome,
            OffsetOutcome::Cancelled("Zero offset distance - offset cancelled")
        );
        assert!(!tool.is_offsetting());
    }

    #[test]
    fn offset_circle_inner_collapse_is_cancelled() {
        // 円 (中心原点・半径5) の内側へ距離5以上 → 半径消滅で拒否。
        let mut doc = Document::new();
        let c = add_circle(&mut doc, Point2::ORIGIN, 5.0);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![c]);
        assert!(tool.start_offset());

        let outcome = tool.offset_click(&doc, Point2::ORIGIN, 0.1, Some(5.0));
        assert_eq!(
            outcome,
            OffsetOutcome::Cancelled("Distance too large for inner offset - cancelled")
        );
        assert!(!tool.is_offsetting());
        // Document は変更されていない（元の円だけ）。
        assert_eq!(doc.entities().count(), 1);
    }

    #[test]
    fn offset_preview_reflects_side_and_degeneracy() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![a]);
        assert!(tool.start_offset());

        // カーソルが対象上（通過点方式で距離ほぼ0）→ 退化してゴーストなし。
        tool.offset_move(Point2::new(0.5, 0.0));
        assert!(tool.offset_preview(&doc, None).is_none());

        // カーソルを上方へ → ゴーストは y=3 の線分。
        tool.offset_move(Point2::new(0.5, 3.0));
        match tool.offset_preview(&doc, None) {
            Some(Shape::Line(l)) => {
                assert!(l.a.distance(Point2::new(0.0, 3.0)) < 1e-9);
                assert!(l.b.distance(Point2::new(1.0, 3.0)) < 1e-9);
            }
            other => panic!("expected line ghost, got {other:?}"),
        }
    }

    #[test]
    fn cancel_offset_keeps_selection_and_document() {
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let before = doc.entity(a).unwrap().geom.clone();
        let mut tool = SelectTool::default();
        tool.set_selection(vec![a]);
        assert!(tool.start_offset());
        assert!(tool.is_offsetting());

        tool.cancel_offset();
        assert!(!tool.is_offsetting());
        assert_eq!(tool.selection(), &[a]);
        assert_eq!(doc.entity(a).unwrap().geom, before);
    }

    #[test]
    fn start_offset_disarms_placement_and_vice_versa() {
        // オフセットと配置モードは排他。
        let mut doc = Document::new();
        let a = add_hline(&mut doc, 0.0);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![a]);

        assert!(tool.start_duplicate());
        assert!(tool.is_placing());
        // オフセット開始で配置が畳まれる。
        assert!(tool.start_offset());
        assert!(tool.is_offsetting());
        assert!(!tool.is_placing());
        // 逆向き: 配置開始でオフセットが畳まれる。
        assert!(tool.start_move());
        assert!(tool.is_placing());
        assert!(!tool.is_offsetting());
    }

    // --- Text ツール / Text ヒットテスト ---

    /// カレントレイヤーに Text エンティティを追加して [`EntityId`] を返す。
    fn add_text(doc: &mut Document, anchor: Point2, content: &str, height: f64) -> EntityId {
        let layer = doc.current_layer();
        let entity = Entity::new(
            EntityGeom::Text(TextGeom {
                anchor,
                content: content.to_owned(),
                height,
                angle: 0.0,
            }),
            layer,
            Style::inherited(),
        );
        doc.apply(Command::AddEntity(entity)).unwrap().entities[0]
    }

    #[test]
    fn pick_hits_text_inside_approx_aabb() {
        // Text "Ab"（ASCII 2 文字）高さ 2、アンカー原点。近似 aabb は x∈[0,2.2], y∈[0,2]。
        let mut doc = Document::new();
        let t = add_text(&mut doc, Point2::ORIGIN, "Ab", 2.0);

        // aabb 内をクリックすれば選択される（tol は小さくても内側は距離 0 扱いで拾える）。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(1.0, 1.0), 0.01, false);
        assert_eq!(tool.selection(), &[t]);

        // aabb 外（右上に大きく離れた点）は拾わない。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(10.0, 10.0), 0.01, false);
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn pick_prefers_text_hit_over_far_shape() {
        // Text の aabb 内はクリック点の距離 0 として扱うため、tol 内の遠い線分より優先される。
        let mut doc = Document::new();
        let t = add_text(&mut doc, Point2::ORIGIN, "Ab", 2.0);
        let _line = add_hline(&mut doc, 0.0); // 線分 (0,0)-(1,0) は aabb と重なる

        let mut tool = SelectTool::default();
        // (0.5, 0.5): Text aabb 内（距離 0）、線分からは 0.5。距離 0 の Text が勝つ。
        tool.on_click(&doc, Point2::new(0.5, 0.5), 1.0, false);
        assert_eq!(tool.selection(), &[t]);
    }

    #[test]
    fn pick_hits_text_outside_approx_aabb_within_tol() {
        // aabb は x∈[0,2.2], y∈[0,2]。境界から x 方向へ 0.1 外側の点は、符号なし
        // 距離 0.1 として扱われるため、tol がそれ以上ならヒットする（旧実装は
        // aabb の外側を即座に不採用にしていたため、tol を尊重できていなかった）。
        let mut doc = Document::new();
        let t = add_text(&mut doc, Point2::ORIGIN, "Ab", 2.0);

        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(2.3, 1.0), 0.15, false);
        assert_eq!(tool.selection(), &[t]);

        // tol が距離未満なら依然ヒットしない。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(2.3, 1.0), 0.05, false);
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn pick_prefers_real_shape_over_text_blank_space_on_tie() {
        // Text の近似 aabb は文字の描かれていない空白にも及ぶ。そこにちょうど
        // 重なる実形状（Shape）があれば、Shape 側も距離 0 で先に見つかっているため
        // 後から見つかる Text の近似距離 0 には上書きされない（`d < bd` の厳密比較で
        // 先着優先）。旧実装は aabb 内側を無条件に距離 0 扱いしていたため、
        // Shape の実距離に関わらず常に Text が勝っていた。
        let mut doc = Document::new();
        let line = add_hline(&mut doc, 0.0); // 線分 (0,0)-(1,0)。Shape を先に追加する。
        let _t = add_text(&mut doc, Point2::ORIGIN, "Ab", 2.0); // aabb x∈[0,2.2], y∈[0,2] が重なる

        let mut tool = SelectTool::default();
        // (0.5, 0.0): 線分上（距離 0）かつ Text aabb 内（距離 0）。先着の Shape を優先する。
        tool.on_click(&doc, Point2::new(0.5, 0.0), 0.01, false);
        assert_eq!(tool.selection(), &[line]);
    }

    #[test]
    fn pick_respects_tol_for_rotated_text_aabb() {
        // 90 度回転した Text は近似 aabb が縦長に広がる
        // （局所 (width,height) の四隅を回転 → x∈[-2.0,0.0], y∈[0.0,2.2]）。
        // 回転後も符号なし距離 + tol 判定は一貫して機能する。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let entity = Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::ORIGIN,
                content: "Ab".to_owned(),
                height: 2.0,
                angle: std::f64::consts::FRAC_PI_2,
            }),
            layer,
            Style::inherited(),
        );
        let t = doc.apply(Command::AddEntity(entity)).unwrap().entities[0];

        // aabb 境界 (x = -2.0) から 0.1 外側。tol 0.15 ならヒットする。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(-2.1, 1.0), 0.15, false);
        assert_eq!(tool.selection(), &[t]);

        // tol が距離未満なら依然ヒットしない。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(-2.1, 1.0), 0.05, false);
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn text_tool_click_sets_anchor_and_enters_editing() {
        let (_doc, ctx) = ctx();
        let mut tool = TextTool::default();
        assert_eq!(tool.pending_text_anchor(), None);

        // クリックでアンカー確定・入力待ちへ（Commit はまだ返さない = app 層が確定する）。
        let anchor = Point2::new(3.0, 4.0);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(anchor)),
            ToolResult::Continue
        );
        assert_eq!(tool.pending_text_anchor(), Some(anchor));

        // 入力待ち中の追加クリックはアンカーを動かさない。
        tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0)));
        assert_eq!(tool.pending_text_anchor(), Some(anchor));
    }

    #[test]
    fn text_tool_esc_behavior_by_state() {
        let (_doc, ctx) = ctx();
        let mut tool = TextTool::default();

        // 入力待ち中の Esc はアンカーを捨てて未確定へ戻る（ツールは維持 = Continue）。
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 1.0)));
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Cancel),
            ToolResult::Continue
        );
        assert_eq!(tool.pending_text_anchor(), None);

        // 未確定の Esc は作図終了（app 層が Select へ戻す）= Cancel。
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
    }

    #[test]
    fn offset_rejects_text_entity() {
        // Text は選択できるようになったが、オフセット対象外（DESIGN.md M6 L385）。
        // start_offset は選択数 1 で起動するが、確定クリックで明示的に拒否される。
        let mut doc = Document::new();
        let t = add_text(&mut doc, Point2::ORIGIN, "Ab", 2.0);
        let mut tool = SelectTool::default();
        tool.set_selection(vec![t]);

        assert!(tool.start_offset());
        // プレビューは描かない（結果を作れない）。
        assert!(tool.offset_preview(&doc, None).is_none());
        let outcome = tool.offset_click(&doc, Point2::new(1.0, 1.0), 0.1, None);
        assert!(matches!(outcome, OffsetOutcome::Cancelled(_)));
        assert!(!tool.is_offsetting());
        // Document は不変（Text は消えも増えもしない）。
        assert_eq!(doc.entity_count(), 1);
    }

    // --- 長さ寸法ツール（DimLinear）---

    /// Commit された [`EntityGeom`] を取り出す（それ以外なら panic）。
    fn committed_geom(result: ToolResult) -> EntityGeom {
        match result {
            ToolResult::Commit(Command::AddEntity(entity)) => entity.geom,
            other => panic!("expected Commit(AddEntity(..)), got {other:?}"),
        }
    }

    #[test]
    fn dim_linear_tool_three_clicks_commit() {
        let (_doc, ctx) = ctx();
        let mut tool = DimLinearTool::default();

        // 1・2 クリック目は Continue。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0))),
            ToolResult::Continue
        );
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(4.0, 0.0))),
            ToolResult::Continue
        );
        // 3 クリック目（寸法線位置 y=2）で確定。offset は計測線からの符号付き距離 +2。
        let geom = committed_geom(tool.on_input(&ctx, InputEvent::Click(Point2::new(2.0, 2.0))));
        let EntityGeom::DimLinear(dim) = geom else {
            panic!("expected DimLinear, got {geom:?}");
        };
        assert!(dim.p1.distance(Point2::new(0.0, 0.0)) < 1e-9);
        assert!(dim.p2.distance(Point2::new(4.0, 0.0)) < 1e-9);
        assert!((dim.offset - 2.0).abs() < 1e-9);
    }

    #[test]
    fn dim_linear_tool_rejects_coincident_p1_p2_with_reason() {
        let (_doc, ctx) = ctx();
        let mut tool = DimLinearTool::default();

        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 1.0)));
        // p2 が p1 と（幾何許容値内で）一致 → 退化拒否。理由付きで状態は据え置き。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 1.0))),
            ToolResult::Rejected("Linear dim: measure points coincide")
        );
        // まだ p2 待ちなので、離れた点を打てば受理して寸法線待ちへ進む。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(5.0, 1.0))),
            ToolResult::Continue
        );
        // 次のクリックで確定できる = WaitingLine に居る。
        let geom = committed_geom(tool.on_input(&ctx, InputEvent::Click(Point2::new(3.0, 3.0))));
        assert!(matches!(geom, EntityGeom::DimLinear(_)));
    }

    #[test]
    fn dim_linear_tool_accepts_short_but_valid_separation() {
        // 退化判定はスケール非依存の幾何許容値（1e-9）で行うため、画面上の
        // ピック許容量より短い（が方向は定まる）計測 2 点も受理する。旧実装は
        // ズーム依存の pick_tol で判定しており、ズームアウト時にこうした有効な短い
        // 寸法まで拒否していた（Codex 指摘2 の回帰確認）。
        let (_doc, ctx) = ctx();
        let mut tool = DimLinearTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        // 距離 0.001（1e-9 より十分大きい）は有効。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(0.001, 0.0))),
            ToolResult::Continue
        );
        let geom = committed_geom(tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0005, 1.0))));
        let EntityGeom::DimLinear(dim) = geom else {
            panic!("expected DimLinear, got {geom:?}");
        };
        assert!((dim.p2 - dim.p1).length() > 0.0);
    }

    #[test]
    fn dim_linear_tool_cancel_resets_and_snap_points() {
        let (_doc, ctx) = ctx();
        let mut tool = DimLinearTool::default();
        assert!(tool.snap_points().is_empty());

        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        assert_eq!(tool.snap_points(), vec![Point2::new(0.0, 0.0)]);
        tool.on_input(&ctx, InputEvent::Click(Point2::new(4.0, 0.0)));
        assert_eq!(
            tool.snap_points(),
            vec![Point2::new(0.0, 0.0), Point2::new(4.0, 0.0)]
        );

        // Esc で最初の状態へ戻る（snap 候補も消える）。
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        assert!(tool.snap_points().is_empty());
    }

    // --- 半径寸法ツール（DimRadial）---

    #[test]
    fn dim_radial_tool_wants_circle_pick_then_leader() {
        let (_doc, ctx) = ctx();
        let mut tool = DimRadialTool::default();

        // 1 クリック目は円ヒットテスト段階。
        assert!(tool.wants_circle_pick());
        // 円ヒットが渡されると引出方向待ちへ。
        tool.on_circle_pick(CirclePick {
            center: Point2::new(1.0, 2.0),
            radius: 5.0,
        });
        assert!(!tool.wants_circle_pick());

        // 引出方向のクリックで確定。leader_angle は中心→クリックの角度（+x 方向 → 0）。
        let geom = committed_geom(tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 2.0))));
        let EntityGeom::DimRadial(dim) = geom else {
            panic!("expected DimRadial, got {geom:?}");
        };
        assert!(dim.center.distance(Point2::new(1.0, 2.0)) < 1e-9);
        assert!((dim.radius - 5.0).abs() < 1e-9);
        assert!(dim.leader_angle.abs() < 1e-9);
        // 確定後は円ヒットテスト段階へ戻る。
        assert!(tool.wants_circle_pick());
    }

    #[test]
    fn dim_radial_tool_rejects_leader_at_center() {
        // 引出クリックが中心と（幾何許容値内で）一致 → 方向不定なので退化拒否。
        // 理由付きで、状態は WaitingLeader のまま据え置き、Command を作らない。
        let (_doc, ctx) = ctx();
        let mut tool = DimRadialTool::default();
        let center = Point2::new(1.0, 2.0);
        tool.on_circle_pick(CirclePick {
            center,
            radius: 5.0,
        });

        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(center)),
            ToolResult::Rejected("Radial dim: leader direction undefined at center")
        );
        // 円ヒットテスト段階へは戻っていない（引出方向をまだ待っている）。
        assert!(!tool.wants_circle_pick());
        // 続けて中心から外れた点を打てば正しく確定する。
        let geom = committed_geom(tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 2.0))));
        assert!(matches!(geom, EntityGeom::DimRadial(_)));
    }

    #[test]
    fn dim_radial_tool_click_in_circle_stage_is_noop() {
        // 円ヒットテスト段階では Click（通常経路）が来ても状態を変えない
        // （app 層は on_circle_pick 経由でのみ前進させる）。
        let (_doc, ctx) = ctx();
        let mut tool = DimRadialTool::default();
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(3.0, 3.0))),
            ToolResult::Continue
        );
        assert!(tool.wants_circle_pick());
    }

    // --- pick_circle_or_arc（app 層ヒットテスト）---

    #[test]
    fn pick_circle_or_arc_picks_nearest_circle_outline() {
        let mut doc = Document::new();
        add_circle(&mut doc, Point2::ORIGIN, 5.0);
        add_hline(&mut doc, 0.0); // 線分は円ピック対象外

        // 円周 (5,0) 付近（距離 0.05）を tol 0.1 で当てる。
        let hit = pick_circle_or_arc(&doc, Point2::new(5.05, 0.0), 0.1).expect("circle hit");
        assert!(hit.center.distance(Point2::ORIGIN) < 1e-9);
        assert!((hit.radius - 5.0).abs() < 1e-9);

        // 円周から遠い点（中心付近）は当たらない。
        assert!(pick_circle_or_arc(&doc, Point2::ORIGIN, 0.1).is_none());
    }

    #[test]
    fn pick_circle_or_arc_hits_arc_and_ignores_line() {
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0); // 円/円弧ではないので対象外
        add_arc(
            &mut doc,
            Point2::ORIGIN,
            3.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        );

        // 弧上の点 (3,0)（掃引の始点）付近。
        let hit = pick_circle_or_arc(&doc, Point2::new(3.02, 0.0), 0.1).expect("arc hit");
        assert!((hit.radius - 3.0).abs() < 1e-9);

        // 線分上（(0.5,0)）は円/円弧ではないのでヒットしない。
        assert!(pick_circle_or_arc(&doc, Point2::new(0.5, 0.0), 0.1).is_none());
    }

    // --- pick_shape_entity（app 層の汎用エンティティピック基盤、M7 タスク30）---

    #[test]
    fn pick_shape_entity_picks_nearest_among_multiple() {
        let mut doc = Document::new();
        let near = add_hline(&mut doc, 0.0); // 線分 (0,0)-(1,0)
        let _far = add_hline(&mut doc, 10.0); // 線分 (10,0)-(11,0)
        let _circle = add_circle(&mut doc, Point2::new(50.0, 50.0), 5.0);

        // (0.5, 0.02) は near にごく近く、他の候補からは遠い。
        let hit = pick_shape_entity(&doc, Point2::new(0.5, 0.02), 0.1).expect("shape hit");
        assert_eq!(hit.id, near);
        assert_eq!(
            hit.shape,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(1.0, 0.0)))
        );
        assert_eq!(hit.click, Point2::new(0.5, 0.02));
    }

    #[test]
    fn pick_shape_entity_none_beyond_tolerance() {
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0); // 線分 (0,0)-(1,0)

        // 線分から 1.0 離れた点は tol 0.1 では拾えない。
        assert!(pick_shape_entity(&doc, Point2::new(0.5, 1.0), 0.1).is_none());
    }

    #[test]
    fn pick_shape_entity_excludes_text_and_dimensions() {
        let mut doc = Document::new();
        add_text(&mut doc, Point2::ORIGIN, "hi", 1.0);
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::DimLinear(DimLinear {
                p1: Point2::ORIGIN,
                p2: Point2::new(1.0, 0.0),
                offset: 0.5,
            }),
            doc.current_layer(),
            Style::inherited(),
        )))
        .unwrap();

        // Text・寸法しか無いドキュメントでは（as_shape() が None のため）何もヒットしない。
        assert!(pick_shape_entity(&doc, Point2::ORIGIN, 1.0).is_none());
    }

    #[test]
    fn pick_shape_entity_excludes_hidden_layer() {
        let mut doc = Document::new();
        add_hline(&mut doc, 0.0); // 線分 (0,0)-(1,0)
        let layer_id = doc.current_layer();
        let mut props = doc.layer(layer_id).unwrap().clone();
        props.visible = false;
        doc.apply(Command::SetLayerProps {
            id: layer_id,
            props,
        })
        .unwrap();

        assert!(
            pick_shape_entity(&doc, Point2::new(0.5, 0.0), 0.1).is_none(),
            "非表示レイヤーのエンティティは拾わない"
        );
    }

    // --- トリム／延長ツール（M7 タスク31）---

    /// `doc` へ `shape` を実際に追加して [`EntityId`] を発行し、その結果を
    /// [`ShapePick`] として組み立てる（app 層のヒットテストが返すものと同じ形）。
    /// 状態機械のテストは `Document` を経由せず、この `ShapePick` を直接ツールへ渡す。
    fn shape_pick(doc: &mut Document, shape: Shape, click: Point2) -> ShapePick {
        let layer = doc.current_layer();
        let style = Style::inherited();
        let id = doc
            .apply(Command::AddEntity(Entity::new(shape.clone(), layer, style)))
            .unwrap()
            .entities[0];
        ShapePick {
            id,
            shape,
            click,
            layer,
            style,
        }
    }

    fn hline(x0: f64, x1: f64, y: f64) -> Shape {
        Shape::Line(LineSeg::new(Point2::new(x0, y), Point2::new(x1, y)))
    }

    fn vline(x: f64, y0: f64, y1: f64) -> Shape {
        Shape::Line(LineSeg::new(Point2::new(x, y0), Point2::new(x, y1)))
    }

    fn line_of(shape: &Shape) -> LineSeg {
        match shape {
            Shape::Line(seg) => *seg,
            other => panic!("expected a line, got {other:?}"),
        }
    }

    fn modified_geom(result: &ToolResult, expect_id: EntityId) -> Shape {
        match result {
            ToolResult::Commit(Command::ModifyEntity { id, new_geom }) => {
                assert_eq!(*id, expect_id, "ModifyEntity は対象 ID を維持する");
                match new_geom {
                    EntityGeom::Shape(shape) => shape.clone(),
                    other => panic!("expected a Shape geom, got {other:?}"),
                }
            }
            other => panic!("expected Commit(ModifyEntity), got {other:?}"),
        }
    }

    #[test]
    fn trim_tool_two_stage_pick_commits_single_piece() {
        // 境界 x=1 の縦線、対象 (0,0)-(2,0) の横線。交点は (1,0) の1点だけなので、
        // 交点より右をクリックすると右半分が消え、残るのは1断片 → ID 維持の ModifyEntity。
        let mut doc = Document::new();
        let mut tool = TrimTool::default();
        assert!(tool.wants_shape_pick());

        let boundary = shape_pick(&mut doc, vline(1.0, -1.0, 1.0), Point2::new(1.0, 0.5));
        assert_eq!(tool.on_shape_pick(boundary), ToolResult::Continue);
        assert!(matches!(
            tool.0.state,
            BoundaryTargetState::WaitingTarget { .. }
        ));

        let target = shape_pick(&mut doc, hline(0.0, 2.0, 0.0), Point2::new(1.5, 0.0));
        let target_id = target.id;
        let result = tool.on_shape_pick(target);
        let kept = line_of(&modified_geom(&result, target_id));
        assert_eq!(kept.a, Point2::new(0.0, 0.0));
        assert_eq!(kept.b, Point2::new(1.0, 0.0));
        // 1断片トリムは選択集合を触らない。
        assert_eq!(tool.take_commit_selection(&NewIds::default()), None);
    }

    #[test]
    fn trim_tool_middle_click_splits_into_two_pieces() {
        // 境界は中心 (1,0)・半径 0.5 の円、対象は (0,0)-(2,0)。交点は (0.5,0)・(1.5,0)。
        // その中間をクリックすると両側に断片が残り、Batch(Remove + Add ×2) になる。
        let mut doc = Document::new();
        // 対象を既定と違うレイヤー・スタイルに置き、新規2件へ複製されることを検証する。
        let other_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "other",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let style = Style {
            color: Some(mcad_core::Rgb::new(1, 2, 3)),
            ..Style::inherited()
        };
        let target_shape = hline(0.0, 2.0, 0.0);
        let target_id = doc
            .apply(Command::AddEntity(Entity::new(
                target_shape.clone(),
                other_layer,
                style,
            )))
            .unwrap()
            .entities[0];

        let mut tool = TrimTool::default();
        let boundary = shape_pick(
            &mut doc,
            Shape::Circle(mcad_geom::Circle::new(Point2::new(1.0, 0.0), 0.5)),
            Point2::new(1.5, 0.0),
        );
        assert_eq!(tool.on_shape_pick(boundary), ToolResult::Continue);

        let result = tool.on_shape_pick(ShapePick {
            id: target_id,
            shape: target_shape,
            click: Point2::new(1.0, 0.0),
            layer: other_layer,
            style,
        });
        let ToolResult::Commit(Command::Batch(cmds)) = &result else {
            panic!("expected Commit(Batch), got {result:?}");
        };
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], Command::RemoveEntity(target_id));
        let mut added = Vec::new();
        for cmd in &cmds[1..] {
            let Command::AddEntity(entity) = cmd else {
                panic!("expected AddEntity, got {cmd:?}");
            };
            // 新規2件は元エンティティのレイヤー・スタイルを複製する。
            assert_eq!(entity.layer, other_layer);
            assert_eq!(entity.style, style);
            let EntityGeom::Shape(shape) = &entity.geom else {
                panic!("expected a Shape geom");
            };
            added.push(line_of(shape));
        }
        assert_eq!(added[0].a, Point2::new(0.0, 0.0));
        assert!((added[0].b.x - 0.5).abs() < 1e-9);
        assert!((added[1].a.x - 1.5).abs() < 1e-9);
        assert_eq!(added[1].b, Point2::new(2.0, 0.0));

        // 2断片トリムは確定後の選択集合を新規2件へ載せ替える（1回だけ消費される）。
        let new_ids = NewIds {
            entities: vec![target_id],
            ..NewIds::default()
        };
        assert_eq!(
            tool.take_commit_selection(&new_ids),
            Some(vec![target_id]),
            "NewIds.entities をそのまま新選択にする"
        );
        assert_eq!(tool.take_commit_selection(&new_ids), None, "消費済み");
    }

    #[test]
    fn trim_tool_stays_in_waiting_target_for_consecutive_trims() {
        // 確定後も境界を保持したまま WaitingTarget に留まり、2本目を続けて切れる。
        let mut doc = Document::new();
        let mut tool = TrimTool::default();
        let boundary_shape = vline(1.0, -5.0, 5.0);
        let boundary = shape_pick(&mut doc, boundary_shape.clone(), Point2::new(1.0, 0.5));
        tool.on_shape_pick(boundary);

        let first = shape_pick(&mut doc, hline(0.0, 2.0, 0.0), Point2::new(1.5, 0.0));
        let first_id = first.id;
        let r1 = tool.on_shape_pick(first);
        assert!(matches!(r1, ToolResult::Commit(_)));
        assert_eq!(
            tool.0.state,
            BoundaryTargetState::WaitingTarget {
                boundary: boundary_shape.clone()
            },
            "確定後も同じ境界のまま対象待ちに留まる"
        );

        let second = shape_pick(&mut doc, hline(0.0, 2.0, 1.0), Point2::new(1.5, 1.0));
        let second_id = second.id;
        let r2 = tool.on_shape_pick(second);
        let kept = line_of(&modified_geom(&r2, second_id));
        assert_eq!(kept.b, Point2::new(1.0, 1.0));
        assert_ne!(first_id, second_id);
        assert_eq!(
            tool.0.state,
            BoundaryTargetState::WaitingTarget {
                boundary: boundary_shape
            }
        );
    }

    #[test]
    fn trim_tool_rejects_unsupported_target_and_keeps_state() {
        // 対象が Circle は geom が Unsupported。状態は WaitingTarget のまま据え置き。
        let mut doc = Document::new();
        let mut tool = TrimTool::default();
        let boundary_shape = vline(1.0, -5.0, 5.0);
        let boundary = shape_pick(&mut doc, boundary_shape.clone(), Point2::new(1.0, 0.5));
        tool.on_shape_pick(boundary);

        let target = shape_pick(
            &mut doc,
            Shape::Circle(mcad_geom::Circle::new(Point2::new(1.0, 0.0), 2.0)),
            Point2::new(3.0, 0.0),
        );
        assert_eq!(
            tool.on_shape_pick(target),
            ToolResult::Rejected("Trim: target must be a line or an arc")
        );
        assert_eq!(
            tool.0.state,
            BoundaryTargetState::WaitingTarget {
                boundary: boundary_shape
            },
            "拒否しても同じ境界のまま別対象を試せる"
        );
    }

    #[test]
    fn trim_tool_rejects_target_without_intersection() {
        let mut doc = Document::new();
        let mut tool = TrimTool::default();
        let boundary = shape_pick(&mut doc, vline(1.0, -5.0, 5.0), Point2::new(1.0, 0.5));
        tool.on_shape_pick(boundary);

        // 境界（x=1 の縦線）と交わらない位置の横線。
        let target = shape_pick(&mut doc, hline(5.0, 7.0, 0.0), Point2::new(6.0, 0.0));
        assert_eq!(
            tool.on_shape_pick(target),
            ToolResult::Rejected("Trim: target does not cross the boundary")
        );
    }

    #[test]
    fn extend_tool_commits_modify_entity() {
        // 境界 x=2 の縦線、対象 (0,0)-(1,0)。自由端 (1,0) 側をクリックして境界まで伸ばす。
        let mut doc = Document::new();
        let mut tool = ExtendTool::default();
        assert!(tool.wants_shape_pick());

        let boundary_shape = vline(2.0, -1.0, 1.0);
        let boundary = shape_pick(&mut doc, boundary_shape.clone(), Point2::new(2.0, 0.5));
        assert_eq!(tool.on_shape_pick(boundary), ToolResult::Continue);

        let target = shape_pick(&mut doc, hline(0.0, 1.0, 0.0), Point2::new(0.95, 0.0));
        let target_id = target.id;
        let result = tool.on_shape_pick(target);
        let extended = line_of(&modified_geom(&result, target_id));
        assert_eq!(extended.a, Point2::new(0.0, 0.0));
        assert!((extended.b.x - 2.0).abs() < 1e-9);
        assert!((extended.b.y).abs() < 1e-9);
        // 延長は分裂しないので選択集合は触らない。
        assert_eq!(tool.take_commit_selection(&NewIds::default()), None);
        // 確定後も同じ境界のまま連続適用できる。
        assert_eq!(
            tool.0.state,
            BoundaryTargetState::WaitingTarget {
                boundary: boundary_shape
            }
        );
    }

    #[test]
    fn extend_tool_rejects_unreachable_boundary_and_keeps_state() {
        // 自由端を境界と反対側に選ぶと延長方向に交点が無い。
        let mut doc = Document::new();
        let mut tool = ExtendTool::default();
        let boundary_shape = vline(2.0, -1.0, 1.0);
        let boundary = shape_pick(&mut doc, boundary_shape.clone(), Point2::new(2.0, 0.5));
        tool.on_shape_pick(boundary);

        let target = shape_pick(&mut doc, hline(0.0, 1.0, 0.0), Point2::new(0.05, 0.0));
        assert_eq!(
            tool.on_shape_pick(target),
            ToolResult::Rejected("Extend: boundary is not reachable in that direction")
        );
        assert_eq!(
            tool.0.state,
            BoundaryTargetState::WaitingTarget {
                boundary: boundary_shape
            }
        );
    }

    #[test]
    fn trim_and_extend_tools_reset_on_escape() {
        // Esc は Cancel を返し（app 層が Select へ戻す）、内部状態も初期化する。
        let (mut doc, ctx) = ctx();
        for tool in [
            &mut TrimTool::default() as &mut dyn Tool,
            &mut ExtendTool::default() as &mut dyn Tool,
        ] {
            let boundary = shape_pick(&mut doc, vline(1.0, -1.0, 1.0), Point2::new(1.0, 0.5));
            assert_eq!(tool.on_shape_pick(boundary), ToolResult::Continue);
            assert_eq!(
                tool.on_input(&ctx, InputEvent::Cancel),
                ToolResult::Cancel,
                "Esc は Cancel（app 層が Select へ戻す）"
            );
            // 初期状態へ戻っているので、次のピックは再び境界の採取になる。
            let again = shape_pick(&mut doc, vline(3.0, -1.0, 1.0), Point2::new(3.0, 0.5));
            assert_eq!(tool.on_shape_pick(again), ToolResult::Continue);
        }
    }

    #[test]
    fn trim_tool_click_event_does_not_advance_state() {
        // クリックは常に app 層のヒットテスト経路を通る。素の Click が漏れてきても
        // 状態を進めない（DimRadialTool と同じ扱い）。
        let (_doc, ctx) = ctx();
        let mut tool = TrimTool::default();
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0))),
            ToolResult::Continue
        );
        assert_eq!(tool.0.state, BoundaryTargetState::WaitingBoundary);
    }

    #[test]
    fn trim_two_piece_batch_rolls_back_on_locked_layer() {
        // 2断片トリムの Batch は原子的。対象がロックレイヤー上なら削除も追加も起きない。
        let mut doc = Document::new();
        let locked_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "locked",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let target_shape = hline(0.0, 2.0, 0.0);
        let target_id = doc
            .apply(Command::AddEntity(Entity::new(
                target_shape.clone(),
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

        let entities_before = doc.entities().count();
        let geom_before = doc.entity(target_id).unwrap().geom.clone();

        let mut tool = TrimTool::default();
        let boundary = shape_pick(
            &mut doc,
            Shape::Circle(mcad_geom::Circle::new(Point2::new(1.0, 0.0), 0.5)),
            Point2::new(1.5, 0.0),
        );
        tool.on_shape_pick(boundary);
        let result = tool.on_shape_pick(ShapePick {
            id: target_id,
            shape: target_shape,
            click: Point2::new(1.0, 0.0),
            layer: locked_layer,
            style: Style::inherited(),
        });
        let ToolResult::Commit(cmd) = result else {
            panic!("expected Commit");
        };
        // 境界の追加でエンティティが1本増えている分を差し引いて比較する。
        let entities_after_boundary = doc.entities().count();
        assert_eq!(entities_after_boundary, entities_before + 1);

        assert!(doc.apply(cmd).is_err(), "ロックレイヤーなので失敗する");
        assert_eq!(doc.entities().count(), entities_after_boundary);
        assert_eq!(doc.entity(target_id).unwrap().geom, geom_before);
    }

    // --- フィレットツール（M7 タスク32）---

    /// 直角コーナー用の 2 本。a = (0,0)-(10,0)、b = (0,0)-(0,10)。半径 2 なら
    /// 接点は (2,0)・(0,2)、中心は (2,2)。`near_a`/`near_b` はコーナーから遠い側
    /// （＝残したい側）を指す。
    fn corner_pair(doc: &mut Document) -> (ShapePick, ShapePick) {
        let a = shape_pick(doc, hline(0.0, 10.0, 0.0), Point2::new(8.0, 0.0));
        let b = shape_pick(doc, vline(0.0, 0.0, 10.0), Point2::new(0.0, 8.0));
        (a, b)
    }

    /// フィレット確定の `Batch` を分解し、(trimmed_a, trimmed_b, 弧のエンティティ) を返す。
    fn fillet_commit(
        result: &ToolResult,
        expect_a: EntityId,
        expect_b: EntityId,
    ) -> (LineSeg, LineSeg, Entity) {
        let ToolResult::Commit(Command::Batch(cmds)) = result else {
            panic!("expected Commit(Batch), got {result:?}");
        };
        assert_eq!(cmds.len(), 3, "ModifyEntity ×2 + AddEntity ×1");
        let mut trimmed = Vec::new();
        for (cmd, expect_id) in cmds[..2].iter().zip([expect_a, expect_b]) {
            let Command::ModifyEntity { id, new_geom } = cmd else {
                panic!("expected ModifyEntity, got {cmd:?}");
            };
            assert_eq!(*id, expect_id, "ModifyEntity は対象 ID を維持する");
            let EntityGeom::Shape(shape) = new_geom else {
                panic!("expected a Shape geom");
            };
            trimmed.push(line_of(shape));
        }
        let Command::AddEntity(entity) = &cmds[2] else {
            panic!("expected AddEntity, got {:?}", cmds[2]);
        };
        (trimmed[0], trimmed[1], entity.clone())
    }

    #[test]
    fn fillet_tool_commits_batch_and_inherits_first_line_layer_style() {
        let mut doc = Document::new();
        // 1 本目を既定と違うレイヤー・スタイルに置き、弧がそちらを継承することを検証する
        // （設計判断4: 弧は「1 本目にクリックした線分」のレイヤー・スタイルを継承）。
        let layer_a = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "a",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let style_a = Style {
            color: Some(mcad_core::Rgb::new(1, 2, 3)),
            ..Style::inherited()
        };
        let id_a = doc
            .apply(Command::AddEntity(Entity::new(
                hline(0.0, 10.0, 0.0),
                layer_a,
                style_a,
            )))
            .unwrap()
            .entities[0];
        let pick_a = ShapePick {
            id: id_a,
            shape: hline(0.0, 10.0, 0.0),
            click: Point2::new(8.0, 0.0),
            layer: layer_a,
            style: style_a,
        };
        // 2 本目は既定レイヤー・スタイル（継承元にならないことの確認用）。
        let pick_b = shape_pick(&mut doc, vline(0.0, 0.0, 10.0), Point2::new(0.0, 8.0));
        let id_b = pick_b.id;

        let mut tool = FilletTool::default();
        assert!(tool.wants_shape_pick());
        tool.set_radius_input(Some(2.0));

        assert_eq!(tool.on_shape_pick(pick_a), ToolResult::Continue);
        let result = tool.on_shape_pick(pick_b);
        let (trimmed_a, trimmed_b, arc_entity) = fillet_commit(&result, id_a, id_b);

        // 接点 (2,0)・(0,2) までトリムされ、クリックした側（遠い端）が残る。
        assert!((trimmed_a.a.x - 2.0).abs() < 1e-9 && trimmed_a.a.y.abs() < 1e-9);
        assert_eq!(trimmed_a.b, Point2::new(10.0, 0.0));
        assert!((trimmed_b.a.y - 2.0).abs() < 1e-9 && trimmed_b.a.x.abs() < 1e-9);
        assert_eq!(trimmed_b.b, Point2::new(0.0, 10.0));

        // 弧は中心 (2,2)・半径 2 で、1 本目のレイヤー・スタイルを継承する。
        assert_eq!(arc_entity.layer, layer_a, "弧のレイヤーは 1 本目由来");
        assert_eq!(arc_entity.style, style_a, "弧のスタイルは 1 本目由来");
        let EntityGeom::Shape(Shape::Arc(arc)) = &arc_entity.geom else {
            panic!("expected an Arc geom, got {:?}", arc_entity.geom);
        };
        assert!((arc.center - Point2::new(2.0, 2.0)).length() < 1e-9);
        assert!((arc.radius - 2.0).abs() < 1e-9);

        // 確定後の選択集合は「変更した 2 本 + 新規の弧」の 3 件（設計判断6）。
        let arc_id = doc
            .apply(Command::AddEntity(Entity::new(
                hline(50.0, 51.0, 0.0),
                layer_a,
                style_a,
            )))
            .unwrap()
            .entities[0];
        let new_ids = NewIds {
            entities: vec![arc_id],
            ..NewIds::default()
        };
        assert_eq!(
            tool.take_commit_selection(&new_ids),
            Some(vec![id_a, id_b, arc_id])
        );
        assert_eq!(tool.take_commit_selection(&new_ids), None, "消費済み");
    }

    #[test]
    fn fillet_tool_returns_to_waiting_first_line_after_commit() {
        // 単発仕様: 確定後は 1 本目待ちへ戻る（トリムのような連続適用はしない）。
        let mut doc = Document::new();
        let (a, b) = corner_pair(&mut doc);
        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(2.0));
        tool.on_shape_pick(a);
        assert!(matches!(tool.state, FilletState::WaitingSecondLine { .. }));
        assert!(matches!(tool.on_shape_pick(b), ToolResult::Commit(_)));
        assert_eq!(tool.state, FilletState::WaitingFirstLine);
    }

    #[test]
    fn fillet_tool_rejects_missing_or_invalid_radius_and_keeps_state() {
        let mut doc = Document::new();
        let (a, b) = corner_pair(&mut doc);
        let mut tool = FilletTool::default();
        // 半径未入力（欄が空・不正値のとき app 層は None を渡す）。
        tool.set_radius_input(None);
        tool.on_shape_pick(a.clone());
        let before = tool.state.clone();
        assert_eq!(
            tool.on_shape_pick(b.clone()),
            ToolResult::Rejected("Fillet: enter a radius")
        );
        assert_eq!(tool.state, before, "1 本目を保持して 2 本目だけ選び直せる");
        // 拒否は選択集合も触らない。
        assert_eq!(tool.take_commit_selection(&NewIds::default()), None);

        // 半径を入れ直せば同じ 2 本目のピックで確定できる。
        tool.set_radius_input(Some(2.0));
        assert!(matches!(tool.on_shape_pick(b), ToolResult::Commit(_)));
    }

    #[test]
    fn fillet_tool_rejects_non_line_picks() {
        let mut doc = Document::new();
        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(2.0));

        // 1 本目が円 → 拒否。状態は 1 本目待ちのまま。
        let circle = shape_pick(
            &mut doc,
            Shape::Circle(mcad_geom::Circle::new(Point2::new(20.0, 20.0), 3.0)),
            Point2::new(23.0, 20.0),
        );
        assert_eq!(
            tool.on_shape_pick(circle.clone()),
            ToolResult::Rejected("Fillet: first pick must be a line")
        );
        assert_eq!(tool.state, FilletState::WaitingFirstLine);

        // 2 本目が円 → 拒否。1 本目は保持したまま。
        let (a, _) = corner_pair(&mut doc);
        tool.on_shape_pick(a);
        let before = tool.state.clone();
        assert_eq!(
            tool.on_shape_pick(circle),
            ToolResult::Rejected("Fillet: second pick must be a line")
        );
        assert_eq!(tool.state, before);
    }

    #[test]
    fn fillet_tool_rejects_same_entity_twice() {
        // 同じ線分同士のフィレットは無意味なので拒否する（状態は据え置き）。
        let mut doc = Document::new();
        let (a, _) = corner_pair(&mut doc);
        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(2.0));
        tool.on_shape_pick(a.clone());
        let before = tool.state.clone();
        assert_eq!(
            tool.on_shape_pick(a),
            ToolResult::Rejected("Fillet: pick two different lines")
        );
        assert_eq!(tool.state, before);
    }

    #[test]
    fn fillet_tool_maps_geom_errors_and_keeps_state() {
        let mut doc = Document::new();

        // 平行な 2 本。
        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(1.0));
        let a = shape_pick(&mut doc, hline(0.0, 10.0, 0.0), Point2::new(5.0, 0.0));
        let parallel = shape_pick(&mut doc, hline(0.0, 10.0, 5.0), Point2::new(5.0, 5.0));
        tool.on_shape_pick(a);
        let before = tool.state.clone();
        assert_eq!(
            tool.on_shape_pick(parallel),
            ToolResult::Rejected("Fillet: the two lines are parallel")
        );
        assert_eq!(tool.state, before);

        // 半径が線分長に対して過大（接点が線分の外へ出る）。
        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(100.0));
        let (a, b) = corner_pair(&mut doc);
        tool.on_shape_pick(a);
        let before = tool.state.clone();
        assert_eq!(
            tool.on_shape_pick(b),
            ToolResult::Rejected("Fillet: radius is too large for these lines")
        );
        assert_eq!(tool.state, before);
    }

    #[test]
    fn fillet_tool_resets_on_escape_and_tool_switch() {
        let (mut doc, ctx) = ctx();
        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(2.0));
        let (a, b) = corner_pair(&mut doc);
        assert_eq!(tool.on_shape_pick(a), ToolResult::Continue);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Cancel),
            ToolResult::Cancel,
            "Esc は Cancel（app 層が Select へ戻す）"
        );
        assert_eq!(tool.state, FilletState::WaitingFirstLine);
        // 初期状態へ戻っているので、次のピックは再び 1 本目の採取になる。
        assert_eq!(tool.on_shape_pick(b), ToolResult::Continue);

        // ツール切替は app 層が `ToolKind::spawn()` で作り直す（＝Default 相当）。
        // `caches_picked_shapes` に Fillet が含まれるので undo/redo・ファイル操作でも同じ。
        let fresh = FilletTool::default();
        assert_eq!(fresh.state, FilletState::WaitingFirstLine);
        assert_eq!(fresh.radius, None);
    }

    #[test]
    fn fillet_tool_click_event_does_not_advance_state() {
        // クリックは常に app 層のヒットテスト経路を通る。素の Click が漏れてきても
        // 状態を進めない（トリム・延長と同じ扱い）。
        let (_doc, ctx) = ctx();
        let mut tool = FilletTool::default();
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0))),
            ToolResult::Continue
        );
        assert_eq!(tool.state, FilletState::WaitingFirstLine);
    }

    #[test]
    fn fillet_batch_rolls_back_on_locked_layer() {
        // Batch は原子的: 1 本目のレイヤーがロックされていれば、2 本目の変更も弧の追加も
        // 起きず Document は一切変化しない（設計判断5・8）。
        let mut doc = Document::new();
        let locked_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "locked",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let shape_a = hline(0.0, 10.0, 0.0);
        let id_a = doc
            .apply(Command::AddEntity(Entity::new(
                shape_a.clone(),
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
        // 2 本目はロックされていないレイヤー（部分適用が起きないことを見るため）。
        let pick_b = shape_pick(&mut doc, vline(0.0, 0.0, 10.0), Point2::new(0.0, 8.0));
        let id_b = pick_b.id;

        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(2.0));
        tool.on_shape_pick(ShapePick {
            id: id_a,
            shape: shape_a,
            click: Point2::new(8.0, 0.0),
            layer: locked_layer,
            style: Style::inherited(),
        });
        let ToolResult::Commit(cmd) = tool.on_shape_pick(pick_b) else {
            panic!("expected Commit");
        };

        let count_before = doc.entities().count();
        let geom_a_before = doc.entity(id_a).unwrap().geom.clone();
        let geom_b_before = doc.entity(id_b).unwrap().geom.clone();
        assert!(doc.apply(cmd).is_err(), "ロックレイヤーなので失敗する");
        assert_eq!(doc.entities().count(), count_before, "弧は追加されない");
        assert_eq!(doc.entity(id_a).unwrap().geom, geom_a_before);
        assert_eq!(doc.entity(id_b).unwrap().geom, geom_b_before);
    }

    // --- 分割ツール（M7 タスク33）---

    fn as_line_shape(shape: &Shape) -> LineSeg {
        line_of(shape)
    }

    #[test]
    fn split_tool_line_midpoint_commits_batch_with_same_layer_and_style() {
        let mut doc = Document::new();
        let other_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "other",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let style = Style {
            color: Some(mcad_core::Rgb::new(1, 2, 3)),
            ..Style::inherited()
        };
        let target_shape = hline(0.0, 10.0, 0.0);
        let target_id = doc
            .apply(Command::AddEntity(Entity::new(
                target_shape.clone(),
                other_layer,
                style,
            )))
            .unwrap()
            .entities[0];

        let mut tool = SplitTool::default();
        assert!(tool.wants_shape_pick());
        let result = tool.on_shape_pick(ShapePick {
            id: target_id,
            shape: target_shape,
            click: Point2::new(5.0, 0.0),
            layer: other_layer,
            style,
        });
        let ToolResult::Commit(Command::Batch(cmds)) = result else {
            panic!("expected Commit(Batch), got {result:?}");
        };
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], Command::RemoveEntity(target_id));
        for cmd in &cmds[1..] {
            let Command::AddEntity(entity) = cmd else {
                panic!("expected AddEntity, got {cmd:?}");
            };
            assert_eq!(entity.layer, other_layer);
            assert_eq!(entity.style, style);
        }
        let EntityGeom::Shape(Shape::Line(l1)) = &cmds_entity(&cmds[1]).geom else {
            panic!("expected a line");
        };
        let EntityGeom::Shape(Shape::Line(l2)) = &cmds_entity(&cmds[2]).geom else {
            panic!("expected a line");
        };
        assert_eq!(l1.a, Point2::new(0.0, 0.0));
        assert_eq!(l1.b, Point2::new(5.0, 0.0));
        assert_eq!(l2.a, Point2::new(5.0, 0.0));
        assert_eq!(l2.b, Point2::new(10.0, 0.0));
    }

    fn cmds_entity(cmd: &Command) -> &Entity {
        match cmd {
            Command::AddEntity(entity) => entity,
            other => panic!("expected AddEntity, got {other:?}"),
        }
    }

    #[test]
    fn split_tool_take_commit_selection_returns_new_pair_once() {
        let mut doc = Document::new();
        let target = shape_pick(&mut doc, hline(0.0, 10.0, 0.0), Point2::new(5.0, 0.0));
        let mut tool = SplitTool::default();
        let ToolResult::Commit(cmd) = tool.on_shape_pick(target) else {
            panic!("expected Commit(Batch)");
        };
        let new_ids = doc.apply(cmd).expect("split batch applies cleanly");
        assert_eq!(new_ids.entities.len(), 2);
        assert_eq!(
            tool.take_commit_selection(&new_ids),
            Some(new_ids.entities.clone())
        );
        // 1回消費したら次は None。
        assert_eq!(tool.take_commit_selection(&new_ids), None);
    }

    #[test]
    fn split_tool_arc_midpoint_commits() {
        let mut doc = Document::new();
        let arc = Shape::Arc(Arc::new(Point2::ORIGIN, 5.0, 0.0, std::f64::consts::PI));
        let id = doc
            .apply(Command::AddEntity(Entity::new(
                arc.clone(),
                doc.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let mut tool = SplitTool::default();
        let result = tool.on_shape_pick(ShapePick {
            id,
            shape: arc,
            click: Point2::new(0.0, 5.0),
            layer: doc.current_layer(),
            style: Style::inherited(),
        });
        assert!(matches!(result, ToolResult::Commit(Command::Batch(_))));
    }

    #[test]
    fn split_tool_open_polyline_commits() {
        let mut doc = Document::new();
        let pl = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(10.0, 10.0),
            ],
            false,
        ));
        let id = doc
            .apply(Command::AddEntity(Entity::new(
                pl.clone(),
                doc.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let mut tool = SplitTool::default();
        let result = tool.on_shape_pick(ShapePick {
            id,
            shape: pl,
            click: Point2::new(5.0, 0.0),
            layer: doc.current_layer(),
            style: Style::inherited(),
        });
        assert!(matches!(result, ToolResult::Commit(Command::Batch(_))));
    }

    #[test]
    fn split_tool_near_endpoint_rejected_and_state_unchanged() {
        let mut doc = Document::new();
        let target = shape_pick(&mut doc, hline(0.0, 10.0, 0.0), Point2::new(0.0, 0.0));
        let mut tool = SplitTool::default();
        let result = tool.on_shape_pick(target);
        assert_eq!(
            result,
            ToolResult::Rejected("Split: too close to an endpoint")
        );
        // 単一状態なので「据え置き」は次のピックが通常どおり処理できることで確認する。
        let retry = shape_pick(&mut doc, hline(0.0, 10.0, 20.0), Point2::new(5.0, 20.0));
        assert!(matches!(
            tool.on_shape_pick(retry),
            ToolResult::Commit(Command::Batch(_))
        ));
    }

    #[test]
    fn split_tool_circle_rejected_unsupported() {
        let mut doc = Document::new();
        let circle_id = add_circle(&mut doc, Point2::ORIGIN, 5.0);
        let mut tool = SplitTool::default();
        let result = tool.on_shape_pick(ShapePick {
            id: circle_id,
            shape: Shape::Circle(mcad_geom::Circle::new(Point2::ORIGIN, 5.0)),
            click: Point2::new(5.0, 0.0),
            layer: doc.current_layer(),
            style: Style::inherited(),
        });
        assert_eq!(
            result,
            ToolResult::Rejected("Split: target must be a line, arc, or open polyline")
        );
    }

    #[test]
    fn split_tool_closed_polyline_rejected_unsupported() {
        let mut doc = Document::new();
        let pl = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(5.0, 10.0),
            ],
            true,
        ));
        let id = doc
            .apply(Command::AddEntity(Entity::new(
                pl.clone(),
                doc.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let mut tool = SplitTool::default();
        let result = tool.on_shape_pick(ShapePick {
            id,
            shape: pl,
            click: Point2::new(5.0, 0.0),
            layer: doc.current_layer(),
            style: Style::inherited(),
        });
        assert_eq!(
            result,
            ToolResult::Rejected("Split: target must be a line, arc, or open polyline")
        );
    }

    #[test]
    fn split_tool_projects_off_shape_click_onto_closest_point() {
        // クリック点が線分から少しずれていても closest_point で射影されて
        // 正しい位置 (5,0) で分割される。
        let mut doc = Document::new();
        let target = shape_pick(&mut doc, hline(0.0, 10.0, 0.0), Point2::new(5.0, 3.0));
        let target_shape = target.shape.clone();
        let mut tool = SplitTool::default();
        let result = tool.on_shape_pick(target);
        let ToolResult::Commit(Command::Batch(cmds)) = result else {
            panic!("expected Commit(Batch), got {result:?}");
        };
        let l1 = as_line_shape(match &cmds[1] {
            Command::AddEntity(e) => match &e.geom {
                EntityGeom::Shape(s) => s,
                _ => panic!("expected shape"),
            },
            _ => panic!("expected AddEntity"),
        });
        assert_eq!(l1.a, line_of(&target_shape).a);
        assert_eq!(l1.b, Point2::new(5.0, 0.0));
    }

    #[test]
    fn split_tool_cancel_resets_and_click_event_does_not_advance() {
        let (_doc, ctx) = ctx();
        let mut tool = SplitTool::default();
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0))),
            ToolResult::Continue,
            "クリックは常にヒットテスト経路を通るので on_input には来ない想定"
        );
    }

    #[test]
    fn split_batch_rolls_back_on_locked_layer() {
        // Batch は原子的: 対象レイヤーがロックされていれば削除も追加も起きない。
        let mut doc = Document::new();
        let locked_layer = doc
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "locked",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let target_shape = hline(0.0, 10.0, 0.0);
        let target_id = doc
            .apply(Command::AddEntity(Entity::new(
                target_shape.clone(),
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

        let entities_before = doc.entities().count();
        let geom_before = doc.entity(target_id).unwrap().geom.clone();

        let mut tool = SplitTool::default();
        let ToolResult::Commit(cmd) = tool.on_shape_pick(ShapePick {
            id: target_id,
            shape: target_shape,
            click: Point2::new(5.0, 0.0),
            layer: locked_layer,
            style: Style::inherited(),
        }) else {
            panic!("expected Commit");
        };

        assert!(doc.apply(cmd).is_err(), "ロックレイヤーなので失敗する");
        assert_eq!(doc.entities().count(), entities_before);
        assert_eq!(doc.entity(target_id).unwrap().geom, geom_before);
    }

    // --- 寸法の選択（pick 経由）---

    #[test]
    fn pick_selects_linear_dimension_on_dimension_line() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let dim = doc
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::DimLinear(DimLinear {
                    p1: Point2::new(0.0, 0.0),
                    p2: Point2::new(4.0, 0.0),
                    offset: 2.0,
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];

        // 寸法線 (0,2)-(4,2) 上をクリックすると選択される（DimLinear も選択対象）。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(2.0, 2.02), 0.1, false);
        assert_eq!(tool.selection(), &[dim]);

        // 寸法線から離れた点は拾わない。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(2.0, 5.0), 0.1, false);
        assert!(tool.selection().is_empty());
    }

    #[test]
    fn pick_selects_radial_dimension_on_leader() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let dim = doc
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::DimRadial(DimRadial {
                    center: Point2::ORIGIN,
                    radius: 5.0,
                    leader_angle: 0.0,
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];

        // 引出線 (0,0)-(5,0) 上をクリックすると選択される。
        let mut tool = SelectTool::default();
        tool.on_click(&doc, Point2::new(2.0, 0.02), 0.1, false);
        assert_eq!(tool.selection(), &[dim]);
    }
}
