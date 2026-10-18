//! 拘束ツール（M13 タスク99、DESIGN.md M13 設計判断6）。
//!
//! # 構成
//!
//! - [`ConstraintKind`] は上部パネルのコンボで選ぶ拘束の種類（14 種。接線は対象の種類で
//!   線と円・円どうしへ振り分ける）。
//! - [`ConstraintTool`] は選んだ対象（[`Target`]）とホバー中の候補を持つ状態機械。
//!   [`ConstraintTool::click`] が最後の対象のクリックで [`Command::AddConstraint`] を返す。
//! - [`apply_constraint_command`] はそのコマンドを `apply` し、ステータスバーの文言を返す
//!   （線分が伸びたときの案内つき。判断3 の既知の制約 (b)）。
//! - ヒットテスト（`hit_target`）は自前: 端点・中心は `snap::SnapResult` が
//!   エンティティも端の種類も持たないため、ここで自前に当てる。
//!
//! # なぜ [`crate::tool::Tool`] トレイトに載せないか
//!
//! 作図ツールは [`Document`] を参照しない設計（`tool.rs` 冒頭 doc）で、ヒットテストと
//! コマンドの組み立てに要る `Document` は拡張点（`wants_shape_pick` 等）経由で app 層が
//! 担う。拘束ツールは「ホバー中の候補の判定」「種類ごとの対象の絞り込み」「core の
//! コンストラクタ（`Constraint::distance(doc, ..)` など、現在の形から値・側を決める）」の
//! 3 か所で `Document` が要り、トレイトに載せると拡張点を 4〜5 本足すことになる。そこで
//! [`crate::tool::SelectTool`] の配置（`placement_click(&Document, ..)`）と同じく、
//! `Document` を引数に取るメソッドを持つ独立した状態として `McadApp` が所有する。
//! `ToolKind::Constraint` は `spawn()` で `None` を返し、キャンバス入力は
//! `handle_constraint_input`（main.rs）が振り分ける。

use std::f64::consts::PI;

use mcad_core::{Command, Constraint, Document, EntityGeom, EntityId, PointKind, PointRef};
use mcad_geom::{Point2, Shape, distance_to};

use crate::tool::layer_visible;

/// 拘束ツールで作る拘束の種類（上部パネルのコンボの選択肢）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConstraintKind {
    /// 一致（点・点）。
    #[default]
    Coincident,
    /// 水平（線分）。
    Horizontal,
    /// 垂直（線分）。
    Vertical,
    /// 点どうしの距離（点・点）。
    Distance,
    /// 点と線の距離（点・線分。どちらから選んでもよい）。
    PointLineDistance,
    /// 平行（線分・線分）。
    Parallel,
    /// 直交（線分・線分）。
    Perpendicular,
    /// 等長（線分・線分）。
    EqualLength,
    /// 等半径（円/円弧・円/円弧）。
    EqualRadius,
    /// 半径（円/円弧）。
    Radius,
    /// 角度（線分・線分）。
    Angle,
    /// 接線（線分・円/円弧、または円/円弧・円/円弧）。
    Tangent,
    /// 点の固定（点）。
    FixPoint,
    /// 図形の固定（線分・円・円弧）。
    FixEntity,
}

/// 値の単位（上部パネルの Value 欄の後ろに出す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueUnit {
    /// mm（ワールド単位）。
    Mm,
    /// 度（保存はラジアン）。
    Deg,
}

impl ValueUnit {
    /// 上部パネルの表示。
    pub fn label(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Deg => "deg",
        }
    }
}

impl ConstraintKind {
    /// コンボの表示順。
    pub const ALL: [Self; 14] = [
        Self::Coincident,
        Self::Horizontal,
        Self::Vertical,
        Self::Distance,
        Self::PointLineDistance,
        Self::Parallel,
        Self::Perpendicular,
        Self::EqualLength,
        Self::EqualRadius,
        Self::Radius,
        Self::Angle,
        Self::Tangent,
        Self::FixPoint,
        Self::FixEntity,
    ];

