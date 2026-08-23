//! 寸法の GPS 注記（寸法補助記号・サイズ公差）と、文書単位の寸法スタイル。
//!
//! DESIGN.md M9 設計判断2（公差データモデル）・判断4（文書単位の単一 [`DimStyle`]）と、
//! その実装時追記（Ⓔ 等の修飾子は non-goal / 桁数は `decimals_override` 方式）に対応する層。
//!
//! # 何をここに置くか
//!
//! [`DimAnnotation`] は寸法エンティティ（`DimLinear` / `DimRadial` / 新設予定の
//! `DimDiameter`）へ埋め込む「値そのもの以外の注記」をまとめた値型で、[`DimStyle`] は
//! 図面全体に効くスタイル設定（文字高さ・矢先長さ・桁数など）である。**どちらも
//! 「紙 mm」と「意味」だけを持ち、組版・描画は上位（app 層）の責務**という M8 からの
//! 分担をそのまま引き継ぐ。
//!
//! # 検証の位置づけ（M8 の `Scale` / `WidthMm` と同じ流儀）
//!
//! 不正な注記（種別に許されない記号・`upper < lower`・非有限値・不正なはめあい記号）は
//! **core コマンド境界・`.mcad` 読込・UI 入力の3境界すべてで拒否する**
//! （DESIGN.md M9 設計判断2）。そのための判定は次の 2 つに集約する。
//!
//! - [`FitClass`]: 生成時に検証する型（serde も `try_from` 経由なので、手編集された
//!   `.mcad` の不正値は読込境界で弾かれる）
//! - [`DimAnnotation::validate`]: 種別 × 記号の文法・公差値・桁数・文字位置をまとめて検証
//!
//! [`DimStyle::validate`] だけは [`crate::SheetMeta::validate`] と同じく
//! `Result<(), String>` を返す。文書単位の設定という位置づけが同じで、
//! `Command::SetDimStyle`（M9 タスク47-3 で新設）が境界で [`CoreError`] へ包む形にすると
//! `SetSheet` と対称になるため。
//!
//! # 既定値は「無注記 = 現行描画と同値」
//!
//! [`DimAnnotation::default`] は全フィールドが「指定なし」で、M8 までの寸法描画と
//! 完全に同じ結果になる（DESIGN.md M9 設計判断2）。既存図面を読み込んだときの補完値も
//! これになる。

use serde::{Deserialize, Serialize};

use mcad_geom::{DimSymbol, Point2};

use crate::CoreError;

// ---------------------------------------------------------------------
// 寸法種別
// ---------------------------------------------------------------------

/// 寸法の種別。[`DimAnnotation::validate`] へ「どの寸法に付く注記か」を渡すためのタグ。
///
/// **保存されない**（`.mcad` へは寸法エンティティのバリアントそのものが書かれ、種別は
/// そこから決まる）。注記の文法検証を、寸法エンティティの型に依存しない純粋な関数として
/// 書くためだけに存在する。
///
/// **`#[non_exhaustive]` にしない**理由: 種別が増えれば
/// [`DimKind::allowed_symbols`] の許可記号表も必ず一緒に更新しなければならない
/// （更新漏れは「新種別にどの記号も付かない」または「全部付く」という静かな不具合になる）。
/// 網羅 `match` のコンパイルエラーをその更新漏れの検出器として使いたいので、ここでは
/// 意図的に閉じた列挙にする。角度寸法・弧長寸法は M9 の non-goal（DESIGN.md M9 判断10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DimKind {
    /// 長さ寸法（`DimLinear`）。
    Linear,
    /// 半径寸法（`DimRadial`）。
    Radial,
    /// 直径寸法（`DimDiameter`。M9 タスク47-2 で新設）。
    Diameter,
}

impl DimKind {
    /// この種別の寸法へ付けてよい寸法補助記号（DESIGN.md M9 設計判断2 の文法マトリクス、
    /// 記号の意味は規定 5-3 の表）。
    ///
    /// | 種別 | 許可する記号 |
    /// | --- | --- |
    /// | [`DimKind::Linear`] | φ / Sφ / □ / C / t |
    /// | [`DimKind::Radial`] | R / SR / CR |
    /// | [`DimKind::Diameter`] | φ / Sφ |
    ///
    /// [`DimSymbol`] を `match` せず**許可リストで持つ**のは、`DimSymbol` が
    /// `#[non_exhaustive]` だからである。将来 geom 側に記号が増えたとき、この表に
    /// 載っていない記号は自動的に拒否側へ倒れる（`match` のワイルドカード腕と違い、
    /// 「うっかり許可側の腕へ吸わせる」書き方ができない）。
    ///
    /// UI（M9 タスク50 の記号コンボ）もこの表を唯一の出典として選択肢を作ること。
    /// 表を二重に持つと UI で選べるのに core が拒否する組合せが生まれる。
    #[must_use]
    pub const fn allowed_symbols(self) -> &'static [DimSymbol] {
        match self {
            DimKind::Linear => &[
                DimSymbol::Diameter,
                DimSymbol::SphereDiameter,
                DimSymbol::Square,
                DimSymbol::Chamfer,
                DimSymbol::Thickness,
            ],
            DimKind::Radial => &[
                DimSymbol::Radius,
                DimSymbol::SphereRadius,
                DimSymbol::ControlRadius,
            ],
            DimKind::Diameter => &[DimSymbol::Diameter, DimSymbol::SphereDiameter],
        }
    }

    /// `symbol` をこの種別の寸法へ付けてよいか（[`DimKind::allowed_symbols`] の表引き）。
    #[must_use]
    pub fn allows(self, symbol: DimSymbol) -> bool {
        self.allowed_symbols().contains(&symbol)
    }
}

// ---------------------------------------------------------------------
// はめあい記号
// ---------------------------------------------------------------------

/// IT 公差等級（[`FitClass`] の数字部分）として許す最大値。
///
/// ISO 286 の等級は IT01・IT0・IT1〜IT18。`"01"` / `"0"` は数値としてはそれぞれ 1 / 0 に
/// なるため、上限だけを 18 で見る。
const MAX_IT_GRADE: u32 = 18;

