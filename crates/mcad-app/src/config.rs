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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            snap_enabled: true,
            ortho_enabled: false,
            paper_display_enabled: false,
            default_paper: PaperSize::default(),
            default_orientation: Orientation::default(),
            default_scale: Scale::default(),
            default_title_block: TitleBlockChoice::default(),
            recent_files: Vec::new(),
        }
    }
}

impl Config {
    /// 新規文書用の [`SheetMeta`] を組み立てる。
    ///
    /// `fields`（表題欄記入内容）は空、`frame_visible` は `false`、`unit` は
    /// [`mcad_core::Unit::Millimeter`] 固定 — いずれも config の永続化対象外
    /// （記入内容は図面ごとに人が書くもの、枠表示は都度の作業状態、単位は現状 mm 固定）。
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
    let config: Config = serde_json::from_str(&text)
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
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
    }

    #[test]
    fn serde_round_trip_preserves_all_non_default_fields() {
        let config = Config {
            snap_enabled: false,
            ortho_enabled: true,
            paper_display_enabled: true,
            default_paper: PaperSize::A2,
            default_orientation: Orientation::Portrait,
            default_scale: Scale::new(1, 2).unwrap(),
            default_title_block: TitleBlockChoice::C,
            recent_files: vec![PathBuf::from("/tmp/a.mcad"), PathBuf::from("/tmp/b.mcad")],
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
