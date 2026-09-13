//! 注記つき寸法値の**組版エンジン**（M9 タスク49-1、M11 タスク71 で `mcad-app` から移設）。
//!
//! [`layout_dim_label`] が [`DimAnnotation`]（寸法補助記号・サイズ公差・桁数上書き・
//! 非比例寸法の値上書き）と [`DimStyle`]（桁数・ゼロトリム・公差文字の縮小率）から、
//! 描画可能な要素の集まり（[`DimLabel`]）へ組み上げる。
//!
//! # 単位は抽象（呼び出し側の単位でそのまま返す）
//!
//! 長さの入力は `text_height` ただ 1 つで、出力の座標・幅・高さはすべて**その同じ
//! 単位**で返す。ワールド長を渡せばワールド長が、紙 mm を渡せば紙 mm が返る。
//! [`DimStyle`] から読むのは**無次元の値だけ**（`decimals` / `trim_trailing_zeros` /
//! `tolerance_scale`）で、紙 mm のフィールド（`text_height_mm` など）は読まない。
//!
//! これは M8 タスク37 の教訓（[`DimExpansion`](super::DimExpansion) の文字高さは
//! ワールド長なのに [`TextGeom::height`](crate::TextGeom::height) は紙 mm、という
//! 二重換算の罠）を再発させないための契約である。換算はこのモジュールの外
//! （`mcad-app` の `dim_sizes`・`plot`）で一度だけ行う。
//!
//! この「単位は抽象」があるから、タスク49-2 の外向き矢印の自動判定は
//! **同じ関数を紙 mm で呼び直す**だけでビュー非依存の幅を得られる
//! （`super::dimension` の `arrows_point_outward`）。
//!
//! # 配置はしない
//!
//! 出力はすべて**ラベルのローカル座標**（原点 = 値のベースライン左端、+x = 読み方向、
//! +y = 上）で、ワールドへの回転・平行移動は行わない
//! （`super::dimension` の `place_label` が担う）。

use mcad_geom::{Aabb, DimSymbol, Point2, Shape, SymbolGlyph, Vec2, dim_symbol_glyph};

use crate::{DimAnnotation, DimKind, DimStyle, SizeTolerance};

/// 文字幅の近似係数。[`crate::approx_text_width`] と同じ ASCII 係数 0.55×height で幅を
/// 近似する（純関数のまま中央寄せ配置を決めるための割り切り。DESIGN.md M6 設計判断1 の
/// 近似方針に沿う）。
///
/// 組版は ASCII 以外の文字（`±` や、値上書きに入りうる任意の文字）も同じ係数で数える。
/// フォントメトリクスを持ち込まない以上いずれ近似でしかなく、mcad-geom の
/// `symbol::FONT_CHAR_ADVANCE_RATIO`（記号の送り量）と**同じ量**なので値を食い違わせない
/// ことのほうが重要である。
pub(super) const ASCII_CHAR_WIDTH_RATIO: f64 = 0.55;

/// 上下段公差（[`SizeTolerance::Deviations`]）の上段と下段の間に空ける縦すきま ÷
/// 基準文字高さ。
///
/// 規定 5-12-2 2) は「上公差を上・下公差を下の上下段に置き、文字を 70% に縮小する」
/// ことだけを定め、行間の値は定めていない。上下段が触れ合わずに読め、かつ全高が
/// 膨らみすぎない値として 0.1 を採る（既定の文字高さ 3.5mm で 0.35mm）。
const DEVIATION_LINE_GAP_RATIO: f64 = 0.1;

/// 非比例寸法（[`DimAnnotation::value_override`]）の下線をベースラインから下げる量 ÷
/// 基準文字高さ。
///
/// **規定 5-10「非比例寸法の表し方」は見出しのみで本文が空**のため、JIS B 0001 の
/// 標準慣行（寸法数値の下に実線を 1 本引く）で実装している。規定側に本文が入った
/// 時点で、この定数と [`DimLabel::underline`] の生成規則を突き合わせること。
const UNDERLINE_DROP_RATIO: f64 = 0.15;

/// 組版済みラベルの中の、フォント文字として描く 1 まとまり。
///
/// `origin` は既存の [`TextGeom`](crate::TextGeom) と同じ**ベースライン左端**
/// アンカーなので、タスク49-2 はラベル全体の姿勢を掛けるだけで `TextGeom` へ移せる。
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// 描画する文字列。
    pub content: String,
    /// ベースライン左端（ラベルのローカル座標）。
    pub origin: Point2,
    /// 文字高さ（[`layout_dim_label`] の `text_height` と同じ単位）。
    /// 上下段公差だけは `text_height * DimStyle::tolerance_scale` になる。
    pub height: f64,
}