/// はめあい記号（サイズ公差の記号表記。規定 5-12-3 2)）。例: `"H7"`, `"g6"`, `"js13"`。
///
/// # 受理する表記と、その範囲を選んだ理由
///
/// 受理するのは **単一の公差クラス表記** `[英字1〜2文字][数字1〜2桁]` のみで、数字部分は
/// IT 公差等級の上限 18 以下であることを要求する（文法上、長さは自動的に 4 文字以下になる）。
/// 英字の大小は**区別する**（ISO 286 で穴は大文字・軸は小文字と意味が違うため、
/// 正規化してはならない）。
///
/// `"H7/g6"` のような**組合せ表記（穴/軸の対）は受理しない**。規定 5-12-3 2) が挙げる
/// 記入例は `φ20H7` / `φ20d9` という単一クラスの形だけで、組合せ表記は 1 つの寸法の
/// サイズ公差ではなく「はめあい（2 部品の関係）」を表す別概念だからである。組合せを
/// 1 つの文字列として通してしまうと、後段（M9 タスク49 の組版・将来の偏差値展開）が
/// 「この寸法の公差」として解釈できないデータが図面に残る。
///
/// 一方で **ISO 286 の文字集合そのものの厳密検証は行わない**（`"Q7"` のように実在しない
/// 偏差記号も通る）。はめあい記号は M9 では**表示のみ**で、偏差値の自動換算は non-goal
/// （DESIGN.md M9 設計判断2・規定 5-12-3 2) 末尾）。実在表を内蔵しないまま「実在する記号か」
/// を判定しようとすると、表の欠落がそのまま「正しい図面が保存できない」不具合になる。
/// ここで防ぎたいのは *形として解釈できない文字列*（空・空白混じり・組合せ表記・
/// 数字の欠落）が組版や出力へ流れることであり、その線引きに合わせている。
///
/// serde は透過ではなく `try_from` 経由なので、手編集された `.mcad` の不正値も
/// 読込境界で拒否される（[`crate::Scale`] / [`crate::WidthMm`] と同じ流儀）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct FitClass(String);

impl FitClass {
    /// はめあい記号を検証して作る。
    ///
    /// # Errors
    ///
    /// 受理する表記（型の doc 参照）から外れる場合に [`CoreError::InvalidFitClass`] を返す。
    pub fn new(s: impl Into<String>) -> Result<Self, CoreError> {
        let s = s.into();
        Self::check(&s)?;
        Ok(Self(s))
    }

    /// 記号の文字列表現。
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 文法検査（[`FitClass::new`] の実体）。
    fn check(s: &str) -> Result<(), CoreError> {
        let invalid = |reason: &str| {
            Err(CoreError::InvalidFitClass(format!(
                "invalid fit class {s:?}: {reason}"
            )))
        };

        if s.is_empty() {
            return invalid("must not be empty");
        }
        if s.contains('/') {
            // 組合せ表記を「その他の文法エラー」に混ぜず、意図的な非対応だと分かる
            // メッセージにする（UI がそのまま出す想定）。
            return invalid("combined hole/shaft notation such as \"H7/g6\" is not supported");
        }

        // 英字部（偏差記号）と数字部（IT 等級）に分ける。英字は ASCII なので
        // 「文字数 = 先頭からのバイト数」であり、この位置で分割してよい。
        let letters = s.chars().take_while(char::is_ascii_alphabetic).count();
        if !(1..=2).contains(&letters) {
            return invalid("must start with 1 or 2 ASCII letters (deviation symbol)");
        }
        let grade = &s[letters..];
        if grade.is_empty() || grade.len() > 2 || !grade.chars().all(|c| c.is_ascii_digit()) {
            return invalid("must end with a 1- or 2-digit IT grade");
        }
        // 上の検査で 1〜2 桁の ASCII 数字だけと分かっているのでパースは必ず成功する。
        let Ok(grade) = grade.parse::<u32>() else {
            return invalid("must end with a 1- or 2-digit IT grade");
        };
        if grade > MAX_IT_GRADE {
            return invalid(&format!("IT grade must not exceed {MAX_IT_GRADE}"));
        }
        Ok(())
    }
}

impl TryFrom<String> for FitClass {
    type Error = CoreError;

    #[inline]
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

// ---------------------------------------------------------------------
// サイズ公差
// ---------------------------------------------------------------------

/// サイズ公差（寸法許容差。規定 5-12）。
///
/// 表せるのは「サイズ公差の値そのもの」だけで、Ⓔ 等の ISO 14405 系修飾子は持たない
/// （寸法がサイズ形体との対応づけを持たないため、付けてよいかを core が判定できない。
/// DESIGN.md M9 設計判断2 実装時追記）。
///
/// **`#[non_exhaustive]`**: 表記方式は将来増えうる（規定 5-12-3 1) の上下限値直接記入など）。
/// クレート外の `match` には常にワイルドカード腕を要求し、追加が下流のコンパイルエラーに
/// ならないようにする（[`crate::Linetype`] と同じ方針）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SizeTolerance {
    /// 対称許容差 `± v`（規定 5-12-2 1)）。値は大きさなので非負。
    Symmetric(f64),
    /// 上下偏差（規定 5-12-2 2)・3)）。片側公差は片方を `0.0` にして表す。
    Deviations {
        /// 上の許容差（上寸法許容差）。
        upper: f64,
        /// 下の許容差（下寸法許容差）。`upper` 以下でなければならない。
        lower: f64,
    },
    /// はめあい記号（規定 5-12-3 2)）。**表示のみ**で、偏差値へは展開しない。
    Fit(FitClass),
}