    /// コンボ・ステータスバーの表示名（英語の領域）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Coincident => "Coincident",
            Self::Horizontal => "Horizontal",
            Self::Vertical => "Vertical",
            Self::Distance => "Distance",
            Self::PointLineDistance => "Point-line distance",
            Self::Parallel => "Parallel",
            Self::Perpendicular => "Perpendicular",
            Self::EqualLength => "Equal length",
            Self::EqualRadius => "Equal radius",
            Self::Radius => "Radius",
            Self::Angle => "Angle",
            Self::Tangent => "Tangent",
            Self::FixPoint => "Fix point",
            Self::FixEntity => "Fix entity",
        }
    }

    /// 値を持つ種類なら単位。値の無い種類は `None`（Value 欄を出さない）。
    pub fn value_unit(self) -> Option<ValueUnit> {
        match self {
            Self::Distance | Self::PointLineDistance | Self::Radius => Some(ValueUnit::Mm),
            Self::Angle => Some(ValueUnit::Deg),
            _ => None,
        }
    }

    /// 選ぶ対象の数（1 か 2）。
    pub fn target_count(self) -> usize {
        match self {
            Self::Horizontal | Self::Vertical | Self::Radius | Self::FixPoint | Self::FixEntity => {
                1
            }
            _ => 2,
        }
    }

    /// 既存の拘束の種類（記号の一覧・タスク100 の表示で使う）。
    pub fn of(constraint: &Constraint) -> Self {
        match constraint {
            Constraint::Coincident { .. } => Self::Coincident,
            Constraint::Horizontal { .. } => Self::Horizontal,
            Constraint::Vertical { .. } => Self::Vertical,
            Constraint::Distance { .. } => Self::Distance,
            Constraint::PointLineDistance { .. } => Self::PointLineDistance,
            Constraint::Parallel { .. } => Self::Parallel,
            Constraint::Perpendicular { .. } => Self::Perpendicular,
            Constraint::EqualLength { .. } => Self::EqualLength,
            Constraint::EqualRadius { .. } => Self::EqualRadius,
            Constraint::Radius { .. } => Self::Radius,
            Constraint::Angle { .. } => Self::Angle,
            Constraint::TangentLineCircle { .. } | Constraint::TangentCircles { .. } => {
                Self::Tangent
            }
            Constraint::FixPoint { .. } => Self::FixPoint,
            Constraint::FixEntity { .. } => Self::FixEntity,
        }
    }

    /// 追加で線分が伸びうる種類か（判断3 の既知の制約 (b): 回して満たすと最小ノルム解が
    /// 端点を線分に垂直な向きへ動かし、長さが保たれない）。
    fn may_stretch(self) -> bool {
        matches!(
            self,
            Self::Horizontal | Self::Vertical | Self::Parallel | Self::Perpendicular | Self::Angle
        )
    }
}

/// 拘束の対象 1 つ（選んだもの・ホバー中の候補）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// 端点・中心（[`PointRef`]）。
    Point(PointRef),
    /// 線分。
    Line(EntityId),
    /// 円・円弧。
    Round(EntityId),
}

impl Target {
    /// 対象のエンティティ（点なら点の属するエンティティ）。
    pub fn entity(self) -> EntityId {
        match self {
            Self::Point(p) => p.entity,
            Self::Line(id) | Self::Round(id) => id,
        }
    }
}

/// いまの段階で受け付ける対象の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Accepts {
    points: bool,
    lines: bool,
    rounds: bool,
}

impl Accepts {
    const POINTS: Self = Self {
        points: true,
        lines: false,
        rounds: false,
    };
    const LINES: Self = Self {
        points: false,
        lines: true,
        rounds: false,
    };
    const ROUNDS: Self = Self {
        points: false,
        lines: false,
        rounds: true,
    };
    const SHAPES: Self = Self {
        points: false,
        lines: true,
        rounds: true,
    };
    const POINTS_OR_LINES: Self = Self {
        points: true,
        lines: true,
        rounds: false,
    };

    fn admits(self, target: Target) -> bool {
        match target {
            Target::Point(_) => self.points,
            Target::Line(_) => self.lines,
            Target::Round(_) => self.rounds,
        }
    }

