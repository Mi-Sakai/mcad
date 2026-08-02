//! 色表現（[`Rgb`]）・線幅（[`WidthMm`]）・線種（[`Linetype`]）と、エンティティの
//! 描画スタイル（[`Style`]）。
//!
//! # 線幅・線種の ByLayer モデル（DESIGN.md M8 設計判断5）
//!
//! 線幅と線種は「レイヤー既定 + エンティティ上書き」で解決する。[`crate::Layer`] は
//! 必ず実体値（[`Linetype`] / [`WidthMm`]）を持ち、[`Style`] 側は `Option`（`None` =
//! ByLayer = レイヤー既定に従う）で持つ。解決は [`Style::effective_width`] /
//! [`Style::effective_linetype`]（色の [`Style::effective_color`] と同じ形）。
//!
//! 線幅の単位は **紙 mm**（画面 px ではない）。画面 px への換算は尺度とズームを
//! 掛ける app 層の責務で、この層は「検証済みの紙 mm」だけを持つ。

use serde::{Deserialize, Serialize};

use crate::CoreError;

/// 8bit RGB 色。
///
/// GUI 非依存の生の色値。egui などの色型への変換は上位（app）層で行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb {
    /// 赤成分。
    pub r: u8,
    /// 緑成分。
    pub g: u8,
    /// 青成分。
    pub b: u8,
}

impl Rgb {
    /// 黒 `(0, 0, 0)`。
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);
    /// 白 `(255, 255, 255)`。
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);

    /// 各成分から色を作る。
    #[inline]
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// 線種（DESIGN.md M8 設計判断5）。
///
/// 破線パターンの実長（紙 mm 基準）は描画側（app 層）の定数として持つ。core は
/// 「どの線種か」という意味だけを保持する。
///
/// **`#[non_exhaustive]`**: 規定第4章の線種は将来増えうる（三点鎖線・特殊線など）。
/// クレート外の `match` には常にワイルドカード腕を要求し、線種追加が下流の
/// コンパイルエラーにならないようにする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Linetype {
    /// 実線。
    #[default]
    Continuous,
    /// 破線。
    Dashed,
    /// 一点鎖線。
    DashDot,
    /// 二点鎖線。
    DashDotDot,
}

/// 検証済みの線幅（**紙 mm**）。
///
/// # なぜ生の `f32` ではないか（DESIGN.md M8 設計判断5）
///
/// 線幅は画面描画だけでなく SVG/PDF/DXF の出力座標にも乗る。`0`・負・NaN・∞ が
/// 混ざると、出力先で非有限値やゼロ寸法として伝播する（画面側の 1px 下限クランプが
/// 事故を隠してしまう）。そこで **生成できる時点で範囲を保証** し、エクスポータには
/// 検証済みの値しか到達しないようにする。
///
/// 許容範囲は [`WidthMm::MIN_MM`]`..=`[`WidthMm::MAX_MM`]（ISO 128 の線幅系列
/// 0.13〜2.0 を内包し、DXF lineweight の最大 2.11mm も収まる）。
///
/// serde は透過ではなく `try_from` 経由なので、**手編集された `.mcad` の不正値も
/// 読込境界で拒否**される。
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f32")]
pub struct WidthMm(f32);

impl WidthMm {
    /// 許容する最小線幅 [mm]。
    pub const MIN_MM: f32 = 0.05;
    /// 許容する最大線幅 [mm]。
    pub const MAX_MM: f32 = 5.0;
    /// 既定の線幅 0.35mm（規定 4-1 の「実線_中」相当。レイヤーの初期値）。
    pub const DEFAULT: WidthMm = WidthMm(0.35);

    /// 線幅を検証して作る。
    ///
    /// # Errors
    ///
    /// 非有限（NaN/∞）または [`WidthMm::MIN_MM`]`..=`[`WidthMm::MAX_MM`] の範囲外で
    /// [`CoreError::InvalidLineWidth`] を返す（`0`・負はこの範囲外として弾かれる）。
    pub fn new(mm: f32) -> Result<Self, CoreError> {
        if !mm.is_finite() {
            return Err(CoreError::InvalidLineWidth(format!(
                "line width must be finite: {mm}"
            )));
        }
        if !(Self::MIN_MM..=Self::MAX_MM).contains(&mm) {
            return Err(CoreError::InvalidLineWidth(format!(
                "line width {mm} mm is out of range {}..={} mm",
                Self::MIN_MM,
                Self::MAX_MM
            )));
        }
        Ok(Self(mm))
    }

    /// 有限値を許容範囲へ丸めて作る。非有限（NaN/∞）は `None`。
    ///
    /// 外部データを best-effort で取り込む経路（旧 `.mcad` の線幅移行・DXF import の
    /// lineweight）専用。**丸めたこと自体は呼び出し側が検出して件数計上する**
    /// 責務がある（黙って値を変えてよいのは、呼び出し側が警告を出す前提のときだけ）。
    #[must_use]
    pub fn clamped(mm: f32) -> Option<Self> {
        mm.is_finite()
            .then(|| Self(mm.clamp(Self::MIN_MM, Self::MAX_MM)))
    }

    /// 線幅 [mm]。
    #[inline]
    #[must_use]
    pub const fn mm(self) -> f32 {
        self.0
    }
}

