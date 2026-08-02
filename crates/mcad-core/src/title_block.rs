//! 表題欄テンプレート（宣言的な純データ。DESIGN.md M8 設計判断3、製図規定 2-4）。
//!
//! # なぜ「データ」なのか
//!
//! 図面枠・表題欄はエンティティではなく、[`crate::SheetMeta`] からの派生描画である
//! （グリッドと同格。図形として選択・移動できない）。描画側が様式ごとに if 分岐を
//! 持つのではなく、**セル分割・項目バインド・文字高さの呼びを宣言したデータ**を
//! 読んで描くことで、標準様式もユーザー定義様式も同じ経路で扱える。
//!
//! この方針の副産物として、テンプレートは serde 可能な純値であり、tcad の
//! 「図枠を自分で作る」ゲーム要素からも再利用できる。
//!
//! # 座標系
//!
//! 寸法はすべて **紙 mm**。[`TitleBlockRow`] は上から下へ、[`TitleBlockCell`] は
//! 左から右へ並ぶ。表題欄自体の配置（図面の右下隅・輪郭線に接する。規定 2-4 の 1)）は
//! テンプレートではなく描画側が決める。
//!
//! # M8 のスコープ
//!
//! 標準様式 A/B/C（規定 2-4-1）を組み込み定数で提供し、ユーザー定義様式は
//! **データ構造のみ**を用意する（編集 UI は M8 スコープ外。DESIGN.md M8 設計判断3）。

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// 表題欄のセルに記入する項目（[`crate::TitleBlockFields`] や [`crate::SheetMeta`] 本体への
/// バインド先）。
///
/// [`TitleBlockField::Scale`] / [`TitleBlockField::PaperSize`] は
/// [`crate::TitleBlockFields`] ではなく [`crate::SheetMeta`] 本体から導出する
/// （尺度・用紙サイズを二重に持たないため。DESIGN.md M8 設計判断2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TitleBlockField {
    /// 図面名称。
    DrawingTitle,
    /// 図面番号。
    DrawingNumber,
    /// 尺度（[`crate::SheetMeta::scale`] から導出）。
    Scale,
    /// 投影法（第三角法／第一角法）。
    Projection,
    /// 作成者。
    Author,
    /// 作成日。
    Date,
    /// 用紙サイズ（[`crate::SheetMeta::paper`]・[`crate::SheetMeta::orientation`] から導出）。
    PaperSize,
    /// 改訂記号。
    Revision,
}

/// 表題欄の 1 セル。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitleBlockCell {
    /// セル幅（紙 mm）。
    pub width_mm: f64,
    /// 記入する項目。`None` は予備欄（空欄のままでよい。規定 2-4-1 の様式C）。
    pub bind: Option<TitleBlockField>,
    /// 記入する文字高さの呼び（紙 mm。規定 2-4-3 の e）。
    pub text_height_mm: f64,
}

impl TitleBlockCell {
    /// セルを作る。
    #[must_use]
    pub const fn new(width_mm: f64, bind: Option<TitleBlockField>, text_height_mm: f64) -> Self {
        Self {
            width_mm,
            bind,
            text_height_mm,
        }
    }
}

/// 表題欄の 1 段（上から下へ並ぶ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitleBlockRow {
    /// 段の高さ（紙 mm）。
    pub height_mm: f64,
    /// 段内のセル（左から右へ）。
    pub cells: Vec<TitleBlockCell>,
}

impl TitleBlockRow {
    /// 段を作る。
    #[must_use]
    pub fn new(height_mm: f64, cells: Vec<TitleBlockCell>) -> Self {
        Self { height_mm, cells }
    }

    /// セル幅の合計（紙 mm）。
    #[must_use]
    pub fn cells_width_mm(&self) -> f64 {
        self.cells.iter().map(|c| c.width_mm).sum()
    }
}

/// 表題欄の様式（セル分割 + 項目バインド + 文字高さの呼び）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitleBlockTemplate {
    /// 全体の幅（紙 mm）。
    pub width_mm: f64,
    /// 段（上から下へ）。
    pub rows: Vec<TitleBlockRow>,
}