/// 組版済みの寸法ラベル。
///
/// 座標はすべて**ラベルのローカル座標**（原点 = 値のベースライン左端、+x = 読み方向、
/// +y = 上）で、長さの単位は [`layout_dim_label`] へ渡した `text_height` と同じ。
///
/// # 外形を [`Aabb`] で持つ理由
///
/// 上下段公差の下段と非比例寸法の下線は**ベースラインより下**へ出るため、原点は外形の
/// 左下とは限らない。幅・高さの 2 スカラーだけでは「原点から見て外形がどこにあるか」が
/// 失われ、タスク49-2 の中央寄せが下方向のはみ出しぶんだけずれる。幅・高さは
/// [`DimLabel::width`] / [`DimLabel::height`] で取れる。
///
/// 外形は**送り幅ベースの組版ボックス**（各要素について「送り幅 × 文字高さ」の矩形）で
/// あり、ストロークの実インクの境界ではない。行の中央寄せ・外向き矢印の自動判定は
/// インクではなく組版ボックスを基準にするのが正しいため。
#[derive(Debug, Clone, PartialEq)]
pub struct DimLabel {
    /// フォント文字として描く要素（値・英字記号・公差）。
    pub runs: Vec<TextRun>,
    /// ストロークとして描く記号（φ・□）。[`dim_symbol_glyph`] が返す形状を
    /// ラベルのローカル座標へ配置済み。
    pub strokes: Vec<Shape>,
    /// 非比例寸法（規定 5-10）の下線。`None` は下線なし。値の部分だけに掛かり、
    /// 記号・公差には掛からない。
    pub underline: Option<[Point2; 2]>,
    /// 外形（記号・値・公差・下線をすべて含む組版ボックス）。
    pub bounds: Aabb,
}

impl DimLabel {
    /// 外形の幅。
    #[must_use]
    pub fn width(&self) -> f64 {
        self.bounds.width()
    }

    /// 外形の高さ（上下段公差・下線を含む全高）。
    #[must_use]
    pub fn height(&self) -> f64 {
        self.bounds.height()
    }
}

