//! 色表現（[`Rgb`]）とエンティティの描画スタイル（[`Style`]）。

use serde::{Deserialize, Serialize};

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

/// エンティティの描画スタイル。
///
/// MVP としては最低限、色（レイヤー色を継承するか個別指定するか）と線幅のみを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Style {
    /// 個別指定色。`None` ならレイヤー色を継承する。
    pub color: Option<Rgb>,
    /// 線幅（ピクセル単位。描画時に使う表示属性）。
    pub width: f32,
}

impl Style {
    /// レイヤー色を継承する既定スタイル（線幅 1.0）。
    #[inline]
    #[must_use]
    pub const fn inherited() -> Self {
        Self {
            color: None,
            width: 1.0,
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
}

impl Default for Style {
    #[inline]
    fn default() -> Self {
        Self::inherited()
    }
}