/// 図面名称・図面番号の文字高さの呼び（規定 2-4-3 の e）。
const TITLE_TEXT_MM: f64 = 5.0;
/// その他の欄の文字高さの呼び（規定 2-4-3 の e）。
const FIELD_TEXT_MM: f64 = 3.5;

/// 標準様式A（規定 2-4-1）。幅 170mm × 高さ 30mm、2 段。
static STANDARD_A: LazyLock<TitleBlockTemplate> = LazyLock::new(|| TitleBlockTemplate {
    width_mm: 170.0,
    rows: vec![
        TitleBlockRow::new(
            15.0,
            vec![
                TitleBlockCell::new(100.0, Some(TitleBlockField::DrawingTitle), TITLE_TEXT_MM),
                TitleBlockCell::new(70.0, Some(TitleBlockField::DrawingNumber), TITLE_TEXT_MM),
            ],
        ),
        TitleBlockRow::new(
            15.0,
            vec![
                TitleBlockCell::new(40.0, Some(TitleBlockField::Author), FIELD_TEXT_MM),
                TitleBlockCell::new(30.0, Some(TitleBlockField::Date), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::Scale), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::Projection), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::PaperSize), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::Revision), FIELD_TEXT_MM),
            ],
        ),
    ],
});

/// 標準様式B（規定 2-4-1、A4 推奨）。幅 120mm × 高さ 25mm、2 段。
static STANDARD_B: LazyLock<TitleBlockTemplate> = LazyLock::new(|| TitleBlockTemplate {
    width_mm: 120.0,
    rows: vec![
        TitleBlockRow::new(
            12.5,
            vec![
                TitleBlockCell::new(70.0, Some(TitleBlockField::DrawingTitle), TITLE_TEXT_MM),
                TitleBlockCell::new(50.0, Some(TitleBlockField::DrawingNumber), TITLE_TEXT_MM),
            ],
        ),
        TitleBlockRow::new(
            12.5,
            vec![
                TitleBlockCell::new(25.0, Some(TitleBlockField::Author), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::Date), FIELD_TEXT_MM),
                TitleBlockCell::new(20.0, Some(TitleBlockField::Scale), FIELD_TEXT_MM),
                TitleBlockCell::new(20.0, Some(TitleBlockField::Projection), FIELD_TEXT_MM),
                TitleBlockCell::new(15.0, Some(TitleBlockField::PaperSize), FIELD_TEXT_MM),
                TitleBlockCell::new(15.0, Some(TitleBlockField::Revision), FIELD_TEXT_MM),
            ],
        ),
    ],
});

/// 標準様式C（規定 2-4-1、拡張）。幅 170mm × 高さ 40mm、3 段。予備欄は空欄でよい。
static STANDARD_C: LazyLock<TitleBlockTemplate> = LazyLock::new(|| TitleBlockTemplate {
    width_mm: 170.0,
    rows: vec![
        TitleBlockRow::new(
            15.0,
            vec![TitleBlockCell::new(
                170.0,
                Some(TitleBlockField::DrawingTitle),
                TITLE_TEXT_MM,
            )],
        ),
        TitleBlockRow::new(
            12.5,
            vec![
                TitleBlockCell::new(70.0, Some(TitleBlockField::DrawingNumber), TITLE_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::PaperSize), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::Revision), FIELD_TEXT_MM),
                TitleBlockCell::new(50.0, None, FIELD_TEXT_MM),
            ],
        ),
        TitleBlockRow::new(
            12.5,
            vec![
                TitleBlockCell::new(40.0, Some(TitleBlockField::Author), FIELD_TEXT_MM),
                TitleBlockCell::new(30.0, Some(TitleBlockField::Date), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::Scale), FIELD_TEXT_MM),
                TitleBlockCell::new(25.0, Some(TitleBlockField::Projection), FIELD_TEXT_MM),
                TitleBlockCell::new(50.0, None, FIELD_TEXT_MM),
            ],
        ),
    ],
});

