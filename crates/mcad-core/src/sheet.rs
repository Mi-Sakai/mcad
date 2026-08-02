//! 図面メタデータ（単位・尺度・用紙・表題欄）。DESIGN.md M8 設計判断1・2。
//!
//! # 単位系
//!
//! **1 ワールド単位 = 1mm** と宣言する（内部変換なし）。[`Unit`] はその宣言を
//! ファイルへ記録するためのもので、内部単位を切り替える仕組みではない。
//!
//! # 尺度の換算
//!
//! [`Scale`] は **紙:モデル**（例 1:2 = 紙 1mm がモデル 2mm）。換算係数は
//! `k = den / num`（[`Scale::world_mm_per_paper_mm`]）で、紙上 x mm の注記は
//! ワールド `x * k` になる。
//!
//! # 紙空間は持たない
//!
//! 用紙は「モデル空間上の枠」として原点 `(0,0)` に用紙左下を置いた矩形で表示する
//! （`M8を始める前に.md` 6.1）。複数レイアウト・ビューポートは v1.0 後。

use serde::{Deserialize, Serialize};

use crate::CoreError;
use crate::title_block::TitleBlockTemplate;

/// 図面の長さ単位。
///
/// 現状は mm のみ（内部単位は常に mm）。inch 等は将来の規格プロファイルで
/// **表示・入力側に**足す想定であり、内部単位は変えない（DESIGN.md M8 設計判断1）。
/// そのため `#[non_exhaustive]` にして、単位追加が下流の網羅 `match` を壊さないようにする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Unit {
    /// ミリメートル。
    #[default]
    Millimeter,
}

/// [`Scale`] の分子・分母が取りうる最大値。
///
/// `k = den / num` の演算が f64 で安全な範囲に収まるようにするための上限
/// （DESIGN.md M8 設計判断2）。実用上の尺度（規定第3章は最大 1:10000）から見て
/// 十分に余裕がある。
pub const MAX_SCALE_TERM: u32 = 1_000_000;

/// 図面尺度（**紙:モデル**、既定 1:1）。
///
/// 中間尺度も整数比で表現する（1:2.5 = 2:5）。既約化は行わない（2:5 と 4:10 を
/// 同一視する必要が UI 上なく、表示は入力どおりを保つ）。
///
/// # 不変条件
///
/// `num > 0 && den > 0` かつ両者 `<=` [`MAX_SCALE_TERM`]。`num = 0` は `k` が無限大に、
/// `den = 0` は注記・線幅・枠がゼロ寸法になり、描画や SVG/PDF 座標へ非有限値が
/// 伝播する。フィールドは非公開で、生成は検証済みコンストラクタ [`Scale::new`] のみ。
/// serde も透過 derive ではなく `try_from` 経由なので、**手編集された `.mcad` の
/// 不正値は読込境界で拒否**される。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "ScaleRepr")]
pub struct Scale {
    /// 紙側の項（分子）。
    num: u32,
    /// モデル側の項（分母）。
    den: u32,
}

/// [`Scale`] の読込用の素の表現（検証前）。
///
/// `Scale` 自身のフィールドは非公開なので、`serde(try_from)` の入力型として
/// 同じ形の素の構造体を経由し、[`Scale::new`] の検証を必ず通す。
#[derive(Deserialize)]
struct ScaleRepr {
    num: u32,
    den: u32,
}

impl Scale {
    /// 現尺 1:1。
    pub const ONE: Scale = Scale { num: 1, den: 1 };

    /// 尺度を検証して作る。
    ///
    /// # Errors
    ///
    /// `num` または `den` が `0`、あるいは [`MAX_SCALE_TERM`] を超える場合に
    /// [`CoreError::InvalidScale`] を返す。
    pub fn new(num: u32, den: u32) -> Result<Self, CoreError> {
        if num == 0 || den == 0 {
            return Err(CoreError::InvalidScale(format!(
                "scale terms must be positive: {num}:{den}"
            )));
        }
        if num > MAX_SCALE_TERM || den > MAX_SCALE_TERM {
            return Err(CoreError::InvalidScale(format!(
                "scale term must not exceed {MAX_SCALE_TERM}: {num}:{den}"
            )));
        }
        Ok(Self { num, den })
    }

    /// 紙側の項（分子）。
    #[inline]
    #[must_use]
    pub const fn num(self) -> u32 {
        self.num
    }

    /// モデル側の項（分母）。
    #[inline]
    #[must_use]
    pub const fn den(self) -> u32 {
        self.den
    }

