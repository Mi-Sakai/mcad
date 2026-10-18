//! アプリ設定の永続化（M8 タスク41-1、DESIGN.md 判断8）。
//!
//! `~/.config/mcad/config.json`（`dirs::config_dir()` 経由）へ自前の JSON で保存する。
//! `eframe` の persistence 機能は使わない（プラットフォーム間の格納形式・キー管理を
//! 自前で把握しておきたいため）。**図面内容は一切含まない** — ここに置くのは
//! トグル・既定用紙/尺度/様式・最近使ったファイルの一覧のみで、`Document`/`SheetMeta`
//! そのものではない（保存の起点は都度 [`Config::default_sheet_meta`] が組み立てる）。
//!
//! 読み書き失敗は既定値へフォールバックする（[`load`]/[`load_startup`] の doc 参照）。
//! アプリの起動を設定ファイルの有無・破損で止めない。

use std::fs;
use std::path::{Path, PathBuf};

use mcad_core::{Orientation, PaperSize, Scale, SheetMeta, TitleBlockKind};
use serde::{Deserialize, Serialize};

use crate::plot::PlotColorMode;
use crate::snap::SnapKinds;

/// 「最近使ったファイル」の保持件数上限。
pub const MAX_RECENT_FILES: usize = 5;

/// 既定表題欄様式の選択肢（config.json 用）。
///
/// [`TitleBlockKind::Custom`] はテンプレート本体（`TitleBlockTemplate`）を伴うため
/// 既定値としては保存しない（ユーザー定義様式を作るたびに config.json へ丸ごと
/// 複製するのは意図と合わない）。config 上は A/B/C の3値のみを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TitleBlockChoice {
    /// 標準様式A。
    A,
    /// 標準様式B（既定）。
    #[default]
    B,
    /// 標準様式C。
    C,
}

impl TitleBlockChoice {
    /// 対応する [`TitleBlockKind`] を作る（`Custom` にはならない）。
    #[must_use]
    pub fn to_kind(self) -> TitleBlockKind {
        match self {
            TitleBlockChoice::A => TitleBlockKind::A,
            TitleBlockChoice::B => TitleBlockKind::B,
            TitleBlockChoice::C => TitleBlockKind::C,
        }
    }

    /// [`TitleBlockKind`] から対応する選択肢を取り出す。`Custom` は `None`。
    #[must_use]
    pub fn from_kind(kind: &TitleBlockKind) -> Option<Self> {
        match kind {
            TitleBlockKind::A => Some(TitleBlockChoice::A),
            TitleBlockKind::B => Some(TitleBlockChoice::B),
            TitleBlockKind::C => Some(TitleBlockChoice::C),
            TitleBlockKind::Custom(_) => None,
        }
    }
}

