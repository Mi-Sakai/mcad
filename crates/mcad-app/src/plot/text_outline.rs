//! 文字列をグリフアウトライン（[`PathCmd`] 列）へ変換する（M8 タスク39、
//! DESIGN.md M8 設計判断7）。
//!
//! # なぜアウトライン化か
//!
//! SVG の `<text>` + `font-family` はフォントが閲覧環境に無ければ別書体で描かれ、
//! CJK では最悪 tofu になる。フォント埋め込み（サブセット化）は実装量が大きく、
//! PDF バックエンド（タスク40）とも共有できない。**アウトラインパスにしてしまえば
//! 閲覧環境に依存せず確実に出て、SVG と PDF が完全に同じ中間表現を共有できる**
//! （設計判断7）。代償は「文字として検索・選択できない」ことで、これは許容する。
//!
//! # 画面描画との差（意図した非一致）
//!
//! - 画面（`main.rs` の `draw_text`）は egui のガリー下端をアンカーへ合わせる近似だが、
//!   ここでは [`mcad_core::TextGeom`] の意味論どおり **ベースライン左端 = アンカー**
//!   に置く（設計判断7 が「egui の表示と厳密一致はさせない」と明記している）。
//! - 字送りは `glyph_hor_advance` の単純送りで **カーニングしない**。
//! - ASCII も Noto Sans JP で出す（画面の既定フォントとは字形が微差）。
//!
//! # 座標系
//!
//! フォント単位のアウトライン座標は **y-up**（IR と同符号）なので反転は不要。
//! 出力は紙 mm（原点=用紙左下、y-up）。回転はここで anchor まわりに焼き込むため、
//! バックエンドは角度を知らなくてよい。**`draw_text` の `a = -θ` は egui の
//! スクリーン座標（y-down）のための符号反転なので、ここへ持ち込んではいけない。**

use mcad_geom::Point2;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

use super::PathCmd;

/// フォントに収録されていない文字の送り量（em 比）。
///
/// 文字を落としたことが目に見えるよう、詰めずに全角の 6 割ほどの空白を残す
/// （0 送りだと後続の文字が重なって「消えた」ことに気づけない）。
const MISSING_GLYPH_ADVANCE_EM: f64 = 0.6;

/// 埋め込みフォントのアウトライン取得器（[`crate::fonts::embedded_font_bytes`] を解析済み）。
///
/// 1 ページ分の出力で 1 つ作って使い回す（`Face::parse` を文字列ごとに呼ばない）。
pub struct GlyphOutliner {
    face: Face<'static>,
    /// em ボックスの一辺（フォント単位）。`height_mm / units_per_em` が縮尺になる。
    units_per_em: f64,
}

impl GlyphOutliner {
    /// 埋め込みフォント（Noto Sans JP Regular）を解析して作る。
    ///
    /// 解析不能・`units_per_em == 0` なら `None`。埋め込みフォントは
    /// コンパイル時に固定されており実際には常に成功する（`embedded_font_parses`
    /// でテスト固定）が、**フォントが無いことでアプリを落とさない**ため
    /// `Option` を返し、呼び出し側は「文字を出さない」で継続する。
    #[must_use]
    pub fn embedded() -> Option<Self> {
        let face = Face::parse(crate::fonts::embedded_font_bytes(), 0).ok()?;
        let units_per_em = f64::from(face.units_per_em());
        (units_per_em > 0.0).then_some(Self { face, units_per_em })
    }

    /// 文字列を紙 mm 座標のアウトライン（塗り用パスコマンド列）へ変換する。
    ///
    /// - `anchor`: ベースライン左端（**紙 mm**）。
    /// - `height_mm`: em ボックスの高さ（**紙 mm**）。単位換算は呼び出し側の責務
    ///   （寸法文字は `DimExpansion` のワールド高さを `÷ k` してから渡す。
    ///   `TextGeom::height`・`FrameText::height_mm` は既に紙 mm なのでそのまま渡す。
    ///   タスク37 で `draw_text` を `world_height` 明示引数にしたのと同じ手筋で
    ///   二重換算を防ぐ）。
    /// - `angle`: anchor まわりの回転（ラジアン、CCW。IR は y-up なので符号反転しない）。
    ///
    /// 戻り値は 1 本のパス（複数サブパスを含みうる）として塗る前提で、
    /// **fill-rule は nonzero**（フォントのアウトラインの慣行。穴が正しく抜ける）。
    /// アウトラインを持たない文字（空白など）でも **送りは進む**。
    #[must_use]
    pub fn outline(
        &self,
        content: &str,
        anchor: Point2,
        height_mm: f64,
        angle: f64,
    ) -> Vec<PathCmd> {
        let mut cmds = Vec::new();
        if content.is_empty() || !height_mm.is_finite() || height_mm <= 0.0 {
            return cmds;
        }
        let scale = height_mm / self.units_per_em;
        let (sin, cos) = angle.sin_cos();
        // ベースライン方向の現在位置（フォント単位）。
        let mut pen_x = 0.0;

        for ch in content.chars() {
            let Some(glyph_id) = self.face.glyph_index(ch) else {
                // 未収録グリフはスキップし、送りだけ進める。
                pen_x += MISSING_GLYPH_ADVANCE_EM * self.units_per_em;
                continue;
            };
            let mut sink = OutlineSink {
                cmds: &mut cmds,
                pen_x,
                scale,
                sin,
                cos,
                anchor,
                current: (0.0, 0.0),
            };
            // 戻り値（bbox）は使わない。`None` は「輪郭を持たないグリフ」（空白など）で、
            // その場合もこの下の送りは進める。
            self.face.outline_glyph(glyph_id, &mut sink);
            pen_x += self.advance(glyph_id);
        }
        cmds
    }