impl TitleBlockTemplate {
    /// 標準様式A（幅 170mm × 高さ 30mm、2 段）。
    #[must_use]
    pub fn standard_a() -> &'static TitleBlockTemplate {
        &STANDARD_A
    }

    /// 標準様式B（幅 120mm × 高さ 25mm、2 段。A4 推奨）。
    #[must_use]
    pub fn standard_b() -> &'static TitleBlockTemplate {
        &STANDARD_B
    }

    /// 標準様式C（幅 170mm × 高さ 40mm、3 段）。
    #[must_use]
    pub fn standard_c() -> &'static TitleBlockTemplate {
        &STANDARD_C
    }

    /// 全体の高さ（段の高さの合計、紙 mm）。
    #[must_use]
    pub fn height_mm(&self) -> f64 {
        self.rows.iter().map(|r| r.height_mm).sum()
    }

    /// テンプレートが描画・出力に使える形か検証する。
    ///
    /// 検証するのは次の 3 つ:
    ///
    /// 1. **個々の寸法とその合計**が「有限・正・[`MAX_TITLE_BLOCK_MM`] 以下」であること
    /// 2. **要素数**が [`MAX_TITLE_BLOCK_ROWS`] / [`MAX_TITLE_BLOCK_CELLS_PER_ROW`] 以下であること
    /// 3. **構造の最小条件**（段が 1 つ以上、各段にセルが 1 つ以上）を満たすこと
    ///
    /// セル幅の合計が [`TitleBlockTemplate::width_mm`] と一致することまでは求めない
    /// （端を空けた様式を禁じる理由がないため。標準様式が一致することは本モジュールの
    /// テストで固定する）。
    ///
    /// この検証がある理由は [`crate::Scale`] / [`crate::WidthMm`] と同じで、手編集された
    /// `.mcad` 由来のユーザー定義様式が描画・SVG/PDF 座標へ NaN・ゼロ寸法・巨大値を
    /// 伝播させないようにするため。個々の値が有限でも、合計（[`TitleBlockTemplate::height_mm`] /
    /// [`TitleBlockRow::cells_width_mm`]）は無限大へあふれうるので、**合計側も検証する**
    /// （Codex M8 アドバーサリアルレビュー(2026-08-02) medium 指摘）。要素数を別途
    /// 検証するのは、寸法の上限だけでは極小値（1e-6mm など）を大量に並べる細工を
    /// 防げないため（同 2 周目 medium 指摘。詳細は [`MAX_TITLE_BLOCK_ROWS`] の doc）。
    ///
    /// # Errors
    ///
    /// 不正の理由を人が読める `String` で返す。
    pub fn validate(&self) -> Result<(), String> {
        check_paper_dim(self.width_mm, format_args!("title block width"))?;
        if self.rows.is_empty() {
            return Err("title block has no rows".to_owned());
        }
        if self.rows.len() > MAX_TITLE_BLOCK_ROWS {
            return Err(format!(
                "title block has too many rows: {} (max {MAX_TITLE_BLOCK_ROWS})",
                self.rows.len()
            ));
        }
        for (r, row) in self.rows.iter().enumerate() {
            check_paper_dim(row.height_mm, format_args!("height in row {r}"))?;
            if row.cells.is_empty() {
                return Err(format!("row {r} has no cells"));
            }
            if row.cells.len() > MAX_TITLE_BLOCK_CELLS_PER_ROW {
                return Err(format!(
                    "row {r} has too many cells: {} (max {MAX_TITLE_BLOCK_CELLS_PER_ROW})",
                    row.cells.len()
                ));
            }
            for (c, cell) in row.cells.iter().enumerate() {
                check_paper_dim(cell.width_mm, format_args!("width in cell {r}/{c}"))?;
                check_paper_dim(
                    cell.text_height_mm,
                    format_args!("text height in cell {r}/{c}"),
                )?;
            }
            // 個々のセル幅が上限内でも、段あたりの合計は上限を超えうる。
            check_paper_dim(
                row.cells_width_mm(),
                format_args!("total cell width in row {r}"),
            )?;
        }
        // 同じく、段の高さの合計も上限内であることを要求する。
        check_paper_dim(self.height_mm(), format_args!("total title block height"))?;
        Ok(())
    }
}