/// アプリ設定（config.json の中身そのもの）。
///
/// `#[serde(default)]` によりコンテナ全体を前方互換にする — 旧バージョンで保存した
/// JSON に新フィールドが欠けていても [`Default`] の値で補い、逆に将来のバージョンが
/// 増やしたフィールドを現行バージョンが読んでも未知フィールドは無視される
/// （`serde_json` の既定挙動）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// スナップの有効/無効（F3）。
    pub snap_enabled: bool,
    /// 直交モードの有効/無効（F8）。
    pub ortho_enabled: bool,
    /// 極トラッキングの有効/無効（F10、M12 タスク84、DESIGN.md M12 設計判断4）。
    /// 直交モード（`ortho_enabled`）とは排他 — F8/F10 トグルは互いを OFF にし、
    /// 両方 `true` の config.json（手編集）を読んだ場合は [`Config::normalize_exclusive_modes`]
    /// が直交優先で正規化する。
    pub polar_enabled: bool,
    /// 極トラッキングの刻み角（F10 有効時、上部パネルのコンボで選ぶ。既定 15°）。
    pub polar_step: PolarStep,
    /// 紙基準表示の有効/無効（F9）。
    pub paper_display_enabled: bool,
    /// 新規文書の既定用紙サイズ。
    pub default_paper: PaperSize,
    /// 新規文書の既定向き。
    pub default_orientation: Orientation,
    /// 新規文書の既定尺度。
    pub default_scale: Scale,
    /// 新規文書の既定表題欄様式。
    pub default_title_block: TitleBlockChoice,
    /// 最近使った `.mcad` ファイルのパス（先頭が最新、最大 [`MAX_RECENT_FILES`] 件）。
    pub recent_files: Vec<PathBuf>,
    /// SVG/PDF 出力の色モード（画面表示・図面データには無影響。DESIGN.md 7章
    /// 「随時対応」モノクロ化-2）。旧バージョンの config.json（このフィールドが
    /// 存在しない）はコンテナの `#[serde(default)]` により
    /// [`PlotColorMode::default`]（`Monochrome`）へ補完される。
    pub plot_color_mode: PlotColorMode,
    /// 作図グリッドの種別（矩形/等測。F5）。旧バージョンの config.json（このフィールドが
    /// 存在しない）は [`GridMode::default`]（`Rectangular`）へ補完される。
    pub grid_mode: GridMode,
    /// スナップ種別ごとの ON/OFF（M12 タスク85、DESIGN.md M12 設計判断5）。旧バージョンの
    /// config.json（このフィールドが存在しない）は [`SnapKinds::default`]（近接点のみ
    /// OFF、他は ON）へ補完される。フィールド単位でも `#[serde(default)]` を持つため、
    /// 種別が増えても同じ経路で前方互換になる（[`SnapKinds`] の doc 参照）。
    pub snap_kinds: SnapKinds,
    /// 拘束記号を描くか（右パネル「拘束」セクションのチェックボックス。M13 タスク100、
    /// DESIGN.md M13 設計判断6）。画面表示のみで図面データには無関係。旧バージョンの
    /// config.json（このフィールドが存在しない）はコンテナの `#[serde(default)]` により
    /// [`Config::default`] の `true` へ補完される。
    pub constraint_symbols_visible: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            snap_enabled: true,
            ortho_enabled: false,
            polar_enabled: false,
            polar_step: PolarStep::default(),
            paper_display_enabled: false,
            default_paper: PaperSize::default(),
            default_orientation: Orientation::default(),
            default_scale: Scale::default(),
            default_title_block: TitleBlockChoice::default(),
            recent_files: Vec::new(),
            plot_color_mode: PlotColorMode::default(),
            grid_mode: GridMode::default(),
            snap_kinds: SnapKinds::default(),
            constraint_symbols_visible: true,
        }
    }
}

/// 作図グリッドの種別（F5）。DESIGN.md 7章「随時対応」アイソメ図・アクソメ図の
/// 作図補助（等測投影サポート）設計確定3。
///
/// `grid_mode` は `config.json` に永続化される（画面表示・スナップの挙動のみに
/// 影響し、図面データ（`.mcad`/DXF）には無関係）。旧バージョンの config.json
/// （このフィールドが存在しない）はコンテナの `#[serde(default)]` により
/// [`GridMode::default`]（`Rectangular`）へ補完される。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GridMode {
    /// 矩形グリッド（既定。現行の X/Y 軸グリッド）。
    #[default]
    Rectangular,
    /// 等測グリッド（30°/90°/150° の3線群、格子点は斜交座標）。
    Isometric,
}

/// 極トラッキングの刻み角（F10、M12 タスク84）。`config.json` に永続化される。
///
/// 旧バージョンの config.json（このフィールドが存在しない）はコンテナの
/// `#[serde(default)]` により [`PolarStep::default`]（`Fifteen`）へ補完される。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PolarStep {
    /// 15°刻み（既定）。
    #[default]
    Fifteen,
    /// 30°刻み。
    Thirty,
    /// 45°刻み。
    FortyFive,
    /// 90°刻み。
    Ninety,
}

impl PolarStep {
    /// 刻み角を度で返す（[`crate::ortho::polar_track`] へそのまま渡す）。
    #[must_use]
    pub fn degrees(self) -> f64 {
        match self {
            PolarStep::Fifteen => 15.0,
            PolarStep::Thirty => 30.0,
            PolarStep::FortyFive => 45.0,
            PolarStep::Ninety => 90.0,
        }
    }