    /// 対象が見つからないときのステータス文言。
    fn prompt(self) -> &'static str {
        match (self.points, self.lines, self.rounds) {
            (true, false, false) => "click an endpoint or a center",
            (false, true, false) => "click a line",
            (false, false, true) => "click a circle or an arc",
            (false, true, true) => "click a line, a circle or an arc",
            (true, true, false) => "click an endpoint, a center or a line",
            _ => "click a target",
        }
    }
}

/// 上部パネルの Value 欄を解析した結果（[`crate::tool::ChamferDistInput`] と同じ 3 状態）。
///
/// 空欄は「作成時の現在値」という意味を持つため、不正な文字列を空欄と同じ扱いにすると
/// 打ち間違えた値が黙って現在値として確定してしまう。不正値は確定クリックを拒否する。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ValueInput {
    /// 空欄。作成時の現在値（core のコンストラクタが決める値）を使う。
    #[default]
    Empty,
    /// 有限の数値（mm または度。度→ラジアンは組み立てるときに 1 回だけ変換する）。
    Value(f64),
    /// 非数値・非有限の文字列。確定クリックは拒否する。
    Invalid,
}

/// Value 欄の文字列を解析する（前後の空白は無視）。範囲（正・非負）は種類ごとに
/// [`build_command`] が確かめる。
pub fn parse_value_input(text: &str) -> ValueInput {
    let text = text.trim();
    if text.is_empty() {
        return ValueInput::Empty;
    }
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() => ValueInput::Value(v),
        _ => ValueInput::Invalid,
    }
}

/// `(−π, π]` へ正規化する（すでに範囲内なら値を変えない。core の正規化と同じ規則）。
fn normalize_angle(a: f64) -> f64 {
    if a > -PI && a <= PI {
        return a;
    }
    let w = a.sin().atan2(a.cos());
    if w <= -PI { PI } else { w }
}

/// [`ConstraintTool::click`] の結果。
///
/// `Command` を直接持つ理由と `clippy::large_enum_variant` の抑制は
/// [`crate::tool::ToolResult`] と同じ（1 クリックに 1 つ作るだけの値で、`Box` 化する利点が無い）。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ClickOutcome {
    /// 対象を 1 つ選んだ（まだ確定しない）。
    Continue,
    /// クリックを拒否した（状態は据え置き）。理由はステータスバーへ出す。
    Rejected(String),
    /// 最後の対象を選んだ。呼び出し側は [`apply_constraint_command`] で適用し、成功したら
    /// [`ConstraintTool::on_committed`] を呼ぶ（失敗なら最後の対象だけを選び直せる）。
    Commit(Command),
}

/// 拘束ツールの状態。
///
/// 種類（[`ConstraintKind`]）はツールを抜けても保持する（次に `Q` を押したとき同じ種類で
/// 始まる）。選んだ対象はツールの切り替え・undo/redo・ファイル操作で捨てる
/// （[`ConstraintTool::reset`]）。
#[derive(Debug, Default)]
pub struct ConstraintTool {
    kind: ConstraintKind,
    picks: Vec<Target>,
    hover: Option<Target>,
}

impl ConstraintTool {
    /// 選んでいる種類。
    pub fn kind(&self) -> ConstraintKind {
        self.kind
    }

    /// 種類を変える。変わったら選んだ対象を捨てる（種類ごとに受け付ける対象が違うため）。
    pub fn set_kind(&mut self, kind: ConstraintKind) {
        if kind != self.kind {
            self.kind = kind;
            self.picks.clear();
            self.hover = None;
        }
    }

    /// 選んだ対象（選んだ順）。
    pub fn picks(&self) -> &[Target] {
        &self.picks
    }

    /// ホバー中の候補（次のクリックで選ばれるもの）。
    pub fn hover(&self) -> Option<Target> {
        self.hover
    }

    /// 選んだ対象とホバーを捨てる（種類は保つ）。
    pub fn reset(&mut self) {
        self.picks.clear();
        self.hover = None;
    }