/// 注記つきの寸法値を組版する。
///
/// `value` は座標から計算した実測値を呼び出し側が渡す（半径寸法なら半径、直径寸法なら
/// 直径 `2r`）。`text_height` は基準文字高さで、**出力はすべてこれと同じ単位**になる
/// （モジュール doc の「単位は抽象」を参照）。
///
/// # 組版規則（出典は `製図規定.md`）
///
/// - **桁数**（5-2 2)）: [`DimStyle::resolve_decimals`] で解決する。
///   [`DimAnnotation::decimals_override`] が `Some(n)` ならそれ、`None` なら
///   [`DimStyle::decimals`]（文書スタイルへの生きた参照）。
/// - **ゼロトリム**: [`DimStyle::trim_trailing_zeros`] が真なら末尾の 0 と、
///   結果として余る小数点を落とす（`120.00` → `120`、`12.50` → `12.5`）。
/// - **値の差し替え**（5-10）: [`DimAnnotation::value_override`] が `Some(s)` なら
///   数値の代わりに `s` を使い、値の幅ぶんの下線を引く。
/// - **寸法補助記号**（5-2 4)）: 数値の**左**へ置く。[`DimAnnotation::symbol`] が
///   `None` のときは寸法種別で補う（長さ = なし / 半径 = R / 直径 = φ。表示時の補完
///   であって core のデータには書かない）。
/// - **公差**（5-12）: 対称許容差は `± v` を同じ文字高さで値の右へ（5-12-2 1)）、
///   上下偏差は値の右に上下 2 段で [`DimStyle::tolerance_scale`] 倍に縮小して
///   （5-12-2 2)・3)）、はめあい記号は値の直後へ隙間なく連結して（5-12-3 2)）置く。
///
/// # 未知バリアントの扱い
///
/// [`SizeTolerance`] は `#[non_exhaustive]` だが、**この属性は定義元クレートの中では
/// 効かない**。組版が `mcad-app` にあった M10 まではワイルドカード腕で「公差なし」へ
/// 倒していたのに対し、M11 タスク71 で core へ移した後は網羅 `match` になり、
/// **表記方式を足すとここがコンパイルエラーになる**（情報が黙って落ちない、より強い
/// 保証。挙動は変わらない — 落としていた腕は到達不能だった）。
///
/// 一方 [`DimSymbol`] は `mcad-geom` の型なので `#[non_exhaustive]` が効き、追加された
/// 未対応の記号は**送り幅だけ確保して文字・ストロークを出さない**。黙って描画が壊れる
/// より保守的な既定だが、**追加時にここを更新しないと図面から情報が落ちる**。単体テスト
/// `every_known_symbol_is_typeset` が既知記号の取りこぼしを検出する。
#[must_use]
pub fn layout_dim_label(
    value: f64,
    annotation: &DimAnnotation,
    style: &DimStyle,
    kind: DimKind,
    text_height: f64,
) -> DimLabel {
    let decimals = style.resolve_decimals(annotation.decimals_override);
    let trim = style.trim_trailing_zeros;
    let mut builder = LabelBuilder::new();

    // 1) 寸法補助記号は数値の左（規定 5-2 4)）。
    if let Some(symbol) = annotation.symbol.or_else(|| default_symbol(kind)) {
        builder.push_symbol(symbol, text_height);
    }

    // 2) 値。非比例寸法は数値を差し替えて下線を引く（規定 5-10）。
    let value_x = builder.cursor;
    let value_text = match &annotation.value_override {
        Some(text) => text.clone(),
        None => format_number(value, decimals, trim),
    };
    let value_width = builder.push_run(value_text, text_height, 0.0);
    let mut underline = None;
    if annotation.value_override.is_some() {
        let y = -text_height * UNDERLINE_DROP_RATIO;
        builder.include(value_x, y, value_x + value_width, y);
        underline = Some([
            Point2::new(value_x, y),
            Point2::new(value_x + value_width, y),
        ]);
    }

    // 3) サイズ公差（規定 5-12）。
    if let Some(tolerance) = &annotation.tolerance {
        // 値と公差の間は ASCII 空白 1 文字ぶん空ける（規定 5-12-2 1) の記入例
        // `50 ± 0.5` の空白に相当。独自の定数を作らず文字幅係数をそのまま使う）。
        let space = ASCII_CHAR_WIDTH_RATIO * text_height;
        // 網羅 match（`#[non_exhaustive]` は定義元クレート内では効かない）。表記方式を
        // 足したらここを更新する（関数 doc 「未知バリアントの扱い」参照）。
        match tolerance {
            // 5-12-2 1): ± と公差値は寸法数値と同じ高さ。
            SizeTolerance::Symmetric(v) => {
                builder.advance(space);
                let text = format!("± {}", format_number(*v, decimals, trim));
                builder.push_run(text, text_height, 0.0);
            }
            // 5-12-2 2)・3): 上下段、公差文字は tolerance_scale 倍に縮小。
            SizeTolerance::Deviations { upper, lower } => {
                builder.advance(space);
                builder.push_deviations(
                    &format_deviation(*upper, decimals, trim),
                    &format_deviation(*lower, decimals, trim),
                    text_height,
                    style.tolerance_scale,
                );
            }
            // 5-12-3 2): はめあい記号は基準寸法の直後（φ20H7）。表示のみで偏差値へ
            // 展開しない（core の `FitClass` が検証済みの文字列をそのまま出す）。
            SizeTolerance::Fit(fit) => {
                builder.push_run(fit.as_str().to_string(), text_height, 0.0);
            }
        }
    }

    DimLabel {
        runs: builder.runs,
        strokes: builder.strokes,
        underline,
        bounds: builder.bounds,
    }
}

/// 記号が明示されていないとき（[`DimAnnotation::symbol`] が `None`）に、寸法種別から
/// 補う既定の記号。
///
/// 半径・直径寸法は記号がないと値の意味が決まらない（R12 と φ24 の取り違えを招く。
/// M8 までの `expand_radial` も `R` を値文字列へ焼き込んでいた）。長さ寸法は記号なしが
/// 既定。
///
/// **これは表示時の補完であり、core のデータへは書かない。** 書いてしまうと「未指定」と
/// 「明示的に R を選んだ」が区別できなくなり、`decimals_override` で避けたのと同じ
/// 「生きた参照が死ぬ」問題を起こす。
///
/// [`DimKind`] は `#[non_exhaustive]` ではない（種別追加をコンパイルエラーで検出する
/// 設計）ので、ここも網羅 `match` で書く。
fn default_symbol(kind: DimKind) -> Option<DimSymbol> {
    match kind {
        DimKind::Linear => None,
        DimKind::Radial => Some(DimSymbol::Radius),
        DimKind::Diameter => Some(DimSymbol::Diameter),
    }
}