impl SizeTolerance {
    /// 公差の値が描画・出力へ流せる形か検証する。
    ///
    /// # Errors
    ///
    /// 次の場合に [`CoreError::InvalidDimAnnotation`] を返す。
    ///
    /// - 非有限（NaN / ∞）の値を含む
    /// - [`SizeTolerance::Symmetric`] が負（`±` は大きさなので上下が入れ替わってしまう）
    /// - [`SizeTolerance::Deviations`] が `upper < lower`（上下が逆転した公差域）
    ///
    /// [`SizeTolerance::Fit`] は [`FitClass`] が検証済み型なので常に妥当。
    /// 値の**大きさ**の上限は設けない（寸法値そのものが座標由来で上限を持たないため、
    /// ここだけ絞っても意味がない。非有限の遮断が目的）。
    pub fn validate(&self) -> Result<(), CoreError> {
        match self {
            SizeTolerance::Symmetric(v) => {
                if !v.is_finite() {
                    return Err(CoreError::InvalidDimAnnotation(format!(
                        "symmetric tolerance must be finite: {v}"
                    )));
                }
                if *v < 0.0 {
                    return Err(CoreError::InvalidDimAnnotation(format!(
                        "symmetric tolerance must not be negative: {v}"
                    )));
                }
                Ok(())
            }
            SizeTolerance::Deviations { upper, lower } => {
                if !upper.is_finite() || !lower.is_finite() {
                    return Err(CoreError::InvalidDimAnnotation(format!(
                        "deviations must be finite: upper {upper}, lower {lower}"
                    )));
                }
                if upper < lower {
                    return Err(CoreError::InvalidDimAnnotation(format!(
                        "upper deviation must not be below the lower one: upper {upper}, lower {lower}"
                    )));
                }
                Ok(())
            }
            SizeTolerance::Fit(_) => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------
// 矢印の配置
// ---------------------------------------------------------------------

/// 寸法線の終端（矢）を内向き・外向きのどちらに描くか（DESIGN.md M9 設計判断5）。
///
/// 既定の [`ArrowPlacement::Auto`] は「文字幅 + 矢 2 つが寸法線長に収まらなければ外向き」
/// という自動判定（判定そのものは描画側＝app 層の責務）。[`ArrowPlacement::Inside`] /
/// [`ArrowPlacement::Outside`] はその判定を手で上書きする。
///
/// 3 値で意味的に閉じている（自動 / 内 / 外）ため `#[non_exhaustive]` にしない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ArrowPlacement {
    /// 自動判定（既定）。
    #[default]
    Auto,
    /// 常に内向き。
    Inside,
    /// 常に外向き。
    Outside,
}

// ---------------------------------------------------------------------
// 寸法注記
// ---------------------------------------------------------------------

/// 小数点以下の桁数（[`DimStyle::decimals`] / [`DimAnnotation::decimals_override`]）の上限。
///
/// 規定 5-2 2) a) が挙げる実用範囲は 0〜2 桁（±0.5mm → 整数、±0.05mm → 1 桁、
/// ±0.005mm → 2 桁）で、4 桁は 0.1µm 相当まで書けることになる。実用側に十分な余裕を
/// 持たせつつ上限を置くのは、桁数がそのまま**寸法文字列の長さ**になるためである
/// （`u8` の上限 255 桁をそのまま許すと、1 つの寸法が 200 文字超のテキストへ展開され、
/// 毎フレームの組版・pick・SVG/PDF 出力へ効く。[`crate::MAX_TITLE_BLOCK_ROWS`] で
/// 要素数へ上限を置いたのと同じ考え方）。
pub const MAX_DIM_DECIMALS: u8 = 4;

/// 寸法へ付ける注記（寸法補助記号・サイズ公差・桁数上書き・文字位置・矢配置）。
///
/// 寸法値そのものは持たない（M6 設計判断2 のとおり、値は座標から毎回計算する）。
/// 既定値は「無注記」＝ M8 までの描画と完全に同値（[`DimAnnotation::unannotated`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimAnnotation {
    /// 寸法補助記号（規定 5-3）。`None` は記号なし。
    ///
    /// 付けてよい記号は寸法の種別ごとに決まる（[`DimKind::allowed_symbols`]）。
    pub symbol: Option<DimSymbol>,
    /// サイズ公差（規定 5-12）。`None` は公差の記入なし（規定 5-12-4 1)）。
    pub tolerance: Option<SizeTolerance>,
    /// この寸法だけの桁数上書き。`None` は文書の [`DimStyle::decimals`] に**従い続ける**
    /// （生きた参照であり、後から寸法スタイルを変えれば即座に反映される）。
    ///
    /// `Some(n)` はこの寸法に限った明示上書きで、寸法スタイルの変更に追従しない。
    /// この二者の区別が DESIGN.md M9 設計判断4 実装時追記の要点で、解決は
    /// [`DimStyle::resolve_decimals`] に集約する。上限は [`MAX_DIM_DECIMALS`]。
    pub decimals_override: Option<u8>,
    /// 寸法値テキストの配置位置（ワールド座標）の手動上書き。`None` は自動配置。
    ///
    /// 文字位置の後編集（M9 タスク51）で設定する。`None` に戻す操作が「自動配置に戻す」。
    pub text_anchor: Option<Point2>,
    /// 矢の内外配置。
    pub arrow_placement: ArrowPlacement,
    /// 非比例寸法の表示値上書き（規定 5-10）。`None` は座標から計算した値をそのまま表示する。
    ///
    /// `Some(s)` は寸法値テキストを `s` へ差し替える（下線を引く組版処理は描画側＝
    /// M9 タスク49 の責務で、ここは差し替え値の保持と文法検証のみを持つ）。空文字列・
    /// 空白のみ・制御文字を含む値は [`DimAnnotation::validate`] が拒否する。
    pub value_override: Option<String>,
}

impl DimAnnotation {
    /// 無注記（記号・公差・桁数上書き・文字位置がいずれも指定なし、矢は自動）。
    ///
    /// M8 までの寸法描画と完全に同じ結果になる既定値であり、v1〜v4 の `.mcad` を
    /// 読み込んだときの補完値でもある（DESIGN.md M9 設計判断2・6）。
    /// [`crate::Style::inherited`] と同じく、定数文脈でも使えるよう `const fn` で置く。
    #[inline]
    #[must_use]
    pub const fn unannotated() -> Self {
        Self {
            symbol: None,
            tolerance: None,
            decimals_override: None,
            text_anchor: None,
            arrow_placement: ArrowPlacement::Auto,
            value_override: None,
        }
    }

