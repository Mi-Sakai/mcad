//! 日本語（CJK）フォントの組み込み（M6 タスク23、DESIGN.md M6 設計判断3）。
//!
//! egui の既定フォントは CJK グリフを含まないため、文書内の Text エンティティに
//! 日本語を入れると tofu（□）になる。Noto Sans JP Regular（SIL OFL）をバイナリへ
//! 埋め込み、既定フォントの **後ろ** にフォールバックとして追加する。これにより
//! ラテン文字・UI ラベルの見た目は一切変わらず、既定フォントに無いグリフ（CJK）
//! だけが Noto で描画される。
//!
//! egui の可視 UI 文字列は引き続き ASCII 限定（AGENTS.md コーディング規約）。CJK が
//! 現れるのは文書内の Text エンティティとその入力欄の中身のみ。
//!
//! ライセンス全文はフォントと同じディレクトリの `assets/fonts/LICENSE-OFL.txt`、
//! 帰属表記は README を参照。

/// 埋め込む Noto Sans JP Regular（OTF）。ライセンスは同ディレクトリの
/// `LICENSE-OFL.txt` を参照。
static NOTO_SANS_JP: &[u8] = include_bytes!("../assets/fonts/NotoSansJP-Regular.otf");

/// フォント登録名。
const FONT_NAME: &str = "noto_sans_jp";

/// CJK フォールバックフォントを egui コンテキストへ登録する。
///
/// 既定フォント（ラテン文字はそのまま）の **後ろ** へフォールバックとして追加するため、
/// 英数字・UI の見た目は変わらず、CJK グリフだけが Noto Sans JP で描画される。
/// ホストはアプリ起動時（初回フレームの前）に 1 度だけ呼ぶ。
pub fn install_fallback_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(NOTO_SANS_JP)),
    );
    // 既定の各ファミリの末尾へ push することで、既存グリフは既定フォントが優先され、
    // 欠けているグリフ（CJK）だけが Noto へフォールバックする。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(FONT_NAME.to_owned());
    }
    ctx.set_fonts(fonts);
}