/// フォント文字として組む記号の文字列。
///
/// ストロークを生成する記号（φ・□）と複合記号 Sφ は [`LabelBuilder::push_symbol`] が
/// 先に処理するので、ここへは来ない。`None` は「[`DimSymbol`] に追加されたが組版側が
/// 未対応」の意味（[`DimSymbol`] は `#[non_exhaustive]`）。
fn symbol_font_text(symbol: DimSymbol) -> Option<&'static str> {
    match symbol {
        DimSymbol::Radius => Some("R"),
        DimSymbol::SphereRadius => Some("SR"),
        DimSymbol::ControlRadius => Some("CR"),
        DimSymbol::Chamfer => Some("C"),
        DimSymbol::Thickness => Some("t"),
        _ => None,
    }
}

/// 数値を桁数 `decimals` で文字列化し、`trim_trailing_zeros` なら末尾の 0 と、
/// 結果として余る小数点を落とす（`120.00` → `120`、`12.50` → `12.5`）。
///
/// 小数点を含まないとき（`decimals` が 0）はトリムしない。`120` から末尾の 0 を
/// 落として `12` にしてしまわないための境界である。
fn format_number(value: f64, decimals: u8, trim_trailing_zeros: bool) -> String {
    let precision = usize::from(decimals);
    let mut text = format!("{value:.precision$}");
    if trim_trailing_zeros && text.contains('.') {
        let kept = text.trim_end_matches('0').trim_end_matches('.').len();
        text.truncate(kept);
    }
    text
}

/// 上下偏差の 1 段ぶんを符号つきで文字列化する（`+0.2` / `-0.1`）。
///
/// 桁数・ゼロトリムは寸法値と同じ解決結果を使う。**0 には符号を付けない**（片側公差
/// の記入例 `+0.3` / `0`。規定 5-12-2 3)）。判定は生値ではなく整形後の文字列で行う
/// ので、桁数の丸めで 0 になった値も `+0.00` ではなく `0.00` になる。
fn format_deviation(value: f64, decimals: u8, trim_trailing_zeros: bool) -> String {
    let magnitude = format_number(value.abs(), decimals, trim_trailing_zeros);
    if is_zero_text(&magnitude) {
        magnitude
    } else if value < 0.0 {
        format!("-{magnitude}")
    } else {
        format!("+{magnitude}")
    }
}

/// [`format_number`] の出力が 0 を表すか（`"0"` / `"0.00"`）。
fn is_zero_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '0' || c == '.')
}

/// 左から右へ 1 行ぶんを積み上げる組版カーソル。
///
/// 要素を置くたびに外形へ「送り幅 × 文字高さ」のセルを取り込むので、[`DimLabel::bounds`]
/// は常に実際に置いた内容と一致する（幅・高さを別計算しない）。
struct LabelBuilder {
    runs: Vec<TextRun>,
    strokes: Vec<Shape>,
    /// 次の要素を置く左端（ローカル x）。
    cursor: f64,
    bounds: Aabb,
}

impl LabelBuilder {
    fn new() -> Self {
        Self {
            runs: Vec::new(),
            strokes: Vec::new(),
            cursor: 0.0,
            // 原点（ベースライン左端）は常に外形に含まれる。最初の要素は x = 0 から
            // 始まり、ベースラインより上へ伸びるので、これで外形が膨らむことはない。
            bounds: Aabb::from_point(Point2::ORIGIN),
        }
    }

    /// 外形へ矩形セルを取り込む。
    fn include(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        let cell = Aabb::new(Point2::new(x0, y0), Point2::new(x1, y1));
        self.bounds = self.bounds.union(&cell);
    }

    /// 文字を伴わない送り（すきま）。
    fn advance(&mut self, dx: f64) {
        self.cursor += dx;
    }

    /// 文字列の近似幅（[`ASCII_CHAR_WIDTH_RATIO`] × 文字数 × 高さ）。
    fn run_width(content: &str, height: f64) -> f64 {
        content.chars().count() as f64 * ASCII_CHAR_WIDTH_RATIO * height
    }

    /// 送り位置を動かさずに 1 ランを置き、その幅を返す。
    fn place_run(&mut self, x: f64, content: String, height: f64, baseline: f64) -> f64 {
        let width = Self::run_width(&content, height);
        self.include(x, baseline, x + width, baseline + height);
        self.runs.push(TextRun {
            content,
            origin: Point2::new(x, baseline),
            height,
        });
        width
    }

