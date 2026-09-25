//! 数値入力（M12 タスク83、DESIGN.md M12 設計判断3。AutoCAD の動的入力に相当）。
//!
//! 点を受け付ける作図ツール（[`crate::tool::Tool::accepts_numeric_input`]）と、選択モードの
//! 移動・複製の配置の使用中に `0`〜`9`・`.`・`-`・`@` を打つと、カーソル横に入力欄が開く。
//! `Enter` で確定した点は、通常のクリック（`InputEvent::Click`）としてツールへ渡る。
//!
//! 本モジュールは egui の描画にも [`mcad_core::Document`] にも依存しない。持つのは次の 3 つ:
//!
//! - **書式の解釈**（[`parse`] / [`resolve_point`]）: `&str` → 点。基点とカーソル方向は
//!   引数で受ける（極トラッキング（タスク84）は方向の出どころを差し替えるだけで済む）。
//!   度 → ラジアンの変換はこの境界（[`unit_from_degrees`]）に閉じる（M11 タスク73 と同じ）。
//! - **入力欄の状態**（[`NumericInputState`]）: 開閉・文字列・最後のカーソル位置・表示位置。
//! - **キーの振り分け**（[`route_keys`]）: 1 フレーム分の `egui::Event` 列から入力欄が使う
//!   ものを取り除く。取り除いたキーは下流（ツール切替・`Enter` の `Confirm`・`Esc` の
//!   キャンセル・選択モードの `Delete` 等）へ届かない。
//!
//! # 書式
//!
//! | 書式 | 意味 | 基点 |
//! |---|---|---|
//! | `x,y` | 絶対座標 | 不要 |
//! | `@dx,dy` | 基点からの相対 | 要 |
//! | `L<A`（`@L<A` も同じ） | 基点から長さ `L`・角度 `A` 度（+X から CCW） | 要 |
//! | `L` | 基点からカーソル方向へ長さ `L` | 要（方向も要） |
//!
//! 数値は `[+-]? (数字列 [. 数字列?] | . 数字列)` だけを受ける（指数表記・`inf`・`nan` は
//! 書式の誤り）。**空白は文字列全体の前後だけ無視し、中の空白はすべて誤り**にする（区切りの
//! 周り・`@` の直後も。入力中のパンや貼り付けの改行で混ざった空白を黙って読み飛ばさない）。
//! 負の長さは逆向き（`-10<0` は `10<180` と同じ点）。

use egui::{Event, Key, Modifiers, Pos2};
use mcad_geom::{Point2, Vec2};

/// 書式を解釈した結果（基点・方向を当てる前の段階）。角度は入力どおりの度のまま持つ
/// （ラジアンへの変換は [`resolve_point`] の中、[`unit_from_degrees`] で行う）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ParsedInput {
    /// `x,y`: 絶対座標。
    Absolute(Point2),
    /// `@dx,dy`: 基点からの相対変位。
    Relative(Vec2),
    /// `L<A` / `@L<A`: 基点から長さ・角度（度、+X から CCW）。
    Polar { length: f64, angle_deg: f64 },
    /// `L`: 基点からカーソル方向（極トラッキング中は追跡方向。タスク84）へ長さ。
    Distance(f64),
}

/// 数値入力を点にできなかった理由。文言はステータスバー（英語領域。AGENTS.md の
/// 日本語化規約）に出す [`NumericInputError::message`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumericInputError {
    /// 書式に合わない（空・`@` 単独・区切りの過不足・数値でない等）。
    InvalidFormat,
    /// 数値または結果の座標が有限でない（桁あふれ）。
    NonFinite,
    /// 相対系の書式（`@dx,dy`・`L<A`・`L`）だが、まだ基点が無い。
    NoBasePoint,
    /// `L`（長さのみ）だが、カーソルが基点に重なっていて方向が定まらない。
    NoDirection,
}

impl NumericInputError {
    /// ステータスバーへ出す文言（英語。ステータスバーは未日本語化領域）。
    #[must_use]
    pub(crate) fn message(self) -> &'static str {
        match self {
            NumericInputError::InvalidFormat => {
                "Numeric input: use x,y  @dx,dy  length<angle  or length"
            }
            NumericInputError::NonFinite => "Numeric input: value out of range",
            NumericInputError::NoBasePoint => {
                "Numeric input: no base point yet - enter absolute x,y"
            }
            NumericInputError::NoDirection => {
                "Numeric input: move the cursor away from the base point to set the direction"
            }
        }
    }
}

/// 入力欄に打った文字列を解釈する（基点・方向は見ない）。
///
/// 判定の順序: `@` 始まりなら相対系（`<` を含めば極座標、`,` を含めば `@dx,dy`、どちらも
/// 無ければ誤り — `@` 単独を含む）。それ以外は `<` を含めば極座標、`,` を含めば絶対座標、
/// どちらも無ければ長さのみ。`,` と `<` が混ざると片側が数値にならず誤りになる。
///
/// 空白は全体の前後だけ無視する（末尾の改行を含む貼り付け `100\n` は通る）。中に空白
/// （制御文字を置き換えた空白を含む。`NumericInputState::push_text`）があれば位置に関わらず
/// 誤り: `1 2` を `12` と読まないのはもちろん、`@1\n,2`（貼り付けで `,` の前に改行）のように
/// 区切りの周りに紛れた空白も、打ち間違いか貼り付けの崩れか分からないので確定させない
/// （Codex adversarial review [high] 指摘への対応、2026-09-25）。
pub(crate) fn parse(text: &str) -> Result<ParsedInput, NumericInputError> {
    let text = text.trim();
    if text.chars().any(char::is_whitespace) {
        return Err(NumericInputError::InvalidFormat);
    }
    if let Some(rest) = text.strip_prefix('@') {
        if rest.contains('<') {
            let (length, angle_deg) = parse_pair(rest, '<')?;
            return Ok(ParsedInput::Polar { length, angle_deg });
        }
        if rest.contains(',') {
            let (dx, dy) = parse_pair(rest, ',')?;
            return Ok(ParsedInput::Relative(Vec2::new(dx, dy)));
        }
        return Err(NumericInputError::InvalidFormat);
    }
    if text.contains('<') {
        let (length, angle_deg) = parse_pair(text, '<')?;
        return Ok(ParsedInput::Polar { length, angle_deg });
    }
    if text.contains(',') {
        let (x, y) = parse_pair(text, ',')?;
        return Ok(ParsedInput::Absolute(Point2::new(x, y)));
    }
    Ok(ParsedInput::Distance(parse_number(text)?))
}

/// 入力欄の文字列を点にする（[`parse`] + 基点・方向の適用）。
///
/// - `base`: 基点（作図ツールは [`crate::tool::Tool::numeric_input_base`]、移動・複製は
///   配置の基準点）。無ければ相対系の書式は [`NumericInputError::NoBasePoint`]。
/// - `direction`: `L`（長さのみ）の向き。呼び出し側はカーソル（スナップ・直交を適用した
///   ラバーバンドの先）− 基点を渡す。長さは問わず、零・非有限なら
///   [`NumericInputError::NoDirection`]。
///
/// 結果にスナップ・直交は掛けない（明示値が優先。設計判断3）。書式の誤りは基点の有無より
/// 先に判定する（`@abc` は基点が無くても書式の誤り）。
pub(crate) fn resolve_point(
    text: &str,
    base: Option<Point2>,
    direction: Option<Vec2>,
) -> Result<Point2, NumericInputError> {
    let parsed = parse(text)?;
    let point = match parsed {
        ParsedInput::Absolute(p) => p,
        ParsedInput::Relative(delta) => base.ok_or(NumericInputError::NoBasePoint)? + delta,
        ParsedInput::Polar { length, angle_deg } => {
            base.ok_or(NumericInputError::NoBasePoint)? + unit_from_degrees(angle_deg) * length
        }
        ParsedInput::Distance(length) => {
            let base = base.ok_or(NumericInputError::NoBasePoint)?;
            let unit = direction
                .filter(|d| d.x.is_finite() && d.y.is_finite())
                .and_then(Vec2::normalize)
                .ok_or(NumericInputError::NoDirection)?;
            base + unit * length
        }
    };
    if point.x.is_finite() && point.y.is_finite() {
        Ok(point)
    } else {
        Err(NumericInputError::NonFinite)
    }
}