    /// `Esc`: 選んだ対象があれば捨てて `true`（ツールに留まる）。無ければ `false`
    /// （呼び出し側が選択モードへ戻る）。
    pub fn cancel(&mut self) -> bool {
        self.hover = None;
        if self.picks.is_empty() {
            false
        } else {
            self.picks.clear();
            true
        }
    }

    /// 確定（`apply` の成功）の後で呼ぶ。次の拘束の 1 つ目待ちへ戻る。
    pub fn on_committed(&mut self) {
        self.picks.clear();
    }

    /// 次のクリックで受け付ける対象の種類。
    fn accepts(&self) -> Accepts {
        let first = self.picks.first().copied();
        match self.kind {
            ConstraintKind::Coincident | ConstraintKind::Distance | ConstraintKind::FixPoint => {
                Accepts::POINTS
            }
            ConstraintKind::Horizontal
            | ConstraintKind::Vertical
            | ConstraintKind::Parallel
            | ConstraintKind::Perpendicular
            | ConstraintKind::EqualLength
            | ConstraintKind::Angle => Accepts::LINES,
            ConstraintKind::EqualRadius | ConstraintKind::Radius => Accepts::ROUNDS,
            ConstraintKind::FixEntity => Accepts::SHAPES,
            ConstraintKind::PointLineDistance => match first {
                None => Accepts::POINTS_OR_LINES,
                Some(Target::Point(_)) => Accepts::LINES,
                Some(_) => Accepts::POINTS,
            },
            ConstraintKind::Tangent => match first {
                None | Some(Target::Round(_)) => Accepts::SHAPES,
                Some(_) => Accepts::ROUNDS,
            },
        }
    }

    /// 選び済みの対象と重なるので選べないか。一致・点どうしの距離は同じ点だけ
    /// （同じ線分の始点と終点は可）、それ以外は同じエンティティを拒否する。
    fn duplicates(&self, target: Target) -> bool {
        self.picks.iter().any(|&picked| match self.kind {
            ConstraintKind::Coincident | ConstraintKind::Distance => picked == target,
            _ => picked.entity() == target.entity(),
        })
    }

    /// 次の対象の案内（上部パネルに出す。`Horizontal: click a line (1/1)` の形）。
    pub fn prompt(&self) -> String {
        format!(
            "{}: {} ({}/{})",
            self.kind.label(),
            self.accepts().prompt(),
            self.picks.len() + 1,
            self.kind.target_count()
        )
    }

    /// カーソル `world` の下の、次に選べる候補を `hover` に入れる（`None` ならホバーを消す）。
    pub fn update_hover(&mut self, document: &Document, world: Option<Point2>, tol: PickTol) {
        self.hover = world.and_then(|w| match self.hit(document, w, tol) {
            Hit::Admissible(t) => Some(t),
            Hit::Duplicate | Hit::Nothing => None,
        });
    }

    fn hit(&self, document: &Document, world: Point2, tol: PickTol) -> Hit {
        let accepts = self.accepts();
        let mut duplicate = false;
        let found = hit_target(document, world, tol, accepts, |t| {
            let dup = self.duplicates(t);
            duplicate |= dup;
            !dup
        });
        match found {
            Some(t) => Hit::Admissible(t),
            None if duplicate => Hit::Duplicate,
            None => Hit::Nothing,
        }
    }