    /// 現在の送り位置へ 1 ランを置き、その幅ぶん送る。戻り値はランの幅。
    fn push_run(&mut self, content: String, height: f64, baseline: f64) -> f64 {
        let width = self.place_run(self.cursor, content, height, baseline);
        self.cursor += width;
        width
    }

    /// 幅 `advance`・高さ `height` の記号セルを外形へ取り込み、送り位置を進める。
    fn reserve_symbol_cell(&mut self, advance: f64, height: f64) {
        self.include(self.cursor, 0.0, self.cursor + advance, height);
        self.cursor += advance;
    }

    /// ストローク記号を現在位置へ置き、[`SymbolGlyph::advance`] ぶん送る。
    fn push_symbol_strokes(&mut self, glyph: &SymbolGlyph, height: f64) {
        let delta = Vec2::new(self.cursor, 0.0);
        self.strokes
            .extend(glyph.shapes.iter().map(|shape| shape.translated(delta)));
        self.reserve_symbol_cell(glyph.advance, height);
    }

    /// フォント文字の記号を現在位置へ置き、`advance` ぶん送る。
    ///
    /// 送り幅を文字数から計算せず [`SymbolGlyph::advance`] に従うのは、記号の幅の
    /// 出所を mcad-geom 側へ一本化するためである（`ASCII_CHAR_WIDTH_RATIO` と
    /// `symbol::FONT_CHAR_ADVANCE_RATIO` は同じ量なので現状は一致するが、二重に
    /// 計算していると片方だけ変えたときに黙ってずれる）。外形はランの近似幅と
    /// `advance` の両方を取り込むので、仮に食い違っても内容を覆い続ける。
    fn push_symbol_text(&mut self, content: &str, advance: f64, height: f64) {
        self.place_run(self.cursor, content.to_string(), height, 0.0);
        self.reserve_symbol_cell(advance, height);
    }

    /// 寸法補助記号 1 つを現在位置へ組む。
    ///
    /// ストロークかフォント文字かの振り分けは **`match` の腕ではなく
    /// [`SymbolGlyph::shapes`] の空 / 非空**で行う（`DimSymbol` は
    /// `#[non_exhaustive]` なので、ワイルドカード腕でフォント文字へ倒すと将来
    /// ストローク生成が必要な記号が追加されたとき黙って崩れる。空 = 「フォント文字と
    /// して組め」は mcad-geom の `symbol` モジュール doc が定める契約）。
    fn push_symbol(&mut self, symbol: DimSymbol, height: f64) {
        let glyph = dim_symbol_glyph(symbol, height);
        if !glyph.shapes.is_empty() {
            self.push_symbol_strokes(&glyph, height);
            return;
        }
        if symbol == DimSymbol::SphereDiameter {
            // Sφ だけは「フォント文字 S + ストローク φ」の複合で、geom は shapes を
            // 空にしたまま advance に両者の合算を返す。S の送りをその合算から φ の
            // advance を引いて求めることで、合計を必ず geom の advance に一致させる。
            let phi = dim_symbol_glyph(DimSymbol::Diameter, height);
            let s_advance = (glyph.advance - phi.advance).max(0.0);
            self.push_symbol_text("S", s_advance, height);
            self.push_symbol_strokes(&phi, height);
            return;
        }
        match symbol_font_text(symbol) {
            Some(text) => self.push_symbol_text(text, glyph.advance, height),
            // 未対応の記号。文字は出せないが送りだけは geom の advance どおり確保し、
            // 外形（＝タスク49-2 の中央寄せ・矢印判定の入力）が実際の描画とずれない
            // ようにする。
            None => self.reserve_symbol_cell(glyph.advance, height),
        }
    }

    /// 上下段公差を現在位置へ組み、広いほうの幅ぶん送る（規定 5-12-2 2)）。
    ///
    /// 2 段は左揃えで、上下段の中間が寸法数値の中央高さ（`text_height / 2`）へ来るよう
    /// 対称に置く。上段・下段の文字高さはどちらも `text_height * tolerance_scale`。
    fn push_deviations(
        &mut self,
        upper: &str,
        lower: &str,
        text_height: f64,
        tolerance_scale: f64,
    ) {
        let height = text_height * tolerance_scale;
        let gap = text_height * DEVIATION_LINE_GAP_RATIO;
        let middle = text_height * 0.5;
        let x = self.cursor;
        let upper_width = self.place_run(x, upper.to_string(), height, middle + gap * 0.5);
        let lower_width = self.place_run(x, lower.to_string(), height, middle - gap * 0.5 - height);
        self.cursor += upper_width.max(lower_width);
    }
}