/// 角度（度、+X から CCW）の単位方向ベクトル。度 → ラジアンの変換はここに閉じる。
///
/// 角度は `[0, 360)` へ正規化してから変換する。**90° の倍数は厳密な軸方向を返す**
/// （`(90°).to_radians().cos()` は `6.1e-17` で厳密な 0 にならず、`100<90` が
/// `(6.1e-15, 100)` になるのを避ける。`ortho::RECT_AXES` を単位ベクトルで持つのと同じ理由）。
#[must_use]
pub(crate) fn unit_from_degrees(angle_deg: f64) -> Vec2 {
    let mut deg = angle_deg.rem_euclid(360.0);
    // 絶対値の小さい負の角度は `rem_euclid` の丸めで 360.0 ちょうどになりうる。
    if deg >= 360.0 {
        deg = 0.0;
    }
    if deg == 0.0 {
        Vec2::new(1.0, 0.0)
    } else if deg == 90.0 {
        Vec2::new(0.0, 1.0)
    } else if deg == 180.0 {
        Vec2::new(-1.0, 0.0)
    } else if deg == 270.0 {
        Vec2::new(0.0, -1.0)
    } else {
        let (sin, cos) = deg.to_radians().sin_cos();
        Vec2::new(cos, sin)
    }
}

/// `sep` でちょうど 2 つに分けた数値の組。区切りが 2 つ以上・片側が空なら誤り。
fn parse_pair(text: &str, sep: char) -> Result<(f64, f64), NumericInputError> {
    let mut parts = text.split(sep);
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(NumericInputError::InvalidFormat);
    };
    Ok((parse_number(a)?, parse_number(b)?))
}

/// 1 つの数値（空白は [`parse`] が先に拒否済み）。`[+-]? (数字列 [. 数字列?] | . 数字列)`
/// だけを受け、有限でなければ（桁あふれ）[`NumericInputError::NonFinite`]。
fn parse_number(text: &str) -> Result<f64, NumericInputError> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (int_part, frac_part) = match unsigned.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (unsigned, None),
    };
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let well_formed = all_digits(int_part)
        && frac_part.is_none_or(all_digits)
        && (!int_part.is_empty() || frac_part.is_some_and(|f| !f.is_empty()));
    if !well_formed {
        return Err(NumericInputError::InvalidFormat);
    }
    let value: f64 = text.parse().map_err(|_| NumericInputError::InvalidFormat)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(NumericInputError::NonFinite)
    }
}

/// 入力欄を開くきっかけになる文字（設計判断3: `0`〜`9`・`.`・`-`・`@`）。
#[must_use]
pub(crate) fn is_trigger_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '.' | '-' | '@')
}

/// 入力欄へ入れる 1 文字を半角へ寄せる写像（捨てる文字は無い）。全角の ASCII 文字
/// （U+FF01〜U+FF5E。IME が有効なままの入力）は半角へ、数学のマイナス記号（U+2212）は
/// `-` へ写し、それ以外はそのまま返す。
///
/// **書式に使わない文字も捨てない**（判定は [`parse`] に任せる）。許可外の文字を黙って
/// 捨てると `1e3` → `13`、`1 2` → `12`、貼り付けた `@1foo,2` → `@1,2` のように、打った値と
/// 違う点で確定してしまう（Codex adversarial review [high] 指摘、2026-09-25）。捨てずに
/// 残せば欄に見えたまま確定が書式の誤りとして拒否され、`Backspace` で直せる。
#[must_use]
pub(crate) fn to_half_width(c: char) -> char {
    match c {
        // 全角 ASCII は U+FEE0 だけずらすと半角になる（範囲内なら常に有効な char）。
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(u32::from(c) - 0xFEE0).unwrap_or(c),
        '\u{2212}' => '-',
        c => c,
    }
}

/// カーソル横の入力欄の状態（`McadApp` が 1 つ持つ）。
///
/// `text` が `Some` の間は開いている。基点は持たない（ツール・配置の現在の状態から毎回
/// 取り直す）。`cursor` / `anchor` は入力欄が閉じている間も更新し続け、開いた瞬間から
/// `L` の方向と表示位置が分かるようにする。
#[derive(Debug, Default)]
pub(crate) struct NumericInputState {
    /// 入力中の文字列。`None` なら閉じている。
    text: Option<String>,
    /// 最後に分かったカーソル位置（ワールド。作図ツールはスナップ・直交を適用した
    /// ラバーバンドの先、移動・複製はスナップを適用した点）。`L` の方向に使う。
    /// ポインタがキャンバスから外れても最後の値を保つ。
    cursor: Option<Point2>,
    /// 入力欄を描く位置（スクリーン。ポインタの位置）。
    anchor: Option<Pos2>,
    /// いまの文字列で確定したときの点（プレビュー用）。解釈できなければ `None`。
    preview: Option<Point2>,
    /// 空でない `Enter` より後に同じフレームで届いたキーボードのイベント。次のフレームの
    /// 先頭へ戻して到着順に処理する（[`route_keys`]・[`NumericInputState::restore_pending`]）。
    /// 入力欄を閉じても捨てない（確定に成功して閉じた後の打鍵こそ、ここに残っている）。
    pending: Vec<Event>,
}

impl NumericInputState {
    /// 入力欄が開いているか。
    #[must_use]
    pub(crate) fn is_open(&self) -> bool {
        self.text.is_some()
    }

    /// 入力中の文字列（閉じていれば `None`）。
    #[must_use]
    pub(crate) fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// 入力欄を閉じる（文字列とプレビューを捨てる）。閉じていれば何もしない。保留中の
    /// イベント（`pending`）は捨てない（確定の後に打たれた入力なので、次のフレームで流す）。
    pub(crate) fn close(&mut self) {
        self.text = None;
        self.preview = None;
    }

    /// 最後のカーソル位置を更新する（`L` の方向の出どころ）。
    pub(crate) fn set_cursor(&mut self, world: Point2) {
        self.cursor = Some(world);
    }

    /// 入力欄を描く位置を更新する。
    pub(crate) fn set_anchor(&mut self, screen: Pos2) {
        self.anchor = Some(screen);
    }

    /// 入力欄を描く位置（一度もポインタがキャンバスに乗っていなければ `None`）。
    #[must_use]
    pub(crate) fn anchor(&self) -> Option<Pos2> {
        self.anchor
    }

    /// プレビュー点（[`NumericInputState::refresh_preview`] が最後に求めたもの）。
    #[must_use]
    pub(crate) fn preview(&self) -> Option<Point2> {
        self.preview
    }

    /// いまの文字列を `base` と最後のカーソル位置で点にする（[`resolve_point`]）。
    /// 閉じていれば [`NumericInputError::InvalidFormat`]（呼び出し側は開いているときだけ呼ぶ）。
    pub(crate) fn resolve(&self, base: Option<Point2>) -> Result<Point2, NumericInputError> {
        let text = self.text.as_deref().unwrap_or("");
        let direction = base.zip(self.cursor).map(|(b, c)| c - b);
        resolve_point(text, base, direction)
    }

    /// プレビュー点を求め直して返す。閉じているか解釈できなければ `None`。
    pub(crate) fn refresh_preview(&mut self, base: Option<Point2>) -> Option<Point2> {
        self.preview = if self.is_open() {
            self.resolve(base).ok()
        } else {
            None
        };
        self.preview
    }

    /// 打った文字・貼り付けた文字列を足す。[`to_half_width`] で半角へ寄せるだけで、
    /// 書式外の文字も捨てない（判定は確定時の [`parse`]）。
    ///
    /// **制御文字（貼り付けに混ざる改行・タブ等）は空白 `' '` に置き換える**（1 文字ずつ。
    /// `\r\n` は空白 2 つ）。捨てると `1\n2` が `12` として確定してしまう（Codex adversarial
    /// review [high] 指摘、2026-09-25）。空白にすれば欄に見え、文字列の中にあれば [`parse`] が拒否し、
    /// 末尾の改行（`100\n` の貼り付け）は [`parse`] の trim で通る。
    ///
    /// **空白も残す**。入力中に `Space`（パンの修飾キー）を押すと空白が混ざるが、欄に
    /// 見えており、`1 2` のような数値の中の空白は [`parse`] が拒否するので、黙って別の値に
    /// なることは無い（前後の空白は [`parse`] の trim で許す）。
    fn push_text(&mut self, typed: &str) {
        if let Some(text) = self.text.as_mut() {
            text.extend(typed.chars().map(|c| {
                if c.is_control() {
                    ' '
                } else {
                    to_half_width(c)
                }
            }));
        }
    }