    /// 左クリック。対象を当て、最後の対象なら拘束を組み立てて [`ClickOutcome::Commit`]。
    ///
    /// 確定まで選んだ対象は変えない（`apply` が失敗したら最後の対象だけ選び直せる。
    /// フィレットの 2 本目と同じ流儀。DESIGN.md M7 設計判断6）。
    pub fn click(
        &mut self,
        document: &Document,
        world: Point2,
        tol: PickTol,
        value: ValueInput,
    ) -> ClickOutcome {
        let target = match self.hit(document, world, tol) {
            Hit::Admissible(t) => t,
            Hit::Duplicate => {
                return ClickOutcome::Rejected(format!(
                    "{}: already picked; pick a different {}",
                    self.kind.label(),
                    if matches!(
                        self.kind,
                        ConstraintKind::Coincident | ConstraintKind::Distance
                    ) {
                        "point"
                    } else {
                        "entity"
                    }
                ));
            }
            Hit::Nothing => {
                return ClickOutcome::Rejected(format!(
                    "{}: {}",
                    self.kind.label(),
                    self.accepts().prompt()
                ));
            }
        };
        if self.picks.len() + 1 < self.kind.target_count() {
            self.picks.push(target);
            return ClickOutcome::Continue;
        }
        let mut targets = self.picks.clone();
        targets.push(target);
        match build_command(document, self.kind, &targets, value) {
            Ok(cmd) => ClickOutcome::Commit(cmd),
            Err(msg) => ClickOutcome::Rejected(msg),
        }
    }
}

/// [`ConstraintTool::hit`] の結果。
enum Hit {
    Admissible(Target),
    /// 候補はあったが、すべて選び済みの対象と重なる。
    Duplicate,
    Nothing,
}

/// ピックの許容量（ワールド単位）。点は端点スナップと同じ半径、図形は選択と同じ半径。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickTol {
    /// 端点・中心の許容量。
    pub point: f64,
    /// 線分・円・円弧の許容量。
    pub shape: f64,
}

/// 図形の上の、拘束が指せる点（線分の始点・終点、円の中心、円弧の始点・終点・中心）。
pub fn point_candidates(shape: &Shape) -> Vec<(PointKind, Point2)> {
    match shape {
        Shape::Line(l) => vec![(PointKind::Start, l.a), (PointKind::End, l.b)],
        Shape::Circle(c) => vec![(PointKind::Center, c.center)],
        Shape::Arc(a) => vec![
            (PointKind::Start, a.start_point()),
            (PointKind::End, a.end_point()),
            (PointKind::Center, a.center),
        ],
        Shape::Point(_) | Shape::Polyline(_) => Vec::new(),
    }
}

/// 拘束の対象にできる図形（線分・円・円弧）か。
fn constrainable(geom: &EntityGeom) -> Option<&Shape> {
    match geom.as_shape()? {
        s @ (Shape::Line(_) | Shape::Circle(_) | Shape::Arc(_)) => Some(s),
        Shape::Point(_) | Shape::Polyline(_) => None,
    }
}

/// 受け付ける種類の候補の中から、`world` に最も近いものを返す（`keep` が `false` の候補は
/// 除く）。点と図形の両方を受け付ける段階では、点の候補が許容量内にあれば点を優先する
/// （線分の端の近くをクリックしたら端点）。
fn hit_target(
    document: &Document,
    world: Point2,
    tol: PickTol,
    accepts: Accepts,
    mut keep: impl FnMut(Target) -> bool,
) -> Option<Target> {
    let mut best_point: Option<(f64, Target)> = None;
    let mut best_shape: Option<(f64, Target)> = None;
    for (id, entity) in document.entities() {
        if !layer_visible(document, entity) {
            continue;
        }
        let Some(shape) = constrainable(&entity.geom) else {
            continue;
        };
        if accepts.points {
            for (kind, p) in point_candidates(shape) {
                let d = p.distance(world);
                let t = Target::Point(PointRef::new(id, kind));
                if d <= tol.point && best_point.is_none_or(|(bd, _)| d < bd) && keep(t) {
                    best_point = Some((d, t));
                }
            }
        }
        let shape_target = match shape {
            Shape::Line(_) => Target::Line(id),
            _ => Target::Round(id),
        };
        if accepts.admits(shape_target) {
            let d = distance_to(shape, world);
            if d <= tol.shape && best_shape.is_none_or(|(bd, _)| d < bd) && keep(shape_target) {
                best_shape = Some((d, shape_target));
            }
        }
    }
    best_point.or(best_shape).map(|(_, t)| t)
}

