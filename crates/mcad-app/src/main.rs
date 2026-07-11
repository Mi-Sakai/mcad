//! `mcad-app` — eguiアプリ本体（バイナリ）。
//!
//! 現段階（ワークスペース雛形）では空のウィンドウを表示するのみ。
//! 後続タスクでViewport・Tool状態機械・スナップエンジン・UIレイアウトを追加する。

struct McadApp;

impl eframe::App for McadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |_ui| {
            // 雛形段階では何も描画しない。
        });
    }
}

fn main() -> anyhow::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "mcad",
        native_options,
        Box::new(|_cc| Ok(Box::new(McadApp))),
    )
    .map_err(|err| anyhow::anyhow!("failed to run mcad-app: {err}"))
}