    fn backspace(&mut self) {
        if let Some(text) = self.text.as_mut() {
            text.pop();
        }
    }

    /// 前のフレームで保留したイベント（空でない `Enter` より後に届いたもの）を、`replay` の
    /// 範囲で今フレームの `events` の先頭へ戻し、保留を空にする（戻さなかったものは捨てる）。
    /// 呼び出し側は毎フレーム、[`route_keys`] より前に、そのフレームの状況から
    /// [`PendingReplay::for_frame`] で範囲を決めて呼ぶ。
    ///
    /// **押下時と修飾キーの状態が変わったキー押下は戻さずに捨てる**（どの範囲でも）。下流の
    /// ショートカットはイベント自身ではなくフレームの `modifiers`（`i.modifiers`）で
    /// `Ctrl`/`Shift` を見るため、押下時と違う修飾状態で戻すと別の操作になる（保留した
    /// `Ctrl+D` が、`Ctrl` を離した次のフレームでは `D` = 長さ寸法への切替になる等）。
    pub(crate) fn restore_pending(
        &mut self,
        events: &mut Vec<Event>,
        modifiers: Modifiers,
        replay: PendingReplay,
    ) {
        if self.pending.is_empty() {
            return;
        }
        let restored: Vec<Event> = self
            .pending
            .drain(..)
            .filter(|event| match event {
                Event::Key {
                    pressed: true,
                    modifiers: pressed_with,
                    ..
                } => replay != PendingReplay::Nothing && *pressed_with == modifiers,
                _ => replay == PendingReplay::All,
            })
            .collect();
        events.splice(0..0, restored);
    }
}

/// 保留したイベント（[`NumericInputState::restore_pending`]）を、そのフレームでどこまで戻すか。
///
/// egui のイベント列はフレーム内のすべてのウィジェットが共有する。戻したイベントのうち
/// 数値入力欄（[`route_keys`]）もショートカットも使わなかったものは列に残り、そのフレームで
/// フォーカスを持つ `TextEdit` へ入ってしまう（前のフレームの文字・貼り付けが挿入され、
/// `Cut` が作用する。Codex adversarial review [medium] 指摘、2026-09-25）。そこで数値入力欄が
/// 処理できるフレームでだけ全部を戻す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingReplay {
    /// 全部戻す（[`route_keys`] が到着順に処理する）。
    All,
    /// 押下の `Event::Key` だけ戻す（ツール切替・`Esc` 等のショートカットとして下流へ流す）。
    /// 文字・貼り付け・コピー・切り取り・IME・キーを離したイベントは捨てる。
    KeysOnly,
    /// 何も戻さない（全部捨てる）。
    Nothing,
}

impl PendingReplay {
    /// そのフレームで保留をどこまで戻すか。`routable` は「数値入力の受け手があり、モーダルも
    /// テキスト欄のフォーカスも無い」（[`route_keys`] を呼ぶフレーム）、`text_focused` は
    /// フレーム開始時に egui のフォーカスがあるか、`pointer_pressed` はそのフレームにマウス
    /// ボタンの押下があるか（[`frame_has_pointer_press`]）。
    ///
    /// - テキスト欄にフォーカスがある: [`PendingReplay::Nothing`]。キーだけ戻しても、下流の
    ///   ショートカットはフォーカス中は止まっている（`app_shortcuts_enabled`）ので効かず、
    ///   `Backspace`・`Delete`・矢印・`Enter` を処理するのはフォーカス中の `TextEdit` だけに
    ///   なる（前のフレームの打鍵で欄の文字が消える・確定される）。
    /// - マウスボタンの押下がある: [`PendingReplay::Nothing`]（`text_focused` と同格。Codex
    ///   adversarial review 5 回目 [medium] 指摘へのユーザー選択の対処、2026-09-25）。押下で
    ///   同じフレームのうちにフォーカスを得たウィジェットへ、戻したイベントが届く経路を塞ぐ。
    ///   egui 0.35 の `TextEdit` は、そのフレームで得たフォーカスではそのフレームのイベントを
    ///   処理しない（イベント処理が自分の押下判定より前にあるため。ヘッドレスで確認:
    ///   `egui_text_edit_ignores_events_in_the_frame_it_gains_focus`）ので、今の egui では
    ///   防御としての意味。egui の更新や、先にフォーカスを取るウィジェットに備える。
    /// - 数値入力欄が処理できる: [`PendingReplay::All`]。
    /// - それ以外（受け手が無い・モーダル表示中）: [`PendingReplay::KeysOnly`]。確定で配置が
    ///   終わった後の `M` などは、ショートカットとして届けたい。
    #[must_use]
    pub(crate) fn for_frame(routable: bool, text_focused: bool, pointer_pressed: bool) -> Self {
        if text_focused || pointer_pressed {
            PendingReplay::Nothing
        } else if routable {
            PendingReplay::All
        } else {
            PendingReplay::KeysOnly
        }
    }
}

/// そのフレームの（保留を戻す前の）イベント列 `events` にマウスボタンの押下
/// （`Event::PointerButton { pressed: true }`）があるか（[`PendingReplay::for_frame`]）。
///
/// 同じフレームでフォーカスが移る経路は他に `Tab`（egui がフレーム開始時に生の入力から読み、
/// そのフレームで最初にフォーカスを求めたウィジェットへ移す）と支援技術からのフォーカス要求が
/// あるが、どちらも同じ仕組み（ウィジェットの生成時の `interested_in_focus`）でフォーカスを
/// 渡し、egui 0.35 の `TextEdit` はそのフレームのイベントを処理しない（`Tab` はヘッドレスで
/// 確認）。矢印キーによる移動はフレームの終わりに決まる。そのため押下だけを見る（ユーザー選択の
/// 範囲）。保留から戻した `Tab` は生の入力ではないので、そもそもフォーカスを動かさない。
#[must_use]
pub(crate) fn frame_has_pointer_press(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::PointerButton { pressed: true, .. }))
}

/// [`route_keys`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyRouting {
    /// 入力欄としての確定は無い（開いた・文字を足した・閉じた・何もしなかった）。
    None,
    /// 空でない入力欄で `Enter` が押された。呼び出し側は [`NumericInputState::resolve`]
    /// で点にし、成功したら閉じてクリックとして渡す。失敗したら開いたまま理由を出す
    /// （打ち直せるように文字列を残す）。
    Submit,
}