    /// 上部パネルのコンボボックスに並べる順（刻みの細かい順）。
    pub const ALL: [PolarStep; 4] = [
        PolarStep::Fifteen,
        PolarStep::Thirty,
        PolarStep::FortyFive,
        PolarStep::Ninety,
    ];

    /// コンボボックスの表示ラベル（英語。ステータス表示・上部パネルは未日本語化領域）。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            PolarStep::Fifteen => "15°",
            PolarStep::Thirty => "30°",
            PolarStep::FortyFive => "45°",
            PolarStep::Ninety => "90°",
        }
    }
}

impl Config {
    /// 直交モードと極トラッキングの排他を保証する（直交優先）。
    ///
    /// F8/F10 トグル自体は互いを OFF にするので通常は両方 `true` になり得ないが、
    /// 手編集した config.json は両方 `true` でも構文上は正当なため、読込時にここで
    /// 正規化する（直交優先 = 既存の直交の挙動を変えない側。DESIGN.md M12 設計判断4
    /// 実装時追記）。
    fn normalize_exclusive_modes(&mut self) {
        if self.ortho_enabled && self.polar_enabled {
            self.polar_enabled = false;
        }
    }

    /// 新規文書用の [`SheetMeta`] を組み立てる。
    ///
    /// `fields`（表題欄記入内容）は空、`frame_visible` は `false`、`unit` は
    /// [`mcad_core::Unit::Millimeter`] 固定、`use_bom`（部品表を使う）は `true` — いずれも
    /// config の永続化対象外（記入内容・部品表を使うかは図面ごとに決めるもの、枠表示は
    /// 都度の作業状態、単位は現状 mm 固定。新規図面の「部品表を使う」は ON。DESIGN.md
    /// M12 設計判断1）。
    #[must_use]
    pub fn default_sheet_meta(&self) -> SheetMeta {
        SheetMeta {
            unit: mcad_core::Unit::Millimeter,
            scale: self.default_scale,
            paper: self.default_paper,
            orientation: self.default_orientation,
            title_block: self.default_title_block.to_kind(),
            fields: mcad_core::TitleBlockFields::default(),
            frame_visible: false,
            use_bom: true,
        }
    }

    /// UI で適用された `sheet` の内容へ、既定（用紙・向き・尺度・様式）を追随更新する。
    ///
    /// `sheet.title_block` が [`TitleBlockKind::Custom`] のときは様式の既定値だけ据え置く
    /// （config 上に Custom を保持する枠がないため）。1 つでも値が変わったら `true` を
    /// 返す — 呼び出し側はこの戻り値が `true` のときだけ保存すればよい（`frame_visible`/
    /// `fields` のみの変更のように既定へ影響しない変更で毎回書き込むのを避ける）。
    pub fn remember_sheet_defaults(&mut self, sheet: &SheetMeta) -> bool {
        let mut changed = false;

        if self.default_paper != sheet.paper {
            self.default_paper = sheet.paper;
            changed = true;
        }
        if self.default_orientation != sheet.orientation {
            self.default_orientation = sheet.orientation;
            changed = true;
        }
        if self.default_scale != sheet.scale {
            self.default_scale = sheet.scale;
            changed = true;
        }
        if let Some(choice) = TitleBlockChoice::from_kind(&sheet.title_block)
            && self.default_title_block != choice
        {
            self.default_title_block = choice;
            changed = true;
        }

        changed
    }

    /// 「最近使ったファイル」を更新する。既存の同一パスを取り除いてから先頭へ挿入し、
    /// [`MAX_RECENT_FILES`] 件を超えた分は末尾から切り詰める。
    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(MAX_RECENT_FILES);
    }
}

/// 設定ファイル名（`config_file_path` が返すパスの末尾要素）。
const CONFIG_FILE_NAME: &str = "config.json";