    /// グリフの字送り（フォント単位）。`hmtx` に無い場合は未収録扱いの既定送りにする。
    fn advance(&self, glyph_id: GlyphId) -> f64 {
        self.face
            .glyph_hor_advance(glyph_id)
            .map_or(MISSING_GLYPH_ADVANCE_EM * self.units_per_em, f64::from)
    }
}

/// `ttf-parser` のアウトラインコールバックを [`PathCmd`] 列へ落とす受け皿。
///
/// フォント単位の点を `(pen_x + x, y) * scale` で紙 mm へ縮め、anchor まわりに
/// `angle` 回転して平行移動する。2 次ベジエ（TrueType グリフ）は **厳密な次数上げ**
/// で 3 次へ直す（IR は 3 次のみ。CFF は元々 3 次なので Noto Sans JP では
/// `quad_to` は来ないが、フォントを差し替えても壊れないようにしておく）。
struct OutlineSink<'a> {
    cmds: &'a mut Vec<PathCmd>,
    pen_x: f64,
    scale: f64,
    sin: f64,
    cos: f64,
    anchor: Point2,
    /// 現在点（フォント単位、`pen_x` は含まない生のグリフ座標）。次数上げに使う。
    current: (f64, f64),
}

impl OutlineSink<'_> {
    /// フォント単位の点 → 紙 mm の点。
    fn map(&self, x: f64, y: f64) -> Point2 {
        let lx = (self.pen_x + x) * self.scale;
        let ly = y * self.scale;
        Point2::new(
            self.anchor.x + lx * self.cos - ly * self.sin,
            self.anchor.y + lx * self.sin + ly * self.cos,
        )
    }
}

impl OutlineBuilder for OutlineSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.current = (f64::from(x), f64::from(y));
        let p = self.map(self.current.0, self.current.1);
        self.cmds.push(PathCmd::MoveTo(p));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.current = (f64::from(x), f64::from(y));
        let p = self.map(self.current.0, self.current.1);
        self.cmds.push(PathCmd::LineTo(p));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // 2 次 (P0, Q, P1) → 3 次 (P0, C1, C2, P1) の厳密な次数上げ:
        // C1 = P0 + 2/3 (Q − P0), C2 = P1 + 2/3 (Q − P1)。
        let (p0x, p0y) = self.current;
        let (qx, qy) = (f64::from(x1), f64::from(y1));
        let (p1x, p1y) = (f64::from(x), f64::from(y));
        const TWO_THIRDS: f64 = 2.0 / 3.0;
        let c1 = (p0x + TWO_THIRDS * (qx - p0x), p0y + TWO_THIRDS * (qy - p0y));
        let c2 = (p1x + TWO_THIRDS * (qx - p1x), p1y + TWO_THIRDS * (qy - p1y));
        self.current = (p1x, p1y);
        let cmd = PathCmd::CurveTo(
            self.map(c1.0, c1.1),
            self.map(c2.0, c2.1),
            self.map(p1x, p1y),
        );
        self.cmds.push(cmd);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = (f64::from(x1), f64::from(y1));
        let c2 = (f64::from(x2), f64::from(y2));
        self.current = (f64::from(x), f64::from(y));
        let cmd = PathCmd::CurveTo(
            self.map(c1.0, c1.1),
            self.map(c2.0, c2.1),
            self.map(self.current.0, self.current.1),
        );
        self.cmds.push(cmd);
    }

    fn close(&mut self) {
        // 輪郭の終わり。`ttf-parser` は各輪郭を必ず `move_to` から始めるため、
        // ここで現在点を戻す必要はない。
        self.cmds.push(PathCmd::Close);
    }
}