/// 1 フレーム分のイベント列 `events` から、入力欄が使うキー・文字を取り除いて状態を進める。
///
/// 呼び出し側は、入力欄の受け手（点を受け付ける作図ツール、または移動・複製の配置）が
/// あり、モーダルもテキスト欄のフォーカスも無いときだけ呼ぶ（その前に
/// [`NumericInputState::restore_pending`] で前フレームの保留分を先頭へ戻しておく）。
/// **イベントは届いた順に 1 つずつ扱い**、入力欄の開閉はフレームの途中でも切り替わる。
///
/// - **閉じている区間**: 先頭の文字（半角へ寄せたもの）が [`is_trigger_char`] の最初の
///   `Event::Text` で開く。**開くのはその文字を生んだキー押下から**で、それより前のイベントは
///   開く前の入力として手を付けずに残す（同じフレームに `L` と `1` が届いたら、`L` の押下と
///   文字 `l` は下流のツール切替へ届き、欄は `1` から始まる）。開始キーは
///   [`opening_key_index`]（`@` の `Shift+2` のようにキーと文字が一致しないので、直前の押下
///   キーで対応付ける）。きっかけが無ければ何も取り除かない。
/// - **開いている区間**:
///   - `Event::Text` / `Event::Paste`: 書式外の文字も空白も捨てずに文字列へ足す（半角へ寄せ、
///     制御文字は空白にする。`NumericInputState::push_text` 参照）。
///   - `Tab` とファンクションキー（`F3` スナップ・`F8` 直交等）の押下: 残す（入力中も切り替え
///     られる）。
///   - **`Ctrl`/`Cmd` 併用の押下: 取り除く**（`Ctrl+Z`・`Ctrl+N`・`Ctrl+O` 等を入力中に効かせ
///     ない。開いたのと同じフレームで開始キーより後に届いたものも止まる。開始キーより前の
///     ものは開く前の入力として残る）。
///   - `Backspace`: 1 文字消す。
///   - `Enter`: 空なら**閉じてイベントを残し**（下流が従来どおり `Confirm` として扱う）、残りを
///     閉じている区間として続ける。空でなければ取り除いて [`KeyRouting::Submit`] とし、
///     **そこで打ち切る**: 確定する文字列はその時点のもの。同じフレームでそれより後に届いた
///     キーボードのイベント（キー・文字・貼り付け等）は捨てずに保留し、次のフレームの先頭へ
///     戻す（確定の後に打たれた入力として、閉じた欄・再び開く欄・ツールへ到着順に届く）。
///     ポインタ等のキーボード以外のイベントはこのフレームに残す（egui がフレームの開始時に
///     状態へ反映済みのため）。
///   - `Esc`: 閉じて取り除く（1 回目）。残りは閉じている区間として続ける（2 回目の `Esc` は
///     下流へ届き従来のキャンセル、続く数字は再び欄を開く）。
///   - それ以外の押下（文字キー・`Delete`・`Home`・矢印等）: 取り除く（ツール切替・削除等の
///     ショートカットを止める）。離したキー・ポインタ等は残す。
pub(crate) fn route_keys(state: &mut NumericInputState, events: &mut Vec<Event>) -> KeyRouting {
    let mut slots: Vec<Option<Event>> = std::mem::take(events).into_iter().map(Some).collect();
    let mut open_at = if state.is_open() {
        Some(0)
    } else {
        opening_index(&slots, 0)
    };
    let mut routing = KeyRouting::None;
    let mut kept = Vec::with_capacity(slots.len());
    for index in 0..slots.len() {
        let Some(event) = slots[index].take() else {
            continue;
        };
        if routing == KeyRouting::Submit {
            if is_keyboard_event(&event) {
                state.pending.push(event);
            } else {
                kept.push(event);
            }
            continue;
        }
        if !state.is_open() {
            if open_at == Some(index) {
                state.text = Some(String::new());
            } else {
                kept.push(event);
                continue;
            }
        }
        match &event {
            Event::Text(typed) | Event::Paste(typed) => state.push_text(typed),
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => match key {
                Key::Tab => kept.push(event),
                key if is_function_key(*key) => kept.push(event),
                _ if modifiers.command => {}
                Key::Backspace => state.backspace(),
                Key::Enter => {
                    if state.text().is_some_and(str::is_empty) {
                        state.close();
                        kept.push(event);
                        open_at = opening_index(&slots, index + 1);
                    } else {
                        routing = KeyRouting::Submit;
                    }
                }
                Key::Escape => {
                    state.close();
                    open_at = opening_index(&slots, index + 1);
                }
                _ => {}
            },
            _ => kept.push(event),
        }
    }
    *events = kept;
    routing
}

/// キーボード由来のイベントか（空でない `Enter` の後に保留する対象。[`route_keys`]）。
fn is_keyboard_event(event: &Event) -> bool {
    matches!(
        event,
        Event::Key { .. }
            | Event::Text(_)
            | Event::Paste(_)
            | Event::Copy
            | Event::Cut
            | Event::Ime(_)
    )
}

/// まだ処理していない `events[from..]` のうち、入力欄を開く位置（開始キー、キーが付いて
/// いなければ開始の文字イベント）。開くきっかけが無ければ `None`。
fn opening_index(events: &[Option<Event>], from: usize) -> Option<usize> {
    let trigger = (from..events.len()).find(|&index| {
        matches!(&events[index], Some(Event::Text(text))
            if text.chars().next().map(to_half_width).is_some_and(is_trigger_char))
    })?;
    Some(opening_key_index(&events[from..trigger]).map_or(trigger, |offset| from + offset))
}

/// 入力欄を開く文字イベントより前のイベント `before` のうち、その文字を生んだキー押下の位置。
///
/// egui-winit はキー押下 → 文字の順に送るので、後ろから最初に見つかる押下キーが開始キー。
/// ただし押下キーより先に別の文字イベント（`Text` / `Paste`）に当たったら、その押下キーは
/// 前の文字のものなので `None`（開始の文字にキーが付いていない。IME 等）とし、呼び出し側は
/// 文字イベント自体から開く。キーと文字の対応は名前では取らない（`@` は `Shift+2` 等、
/// 配列によってキーが違う）。
fn opening_key_index(before: &[Option<Event>]) -> Option<usize> {
    for (index, event) in before.iter().enumerate().rev() {
        match event {
            Some(Event::Key { pressed: true, .. }) => return Some(index),
            Some(Event::Text(_) | Event::Paste(_)) => return None,
            _ => {}
        }
    }
    None
}