    /// 紙 1mm あたりのワールド mm（`k = den / num`）。
    ///
    /// 紙基準の量（文字高さ・線幅・破線パターン・枠）をワールド長へ直すときに掛ける。
    /// 不変条件により常に有限の正値。
    #[inline]
    #[must_use]
    pub fn world_mm_per_paper_mm(self) -> f64 {
        f64::from(self.den) / f64::from(self.num)
    }

    /// ワールド 1mm あたりの紙 mm（`1 / k = num / den`）。
    ///
    /// モデル図形の座標を紙 mm（SVG/PDF 出力）へ直すときに掛ける。
    #[inline]
    #[must_use]
    pub fn paper_mm_per_world_mm(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }
}

impl Default for Scale {
    /// 現尺 1:1（[`Scale::ONE`]）。
    #[inline]
    fn default() -> Self {
        Self::ONE
    }
}

impl TryFrom<ScaleRepr> for Scale {
    type Error = CoreError;

    #[inline]
    fn try_from(repr: ScaleRepr) -> Result<Self, Self::Error> {
        Scale::new(repr.num, repr.den)
    }
}

/// 用紙サイズ（A 列。規定 2-1。延長サイズは用いない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PaperSize {
    /// 841 × 1189 mm。
    A0,
    /// 594 × 841 mm。
    A1,
    /// 420 × 594 mm。
    A2,
    /// 297 × 420 mm。
    A3,
    /// 210 × 297 mm。
    #[default]
    A4,
}

impl PaperSize {
    /// 短辺 [mm]。
    #[must_use]
    pub const fn short_mm(self) -> f64 {
        match self {
            PaperSize::A0 => 841.0,
            PaperSize::A1 => 594.0,
            PaperSize::A2 => 420.0,
            PaperSize::A3 => 297.0,
            PaperSize::A4 => 210.0,
        }
    }

    /// 長辺 [mm]。
    #[must_use]
    pub const fn long_mm(self) -> f64 {
        match self {
            PaperSize::A0 => 1189.0,
            PaperSize::A1 => 841.0,
            PaperSize::A2 => 594.0,
            PaperSize::A3 => 420.0,
            PaperSize::A4 => 297.0,
        }
    }
}

/// 用紙の向き（規定 2-2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Orientation {
    /// 長辺を横方向に用いる（X 型）。既定。
    #[default]
    Landscape,
    /// 長辺を縦方向に用いる（Y 型。規定 2-2 では A4 に限り許容）。
    Portrait,
}

/// 投影法（規定第7章。第三角法が標準）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ProjectionMethod {
    /// 第三角法。
    #[default]
    ThirdAngle,
    /// 第一角法。
    FirstAngle,
}

/// 表題欄の様式（規定 2-4-1 の標準様式 A/B/C とユーザー定義様式）。
///
/// [`TitleBlockKind::Custom`] は様式そのもの（[`TitleBlockTemplate`]）を保持する。
/// M8 で実装するのは **データ構造まで**で、編集 UI はスコープ外
/// （DESIGN.md M8 設計判断3）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum TitleBlockKind {
    /// 標準様式A（170 × 30mm）。
    A,
    /// 標準様式B（120 × 25mm）。A4 推奨（規定 2-4 の 4)）なので既定。
    #[default]
    B,
    /// 標準様式C（170 × 40mm）。
    C,
    /// ユーザー定義様式（規定 2-4-2）。
    Custom(TitleBlockTemplate),
}

impl TitleBlockKind {
    /// この様式のテンプレート（セル分割・項目バインド・文字高さ）。
    #[must_use]
    pub fn template(&self) -> &TitleBlockTemplate {
        match self {
            TitleBlockKind::A => TitleBlockTemplate::standard_a(),
            TitleBlockKind::B => TitleBlockTemplate::standard_b(),
            TitleBlockKind::C => TitleBlockTemplate::standard_c(),
            TitleBlockKind::Custom(template) => template,
        }
    }
}

/// 表題欄の記入内容（規定 2-4 の 2) の基本項目のうち、図面ごとに人が書くもの）。
///
/// **尺度・用紙サイズは含まない**。どちらも [`SheetMeta`] 本体が持つ値から導出し、
/// 二重に保持しない（DESIGN.md M8 設計判断2）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TitleBlockFields {
    /// 図面番号（体系は別プロジェクトの図面管理システムが定めるまで自由記入。
    /// 規定 2-4-3 の d)）。
    pub drawing_number: String,
    /// 図面名称。
    pub drawing_title: String,
    /// 投影法（規定 2-4-3 の b) により当面は文字で記入する）。
    pub projection: ProjectionMethod,
    /// 作成者。
    pub author: String,
    /// 作成日（書式は規定していないので文字列で持つ）。
    pub date: String,
    /// 改訂記号。最新の改訂記号のみを記入する（規定 2-4-3 の c)）。
    pub revision: String,
}