impl Default for WidthMm {
    /// [`WidthMm::DEFAULT`]（0.35mm）。
    #[inline]
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<f32> for WidthMm {
    type Error = CoreError;

    #[inline]
    fn try_from(mm: f32) -> Result<Self, Self::Error> {
        Self::new(mm)
    }
}

/// エンティティの描画スタイル。
///
/// 色・線幅・線種のいずれも `None` は **ByLayer**（所属レイヤーの値を継承）を意味する
/// （DESIGN.md M8 設計判断5）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Style {
    /// 個別指定色。`None` ならレイヤー色を継承する。
    pub color: Option<Rgb>,
    /// 個別指定の線幅（紙 mm）。`None` ならレイヤーの線幅を継承する。
    pub width_mm: Option<WidthMm>,
    /// 個別指定の線種。`None` ならレイヤーの線種を継承する。
    pub linetype: Option<Linetype>,
}

impl Style {
    /// 色・線幅・線種のすべてをレイヤーから継承する既定スタイル（全 ByLayer）。
    #[inline]
    #[must_use]
    pub const fn inherited() -> Self {
        Self {
            color: None,
            width_mm: None,
            linetype: None,
        }
    }

    /// このスタイルが実際に描画で使う色を解決する。
    ///
    /// 個別指定色があればそれを、なければ引数の `layer_color` を返す。
    #[inline]
    #[must_use]
    pub fn effective_color(&self, layer_color: Rgb) -> Rgb {
        self.color.unwrap_or(layer_color)
    }

    /// このスタイルが実際に描画・出力で使う線幅（紙 mm）を解決する。
    #[inline]
    #[must_use]
    pub fn effective_width(&self, layer_width: WidthMm) -> WidthMm {
        self.width_mm.unwrap_or(layer_width)
    }

    /// このスタイルが実際に描画・出力で使う線種を解決する。
    #[inline]
    #[must_use]
    pub fn effective_linetype(&self, layer_linetype: Linetype) -> Linetype {
        self.linetype.unwrap_or(layer_linetype)
    }
}

impl Default for Style {
    #[inline]
    fn default() -> Self {
        Self::inherited()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_mm_accepts_range_bounds_and_typical_series() {
        // ISO 128 系列（規定 7.1 のプリセット）はすべて範囲内。
        for mm in [0.13_f32, 0.18, 0.25, 0.35, 0.5, 0.7, 1.0, 1.4, 2.0] {
            assert!(WidthMm::new(mm).is_ok(), "{mm} should be accepted");
        }
        assert_eq!(WidthMm::new(WidthMm::MIN_MM).unwrap().mm(), WidthMm::MIN_MM);
        assert_eq!(WidthMm::new(WidthMm::MAX_MM).unwrap().mm(), WidthMm::MAX_MM);
    }

    #[test]
    fn width_mm_rejects_zero_negative_non_finite_and_out_of_range() {
        for mm in [
            0.0_f32,
            -0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            0.049,
            5.001,
        ] {
            assert!(
                matches!(WidthMm::new(mm), Err(CoreError::InvalidLineWidth(_))),
                "{mm} should be rejected"
            );
        }
    }

    #[test]
    fn width_mm_clamped_rounds_into_range_and_rejects_non_finite() {
        assert_eq!(WidthMm::clamped(100.0).unwrap().mm(), WidthMm::MAX_MM);
        assert_eq!(WidthMm::clamped(0.001).unwrap().mm(), WidthMm::MIN_MM);
        assert_eq!(WidthMm::clamped(-3.0).unwrap().mm(), WidthMm::MIN_MM);
        assert_eq!(WidthMm::clamped(0.5).unwrap().mm(), 0.5);
        assert!(WidthMm::clamped(f32::NAN).is_none());
        assert!(WidthMm::clamped(f32::INFINITY).is_none());
    }

    #[test]
    fn width_mm_serde_rejects_out_of_range_at_the_read_boundary() {
        // 透過 derive ではなく try_from 経由なので、手編集された値も読込で弾かれる。
        assert_eq!(
            serde_json::from_str::<WidthMm>("0.35").unwrap(),
            WidthMm::DEFAULT
        );
        assert!(serde_json::from_str::<WidthMm>("0.0").is_err());
        assert!(serde_json::from_str::<WidthMm>("-1.0").is_err());
        assert!(serde_json::from_str::<WidthMm>("9.0").is_err());
        // 直列化は透過（数値そのもの）。
        assert_eq!(serde_json::to_string(&WidthMm::DEFAULT).unwrap(), "0.35");
    }

    #[test]
    fn style_defaults_to_all_bylayer() {
        let style = Style::inherited();
        assert_eq!(style, Style::default());
        assert!(style.color.is_none());
        assert!(style.width_mm.is_none());
        assert!(style.linetype.is_none());
    }

    #[test]
    fn style_resolves_bylayer_and_overrides() {
        let layer_width = WidthMm::new(0.7).unwrap();
        let inherited = Style::inherited();
        assert_eq!(inherited.effective_color(Rgb::BLACK), Rgb::BLACK);
        assert_eq!(inherited.effective_width(layer_width), layer_width);
        assert_eq!(
            inherited.effective_linetype(Linetype::DashDot),
            Linetype::DashDot
        );

        let overridden = Style {
            color: Some(Rgb::WHITE),
            width_mm: Some(WidthMm::new(0.13).unwrap()),
            linetype: Some(Linetype::Dashed),
        };
        assert_eq!(overridden.effective_color(Rgb::BLACK), Rgb::WHITE);
        assert_eq!(
            overridden.effective_width(layer_width).mm(),
            WidthMm::new(0.13).unwrap().mm()
        );
        assert_eq!(
            overridden.effective_linetype(Linetype::DashDot),
            Linetype::Dashed
        );
    }
}