/// config.json の保存先パス（`dirs::config_dir()/mcad/config.json`）。
///
/// プラットフォームの設定ディレクトリが解決できない環境（`dirs` が `None` を返す
/// 稀な環境）では `None`。呼び出し側は「設定を保存できない」ものとして扱う。
#[must_use]
pub fn config_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("mcad").join(CONFIG_FILE_NAME))
}

/// `path` から設定を読み込む。
///
/// - ファイルが存在しない（初回起動）: `Ok(None)`。エラー扱いしない。
/// - 読取・パース失敗: `Err(理由)`（英文。ステータスバー表示に使う）。
/// - 読込成功: `Ok(Some(config))`。
pub fn load(path: &Path) -> Result<Option<Config>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("failed to read {}: {err}", path.display())),
    };
    let mut config: Config = serde_json::from_str(&text)
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
    config.normalize_exclusive_modes();
    Ok(Some(config))
}

/// `config` を `path` へ保存する。
///
/// 親ディレクトリを `create_dir_all` で作り、`path` と同じディレクトリの一時ファイル
/// （`config.json.tmp`）へ pretty JSON を書き込んでから `fs::rename` で本ファイルへ
/// 置き換える。クラッシュ・電源断が書込み途中で起きても、`rename` はアトミックなので
/// 半端に壊れた `config.json` を残さない。
pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;

    let json = serde_json::to_string_pretty(config)
        .map_err(|err| format!("failed to serialize config: {err}"))?;

    let tmp_path = parent.join(format!("{CONFIG_FILE_NAME}.tmp"));
    fs::write(&tmp_path, json)
        .map_err(|err| format!("failed to write {}: {err}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .map_err(|err| format!("failed to replace {}: {err}", path.display()))?;
    Ok(())
}

/// アプリ起動時に確定する設定の状態。
#[derive(Default)]
pub struct Startup {
    /// 使用する設定（読込失敗・不在時は既定値）。
    pub config: Config,
    /// 以後の保存先。`None` なら保存不能（`config_file_path` が解決できなかった）。
    pub path: Option<PathBuf>,
    /// 起動直後にステータスバーへ出す警告。正常時は `None`。
    pub warning: Option<String>,
}

/// `main()` 専用の起動時実 IO まとめ（設定ファイルの位置解決 + 読込）。
///
/// - `config_file_path()` が `None`: 既定値 + 警告
///   "Settings directory unavailable; settings will not be saved"。
/// - `load` が `Ok(None)`（初回起動）: 既定値、警告なし。
/// - `load` が `Err(e)`: 既定値 + 警告 "Settings load failed (using defaults): {e}"。
/// - `load` が `Ok(Some(config))`: その値、警告なし。
#[must_use]
pub fn load_startup() -> Startup {
    let Some(path) = config_file_path() else {
        return Startup {
            config: Config::default(),
            path: None,
            warning: Some("Settings directory unavailable; settings will not be saved".to_owned()),
        };
    };

    match load(&path) {
        Ok(Some(config)) => Startup {
            config,
            path: Some(path),
            warning: None,
        },
        Ok(None) => Startup {
            config: Config::default(),
            path: Some(path),
            warning: None,
        },
        Err(err) => Startup {
            config: Config::default(),
            path: Some(path),
            warning: Some(format!("Settings load failed (using defaults): {err}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::{TitleBlockFields, Unit};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// テストごとに一意な一時ファイルパスを作る
    /// （`std::env::temp_dir()/mcad-app-test/<counter>-<name>.json`）。並列テスト実行でも
    /// 衝突しないよう、プロセス内カウンタでファイル名を分ける。
    fn unique_temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join("mcad-app-test");
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir.join(format!("{n}-{name}.json"))
    }

    #[test]
    fn default_config_has_expected_values() {
        let config = Config::default();
        assert!(config.snap_enabled);
        assert!(!config.ortho_enabled);
        assert!(!config.paper_display_enabled);
        assert_eq!(config.default_paper, PaperSize::A4);
        assert_eq!(config.default_orientation, Orientation::Landscape);
        assert_eq!(config.default_scale, Scale::ONE);
        assert_eq!(config.default_title_block, TitleBlockChoice::B);
        assert!(config.recent_files.is_empty());
        assert_eq!(config.plot_color_mode, PlotColorMode::Monochrome);
        assert_eq!(config.grid_mode, GridMode::Rectangular);
        assert!(!config.polar_enabled);
        assert_eq!(config.polar_step, PolarStep::Fifteen);
        assert_eq!(config.snap_kinds, SnapKinds::default());
        assert!(config.constraint_symbols_visible);
    }

    #[test]
    fn serde_round_trip_preserves_all_non_default_fields() {
        let config = Config {
            snap_enabled: false,
            ortho_enabled: true,
            polar_enabled: false,
            polar_step: PolarStep::Thirty,
            paper_display_enabled: true,
            default_paper: PaperSize::A2,
            default_orientation: Orientation::Portrait,
            default_scale: Scale::new(1, 2).unwrap(),
            default_title_block: TitleBlockChoice::C,
            recent_files: vec![PathBuf::from("/tmp/a.mcad"), PathBuf::from("/tmp/b.mcad")],
            plot_color_mode: PlotColorMode::Blueprint,
            grid_mode: GridMode::Isometric,
            snap_kinds: SnapKinds {
                endpoint: true,
                intersection: false,
                midpoint: true,
                center: false,
                quadrant: false,
                perpendicular: true,
                tangent: false,
                virtual_intersection: true,
                nearest: true,
                grid: false,
            },
            constraint_symbols_visible: false,
        };
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn missing_fields_are_filled_with_defaults() {
        let empty: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Config::default());

        let partial: Config =
            serde_json::from_str(r#"{"snap_enabled": false, "default_paper": "A1"}"#).unwrap();
        assert!(!partial.snap_enabled);
        assert_eq!(partial.default_paper, PaperSize::A1);
        // 残りは既定のまま。
        assert!(!partial.ortho_enabled);
        assert_eq!(partial.default_orientation, Orientation::Landscape);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let config: Config =
            serde_json::from_str(r#"{"snap_enabled": false, "totally_unknown_field": 123}"#)
                .unwrap();
        assert!(!config.snap_enabled);
    }

    #[test]
    fn invalid_scale_value_fails_to_load() {
        let path = unique_temp_path("invalid-scale");
        fs::write(&path, r#"{"default_scale": {"num": 0, "den": 1}}"#).unwrap();
        let result = load(&path);
        assert!(result.is_err());
    }

    /// 旧バージョンの config.json（`plot_color_mode` フィールドが存在しない）を
    /// 読み込んだとき、[`PlotColorMode::default`]（`Monochrome`）へ補完される
    /// （モノクロ化-2、DESIGN.md 7章「随時対応」検収基準）。
    #[test]
    fn missing_plot_color_mode_field_defaults_to_monochrome() {
        let config: Config =
            serde_json::from_str(r#"{"snap_enabled": false, "default_paper": "A1"}"#).unwrap();
        assert_eq!(config.plot_color_mode, PlotColorMode::Monochrome);
    }

    /// 旧バージョンの config.json（`constraint_symbols_visible` フィールドが存在しない）を
    /// 読み込んだとき、`true`（記号を表示）へ補完される。`false` の保存は往復する
    /// （M13 タスク100）。
    #[test]
    fn missing_constraint_symbols_visible_field_defaults_to_true() {
        let config: Config = serde_json::from_str(r#"{"snap_enabled": false}"#).unwrap();
        assert!(config.constraint_symbols_visible);
        let off: Config = serde_json::from_str(r#"{"constraint_symbols_visible": false}"#).unwrap();
        assert!(!off.constraint_symbols_visible);
    }

    /// `plot_color_mode` に存在しない値（不正値）が入っている config.json は、
    /// 既存の「不正値で `load` が `Err` になる」規約に従う（起動時は
    /// [`load_startup`] の既定+警告経路が働く）。
    #[test]
    fn invalid_plot_color_mode_value_fails_to_load() {
        let path = unique_temp_path("invalid-plot-color-mode");
        fs::write(&path, r#"{"plot_color_mode": "Rainbow"}"#).unwrap();
        let result = load(&path);
        assert!(result.is_err());
    }

    /// 旧バージョンの config.json（`grid_mode` フィールドが存在しない）を読み込んだとき、
    /// [`GridMode::default`]（`Rectangular`）へ補完される（アイソメ-1、DESIGN.md 7章
    /// 「随時対応」アイソメ図・アクソメ図の作図補助 設計確定1・検収基準）。
    #[test]
    fn missing_grid_mode_field_defaults_to_rectangular() {
        let config: Config =
            serde_json::from_str(r#"{"snap_enabled": false, "default_paper": "A1"}"#).unwrap();
        assert_eq!(config.grid_mode, GridMode::Rectangular);
    }

    /// `grid_mode` に存在しない値（不正値）が入っている config.json は、既存の
    /// 「不正値で `load` が `Err` になる」規約に従う。
    #[test]
    fn invalid_grid_mode_value_fails_to_load() {
        let path = unique_temp_path("invalid-grid-mode");
        fs::write(&path, r#"{"grid_mode": "Perspective"}"#).unwrap();
        let result = load(&path);
        assert!(result.is_err());
    }

    /// 旧バージョンの config.json（`polar_enabled`/`polar_step` フィールドが存在しない）を
    /// 読み込んだとき、既定値（OFF・15°）へ補完される（M12 タスク84）。
    #[test]
    fn missing_polar_fields_default_to_off_and_fifteen_degrees() {
        let config: Config =
            serde_json::from_str(r#"{"snap_enabled": false, "default_paper": "A1"}"#).unwrap();
        assert!(!config.polar_enabled);
        assert_eq!(config.polar_step, PolarStep::Fifteen);
    }

    /// `polar_step` に存在しない値（不正値）が入っている config.json は、既存の
    /// 「不正値で `load` が `Err` になる」規約に従う。
    #[test]
    fn invalid_polar_step_value_fails_to_load() {
        let path = unique_temp_path("invalid-polar-step");
        fs::write(&path, r#"{"polar_step": "Sixty"}"#).unwrap();
        let result = load(&path);
        assert!(result.is_err());
    }

    /// 旧バージョンの config.json（`snap_kinds` フィールドが存在しない）を読み込んだとき、
    /// [`SnapKinds::default`]（近接点のみ OFF、他は ON）へ補完される（M12 タスク85）。
    #[test]
    fn missing_snap_kinds_field_defaults_to_all_on_except_nearest() {
        let config: Config =
            serde_json::from_str(r#"{"snap_enabled": false, "default_paper": "A1"}"#).unwrap();
        assert_eq!(config.snap_kinds, SnapKinds::default());
        assert!(config.snap_kinds.endpoint);
        assert!(config.snap_kinds.intersection);
        assert!(config.snap_kinds.midpoint);
        assert!(config.snap_kinds.center);
        assert!(config.snap_kinds.quadrant);
        assert!(config.snap_kinds.perpendicular);
        assert!(config.snap_kinds.tangent);
        assert!(config.snap_kinds.virtual_intersection);
        assert!(!config.snap_kinds.nearest);
        assert!(config.snap_kinds.grid);
    }

    /// `snap_kinds` オブジェクト自体はあるが一部フィールドが欠けている場合も、
    /// フィールド単位の `#[serde(default)]` により欠けたフィールドだけ既定値へ補完される
    /// （種別が増えたときも同じ経路で前方互換になることの固定）。
    #[test]
    fn partial_snap_kinds_object_fills_missing_fields_with_defaults() {
        let config: Config =
            serde_json::from_str(r#"{"snap_kinds": {"nearest": true, "grid": false}}"#).unwrap();
        assert!(config.snap_kinds.endpoint);
        assert!(config.snap_kinds.intersection);
        assert!(config.snap_kinds.midpoint);
        assert!(config.snap_kinds.center);
        assert!(config.snap_kinds.quadrant);
        assert!(config.snap_kinds.perpendicular);
        assert!(config.snap_kinds.tangent);
        assert!(config.snap_kinds.virtual_intersection);
        assert!(config.snap_kinds.nearest);
        assert!(!config.snap_kinds.grid);
    }

    /// `snap_kinds.endpoint` に不正値（数値）が入っている config.json は、既存の
    /// 「不正値で `load` が `Err` になる」規約に従う。
    #[test]
    fn invalid_snap_kinds_field_value_fails_to_load() {
        let path = unique_temp_path("invalid-snap-kinds");
        fs::write(&path, r#"{"snap_kinds": {"endpoint": "yes"}}"#).unwrap();
        let result = load(&path);
        assert!(result.is_err());
    }

    /// `polar_step` の全ラベル・度数の対応を固定する（上部パネルのコンボボックス用）。
    #[test]
    fn polar_step_all_labels_and_degrees() {
        assert_eq!(PolarStep::Fifteen.degrees(), 15.0);
        assert_eq!(PolarStep::Thirty.degrees(), 30.0);
        assert_eq!(PolarStep::FortyFive.degrees(), 45.0);
        assert_eq!(PolarStep::Ninety.degrees(), 90.0);
        assert_eq!(PolarStep::Fifteen.label(), "15°");
        assert_eq!(PolarStep::Thirty.label(), "30°");
        assert_eq!(PolarStep::FortyFive.label(), "45°");
        assert_eq!(PolarStep::Ninety.label(), "90°");
        assert_eq!(PolarStep::ALL.len(), 4);
    }

    /// 直交・極トラッキングが両方 `true` の config.json（手編集を想定）は、`load` で
    /// 直交優先へ正規化される（極が OFF になる。DESIGN.md M12 設計判断4 実装時追記）。
    #[test]
    fn load_normalizes_both_ortho_and_polar_enabled_to_ortho_only() {
        let path = unique_temp_path("both-ortho-and-polar-enabled");
        fs::write(&path, r#"{"ortho_enabled": true, "polar_enabled": true}"#).unwrap();
        let config = load(&path).unwrap().expect("file exists");
        assert!(config.ortho_enabled);
        assert!(!config.polar_enabled);
    }

    /// 正規化は直交側の値を変えない（`ortho_enabled` が `false` のときは
    /// `polar_enabled: true` をそのまま通す）。
    #[test]
    fn load_leaves_polar_enabled_alone_when_ortho_is_off() {
        let path = unique_temp_path("polar-enabled-ortho-off");
        fs::write(&path, r#"{"ortho_enabled": false, "polar_enabled": true}"#).unwrap();
        let config = load(&path).unwrap().expect("file exists");
        assert!(!config.ortho_enabled);
        assert!(config.polar_enabled);
    }

    #[test]
    fn corrupt_json_fails_to_load() {
        let path = unique_temp_path("corrupt");
        fs::write(&path, "not json at all {{{").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn missing_file_loads_as_none() {
        let path = unique_temp_path("does-not-exist");
        assert_eq!(load(&path), Ok(None));
    }

    #[test]
    fn save_then_load_round_trips_through_a_real_file() {
        let dir = std::env::temp_dir()
            .join("mcad-app-test")
            .join("nested-does-not-exist-yet");
        let path = dir.join("config.json");
        let _ = fs::remove_dir_all(&dir);

        let mut config = Config {
            snap_enabled: false,
            ..Config::default()
        };
        config.push_recent(PathBuf::from("/tmp/example.mcad"));

        save(&path, &config).expect("save should create parent dir and write file");
        assert!(path.exists());

        let loaded = load(&path)
            .expect("load should succeed")
            .expect("file exists");
        assert_eq!(loaded, config);
    }

    #[test]
    fn push_recent_inserts_at_front_dedups_and_caps_length() {
        let mut config = Config::default();
        for i in 0..MAX_RECENT_FILES {
            config.push_recent(PathBuf::from(format!("/tmp/{i}.mcad")));
        }
        assert_eq!(config.recent_files.len(), MAX_RECENT_FILES);
        assert_eq!(config.recent_files[0], PathBuf::from("/tmp/4.mcad"));

        // 既存パスを再度 push すると重複せず先頭へ移動する。
        config.push_recent(PathBuf::from("/tmp/1.mcad"));
        assert_eq!(config.recent_files.len(), MAX_RECENT_FILES);
        assert_eq!(config.recent_files[0], PathBuf::from("/tmp/1.mcad"));
        assert_eq!(
            config
                .recent_files
                .iter()
                .filter(|p| p.as_path() == Path::new("/tmp/1.mcad"))
                .count(),
            1
        );

        // 6件目を push すると末尾が溢れる。
        config.push_recent(PathBuf::from("/tmp/overflow.mcad"));
        assert_eq!(config.recent_files.len(), MAX_RECENT_FILES);
        assert!(!config.recent_files.contains(&PathBuf::from("/tmp/0.mcad")));
    }

    #[test]
    fn remember_sheet_defaults_updates_on_change_and_reports_true() {
        let mut config = Config::default();
        let sheet = SheetMeta {
            unit: Unit::Millimeter,
            scale: Scale::new(1, 2).unwrap(),
            paper: PaperSize::A2,
            orientation: Orientation::Portrait,
            title_block: TitleBlockKind::C,
            fields: TitleBlockFields::default(),
            frame_visible: true,
            use_bom: true,
        };
        assert!(config.remember_sheet_defaults(&sheet));
        assert_eq!(config.default_paper, PaperSize::A2);
        assert_eq!(config.default_orientation, Orientation::Portrait);
        assert_eq!(config.default_scale, Scale::new(1, 2).unwrap());
        assert_eq!(config.default_title_block, TitleBlockChoice::C);
    }

    #[test]
    fn remember_sheet_defaults_returns_false_when_nothing_changes() {
        let mut config = Config::default();
        let sheet = config.default_sheet_meta();
        assert!(!config.remember_sheet_defaults(&sheet));
    }

    #[test]
    fn remember_sheet_defaults_leaves_title_block_default_alone_for_custom() {
        let mut config = Config {
            default_title_block: TitleBlockChoice::A,
            ..Config::default()
        };
        let custom_template = TitleBlockKind::C.template().clone();
        let sheet = SheetMeta {
            unit: Unit::Millimeter,
            scale: Scale::ONE,
            paper: PaperSize::A4,
            orientation: Orientation::Landscape,
            title_block: TitleBlockKind::Custom(custom_template),
            fields: TitleBlockFields::default(),
            frame_visible: false,
            use_bom: true,
        };
        // 様式は据え置き(Custom は config に選択肢がないため)だが、それ以外が
        // 既定と一致しているので false。
        let changed = config.remember_sheet_defaults(&sheet);
        assert!(!changed);
        assert_eq!(config.default_title_block, TitleBlockChoice::A);
    }

    #[test]
    fn title_block_choice_round_trips_and_custom_maps_to_none() {
        for choice in [
            TitleBlockChoice::A,
            TitleBlockChoice::B,
            TitleBlockChoice::C,
        ] {
            let kind = choice.to_kind();
            assert_eq!(TitleBlockChoice::from_kind(&kind), Some(choice));
        }
        let custom = TitleBlockKind::Custom(TitleBlockKind::A.template().clone());
        assert_eq!(TitleBlockChoice::from_kind(&custom), None);
    }

    #[test]
    fn default_sheet_meta_uses_config_values_with_no_persisted_extras() {
        let config = Config {
            default_paper: PaperSize::A1,
            default_orientation: Orientation::Portrait,
            default_scale: Scale::new(1, 5).unwrap(),
            default_title_block: TitleBlockChoice::A,
            ..Config::default()
        };

        let sheet = config.default_sheet_meta();
        assert_eq!(sheet.unit, Unit::Millimeter);
        assert!(!sheet.frame_visible);
        assert_eq!(sheet.fields, TitleBlockFields::default());
        assert_eq!(sheet.paper, PaperSize::A1);
        assert_eq!(sheet.orientation, Orientation::Portrait);
        assert_eq!(sheet.scale, Scale::new(1, 5).unwrap());
        assert_eq!(sheet.title_block, TitleBlockKind::A);
    }
}