/// 図面メタデータ。[`crate::Document`] が 1 つ保持し、変更は
/// [`crate::Command::SetSheet`] 経由でのみ行う（undo 対象）。
///
/// 既定は **A4 横・1:1・様式B・枠非表示**（DESIGN.md M8 設計判断6。v1〜v3 の
/// `.mcad` を読み込んだときもこの既定になる）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SheetMeta {
    /// 図面の単位（現状 mm 固定。記録用）。
    pub unit: Unit,
    /// 図面尺度（紙:モデル）。
    pub scale: Scale,
    /// 用紙サイズ。
    pub paper: PaperSize,
    /// 用紙の向き。
    pub orientation: Orientation,
    /// 表題欄の様式。
    pub title_block: TitleBlockKind,
    /// 表題欄の記入内容。
    pub fields: TitleBlockFields,
    /// 図面枠・表題欄を表示するか。
    pub frame_visible: bool,
}

impl SheetMeta {
    /// 用紙の寸法 `(幅, 高さ)` [mm]（向きを反映した紙上の寸法）。
    #[must_use]
    pub fn paper_extent_mm(&self) -> (f64, f64) {
        match self.orientation {
            Orientation::Landscape => (self.paper.long_mm(), self.paper.short_mm()),
            Orientation::Portrait => (self.paper.short_mm(), self.paper.long_mm()),
        }
    }