/// 選んだ対象（選んだ順）から `Command::AddConstraint` を組み立てる。
///
/// - 値を持つ種類: Value 欄が空なら core のコンストラクタが決める現在値、数値ならその値
///   （角度は度→ラジアンへ 1 回だけ変換し `(−π, π]` へ正規化）。値を打った点どうしの距離は
///   コンストラクタを通さず直接組む（コンストラクタは現在の 2 点が一致していると拒否するが、
///   打った値が正なら作れるため）。
/// - `hold`: 2 対象なら先に選んだ対象のエンティティ。ただし 2 つが同じエンティティ
///   （同じ線分の始点と終点）なら `None`（止めると唯一の図形が動けない）。1 対象は `None`。
///
/// # Errors
///
/// 値が不正・範囲外のとき、core のコンストラクタが拒否したとき（文言はステータスバー用）。
pub fn build_command(
    document: &Document,
    kind: ConstraintKind,
    targets: &[Target],
    value: ValueInput,
) -> Result<Command, String> {
    let fail = |err: mcad_core::CoreError| format!("Constraint failed: {err}");
    let typed = match (kind.value_unit(), value) {
        (None, _) | (Some(_), ValueInput::Empty) => None,
        (Some(unit), ValueInput::Invalid) => {
            return Err(format!(
                "{}: the value is not a number ({})",
                kind.label(),
                unit.label()
            ));
        }
        (Some(ValueUnit::Deg), ValueInput::Value(deg)) => Some(normalize_angle(deg.to_radians())),
        (Some(ValueUnit::Mm), ValueInput::Value(mm)) => Some(mm),
    };
    if let Some(v) = typed {
        match kind {
            ConstraintKind::Distance if v <= 0.0 => {
                return Err(
                    "Distance must be positive; use Coincident for zero distance".to_owned(),
                );
            }
            ConstraintKind::PointLineDistance if v < 0.0 => {
                return Err("Point-line distance must not be negative".to_owned());
            }
            ConstraintKind::Radius if v <= 0.0 => {
                return Err("Radius must be positive".to_owned());
            }
            _ => {}
        }
    }

    let first = targets.first().copied();
    let second = targets.get(1).copied();
    let point = |t: Option<Target>| match t {
        Some(Target::Point(p)) => Ok(p),
        _ => Err(format!("{}: expected a point", kind.label())),
    };
    let entity = |t: Option<Target>| {
        t.map(Target::entity)
            .ok_or_else(|| format!("{}: missing target", kind.label()))
    };

    let constraint = match kind {
        ConstraintKind::Coincident => Constraint::Coincident {
            a: point(first)?,
            b: point(second)?,
        },
        ConstraintKind::Horizontal => Constraint::Horizontal {
            line: entity(first)?,
        },
        ConstraintKind::Vertical => Constraint::Vertical {
            line: entity(first)?,
        },
        ConstraintKind::Distance => {
            let (a, b) = (point(first)?, point(second)?);
            match typed {
                Some(value) => Constraint::Distance { a, b, value },
                None => Constraint::distance(document, a, b).map_err(fail)?,
            }
        }
        ConstraintKind::PointLineDistance => {
            let (p, line) = match (first, second) {
                (Some(Target::Point(p)), Some(line)) | (Some(line), Some(Target::Point(p))) => {
                    (p, line.entity())
                }
                _ => return Err("Point-line distance: expected a point and a line".to_owned()),
            };
            let mut c = Constraint::point_line_distance(document, p, line).map_err(fail)?;
            if let (Some(v), Constraint::PointLineDistance { value, .. }) = (typed, &mut c) {
                *value = v;
            }
            c
        }
        ConstraintKind::Parallel => Constraint::Parallel {
            a: entity(first)?,
            b: entity(second)?,
        },
        ConstraintKind::Perpendicular => Constraint::Perpendicular {
            a: entity(first)?,
            b: entity(second)?,
        },
        ConstraintKind::EqualLength => Constraint::EqualLength {
            a: entity(first)?,
            b: entity(second)?,
        },
        ConstraintKind::EqualRadius => Constraint::EqualRadius {
            a: entity(first)?,
            b: entity(second)?,
        },
        ConstraintKind::Radius => {
            let mut c = Constraint::radius(document, entity(first)?).map_err(fail)?;
            if let (Some(v), Constraint::Radius { value, .. }) = (typed, &mut c) {
                *value = v;
            }
            c
        }
        ConstraintKind::Angle => {
            let mut c =
                Constraint::angle(document, entity(first)?, entity(second)?).map_err(fail)?;
            if let (Some(v), Constraint::Angle { value, .. }) = (typed, &mut c) {
                *value = v;
            }
            c
        }
        ConstraintKind::Tangent => match (first, second) {
            (Some(Target::Line(line)), Some(Target::Round(circle)))
            | (Some(Target::Round(circle)), Some(Target::Line(line))) => {
                Constraint::tangent_line_circle(document, line, circle).map_err(fail)?
            }
            (Some(Target::Round(a)), Some(Target::Round(b))) => {
                Constraint::tangent_circles(document, a, b).map_err(fail)?
            }
            _ => {
                return Err(
                    "Tangent: expected a line and a circle/arc, or two circles/arcs".to_owned(),
                );
            }
        },
        ConstraintKind::FixPoint => Constraint::fix_point(document, point(first)?).map_err(fail)?,
        ConstraintKind::FixEntity => Constraint::FixEntity {
            entity: entity(first)?,
        },
    };

    let hold = match (first, second) {
        (Some(a), Some(b)) if a.entity() != b.entity() => Some(a.entity()),
        _ => None,
    };
    Ok(Command::AddConstraint { constraint, hold })
}