/// 表題欄の寸法（および合計）として許す最大値 [mm]。
///
/// 用紙の最大は A0 の 1189mm なので、実用上の様式は 2 桁の余裕をもって収まる。
/// にもかかわらず `f64::MAX` の 10^300 倍以上小さいので、**この上限を個々の値へ
/// 課すだけで合計のオーバーフローが構造的に起きなくなる**（あふれさせるには
/// 10^304 段が必要）。「用紙に収まらない様式を早期に弾く」と「非有限値を後段の
/// 配置計算・SVG/PDF 座標へ流さない」を 1 つの定数で兼ねるための値
/// （Codex M8 アドバーサリアルレビュー(2026-08-02) medium 指摘）。
pub const MAX_TITLE_BLOCK_MM: f64 = 10_000.0;

/// 表題欄が持てる段数の上限。
///
/// 寸法の上限（[`MAX_TITLE_BLOCK_MM`]）だけでは、極小値（高さ 1e-6mm など）の段を
/// 大量に並べれば合計を上限以下に保ったまま任意個数の要素を通せてしまう。表題欄は
/// **毎フレーム全要素を走査して描く**派生描画なので、細工された `.mcad` が持続的な
/// GUI 停止やメモリ消費を起こしうる（Codex M8 アドバーサリアルレビュー 2 周目
/// (2026-08-02) medium 指摘）。
///
/// 値 32 の根拠: 標準様式は最大 3 段・6 セル（規定 2-4-1）なので、凝った
/// ユーザー定義様式にも 2 桁の余裕がありながら、走査コストを構造的に定数へ抑えられる。
///
/// **deserialize 時の割当上限（カスタム `Deserialize`）は設けない。** 割当は入力ファイル
/// サイズに比例する一過性のコストで任意の JSON パースと同等であり、持続的なコスト
/// （毎フレーム走査）のほうは `Command::SetSheet` と v4 読込の両経路が
/// [`TitleBlockTemplate::validate`] を先に通すことで断てるため。
pub const MAX_TITLE_BLOCK_ROWS: usize = 32;

/// 表題欄の 1 段が持てるセル数の上限。値と根拠は [`MAX_TITLE_BLOCK_ROWS`] と同じ。
pub const MAX_TITLE_BLOCK_CELLS_PER_ROW: usize = 32;