/// ファンクションキー（`F1`〜`F35`）か。egui の [`Key`] には判定メソッドが無いので名前で見る。
fn is_function_key(key: Key) -> bool {
    let name = key.name();
    name.len() >= 2 && name.starts_with('F') && name[1..].bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn assert_close(actual: Point2, expected: Point2) {
        assert!(
            (actual.x - expected.x).abs() < 1e-9 && (actual.y - expected.y).abs() < 1e-9,
            "{actual:?} != {expected:?}"
        );
    }

    const BASE: Option<Point2> = Some(Point2::new(10.0, 20.0));

    // --- 書式ごとの解釈 ---

    #[test]
    fn absolute_ignores_base_and_direction() {
        assert_eq!(resolve_point("30,40", None, None), Ok(p(30.0, 40.0)));
        assert_eq!(
            resolve_point("30,40", BASE, Some(Vec2::new(1.0, 0.0))),
            Ok(p(30.0, 40.0))
        );
    }

    #[test]
    fn relative_adds_to_base() {
        assert_eq!(resolve_point("@5,-3", BASE, None), Ok(p(15.0, 17.0)));
    }

    #[test]
    fn polar_is_measured_ccw_from_plus_x() {
        assert_close(
            resolve_point("100<30", BASE, None).unwrap(),
            p(10.0 + 100.0 * 30f64.to_radians().cos(), 20.0 + 50.0),
        );
        // `@L<A` は `L<A` と同じ（どちらも基点から）。
        assert_eq!(
            resolve_point("@100<30", BASE, None),
            resolve_point("100<30", BASE, None)
        );
    }

    #[test]
    fn distance_goes_toward_the_direction_regardless_of_its_length() {
        assert_eq!(
            resolve_point("50", BASE, Some(Vec2::new(0.0, 7.0))),
            Ok(p(10.0, 70.0))
        );
        assert_close(
            resolve_point("5", BASE, Some(Vec2::new(3.0, 4.0))).unwrap(),
            p(13.0, 24.0),
        );
    }

    // --- 基点・方向が無い場合 ---

    #[test]
    fn relative_forms_without_base_are_rejected() {
        for text in ["@5,5", "100<30", "@100<30", "50"] {
            assert_eq!(
                resolve_point(text, None, Some(Vec2::new(1.0, 0.0))),
                Err(NumericInputError::NoBasePoint),
                "{text}"
            );
        }
    }

    #[test]
    fn format_errors_win_over_missing_base() {
        assert_eq!(
            resolve_point("@abc", None, None),
            Err(NumericInputError::InvalidFormat)
        );
    }

    #[test]
    fn distance_without_usable_direction_is_rejected() {
        for direction in [
            None,
            Some(Vec2::ZERO),
            Some(Vec2::new(f64::NAN, 1.0)),
            Some(Vec2::new(f64::INFINITY, 0.0)),
        ] {
            assert_eq!(
                resolve_point("50", BASE, direction),
                Err(NumericInputError::NoDirection),
                "{direction:?}"
            );
        }
        // 方向が要らない書式は方向が無くても通る。
        assert!(resolve_point("@1,1", BASE, None).is_ok());
    }

    // --- 数値の形 ---

    #[test]
    fn decimals_and_signs() {
        assert_eq!(resolve_point("1.5,-.25", None, None), Ok(p(1.5, -0.25)));
        assert_eq!(resolve_point("+2.,3", None, None), Ok(p(2.0, 3.0)));
        assert_eq!(resolve_point("@-0.5,+0.5", BASE, None), Ok(p(9.5, 20.5)));
    }

    #[test]
    fn negative_length_points_the_other_way() {
        assert_eq!(
            resolve_point("-10<0", BASE, None),
            resolve_point("10<180", BASE, None)
        );
        assert_eq!(
            resolve_point("-5", BASE, Some(Vec2::new(1.0, 0.0))),
            Ok(p(5.0, 20.0))
        );
    }

    #[test]
    fn only_leading_and_trailing_whitespace_is_ignored() {
        assert_eq!(resolve_point("  30,40  ", None, None), Ok(p(30.0, 40.0)));
        assert_eq!(resolve_point("\t@5,5\n", BASE, None), Ok(p(15.0, 25.0)));
        assert_eq!(resolve_point(" 100<90 ", BASE, None), Ok(p(10.0, 120.0)));
        assert_eq!(
            resolve_point(" 5 ", BASE, Some(Vec2::new(1.0, 0.0))),
            Ok(p(15.0, 20.0))
        );
        // 中の空白は位置に関わらず誤り（数値の中・区切りの周り・`@` の直後）。
        for text in [
            "3 0,40", "- 3,40", "30 ,40", "30, 40", "@ 5,5", "@5 ,5", "100 <90", "100< 90",
            "@1\n,2", "1\t2",
        ] {
            assert_eq!(
                resolve_point(text, BASE, Some(Vec2::new(1.0, 0.0))),
                Err(NumericInputError::InvalidFormat),
                "{text:?}"
            );
        }
    }

    #[test]
    fn malformed_inputs_are_rejected() {
        for text in [
            "", "   ", "@", " @ ", "@5", ",", "1,", ",1", "1,2,3", "<30", "100<", "1<2<3", "1,2<3",
            "@1,2<3", ".", "-", "+", "1.2.3", "--1", "1-", "1e3", "abc", "@@1,2", "1,,2",
        ] {
            assert_eq!(
                resolve_point(text, BASE, Some(Vec2::new(1.0, 0.0))),
                Err(NumericInputError::InvalidFormat),
                "{text:?}"
            );
        }
    }

    #[test]
    fn non_finite_values_are_rejected() {
        // Rust の `f64::from_str` が受ける綴りは書式の段階で落とす。
        for text in ["inf,0", "NaN,0", "infinity", "0<nan"] {
            assert_eq!(
                resolve_point(text, BASE, Some(Vec2::new(1.0, 0.0))),
                Err(NumericInputError::InvalidFormat),
                "{text:?}"
            );
        }
        // 桁あふれで無限大になる数値。
        let huge = "9".repeat(400);
        assert_eq!(
            resolve_point(&format!("{huge},0"), None, None),
            Err(NumericInputError::NonFinite)
        );
        // 数値は有限だが足した結果があふれる。
        let near_max = format!("{:.0}", f64::MAX);
        assert_eq!(
            resolve_point(&format!("@{near_max},0"), Some(p(f64::MAX, 0.0)), None),
            Err(NumericInputError::NonFinite)
        );
    }

    // --- 角度の正規化 ---

    #[test]
    fn right_angles_are_exact() {
        assert_eq!(resolve_point("100<90", BASE, None), Ok(p(10.0, 120.0)));
        assert_eq!(resolve_point("100<180", BASE, None), Ok(p(-90.0, 20.0)));
        assert_eq!(resolve_point("100<270", BASE, None), Ok(p(10.0, -80.0)));
        assert_eq!(resolve_point("100<360", BASE, None), Ok(p(110.0, 20.0)));
        assert_eq!(resolve_point("100<-90", BASE, None), Ok(p(10.0, -80.0)));
        assert_eq!(resolve_point("100<450", BASE, None), Ok(p(10.0, 120.0)));
        assert_eq!(resolve_point("100<-720", BASE, None), Ok(p(110.0, 20.0)));
    }

    #[test]
    fn angles_are_normalized_into_one_turn() {
        for (a, b) in [(30.0, 390.0), (30.0, -330.0), (-45.0, 315.0), (725.5, 5.5)] {
            let (ua, ub) = (unit_from_degrees(a), unit_from_degrees(b));
            assert!(
                (ua.x - ub.x).abs() < 1e-12 && (ua.y - ub.y).abs() < 1e-12,
                "{a} vs {b}"
            );
        }
        // 0 に極めて近い負の角度が 360.0 に丸められても +X を返す。
        assert_eq!(unit_from_degrees(-1e-20), Vec2::new(1.0, 0.0));
        assert_eq!(unit_from_degrees(-0.0), Vec2::new(1.0, 0.0));
    }

    // --- 文字の正規化 ---

    #[test]
    fn full_width_characters_map_to_half_width_and_nothing_is_dropped() {
        assert_eq!(to_half_width('１'), '1');
        assert_eq!(to_half_width('＠'), '@');
        assert_eq!(to_half_width('，'), ',');
        assert_eq!(to_half_width('＜'), '<');
        assert_eq!(to_half_width('．'), '.');
        assert_eq!(to_half_width('－'), '-');
        assert_eq!(to_half_width('\u{2212}'), '-');
        assert_eq!(to_half_width('Ａ'), 'A');
        // 書式外の文字もそのまま（判定は parse に任せる）。
        assert_eq!(to_half_width(' '), ' ');
        assert_eq!(to_half_width('a'), 'a');
        assert_eq!(to_half_width('あ'), 'あ');
    }

    #[test]
    fn trigger_characters_match_the_design() {
        for c in "0123456789.-@".chars() {
            assert!(is_trigger_char(c), "{c}");
        }
        for c in "+,<aL ".chars() {
            assert!(!is_trigger_char(c), "{c}");
        }
    }

    // --- キーの振り分け ---

    fn key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn text(s: &str) -> Event {
        Event::Text(s.to_owned())
    }

    /// `1` を打ったときに egui-winit が送る 2 つのイベント（キー押下と文字）。
    fn typed(c: char, k: Key) -> [Event; 2] {
        [key(k), text(&c.to_string())]
    }

    #[test]
    fn closed_box_leaves_non_trigger_input_untouched() {
        let mut state = NumericInputState::default();
        let mut events = vec![key(Key::L), text("l"), key(Key::Enter), key(Key::Escape)];
        let before = events.clone();
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::None);
        assert!(!state.is_open());
        assert_eq!(events, before);
    }

    #[test]
    fn digit_opens_the_box_and_swallows_the_tool_switch_key() {
        let mut state = NumericInputState::default();
        let mut events = typed('1', Key::Num1).to_vec();
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::None);
        assert_eq!(state.text(), Some("1"));
        assert!(events.is_empty(), "`1` must not reach the tool switch");
    }

    #[test]
    fn at_minus_and_dot_open_the_box() {
        for (c, k) in [('@', Key::Num2), ('-', Key::Minus), ('.', Key::Period)] {
            let mut state = NumericInputState::default();
            let mut events = typed(c, k).to_vec();
            route_keys(&mut state, &mut events);
            assert_eq!(state.text(), Some(c.to_string().as_str()));
            assert!(events.is_empty());
        }
    }

    #[test]
    fn open_box_swallows_letters_delete_and_editing_keys() {
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('1', Key::Num1).to_vec());
        let mut events = vec![
            key(Key::L),
            key(Key::Delete),
            key(Key::Home),
            key(Key::ArrowLeft),
            key(Key::Num0),
            text("0"),
        ];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::None);
        assert_eq!(state.text(), Some("10"));
        assert!(events.is_empty(), "no key press may reach the shortcuts");
        // 文字キーの文字も（書式外でも）入力欄に入り、ツール切替へは届かない。
        let mut events = vec![key(Key::L), text("l")];
        route_keys(&mut state, &mut events);
        assert_eq!(state.text(), Some("10l"));
        assert!(events.is_empty());
    }

    #[test]
    fn backspace_edits_and_does_not_leak() {
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('1', Key::Num1).to_vec());
        route_keys(&mut state, &mut typed('2', Key::Num2).to_vec());
        let mut events = vec![key(Key::Backspace)];
        route_keys(&mut state, &mut events);
        assert_eq!(state.text(), Some("1"));
        assert!(
            events.is_empty(),
            "Backspace must not reach the delete shortcut"
        );
        // 空にしても開いたまま（Enter / Esc まで）。
        route_keys(
            &mut state,
            &mut vec![key(Key::Backspace), key(Key::Backspace)],
        );
        assert_eq!(state.text(), Some(""));
    }

    #[test]
    fn enter_on_text_submits_and_is_consumed() {
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        let mut events = vec![key(Key::Enter)];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::Submit);
        assert!(events.is_empty());
        // 確定の成否は呼び出し側が決めるので、開いたまま文字列も残る。
        assert_eq!(state.text(), Some("5"));
    }

    #[test]
    fn enter_on_empty_box_closes_and_passes_through_as_confirm() {
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        route_keys(&mut state, &mut vec![key(Key::Backspace)]);
        let mut events = vec![key(Key::Enter)];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::None);
        assert!(!state.is_open());
        assert_eq!(events, vec![key(Key::Enter)]);
    }

    #[test]
    fn escape_closes_first_then_passes_through() {
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        let mut first = vec![key(Key::Escape)];
        route_keys(&mut state, &mut first);
        assert!(!state.is_open());
        assert!(first.is_empty(), "1st Esc only closes the box");
        let mut second = vec![key(Key::Escape)];
        route_keys(&mut state, &mut second);
        assert_eq!(second, vec![key(Key::Escape)], "2nd Esc reaches the tool");
    }

    #[test]
    fn events_after_closing_in_the_same_frame_are_left_alone() {
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        let mut events = vec![key(Key::Escape), key(Key::L), text("l")];
        route_keys(&mut state, &mut events);
        assert_eq!(events, vec![key(Key::L), text("l")]);
    }

    #[test]
    fn function_keys_tab_releases_and_pointer_are_kept() {
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        let release = Event::Key {
            key: Key::L,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut events = vec![
            key(Key::F3),
            key(Key::F8),
            key(Key::F10),
            key(Key::Tab),
            release.clone(),
            Event::PointerMoved(Pos2::new(1.0, 2.0)),
        ];
        let before = events.clone();
        route_keys(&mut state, &mut events);
        assert_eq!(events, before);
        assert_eq!(state.text(), Some("5"));
    }

    fn with_command(k: Key) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }
    }

    /// 開いている区間の `Ctrl`/`Cmd` 併用の押下は取り除く。開いたのと同じフレームでも、
    /// 開始キーより後なら止まり、前なら開く前の入力として残る（Codex adversarial review
    /// [high]、2026-09-25）。
    #[test]
    fn command_shortcuts_after_the_opening_key_are_swallowed() {
        let mut state = NumericInputState::default();
        let mut events = vec![
            with_command(Key::Z),
            key(Key::Num1),
            text("1"),
            with_command(Key::Z),
            with_command(Key::N),
            with_command(Key::O),
        ];
        route_keys(&mut state, &mut events);
        assert_eq!(
            events,
            vec![with_command(Key::Z)],
            "only the one before opening"
        );
        assert_eq!(state.text(), Some("1"));
        // 開いている間の `Ctrl+Enter`・`Ctrl+Backspace` も何もしない（確定・消去にしない）。
        let mut events = vec![with_command(Key::Enter), with_command(Key::Backspace)];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::None);
        assert!(events.is_empty());
        assert_eq!(state.text(), Some("1"));
    }

    /// 空でない `Enter` で打ち切り、その時点の文字列で確定する。後に届いたキーボードの
    /// イベントは保留して次のフレームの先頭へ戻す（Codex adversarial review [medium]、
    /// 2026-09-25）。
    #[test]
    fn enter_terminates_the_frame_and_later_keys_are_replayed_next_frame() {
        let mut state = NumericInputState::default();
        let mut events = vec![
            key(Key::Num1),
            text("1"),
            key(Key::Enter),
            key(Key::Num2),
            text("2"),
            Event::PointerMoved(Pos2::new(1.0, 2.0)),
        ];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::Submit);
        assert_eq!(state.text(), Some("1"), "submitted as typed up to Enter");
        // キーボード以外はこのフレームに残る。
        assert_eq!(events, vec![Event::PointerMoved(Pos2::new(1.0, 2.0))]);

        // 呼び出し側が確定に成功して閉じた後、次のフレームで `2` が新しい入力欄を開く。
        state.close();
        let mut next = Vec::new();
        state.restore_pending(&mut next, egui::Modifiers::NONE, PendingReplay::All);
        assert_eq!(next, vec![key(Key::Num2), text("2")]);
        assert_eq!(route_keys(&mut state, &mut next), KeyRouting::None);
        assert_eq!(state.text(), Some("2"));
        assert!(next.is_empty());
        // 保留は 1 回戻したら空になる。
        let mut again = Vec::new();
        state.restore_pending(&mut again, egui::Modifiers::NONE, PendingReplay::All);
        assert!(again.is_empty());
    }

    #[test]
    fn replayed_escape_reaches_the_tool_once_the_box_is_closed() {
        let mut state = NumericInputState::default();
        let mut events = vec![key(Key::Num1), text("1"), key(Key::Enter), key(Key::Escape)];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::Submit);
        assert!(events.is_empty());
        state.close(); // 確定に成功
        let mut next = vec![Event::PointerMoved(Pos2::new(3.0, 4.0))];
        state.restore_pending(&mut next, egui::Modifiers::NONE, PendingReplay::All);
        // 保留分は今フレームのイベントより前（到着順）。
        assert_eq!(
            next,
            vec![key(Key::Escape), Event::PointerMoved(Pos2::new(3.0, 4.0))]
        );
        route_keys(&mut state, &mut next);
        assert_eq!(
            next,
            vec![key(Key::Escape), Event::PointerMoved(Pos2::new(3.0, 4.0))],
            "the closed box leaves Esc to the tool"
        );
    }

    #[test]
    fn restore_pending_returns_only_what_the_frame_can_use() {
        let release = Event::Key {
            key: Key::L,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let later = vec![
            key(Key::L),
            text("l"),
            Event::Paste("x".to_owned()),
            Event::Copy,
            Event::Cut,
            release.clone(),
        ];
        let submitted = || {
            let mut state = NumericInputState::default();
            let mut events = vec![key(Key::Num1), text("1"), key(Key::Enter)];
            events.extend(later.clone());
            assert_eq!(route_keys(&mut state, &mut events), KeyRouting::Submit);
            state.close();
            state
        };

        let mut state = submitted();
        let mut all = Vec::new();
        state.restore_pending(&mut all, egui::Modifiers::NONE, PendingReplay::All);
        assert_eq!(all, later);

        // 受け手が無いフレーム: 押下のキーだけ（文字・貼り付け・コピー・切り取り・離したキーは捨てる）。
        let mut state = submitted();
        let mut keys = vec![Event::PointerMoved(Pos2::new(1.0, 2.0))];
        state.restore_pending(&mut keys, egui::Modifiers::NONE, PendingReplay::KeysOnly);
        assert_eq!(
            keys,
            vec![key(Key::L), Event::PointerMoved(Pos2::new(1.0, 2.0))]
        );

        // テキスト欄にフォーカスがあるフレーム: 何も戻さない。
        let mut state = submitted();
        let mut nothing = Vec::new();
        state.restore_pending(&mut nothing, egui::Modifiers::NONE, PendingReplay::Nothing);
        assert!(nothing.is_empty());

        // どの範囲でも、戻さなかった分は保留に残らない（次のフレームで出てこない）。
        let mut again = Vec::new();
        state.restore_pending(&mut again, egui::Modifiers::NONE, PendingReplay::All);
        assert!(again.is_empty());
    }

    #[test]
    fn pending_replay_depends_on_the_frame() {
        // (routable, text_focused, pointer_pressed)
        for (routable, focused, pressed, expected) in [
            (true, false, false, PendingReplay::All),
            (false, false, false, PendingReplay::KeysOnly),
            (false, true, false, PendingReplay::Nothing),
            (true, true, false, PendingReplay::Nothing),
            // マウスボタンの押下があるフレームは、受け手の有無に関わらず何も戻さない。
            (true, false, true, PendingReplay::Nothing),
            (false, false, true, PendingReplay::Nothing),
            (false, true, true, PendingReplay::Nothing),
        ] {
            assert_eq!(
                PendingReplay::for_frame(routable, focused, pressed),
                expected,
                "{routable} {focused} {pressed}"
            );
        }
    }

    fn pointer_button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn frame_has_pointer_press_sees_only_presses() {
        let at = Pos2::new(5.0, 5.0);
        assert!(frame_has_pointer_press(&[pointer_button(at, true)]));
        assert!(frame_has_pointer_press(&[
            Event::PointerMoved(at),
            key(Key::L),
            pointer_button(at, true),
        ]));
        assert!(!frame_has_pointer_press(&[]));
        assert!(!frame_has_pointer_press(&[
            pointer_button(at, false),
            Event::PointerMoved(at),
            key(Key::Tab),
            key(Key::Enter),
            text("1"),
        ]));
    }

    const FIELD: &str = "numeric_input_test_field";

    /// 1 フレーム回す: `McadApp::route_numeric_input` と同じ順で保留を戻し（判定は `replay`
    /// が `None` なら [`PendingReplay::for_frame`] で「受け手なし・フォーカスなし」として決め、
    /// `Some` ならそれを強制する）、`TextEdit` を 1 つ描く。戻した後のイベント列を返す。
    fn run_field_frame(
        ctx: &egui::Context,
        field: &mut String,
        state: &mut NumericInputState,
        events: Vec<Event>,
        replay: Option<PendingReplay>,
    ) -> Vec<Event> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(400.0, 200.0),
            )),
            events,
            ..Default::default()
        };
        let mut restored = Vec::new();
        let _ = ctx.run_ui(input, |ui| {
            ui.input_mut(|i| {
                let replay = replay.unwrap_or_else(|| {
                    PendingReplay::for_frame(false, false, frame_has_pointer_press(&i.events))
                });
                let modifiers = i.modifiers;
                state.restore_pending(&mut i.events, modifiers, replay);
                restored = i.events.clone();
            });
            ui.add(egui::TextEdit::singleline(field).id(egui::Id::new(FIELD)));
        });
        restored
    }

    /// 空でない `Enter` の後に `Backspace` 2 回と `z` を保留した状態。
    fn state_with_pending_edits() -> NumericInputState {
        let mut state = NumericInputState::default();
        let mut events = vec![
            key(Key::Num1),
            text("1"),
            key(Key::Enter),
            key(Key::Backspace),
            key(Key::Backspace),
            text("z"),
        ];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::Submit);
        state.close();
        state
    }

    /// 保留を戻すフレームで `TextEdit` を押して離しても（押下のフレームでフォーカスを得る）、
    /// 欄の内容は変わらず、保留は戻らずに捨てられる（Codex 5 回目 [medium]、2026-09-25）。
    #[test]
    fn a_text_edit_clicked_in_the_replay_frame_does_not_receive_pending_input() {
        let ctx = egui::Context::default();
        let mut field = "abc".to_owned();
        let mut idle = NumericInputState::default();
        run_field_frame(&ctx, &mut field, &mut idle, Vec::new(), None);
        let rect = ctx
            .read_response(egui::Id::new(FIELD))
            .expect("the field was drawn")
            .rect;
        let pos = rect.right_center() - egui::vec2(4.0, 0.0);

        let mut state = state_with_pending_edits();
        let events = run_field_frame(
            &ctx,
            &mut field,
            &mut state,
            vec![Event::PointerMoved(pos), pointer_button(pos, true)],
            None,
        );
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(FIELD))));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Key { .. } | Event::Text(_))),
            "nothing is replayed in a frame with a press: {events:?}"
        );
        run_field_frame(
            &ctx,
            &mut field,
            &mut state,
            vec![pointer_button(pos, false)],
            None,
        );
        assert_eq!(field, "abc");
        // 捨てた保留は後のフレームにも出てこない。
        let later = run_field_frame(&ctx, &mut field, &mut state, Vec::new(), None);
        assert!(later.is_empty(), "{later:?}");
        assert_eq!(field, "abc");
    }

    /// egui 0.35 の前提の固定: `TextEdit` は、そのフレームで（押下・`Tab` で）得たフォーカスでは
    /// そのフレームのイベントを処理しない（保留を全部戻しても欄は変わらない）。次のフレームからは
    /// 処理する（ハーネスの確認）。egui を更新してこれが変わったら、押下以外の経路
    /// （`Tab`・支援技術の要求）も [`PendingReplay::for_frame`] で見る必要がある。
    #[test]
    fn egui_text_edit_ignores_events_in_the_frame_it_gains_focus() {
        for focusing in ["press", "tab"] {
            let ctx = egui::Context::default();
            let mut field = "abc".to_owned();
            let mut idle = NumericInputState::default();
            run_field_frame(&ctx, &mut field, &mut idle, Vec::new(), None);
            let rect = ctx
                .read_response(egui::Id::new(FIELD))
                .expect("the field was drawn")
                .rect;
            let pos = rect.right_center() - egui::vec2(4.0, 0.0);
            let events = match focusing {
                "press" => vec![Event::PointerMoved(pos), pointer_button(pos, true)],
                _ => vec![key(Key::Tab)],
            };
            let mut state = state_with_pending_edits();
            let restored = run_field_frame(
                &ctx,
                &mut field,
                &mut state,
                events,
                Some(PendingReplay::All),
            );
            assert!(restored.contains(&text("z")), "{focusing}: replayed");
            assert!(
                ctx.memory(|m| m.has_focus(egui::Id::new(FIELD))),
                "{focusing}: focused in the same frame"
            );
            assert_eq!(field, "abc", "{focusing}: same-frame events are not taken");
            // 次のフレームからはフォーカス中の欄がイベントを処理する。
            run_field_frame(
                &ctx,
                &mut field,
                &mut idle,
                vec![text("q"), key(Key::Backspace), key(Key::Backspace)],
                None,
            );
            assert_eq!(field, "ab", "{focusing}: harness sanity");
        }
    }

    #[test]
    fn replayed_key_presses_need_the_same_modifiers() {
        let mut state = NumericInputState::default();
        let mut events = vec![
            key(Key::Num1),
            text("1"),
            key(Key::Enter),
            with_command(Key::D),
            key(Key::L),
            text("l"),
        ];
        route_keys(&mut state, &mut events);
        state.close();
        // `Ctrl` を押したままなら `Ctrl+D` は戻し、修飾の違う `L` の押下は捨てる（`Ctrl+L` として
        // 読まれないように）。文字イベントは修飾を持たないので戻す。
        let mut held = Vec::new();
        state.restore_pending(&mut held, egui::Modifiers::COMMAND, PendingReplay::All);
        assert_eq!(held, vec![with_command(Key::D), text("l")]);

        let mut state = NumericInputState::default();
        let mut events = vec![
            key(Key::Num1),
            text("1"),
            key(Key::Enter),
            with_command(Key::D),
        ];
        route_keys(&mut state, &mut events);
        state.close();
        // `Ctrl` を離していれば `Ctrl+D` は戻さない（`D` = 長さ寸法への切替として読まれない）。
        let mut released = Vec::new();
        state.restore_pending(&mut released, egui::Modifiers::NONE, PendingReplay::All);
        assert!(released.is_empty());
    }

    #[test]
    fn closing_mid_frame_hands_the_rest_to_the_closed_state() {
        // `Esc` で閉じた後の数字は同じフレームで再び欄を開く。
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        let mut events = vec![key(Key::Escape), key(Key::Num1), text("1")];
        route_keys(&mut state, &mut events);
        assert!(events.is_empty());
        assert_eq!(state.text(), Some("1"));

        // 2 回目の `Esc` は下流へ届く。
        let mut events = vec![key(Key::Escape), key(Key::Escape)];
        route_keys(&mut state, &mut events);
        assert!(!state.is_open());
        assert_eq!(events, vec![key(Key::Escape)]);

        // 空の `Enter` は下流へ残し、続く数字は再び欄を開く。
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        route_keys(&mut state, &mut vec![key(Key::Backspace)]);
        let mut events = vec![key(Key::Enter), key(Key::Num2), text("2")];
        assert_eq!(route_keys(&mut state, &mut events), KeyRouting::None);
        assert_eq!(events, vec![key(Key::Enter)]);
        assert_eq!(state.text(), Some("2"));
    }

    #[test]
    fn typed_text_is_mapped_to_half_width() {
        let mut state = NumericInputState::default();
        let mut events = vec![text("＠"), text("1０"), text("，"), text("５")];
        route_keys(&mut state, &mut events);
        assert_eq!(state.text(), Some("@10,5"));
    }

    /// 制御文字を捨てると、貼り付けた `1\n2` が `12` として確定してしまう（Codex adversarial
    /// review [high] 指摘、2026-09-25）。空白に置き換えて、数値の中なら拒否・末尾なら通す。
    #[test]
    fn pasted_control_characters_become_spaces() {
        let paste = |pasted: &str| {
            let mut state = NumericInputState::default();
            state.set_cursor(p(20.0, 20.0));
            // `5` で開いて消し、空の欄へ貼り付ける。
            route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
            route_keys(&mut state, &mut vec![key(Key::Backspace)]);
            route_keys(&mut state, &mut vec![Event::Paste(pasted.to_owned())]);
            state
        };
        for (pasted, shown) in [("1\n2", "1 2"), ("1\t2", "1 2"), ("@1\n,2", "@1 ,2")] {
            let state = paste(pasted);
            assert_eq!(state.text(), Some(shown), "{pasted:?}");
            assert_eq!(
                state.resolve(BASE),
                Err(NumericInputError::InvalidFormat),
                "{pasted:?}"
            );
        }
        // 末尾の改行は parse の trim で通る。
        let state = paste("100\n");
        assert_eq!(state.text(), Some("100 "));
        assert_eq!(state.resolve(BASE), Ok(p(110.0, 20.0)));
        // `\r\n` は 1 文字ずつ空白になる（2 つ）。
        let state = paste("100\r\n");
        assert_eq!(state.text(), Some("100  "));
        assert_eq!(state.resolve(BASE), Ok(p(110.0, 20.0)));
    }

    /// 書式外の文字を黙って捨てると、打った値と違う点で確定してしまう（`1e3` → `13` 等。
    /// Codex adversarial review [high] 指摘、2026-09-25）。入力どおりに残して確定を拒否し、
    /// `Backspace` で直せることを固定する。
    #[test]
    fn out_of_format_characters_are_kept_so_the_value_is_rejected_not_altered() {
        let cursor_right = |state: &mut NumericInputState| state.set_cursor(p(20.0, 20.0));

        // `1e3`: 指数表記は受けない。`13` として確定しない。
        let mut state = NumericInputState::default();
        cursor_right(&mut state);
        for (c, k) in [('1', Key::Num1), ('e', Key::E), ('3', Key::Num3)] {
            let mut events = typed(c, k).to_vec();
            route_keys(&mut state, &mut events);
            assert!(events.is_empty(), "`{c}` must not reach the shortcuts");
        }
        assert_eq!(state.text(), Some("1e3"));
        assert_eq!(state.resolve(BASE), Err(NumericInputError::InvalidFormat));
        assert_eq!(state.refresh_preview(BASE), None);
        route_keys(
            &mut state,
            &mut vec![key(Key::Backspace), key(Key::Backspace)],
        );
        assert_eq!(state.text(), Some("1"));
        assert_eq!(state.resolve(BASE), Ok(p(11.0, 20.0)));

        // `1 2`: 入力中の Space（パン）で混ざった空白は残り、`12` として確定しない。
        let mut state = NumericInputState::default();
        cursor_right(&mut state);
        let mut events = typed('1', Key::Num1).to_vec();
        events.extend([key(Key::Space), text(" ")]);
        events.extend(typed('2', Key::Num2));
        route_keys(&mut state, &mut events);
        assert!(events.is_empty());
        assert_eq!(state.text(), Some("1 2"));
        assert_eq!(state.resolve(BASE), Err(NumericInputError::InvalidFormat));
        route_keys(
            &mut state,
            &mut vec![key(Key::Backspace), key(Key::Backspace)],
        );
        assert_eq!(state.text(), Some("1"));
        assert_eq!(state.resolve(BASE), Ok(p(11.0, 20.0)));
        // 前後の空白は parse の trim で許す。
        route_keys(&mut state, &mut vec![text(" ")]);
        assert_eq!(state.text(), Some("1 "));
        assert_eq!(state.resolve(BASE), Ok(p(11.0, 20.0)));

        // 貼り付けた `@1foo,2`: `@1,2` として確定しない。
        let mut state = NumericInputState::default();
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        route_keys(&mut state, &mut vec![key(Key::Backspace)]);
        route_keys(&mut state, &mut vec![Event::Paste("@1foo,2".to_owned())]);
        assert_eq!(state.text(), Some("@1foo,2"));
        assert_eq!(state.resolve(BASE), Err(NumericInputError::InvalidFormat));
        route_keys(&mut state, &mut vec![key(Key::Backspace); 5]);
        assert_eq!(state.text(), Some("@1"));
        route_keys(&mut state, &mut vec![text(","), text("2")]);
        assert_eq!(state.resolve(BASE), Ok(p(11.0, 22.0)));
    }

    /// 同じフレームで入力欄が開く前に届いたイベントは、開く前の入力として下流へ残す
    /// （`L` のツール切替が失われない。Codex adversarial review [medium] 指摘、2026-09-25）。
    #[test]
    fn opening_frame_leaves_events_before_the_opening_key_downstream() {
        let mut state = NumericInputState::default();
        let mut events = vec![key(Key::L), text("l"), key(Key::Num1), text("1")];
        route_keys(&mut state, &mut events);
        assert_eq!(events, vec![key(Key::L), text("l")]);
        assert_eq!(state.text(), Some("1"));

        // 開始の文字の押下キーは取り除く（点ツールへの切替にしない）。
        let mut state = NumericInputState::default();
        let mut events = vec![key(Key::Num1), text("1")];
        route_keys(&mut state, &mut events);
        assert!(events.is_empty());
        assert_eq!(state.text(), Some("1"));

        // `@` は `Shift+2`（キーと文字が一致しない）。直前の押下キーで対応付ける。
        let shift_2 = Event::Key {
            key: Key::Num2,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        let mut state = NumericInputState::default();
        let mut events = vec![key(Key::L), text("l"), shift_2, text("@")];
        route_keys(&mut state, &mut events);
        assert_eq!(events, vec![key(Key::L), text("l")]);
        assert_eq!(state.text(), Some("@"));

        // 開始の文字に押下キーが付いていない（IME 等）ときは、直前の押下キーが前の文字の
        // ものなので取らず、文字イベントから開く。離したキー・ポインタは読み飛ばす。
        let release = Event::Key {
            key: Key::L,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut state = NumericInputState::default();
        let mut events = vec![
            key(Key::L),
            text("l"),
            release.clone(),
            Event::PointerMoved(Pos2::new(1.0, 2.0)),
            text("1"),
        ];
        route_keys(&mut state, &mut events);
        assert_eq!(
            events,
            vec![
                key(Key::L),
                text("l"),
                release,
                Event::PointerMoved(Pos2::new(1.0, 2.0)),
            ]
        );
        assert_eq!(state.text(), Some("1"));

        // 開いた後のキーは従来どおり取り除く。
        let mut state = NumericInputState::default();
        let mut events = vec![key(Key::Num1), text("1"), key(Key::C), text("c")];
        route_keys(&mut state, &mut events);
        assert!(events.is_empty());
        assert_eq!(state.text(), Some("1c"));
    }

    #[test]
    fn text_starting_with_a_non_trigger_does_not_open() {
        let mut state = NumericInputState::default();
        let mut events = vec![text("a1"), text(","), text("<"), text("+")];
        route_keys(&mut state, &mut events);
        assert!(!state.is_open());
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn state_resolves_with_the_remembered_cursor_direction() {
        let mut state = NumericInputState::default();
        state.set_cursor(p(10.0, 30.0));
        route_keys(&mut state, &mut typed('5', Key::Num5).to_vec());
        assert_eq!(state.resolve(BASE), Ok(p(10.0, 25.0)));
        assert_eq!(state.refresh_preview(BASE), Some(p(10.0, 25.0)));
        assert_eq!(state.preview(), Some(p(10.0, 25.0)));
        // 解釈できない途中の文字列ではプレビューを出さない。
        route_keys(&mut state, &mut vec![text(",")]);
        assert_eq!(state.refresh_preview(BASE), None);
        // 閉じればプレビューも消える。
        state.close();
        assert_eq!(state.preview(), None);
        assert_eq!(state.refresh_preview(BASE), None);
    }
}