/// 線分の長さの比較で「伸びた」とみなす相対量。ソルバーの許容値（`RTOL = 1e-9` × 代表長さ）
/// より十分大きく、許容値以内の追従（判断3 の既知の制約 (d)）を伸びと誤認しない。
const STRETCH_RTOL: f64 = 1e-6;

/// 拘束が参照する線分と `extra` の線分の長さ（ID 順、重複なし）。ソルバーが動かせるのは
/// 拘束でつながった図形だけなので、伸びを調べるのはこの集合で足りる。
fn constrained_line_lengths(document: &Document, extra: &[EntityId]) -> Vec<(EntityId, f64)> {
    let mut ids: Vec<EntityId> = document
        .constraints()
        .flat_map(|(_, c)| c.entities())
        .chain(extra.iter().copied())
        .collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| match document.entity(id)?.geom.as_shape()? {
            Shape::Line(l) => Some((id, l.length())),
            _ => None,
        })
        .collect()
}

/// `before` のどれかの線分の長さが変わったか。
fn any_line_stretched(document: &Document, before: &[(EntityId, f64)]) -> bool {
    before.iter().any(|&(id, len)| {
        let now = match document.entity(id).and_then(|e| e.geom.as_shape()) {
            Some(Shape::Line(l)) => l.length(),
            _ => return false,
        };
        (now - len).abs() > STRETCH_RTOL * len.max(now).max(1.0)
    })
}

/// 拘束ツールのコマンドを適用し、ステータスバーの文言を返す（成功は `Ok`、失敗は `Err`。
/// どちらも表示する）。
///
/// 水平・垂直・平行・直交・角度の追加で、拘束でつながった線分の長さが変わったときは、
/// 長さを保つには距離拘束を併用する旨を添える（判断3 の既知の制約 (b) の案内）。
pub fn apply_constraint_command(
    document: &mut Document,
    kind: ConstraintKind,
    cmd: Command,
) -> Result<String, String> {
    let before = if kind.may_stretch() {
        let targets = match &cmd {
            Command::AddConstraint { constraint, .. } => constraint.entities(),
            _ => Vec::new(),
        };
        constrained_line_lengths(document, &targets)
    } else {
        Vec::new()
    };
    match document.apply(cmd) {
        Ok(_) => {
            let base = format!("Added {} constraint", kind.label());
            if any_line_stretched(document, &before) {
                Ok(format!(
                    "{base}; a line changed length (add a Distance constraint to keep it)"
                ))
            } else {
                Ok(base)
            }
        }
        Err(err) => Err(format!("Constraint failed: {err}")),
    }
}

#[cfg(test)]
mod tests;