/// 紙寸法 1 つ（個々の値でも合計でもよい）が有限・正・[`MAX_TITLE_BLOCK_MM`] 以下かを
/// 検証する。`what` は失敗時のメッセージに使う説明で、成功時は文字列化しない
/// （`format_args!` を渡すため検証が通る限り確保が起きない）。
fn check_paper_dim(value: f64, what: std::fmt::Arguments<'_>) -> Result<(), String> {
    if !(value.is_finite() && value > 0.0) {
        return Err(format!("invalid {what}: {value}"));
    }
    if value > MAX_TITLE_BLOCK_MM {
        return Err(format!(
            "{what} exceeds the {MAX_TITLE_BLOCK_MM} mm limit: {value}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 規定 2-4 の 2) が要求する基本項目（尺度・用紙サイズを含む）。
    const REQUIRED_FIELDS: [TitleBlockField; 8] = [
        TitleBlockField::DrawingTitle,
        TitleBlockField::DrawingNumber,
        TitleBlockField::Scale,
        TitleBlockField::Projection,
        TitleBlockField::Author,
        TitleBlockField::Date,
        TitleBlockField::PaperSize,
        TitleBlockField::Revision,
    ];

    fn bound_fields(t: &TitleBlockTemplate) -> Vec<TitleBlockField> {
        t.rows
            .iter()
            .flat_map(|r| r.cells.iter())
            .filter_map(|c| c.bind)
            .collect()
    }

    #[test]
    fn standard_templates_match_the_drafting_rules_dimensions() {
        // 規定 2-4-1 の寸法（幅 × 高さ、段数）。
        for (t, width, height, rows) in [
            (TitleBlockTemplate::standard_a(), 170.0, 30.0, 2),
            (TitleBlockTemplate::standard_b(), 120.0, 25.0, 2),
            (TitleBlockTemplate::standard_c(), 170.0, 40.0, 3),
        ] {
            assert_eq!(t.width_mm, width);
            assert!((t.height_mm() - height).abs() < 1e-9, "height mismatch");
            assert_eq!(t.rows.len(), rows);
            // 各段のセル幅の合計は全体幅に一致する（規定の欄割り）。
            for (i, row) in t.rows.iter().enumerate() {
                assert!(
                    (row.cells_width_mm() - width).abs() < 1e-9,
                    "row {i} cells sum to {} (expected {width})",
                    row.cells_width_mm()
                );
            }
            assert_eq!(t.validate(), Ok(()));
        }
    }

    #[test]
    fn standard_templates_bind_all_required_fields_exactly_once() {
        // 規定 2-4 の 2): 基本項目をすべて含む（重複もしない）。
        for t in [
            TitleBlockTemplate::standard_a(),
            TitleBlockTemplate::standard_b(),
            TitleBlockTemplate::standard_c(),
        ] {
            let bound = bound_fields(t);
            for field in REQUIRED_FIELDS {
                assert_eq!(
                    bound.iter().filter(|f| **f == field).count(),
                    1,
                    "{field:?} must appear exactly once"
                );
            }
            assert_eq!(bound.len(), REQUIRED_FIELDS.len());
        }
    }

    #[test]
    fn standard_templates_use_the_specified_text_heights() {
        // 規定 2-4-3 の e): 図面名称・図面番号は呼び5、その他は呼び3.5。
        for t in [
            TitleBlockTemplate::standard_a(),
            TitleBlockTemplate::standard_b(),
            TitleBlockTemplate::standard_c(),
        ] {
            for cell in t.rows.iter().flat_map(|r| r.cells.iter()) {
                let expected = match cell.bind {
                    Some(TitleBlockField::DrawingTitle | TitleBlockField::DrawingNumber) => 5.0,
                    _ => 3.5,
                };
                assert_eq!(cell.text_height_mm, expected, "{cell:?}");
            }
        }
    }

    #[test]
    fn standard_c_has_spare_cells() {
        // 様式C の予備欄（バインドなし）は 2 つ（規定 2-4-1）。
        let spares = TitleBlockTemplate::standard_c()
            .rows
            .iter()
            .flat_map(|r| r.cells.iter())
            .filter(|c| c.bind.is_none())
            .count();
        assert_eq!(spares, 2);
    }

    #[test]
    fn validate_rejects_non_finite_and_non_positive_dimensions() {
        let base = TitleBlockTemplate::standard_b().clone();

        let mut bad_width = base.clone();
        bad_width.width_mm = f64::NAN;
        assert!(bad_width.validate().is_err());

        let mut zero_height = base.clone();
        zero_height.rows[0].height_mm = 0.0;
        assert!(zero_height.validate().is_err());

        let mut negative_cell = base.clone();
        negative_cell.rows[1].cells[0].width_mm = -5.0;
        assert!(negative_cell.validate().is_err());

        let mut bad_text = base;
        bad_text.rows[0].cells[0].text_height_mm = f64::INFINITY;
        assert!(bad_text.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_template_and_empty_rows() {
        // 構造の最小条件: 段が 1 つ以上、各段にセルが 1 つ以上。空のテンプレートは
        // 表題欄として意味を持たない。
        let base = TitleBlockTemplate::standard_b().clone();

        let mut no_rows = base.clone();
        no_rows.rows.clear();
        let err = no_rows.validate().unwrap_err();
        assert!(err.contains("no rows"), "{err}");

        let mut no_cells = base;
        no_cells.rows[0].cells.clear();
        let err = no_cells.validate().unwrap_err();
        assert!(err.contains("no cells"), "{err}");
    }

    /// `rows` 段 × `cells` セルの、寸法としては妥当なテンプレートを作る
    /// （件数上限だけを検証したいときに使う）。
    fn template_with_counts(rows: usize, cells: usize) -> TitleBlockTemplate {
        TitleBlockTemplate {
            width_mm: 170.0,
            rows: (0..rows)
                .map(|_| {
                    TitleBlockRow::new(
                        1.0,
                        (0..cells)
                            .map(|_| {
                                TitleBlockCell::new(1.0, Some(TitleBlockField::DrawingTitle), 3.5)
                            })
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn validate_rejects_too_many_rows_or_cells() {
        // 寸法の上限だけでは、極小値を大量に並べる細工（毎フレーム走査コストの
        // 増大）を防げない。件数の上限で構造的に断つ。
        assert_eq!(
            template_with_counts(MAX_TITLE_BLOCK_ROWS, MAX_TITLE_BLOCK_CELLS_PER_ROW).validate(),
            Ok(()),
            "上限ちょうどは受理されるべき"
        );

        let too_many_rows = template_with_counts(MAX_TITLE_BLOCK_ROWS + 1, 1);
        let err = too_many_rows.validate().unwrap_err();
        assert!(err.contains("too many rows"), "{err}");

        let too_many_cells = template_with_counts(1, MAX_TITLE_BLOCK_CELLS_PER_ROW + 1);
        let err = too_many_cells.validate().unwrap_err();
        assert!(err.contains("too many cells"), "{err}");

        // 極小値で合計を上限以下に保っても、件数で弾かれる（回帰の本体）。
        let mut tiny = template_with_counts(MAX_TITLE_BLOCK_ROWS + 1, 1);
        for row in &mut tiny.rows {
            row.height_mm = 1e-6;
            row.cells[0].width_mm = 1e-6;
        }
        assert!(
            tiny.height_mm() < MAX_TITLE_BLOCK_MM,
            "テスト前提: 合計は上限内"
        );
        assert!(tiny.validate().is_err());
    }

    #[test]
    fn validate_rejects_huge_but_finite_dimensions() {
        // 個々には有限でも上限を超える値は弾く（f64::MAX のような値が後段の
        // 配置計算・SVG/PDF 座標へ届かないようにする）。
        let base = TitleBlockTemplate::standard_b().clone();

        let mut huge_width = base.clone();
        huge_width.width_mm = f64::MAX;
        assert!(huge_width.validate().is_err());

        let mut huge_cell = base.clone();
        huge_cell.rows[0].cells[0].width_mm = f64::MAX;
        assert!(huge_cell.validate().is_err());

        let mut huge_height = base.clone();
        huge_height.rows[0].height_mm = MAX_TITLE_BLOCK_MM + 1.0;
        assert!(huge_height.validate().is_err());

        let mut huge_text = base.clone();
        huge_text.rows[0].cells[0].text_height_mm = f64::MAX;
        assert!(huge_text.validate().is_err());

        // 上限ちょうどは受理する（境界の向きを固定する）。
        let mut at_limit = base;
        at_limit.width_mm = MAX_TITLE_BLOCK_MM;
        at_limit.rows[0].cells[0].width_mm = MAX_TITLE_BLOCK_MM;
        at_limit.rows[0].cells.truncate(1);
        assert_eq!(at_limit.validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_sums_over_the_limit() {
        // 個々の値が上限内でも、合計が上限を超える構成は弾く。個々の上限が
        // MAX_TITLE_BLOCK_MM である以上、合計が f64 であふれるには 10^304 段が
        // 必要なので、実際に守っているのは「合計も紙寸法として妥当」という条件。
        let big = MAX_TITLE_BLOCK_MM * 0.9;

        // 段あたりのセル幅合計が上限超え。
        let wide_row = TitleBlockTemplate {
            width_mm: 170.0,
            rows: vec![TitleBlockRow::new(
                15.0,
                vec![
                    TitleBlockCell::new(big, Some(TitleBlockField::DrawingTitle), 5.0),
                    TitleBlockCell::new(big, Some(TitleBlockField::DrawingNumber), 5.0),
                ],
            )],
        };
        let err = wide_row.validate().unwrap_err();
        assert!(err.contains("total cell width"), "{err}");

        // 段の高さの合計が上限超え。
        let tall = TitleBlockTemplate {
            width_mm: 170.0,
            rows: (0..2)
                .map(|_| {
                    TitleBlockRow::new(
                        big,
                        vec![TitleBlockCell::new(
                            170.0,
                            Some(TitleBlockField::DrawingTitle),
                            5.0,
                        )],
                    )
                })
                .collect(),
        };
        let err = tall.validate().unwrap_err();
        assert!(err.contains("total title block height"), "{err}");
    }

    #[test]
    fn template_round_trips_through_json() {
        // ユーザー定義様式は `.mcad` へ書かれるので serde 往復が要件。
        let json = serde_json::to_string(TitleBlockTemplate::standard_c()).unwrap();
        let back: TitleBlockTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, TitleBlockTemplate::standard_c());
    }
}