    /// メタデータが描画・出力に使える形か検証する。
    ///
    /// [`Scale`] は型の不変条件で常に正しいので、実質的に検証するのは
    /// [`TitleBlockKind::Custom`] のテンプレート寸法だけ（標準様式 A/B/C は定数で
    /// 常に妥当）。手編集された `.mcad` 由来のユーザー定義様式が非有限・ゼロ寸法を
    /// 描画や SVG/PDF 座標へ持ち込むのを、[`crate::Document`] のコマンド境界で防ぐ。
    ///
    /// # Errors
    ///
    /// 不正の理由を人が読める `String` で返す。
    pub fn validate(&self) -> Result<(), String> {
        self.title_block.template().validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_defaults_to_one_to_one() {
        let s = Scale::default();
        assert_eq!(s, Scale::ONE);
        assert_eq!((s.num(), s.den()), (1, 1));
        assert!((s.world_mm_per_paper_mm() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_conversion_factors() {
        // 1:2（縮尺）→ 紙 1mm はワールド 2mm。
        let half = Scale::new(1, 2).unwrap();
        assert!((half.world_mm_per_paper_mm() - 2.0).abs() < 1e-12);
        assert!((half.paper_mm_per_world_mm() - 0.5).abs() < 1e-12);

        // 2:1（倍尺）→ 紙 1mm はワールド 0.5mm。
        let twice = Scale::new(2, 1).unwrap();
        assert!((twice.world_mm_per_paper_mm() - 0.5).abs() < 1e-12);

        // 中間尺度 1:2.5 は整数比 2:5 で表す。
        let mid = Scale::new(2, 5).unwrap();
        assert!((mid.world_mm_per_paper_mm() - 2.5).abs() < 1e-12);
    }

    #[test]
    fn scale_rejects_zero_terms_and_oversized_terms() {
        for (num, den) in [(0, 1), (1, 0), (0, 0)] {
            assert!(
                matches!(Scale::new(num, den), Err(CoreError::InvalidScale(_))),
                "{num}:{den} should be rejected"
            );
        }
        assert!(Scale::new(MAX_SCALE_TERM, MAX_SCALE_TERM).is_ok());
        assert!(matches!(
            Scale::new(MAX_SCALE_TERM + 1, 1),
            Err(CoreError::InvalidScale(_))
        ));
        assert!(matches!(
            Scale::new(1, MAX_SCALE_TERM + 1),
            Err(CoreError::InvalidScale(_))
        ));
    }

    #[test]
    fn scale_is_not_reduced() {
        // 既約化しない（2:5 と 4:10 は別の値として保持され、表示は入力どおり）。
        let a = Scale::new(2, 5).unwrap();
        let b = Scale::new(4, 10).unwrap();
        assert_ne!(a, b);
        assert!((a.world_mm_per_paper_mm() - b.world_mm_per_paper_mm()).abs() < 1e-12);
    }

    #[test]
    fn scale_serde_rejects_invalid_values_at_the_read_boundary() {
        let json = serde_json::to_string(&Scale::new(2, 5).unwrap()).unwrap();
        assert_eq!(json, r#"{"num":2,"den":5}"#);
        assert_eq!(
            serde_json::from_str::<Scale>(&json).unwrap(),
            Scale::new(2, 5).unwrap()
        );
        // 手編集された不正値は読込境界（try_from）で拒否される。
        assert!(serde_json::from_str::<Scale>(r#"{"num":0,"den":1}"#).is_err());
        assert!(serde_json::from_str::<Scale>(r#"{"num":1,"den":0}"#).is_err());
        assert!(serde_json::from_str::<Scale>(r#"{"num":1,"den":2000000}"#).is_err());
        // 負値・非整数は u32 の型で弾かれる。
        assert!(serde_json::from_str::<Scale>(r#"{"num":-1,"den":2}"#).is_err());
    }

    #[test]
    fn paper_sizes_follow_the_a_series() {
        // 規定 2-1 の表。
        for (paper, short, long) in [
            (PaperSize::A0, 841.0, 1189.0),
            (PaperSize::A1, 594.0, 841.0),
            (PaperSize::A2, 420.0, 594.0),
            (PaperSize::A3, 297.0, 420.0),
            (PaperSize::A4, 210.0, 297.0),
        ] {
            assert_eq!(paper.short_mm(), short);
            assert_eq!(paper.long_mm(), long);
        }
    }

    #[test]
    fn sheet_default_is_a4_landscape_one_to_one_form_b_without_frame() {
        // DESIGN.md M8 設計判断6 の既定値。
        let sheet = SheetMeta::default();
        assert_eq!(sheet.unit, Unit::Millimeter);
        assert_eq!(sheet.scale, Scale::ONE);
        assert_eq!(sheet.paper, PaperSize::A4);
        assert_eq!(sheet.orientation, Orientation::Landscape);
        assert_eq!(sheet.title_block, TitleBlockKind::B);
        assert!(!sheet.frame_visible);
        assert_eq!(sheet.fields, TitleBlockFields::default());
        assert_eq!(sheet.fields.projection, ProjectionMethod::ThirdAngle);
        assert_eq!(sheet.paper_extent_mm(), (297.0, 210.0));
    }

    #[test]
    fn portrait_swaps_paper_extent() {
        let sheet = SheetMeta {
            orientation: Orientation::Portrait,
            ..SheetMeta::default()
        };
        assert_eq!(sheet.paper_extent_mm(), (210.0, 297.0));
    }

    #[test]
    fn title_block_kind_resolves_to_standard_templates() {
        assert_eq!(
            TitleBlockKind::A.template(),
            TitleBlockTemplate::standard_a()
        );
        assert_eq!(
            TitleBlockKind::B.template(),
            TitleBlockTemplate::standard_b()
        );
        assert_eq!(
            TitleBlockKind::C.template(),
            TitleBlockTemplate::standard_c()
        );
        let custom = TitleBlockTemplate::standard_a().clone();
        assert_eq!(TitleBlockKind::Custom(custom.clone()).template(), &custom);
    }

    #[test]
    fn sheet_validate_rejects_broken_custom_template() {
        let mut broken = TitleBlockTemplate::standard_b().clone();
        broken.rows[0].height_mm = f64::NAN;
        let sheet = SheetMeta {
            title_block: TitleBlockKind::Custom(broken),
            ..SheetMeta::default()
        };
        assert!(sheet.validate().is_err());
        // 標準様式は常に妥当。
        assert_eq!(SheetMeta::default().validate(), Ok(()));
    }

    #[test]
    fn sheet_round_trips_through_json() {
        let sheet = SheetMeta {
            unit: Unit::Millimeter,
            scale: Scale::new(2, 5).unwrap(),
            paper: PaperSize::A3,
            orientation: Orientation::Portrait,
            title_block: TitleBlockKind::C,
            fields: TitleBlockFields {
                drawing_number: "MCAD-001".to_owned(),
                drawing_title: "部品図".to_owned(),
                projection: ProjectionMethod::FirstAngle,
                author: "almaz".to_owned(),
                date: "2026-08-01".to_owned(),
                revision: "A".to_owned(),
            },
            frame_visible: true,
        };
        let json = serde_json::to_string(&sheet).unwrap();
        assert_eq!(serde_json::from_str::<SheetMeta>(&json).unwrap(), sheet);
    }
}