    /// 何も注記されていない（[`DimAnnotation::unannotated`] と等しい）か。
    ///
    /// 用途は 2 つ。**保存形式の後方互換**（`DimLinear` 等は
    /// `#[serde(skip_serializing_if = "DimAnnotation::is_unannotated")]` を付けており、
    /// 無注記の寸法は `annotation` フィールドごと出力されない ＝ M8 までの `.mcad` と
    /// バイト単位で同じ JSON になる）と、UI が「注記あり」を表示し分けるための述語。
    #[inline]
    #[must_use]
    pub fn is_unannotated(&self) -> bool {
        *self == Self::unannotated()
    }

    /// 注記が `kind` の寸法に対して妥当か検証する。
    ///
    /// **core コマンド境界・`.mcad` 読込・UI 入力の3境界すべて**から呼ぶ
    /// （DESIGN.md M9 設計判断2）。検証内容は次の 5 つ。
    ///
    /// 1. 種別 × 記号の文法（[`DimKind::allowed_symbols`]。例: 長さ寸法に SR は不可）
    /// 2. 公差値（[`SizeTolerance::validate`]。非有限・`upper < lower` の拒否）
    /// 3. 桁数上書きが [`MAX_DIM_DECIMALS`] 以下であること
    /// 4. 文字位置が有限座標であること（非有限が組版・SVG/PDF 座標へ流れるのを防ぐ）
    /// 5. 値上書きが空文字列・空白のみ・制御文字混じりでないこと
    ///
    /// # Errors
    ///
    /// 上記のいずれかに反する場合に [`CoreError::InvalidDimAnnotation`] を返す。
    pub fn validate(&self, kind: DimKind) -> Result<(), CoreError> {
        if let Some(symbol) = self.symbol
            && !kind.allows(symbol)
        {
            return Err(CoreError::InvalidDimAnnotation(format!(
                "symbol {symbol:?} is not allowed on a {kind:?} dimension"
            )));
        }
        if let Some(tolerance) = &self.tolerance {
            tolerance.validate()?;
        }
        if let Some(decimals) = self.decimals_override
            && decimals > MAX_DIM_DECIMALS
        {
            return Err(CoreError::InvalidDimAnnotation(format!(
                "decimals override {decimals} exceeds the limit of {MAX_DIM_DECIMALS}"
            )));
        }
        if let Some(anchor) = self.text_anchor
            && !(anchor.x.is_finite() && anchor.y.is_finite())
        {
            return Err(CoreError::InvalidDimAnnotation(format!(
                "text anchor must be finite: ({}, {})",
                anchor.x, anchor.y
            )));
        }
        if let Some(value) = &self.value_override {
            if value.trim().is_empty() {
                return Err(CoreError::InvalidDimAnnotation(
                    "value override must not be empty or whitespace-only".to_string(),
                ));
            }
            if value.chars().any(char::is_control) {
                return Err(CoreError::InvalidDimAnnotation(format!(
                    "value override must not contain control characters: {value:?}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for DimAnnotation {
    /// 無注記（[`DimAnnotation::unannotated`]）。
    #[inline]
    fn default() -> Self {
        Self::unannotated()
    }
}

// ---------------------------------------------------------------------
// 寸法スタイル
// ---------------------------------------------------------------------

/// [`DimStyle`] の紙 mm 値として許す最大値。
///
/// 用紙の最大は A0 の 1189mm なので、実用値（文字 3.5mm・矢 3mm・すきま 1mm）から見て
/// 桁違いの余裕がある。それでも上限を置くのは [`crate::MAX_TITLE_BLOCK_MM`] と同じ理由で、
/// 「用紙に載らない値を早期に弾く」と「非有限・巨大値を配置計算や SVG/PDF 座標へ
/// 流さない」を 1 つの定数で兼ねるため。
pub const MAX_DIM_STYLE_MM: f64 = 1_000.0;

/// 文書単位の寸法スタイル（DESIGN.md M9 設計判断4）。
///
/// 名前付き複数スタイルは non-goal で、図面に 1 つだけ持つ。変更は
/// `Command::SetDimStyle`（M9 タスク47-3）経由で undo 1 単位。[`crate::SheetMeta`] へ
/// 入れないのは、用紙の関心事ではなく作図内容のスタイルだからである。
///
/// 長さの単位はすべて **紙 mm**（[`crate::WidthMm`] と同じ契約）。ワールド長への換算は
/// `k = Scale::world_mm_per_paper_mm` を掛ける app 層の責務。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DimStyle {
    /// 寸法値の文字高さ [紙 mm]。既定 3.5（規定 6-2 b) の標準呼び 3.5。現行の
    /// `mcad_app::plot::DIM_TEXT_MM` と同値で、既定のままなら見た目が変わらない）。
    pub text_height_mm: f64,
    /// 矢先の長さ [紙 mm]。既定 3.0（現行の `mcad_app::plot::DIM_ARROW_MM` と同値。
    /// 矢羽の開き角度 15° は規定 5-4 4) 側で持つ）。
    pub arrow_len_mm: f64,
    /// 寸法値の小数点以下の桁数。既定 2（規定 5-2 2) a) の「±0.005mm 程度 → 2 桁」。
    /// 現行の `format!("{:.2}")` と同値）。上限は [`MAX_DIM_DECIMALS`]。
    pub decimals: u8,
    /// 末尾の 0 を落とすか（`120.00` → `120`）。既定 true。
    ///
    /// 規定 5-2 2) a) は「加工精度により最も適切な桁数を選ぶ」であって、意味のない 0 を
    /// 並べることは求めていない。DESIGN.md M9 設計判断5 (a) で既定 ON を確定済み
    /// （M8 までの `{:.2}` 固定表示に対する意図した可視差）。
    pub trim_trailing_zeros: bool,
    /// 寸法補助線と図形の間のすきま [紙 mm]。既定 1.0（規定 5-4 2) b) の標準値）。
    pub ext_gap_mm: f64,
    /// 寸法補助線が寸法線を越えて突き出す長さ [紙 mm]。既定 2.0（規定 5-4 2) c) の標準値）。
    pub ext_overshoot_mm: f64,
    /// 寸法線と寸法値テキストの間のすきま [紙 mm]。既定 1.4。
    ///
    /// M8 までの実装は `mcad_app::dimension` が `gap = height * 0.4` と文字高さ比で
    /// 持っていた。既定の文字高さ 3.5mm では `3.5 * 0.4 = 1.4` なので、**既定値のままなら
    /// 現行描画と同値**になる。比率ではなく実寸で持つのは、文字高さを変えても
    /// すきまを規定 5-4 の他の値（すきま 1.0・突き出し 2.0）と揃えて調整できるようにするため。
    pub text_gap_mm: f64,
    /// 公差文字を寸法値に対して何倍の高さで書くか。既定 0.7
    /// （規定 5-12-2 2)「公差数値の文字サイズは、寸法数値の 70% 程度に縮小する」）。
    pub tolerance_scale: f64,
}

impl DimStyle {
    /// 既定の寸法スタイル（各フィールドの doc に根拠を記載）。
    ///
    /// [`Default`] は `const` 文脈で使えないので、定数として使える形も置く。
    pub const DEFAULT: DimStyle = DimStyle {
        text_height_mm: 3.5,
        arrow_len_mm: 3.0,
        decimals: 2,
        trim_trailing_zeros: true,
        ext_gap_mm: 1.0,
        ext_overshoot_mm: 2.0,
        text_gap_mm: 1.4,
        tolerance_scale: 0.7,
    };

    /// スタイルが描画・出力に使える形か検証する。
    ///
    /// 検証するのは次の 3 つ。
    ///
    /// 1. 紙 mm の各値が有限・[`MAX_DIM_STYLE_MM`] 以下であること。文字高さと矢先長さは
    ///    さらに正であること（0 だと寸法値・矢が消え、寸法として読めなくなる）。
    ///    すきま・突き出しは 0 を許す（「すきまなし」は意味のある指定）。
    /// 2. 桁数が [`MAX_DIM_DECIMALS`] 以下であること。
    /// 3. 公差文字の縮小率が `0 <` かつ `<= 1.0` であること。規定 5-12-2 2) は公差文字を
    ///    寸法数値より**縮小**すると定めており、拡大（> 1.0）はその規定に反する。
    ///
    /// # Errors
    ///
    /// 不正の理由を人が読める `String` で返す（[`crate::SheetMeta::validate`] と同じ流儀。
    /// [`CoreError`] へ包むのは `Command::SetDimStyle` の側＝M9 タスク47-3）。
    pub fn validate(&self) -> Result<(), String> {
        check_positive_style_mm(self.text_height_mm, "dimension text height")?;
        check_positive_style_mm(self.arrow_len_mm, "dimension arrow length")?;
        check_style_mm(self.ext_gap_mm, "extension line gap")?;
        check_style_mm(self.ext_overshoot_mm, "extension line overshoot")?;
        check_style_mm(self.text_gap_mm, "dimension text gap")?;
        if self.decimals > MAX_DIM_DECIMALS {
            return Err(format!(
                "dimension decimals {} exceeds the limit of {MAX_DIM_DECIMALS}",
                self.decimals
            ));
        }
        if !(self.tolerance_scale.is_finite()
            && self.tolerance_scale > 0.0
            && self.tolerance_scale <= 1.0)
        {
            return Err(format!(
                "tolerance text scale must be within (0, 1]: {}",
                self.tolerance_scale
            ));
        }
        Ok(())
    }

    /// この寸法に使う小数点以下の桁数を解決する（DESIGN.md M9 設計判断4 実装時追記）。
    ///
    /// `decimals_override`（[`DimAnnotation::decimals_override`]）が `Some(n)` ならその
    /// 明示上書きを、`None` なら文書側の [`DimStyle::decimals`] を返す。`None` の寸法は
    /// 文書スタイルへの生きた参照なので、`Command::SetDimStyle` の変更が即座に反映される。
    #[inline]
    #[must_use]
    pub const fn resolve_decimals(&self, decimals_override: Option<u8>) -> u8 {
        match decimals_override {
            Some(n) => n,
            None => self.decimals,
        }
    }
}

impl Default for DimStyle {
    /// [`DimStyle::DEFAULT`]。
    #[inline]
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 紙 mm のスタイル値が「有限・0 以上・[`MAX_DIM_STYLE_MM`] 以下」かを検証する。
/// `what` は失敗時のメッセージに使う説明。
fn check_style_mm(value: f64, what: &str) -> Result<(), String> {
    if !(value.is_finite() && value >= 0.0) {
        return Err(format!("invalid {what}: {value}"));
    }
    if value > MAX_DIM_STYLE_MM {
        return Err(format!(
            "{what} exceeds the {MAX_DIM_STYLE_MM} mm limit: {value}"
        ));
    }
    Ok(())
}

/// [`check_style_mm`] に加えて 0 を許さない（描画に必ず長さが要る値のための検証）。
fn check_positive_style_mm(value: f64, what: &str) -> Result<(), String> {
    check_style_mm(value, what)?;
    if value <= 0.0 {
        return Err(format!("{what} must be positive: {value}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`DimSymbol`] の全バリアント（geom 側は `#[non_exhaustive]` なのでここに列挙する。
    /// 記号が増えたらこの配列と下の [`GRAMMAR`] を更新すること。更新しなくても
    /// 「新記号はどの種別でも拒否」という安全側の挙動になる）。
    const ALL_SYMBOLS: [DimSymbol; 8] = [
        DimSymbol::Diameter,
        DimSymbol::SphereDiameter,
        DimSymbol::Square,
        DimSymbol::Radius,
        DimSymbol::SphereRadius,
        DimSymbol::ControlRadius,
        DimSymbol::Chamfer,
        DimSymbol::Thickness,
    ];

    /// 種別 × 記号の期待値（DESIGN.md M9 設計判断2 の文法マトリクスを実装とは独立に
    /// 転記したオラクル）。列は [`ALL_SYMBOLS`] の順（φ / Sφ / □ / R / SR / CR / C / t）。
    const GRAMMAR: [(DimKind, [bool; 8]); 3] = [
        (
            DimKind::Linear,
            [true, true, true, false, false, false, true, true],
        ),
        (
            DimKind::Radial,
            [false, false, false, true, true, true, false, false],
        ),
        (
            DimKind::Diameter,
            [true, true, false, false, false, false, false, false],
        ),
    ];

    fn with_symbol(symbol: DimSymbol) -> DimAnnotation {
        DimAnnotation {
            symbol: Some(symbol),
            ..DimAnnotation::default()
        }
    }

    // -----------------------------------------------------------------
    // (a) 種別 × 記号の網羅
    // -----------------------------------------------------------------

    #[test]
    fn symbol_grammar_matrix_is_exhaustively_enforced() {
        for (kind, expected_row) in GRAMMAR {
            for (symbol, expected) in ALL_SYMBOLS.into_iter().zip(expected_row) {
                assert_eq!(
                    kind.allows(symbol),
                    expected,
                    "allows({kind:?}, {symbol:?}) should be {expected}"
                );
                let result = with_symbol(symbol).validate(kind);
                assert_eq!(
                    result.is_ok(),
                    expected,
                    "validate({symbol:?} on {kind:?}) should be {}: {result:?}",
                    if expected { "accepted" } else { "rejected" }
                );
                if !expected {
                    assert!(matches!(result, Err(CoreError::InvalidDimAnnotation(_))));
                }
            }
        }
    }

    #[test]
    fn allowed_symbols_and_allows_agree() {
        // 表引きの 2 つの API が同じ表を見ていること（UI が `allowed_symbols` で
        // 選択肢を作り、core が `allows` で検証しても食い違わない）。
        for (kind, _) in GRAMMAR {
            for symbol in ALL_SYMBOLS {
                assert_eq!(
                    kind.allows(symbol),
                    kind.allowed_symbols().contains(&symbol),
                    "{kind:?} / {symbol:?}"
                );
            }
        }
    }

    #[test]
    fn no_symbol_is_allowed_on_every_kind() {
        for (kind, _) in GRAMMAR {
            assert!(DimAnnotation::default().validate(kind).is_ok());
        }
    }

    // -----------------------------------------------------------------
    // (b) 値の検証
    // -----------------------------------------------------------------

    #[test]
    fn symmetric_tolerance_rejects_negative_and_non_finite() {
        assert!(SizeTolerance::Symmetric(0.05).validate().is_ok());
        // ±0 は「公差なし」と同義だが表記としては矛盾しないので受理する。
        assert!(SizeTolerance::Symmetric(0.0).validate().is_ok());
        for bad in [-0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                matches!(
                    SizeTolerance::Symmetric(bad).validate(),
                    Err(CoreError::InvalidDimAnnotation(_))
                ),
                "±{bad} should be rejected"
            );
        }
    }

    #[test]
    fn deviations_reject_upper_below_lower_and_non_finite() {
        // 規定 5-12-2 2) の例（50 の上に +0.2、下に -0.1）。
        assert!(
            SizeTolerance::Deviations {
                upper: 0.2,
                lower: -0.1,
            }
            .validate()
            .is_ok()
        );
        // 片側公差（規定 5-12-2 3)）。
        assert!(
            SizeTolerance::Deviations {
                upper: 0.3,
                lower: 0.0,
            }
            .validate()
            .is_ok()
        );
        // 上下が同値（公差域 0 の一定オフセット）は矛盾ではないので受理する。
        assert!(
            SizeTolerance::Deviations {
                upper: 0.1,
                lower: 0.1,
            }
            .validate()
            .is_ok()
        );

        assert!(matches!(
            SizeTolerance::Deviations {
                upper: -0.1,
                lower: 0.2,
            }
            .validate(),
            Err(CoreError::InvalidDimAnnotation(_))
        ));
        for (upper, lower) in [
            (f64::NAN, 0.0),
            (0.0, f64::NAN),
            (f64::INFINITY, 0.0),
            (0.0, f64::NEG_INFINITY),
        ] {
            assert!(
                matches!(
                    SizeTolerance::Deviations { upper, lower }.validate(),
                    Err(CoreError::InvalidDimAnnotation(_))
                ),
                "upper {upper}, lower {lower} should be rejected"
            );
        }
    }

    #[test]
    fn annotation_validate_rejects_bad_tolerance_regardless_of_kind() {
        let bad = DimAnnotation {
            tolerance: Some(SizeTolerance::Deviations {
                upper: -1.0,
                lower: 1.0,
            }),
            ..DimAnnotation::default()
        };
        for (kind, _) in GRAMMAR {
            assert!(matches!(
                bad.validate(kind),
                Err(CoreError::InvalidDimAnnotation(_))
            ));
        }
    }

    #[test]
    fn fit_class_accepts_single_class_notation() {
        // 規定 5-12-3 2) の例（φ20H7 / φ20d9）と、2 文字偏差記号・IT 等級の両端。
        for s in ["H7", "d9", "g6", "js6", "JS13", "H01", "h0", "H18"] {
            let fit = FitClass::new(s).unwrap_or_else(|e| panic!("{s} should be accepted: {e}"));
            assert_eq!(
                fit.as_str(),
                s,
                "文字列はそのまま保つ（大小は正規化しない）"
            );
        }
        // 大文字（穴）と小文字（軸）は別物として区別される。
        assert_ne!(FitClass::new("H7").unwrap(), FitClass::new("h7").unwrap());
    }

    #[test]
    fn fit_class_rejects_malformed_notation() {
        for s in [
            "",         // 空
            " ",        // 空白のみ
            "H",        // 等級なし
            "7",        // 偏差記号なし
            "H7/g6",    // 組合せ表記（非対応）
            "h 7",      // 空白混じり
            "H7 ",      // 末尾空白
            " H7",      // 先頭空白
            "ABC7",     // 偏差記号が 3 文字
            "H123",     // 等級が 3 桁
            "H19",      // IT 等級の上限超過
            "φ20H7",    // 寸法値ごと入力
            "Ｈ７",     // 全角
            "H-7",      // 記号混じり
            "H7g6",     // 区切りなしの組合せ
            "H7\u{a0}", // 非 ASCII 空白
        ] {
            assert!(
                matches!(FitClass::new(s), Err(CoreError::InvalidFitClass(_))),
                "{s:?} should be rejected"
            );
        }
    }

    #[test]
    fn combined_fit_notation_is_rejected_with_a_dedicated_message() {
        let Err(CoreError::InvalidFitClass(msg)) = FitClass::new("H7/g6") else {
            panic!("combined notation should be rejected");
        };
        assert!(msg.contains("combined"), "{msg}");
    }

    #[test]
    fn fit_tolerance_is_always_valid() {
        let fit = SizeTolerance::Fit(FitClass::new("H7").unwrap());
        assert!(fit.validate().is_ok());
        let annotation = DimAnnotation {
            tolerance: Some(fit),
            ..DimAnnotation::default()
        };
        assert!(annotation.validate(DimKind::Linear).is_ok());
    }

    #[test]
    fn decimals_override_is_capped() {
        for decimals in 0..=MAX_DIM_DECIMALS {
            let annotation = DimAnnotation {
                decimals_override: Some(decimals),
                ..DimAnnotation::default()
            };
            assert!(annotation.validate(DimKind::Linear).is_ok(), "{decimals}");
        }
        for decimals in [MAX_DIM_DECIMALS + 1, u8::MAX] {
            let annotation = DimAnnotation {
                decimals_override: Some(decimals),
                ..DimAnnotation::default()
            };
            assert!(
                matches!(
                    annotation.validate(DimKind::Linear),
                    Err(CoreError::InvalidDimAnnotation(_))
                ),
                "{decimals} should be rejected"
            );
        }
    }

    #[test]
    fn text_anchor_must_be_finite() {
        let ok = DimAnnotation {
            text_anchor: Some(Point2::new(10.0, -20.0)),
            ..DimAnnotation::default()
        };
        assert!(ok.validate(DimKind::Radial).is_ok());

        for p in [
            Point2::new(f64::NAN, 0.0),
            Point2::new(0.0, f64::NAN),
            Point2::new(f64::INFINITY, 0.0),
            Point2::new(0.0, f64::NEG_INFINITY),
        ] {
            let bad = DimAnnotation {
                text_anchor: Some(p),
                ..DimAnnotation::default()
            };
            assert!(
                matches!(
                    bad.validate(DimKind::Radial),
                    Err(CoreError::InvalidDimAnnotation(_))
                ),
                "{p:?} should be rejected"
            );
        }
    }

    #[test]
    fn value_override_accepts_non_blank_text() {
        for s in ["5-10", "  50 (参考)  ", "A"] {
            let annotation = DimAnnotation {
                value_override: Some(s.to_string()),
                ..DimAnnotation::default()
            };
            assert!(
                annotation.validate(DimKind::Linear).is_ok(),
                "{s:?} should be accepted"
            );
        }
    }

    #[test]
    fn value_override_rejects_blank_or_control_characters() {
        for s in ["", "   ", "\t", "50\n", "50\u{0007}mm"] {
            let annotation = DimAnnotation {
                value_override: Some(s.to_string()),
                ..DimAnnotation::default()
            };
            assert!(
                matches!(
                    annotation.validate(DimKind::Linear),
                    Err(CoreError::InvalidDimAnnotation(_))
                ),
                "{s:?} should be rejected"
            );
        }
    }

    // -----------------------------------------------------------------
    // (c) serde
    // -----------------------------------------------------------------

    #[test]
    fn annotation_survives_a_serde_round_trip() {
        let annotation = DimAnnotation {
            symbol: Some(DimSymbol::Diameter),
            tolerance: Some(SizeTolerance::Deviations {
                upper: 0.2,
                lower: -0.1,
            }),
            decimals_override: Some(3),
            text_anchor: Some(Point2::new(12.5, -4.25)),
            arrow_placement: ArrowPlacement::Outside,
            value_override: Some("5-10".to_string()),
        };
        let json = serde_json::to_string(&annotation).unwrap();
        assert_eq!(
            serde_json::from_str::<DimAnnotation>(&json).unwrap(),
            annotation
        );

        // 無注記も往復する（v1〜v4 からの移行値がこれになる）。
        let default_json = serde_json::to_string(&DimAnnotation::default()).unwrap();
        assert_eq!(
            serde_json::from_str::<DimAnnotation>(&default_json).unwrap(),
            DimAnnotation::default()
        );
    }

    #[test]
    fn fit_class_serializes_as_a_plain_string_and_round_trips() {
        let annotation = DimAnnotation {
            tolerance: Some(SizeTolerance::Fit(FitClass::new("js6").unwrap())),
            ..DimAnnotation::default()
        };
        let json = serde_json::to_string(&FitClass::new("H7").unwrap()).unwrap();
        assert_eq!(json, r#""H7""#);
        assert_eq!(
            serde_json::from_str::<FitClass>(&json).unwrap(),
            FitClass::new("H7").unwrap()
        );
        let json = serde_json::to_string(&annotation).unwrap();
        assert_eq!(
            serde_json::from_str::<DimAnnotation>(&json).unwrap(),
            annotation
        );
    }

    #[test]
    fn fit_class_serde_rejects_invalid_values_at_the_read_boundary() {
        // 手編集された `.mcad` の不正値は try_from で弾かれる。
        for json in [r#""""#, r#""H7/g6""#, r#""H19""#, r#""20H7""#] {
            assert!(
                serde_json::from_str::<FitClass>(json).is_err(),
                "{json} should be rejected"
            );
        }
        // 注記の中に埋まっていても同じ（読込境界での拒否）。
        let embedded = r#"{"symbol":null,"tolerance":{"Fit":"H7/g6"},"decimals_override":null,"text_anchor":null,"arrow_placement":"Auto"}"#;
        assert!(serde_json::from_str::<DimAnnotation>(embedded).is_err());
        // 同じ形で記号だけ正しければ読める（上の失敗が構造ではなく値によることの確認）。
        let valid = r#"{"symbol":null,"tolerance":{"Fit":"H7"},"decimals_override":null,"text_anchor":null,"arrow_placement":"Auto"}"#;
        let parsed = serde_json::from_str::<DimAnnotation>(valid).unwrap();
        assert_eq!(
            parsed.tolerance,
            Some(SizeTolerance::Fit(FitClass::new("H7").unwrap()))
        );
    }

    #[test]
    fn dim_style_survives_a_serde_round_trip() {
        let style = DimStyle {
            text_height_mm: 5.0,
            decimals: 1,
            trim_trailing_zeros: false,
            ..DimStyle::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        assert_eq!(serde_json::from_str::<DimStyle>(&json).unwrap(), style);
    }

    // -----------------------------------------------------------------
    // (d) 既定値
    // -----------------------------------------------------------------

    #[test]
    fn annotation_default_is_unannotated() {
        let a = DimAnnotation::default();
        assert_eq!(a, DimAnnotation::unannotated());
        assert_eq!(a.symbol, None);
        assert_eq!(a.tolerance, None);
        assert_eq!(a.decimals_override, None);
        assert_eq!(a.text_anchor, None);
        assert_eq!(a.arrow_placement, ArrowPlacement::Auto);
        assert_eq!(ArrowPlacement::default(), ArrowPlacement::Auto);
        assert_eq!(a.value_override, None);
    }

    #[test]
    fn dim_style_default_matches_the_drafting_standard() {
        let s = DimStyle::default();
        assert_eq!(s, DimStyle::DEFAULT);
        // 規定 6-2 b)（文字高さ 3.5）・現行 plot::DIM_TEXT_MM / DIM_ARROW_MM。
        assert_eq!(s.text_height_mm, 3.5);
        assert_eq!(s.arrow_len_mm, 3.0);
        // 規定 5-2 2) a)（±0.005mm → 2 桁）と DESIGN.md M9 判断5 (a)（ゼロトリム既定 ON）。
        assert_eq!(s.decimals, 2);
        assert!(s.trim_trailing_zeros);
        // 規定 5-4 2) b) c)（すきま 1mm・突き出し 2mm）。
        assert_eq!(s.ext_gap_mm, 1.0);
        assert_eq!(s.ext_overshoot_mm, 2.0);
        // 規定 5-12-2 2)（公差文字は 70%）。
        assert_eq!(s.tolerance_scale, 0.7);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn default_text_gap_matches_the_current_height_ratio() {
        // M8 までの `mcad_app::dimension` は gap = height * 0.4。既定文字高さ 3.5mm では
        // 1.4mm となり、既定値のままなら現行描画と同値になる。
        let s = DimStyle::default();
        assert!((s.text_gap_mm - s.text_height_mm * 0.4).abs() < 1e-12);
    }

    // -----------------------------------------------------------------
    // 寸法スタイルの検証と桁数解決
    // -----------------------------------------------------------------

    #[test]
    fn dim_style_validate_rejects_unusable_values() {
        let base = DimStyle::default();

        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, MAX_DIM_STYLE_MM + 1.0] {
            assert!(
                DimStyle {
                    text_height_mm: bad,
                    ..base
                }
                .validate()
                .is_err(),
                "text height {bad} should be rejected"
            );
            assert!(
                DimStyle {
                    arrow_len_mm: bad,
                    ..base
                }
                .validate()
                .is_err(),
                "arrow length {bad} should be rejected"
            );
        }

        // すきま・突き出し・文字すきまは 0 を許す（「すきまなし」の指定）。
        assert!(
            DimStyle {
                ext_gap_mm: 0.0,
                ext_overshoot_mm: 0.0,
                text_gap_mm: 0.0,
                ..base
            }
            .validate()
            .is_ok()
        );
        for bad in [-0.5, f64::NAN, f64::NEG_INFINITY, MAX_DIM_STYLE_MM + 1.0] {
            assert!(
                DimStyle {
                    ext_gap_mm: bad,
                    ..base
                }
                .validate()
                .is_err(),
                "gap {bad} should be rejected"
            );
            assert!(
                DimStyle {
                    ext_overshoot_mm: bad,
                    ..base
                }
                .validate()
                .is_err(),
                "overshoot {bad} should be rejected"
            );
            assert!(
                DimStyle {
                    text_gap_mm: bad,
                    ..base
                }
                .validate()
                .is_err(),
                "text gap {bad} should be rejected"
            );
        }

        assert!(
            DimStyle {
                decimals: MAX_DIM_DECIMALS,
                ..base
            }
            .validate()
            .is_ok()
        );
        assert!(
            DimStyle {
                decimals: MAX_DIM_DECIMALS + 1,
                ..base
            }
            .validate()
            .is_err()
        );

        // 公差文字の縮小率は (0, 1]。拡大は規定 5-12-2 2) に反する。
        assert!(
            DimStyle {
                tolerance_scale: 1.0,
                ..base
            }
            .validate()
            .is_ok()
        );
        for bad in [0.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            assert!(
                DimStyle {
                    tolerance_scale: bad,
                    ..base
                }
                .validate()
                .is_err(),
                "tolerance scale {bad} should be rejected"
            );
        }
    }

    #[test]
    fn resolve_decimals_prefers_the_explicit_override() {
        let style = DimStyle {
            decimals: 2,
            ..DimStyle::default()
        };
        // None は文書スタイルへの生きた参照。
        assert_eq!(style.resolve_decimals(None), 2);
        // Some(n) は明示上書き。
        assert_eq!(style.resolve_decimals(Some(0)), 0);
        assert_eq!(style.resolve_decimals(Some(4)), 4);

        // 文書スタイルを変えると None 側だけが追従する（DESIGN.md M9 判断4 実装時追記。
        // `SetDimStyle` を通した反映のテストは M9 タスク47-3 で document 側に置く）。
        let changed = DimStyle {
            decimals: 3,
            ..style
        };
        assert_eq!(changed.resolve_decimals(None), 3);
        assert_eq!(changed.resolve_decimals(Some(0)), 0);
    }
}
