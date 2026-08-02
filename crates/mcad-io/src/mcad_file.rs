//! `.mcad`（JSON）のファイル形式と export/import。
//!
//! # 設計方針（DESIGN.md 3.3）
//!
//! [`mcad_core::Document`] は `SlotMap` の生キー（`LayerId`）と undo/redo 履歴を
//! 持つため **そのままシリアライズしない**。ファイルには次のポータブルな DTO
//! （[`FileDocument`]）だけを書く:
//!
//! - レイヤー参照は `LayerId` ではなく **`layers` 配列へのインデックス**（安定な
//!   ファイル ID）。
//! - レイヤー自体も core の [`Layer`] をそのまま流用せず、io 専用の DTO
//!   （[`FileLayer`]）として書く。core 側の型変更がファイル形式へ直接漏れないように
//!   するため。
//! - 生存中のレイヤー・エンティティのみを列挙する。undo/redo 履歴・墓標は保存しない。
//! - `version` フィールド必須（現行は `4`）。書き出しは常に v4。読込は v1〜v3 を
//!   後方互換で受理し、それ以外の未知バージョンは拒否する（[`from_json`]）。
//!
//! # バージョン履歴と後方互換の変換規則
//!
//! | version | 差分 | 読込時の変換 |
//! |---|---|---|
//! | 1 | 幾何が [`Shape`] のみ。レイヤーは `order` なし | [`EntityGeom::Shape`] で包む + `order = 配列インデックス` |
//! | 2 | 幾何が [`EntityGeom`]（M6 でテキスト・寸法を追加）。レイヤーは `order` なし | `order = 配列インデックス` |
//! | 3 | レイヤーに `order`（重ね順）を追加 | なし |
//! | 4 | 図面メタデータ `sheet`、レイヤーの `linetype`/`width_mm`、`style` の線幅を px から紙 mm の ByLayer モデルへ（M8）。現行 | `sheet = 既定`、レイヤーは `Continuous`/`0.35mm`、旧 `style.width` は下記の規則で移行 |
//!
//! `order` を持たない v1/v2 のレイヤーには **配列内のインデックスをそのまま
//! `order` として採用する**。export は常にデフォルトレイヤーを先頭に列挙してきた
//! ため、この規則で復元した重ね順は「ファイル内の元の並び」に一致し、`SlotMap` の
//! 未規定な反復順に依存しない。
//!
//! # 旧 `Style.width`（px）の移行（v1〜v3 → v4。DESIGN.md M8 設計判断6）
//!
//! 旧 `Style.width` は「画面 px 幅」で、旧描画は `max(width_px, 1.0)` px・ズーム
//! 非依存だった。破棄せず、**旧実装の実効表示幅**を決定的に換算して保存する:
//!
//! 1. **入力領域は有限の f32 のみ**。`.mcad` は JSON であり `NaN`/`Infinity` を
//!    表現できないため、非有限値を含むファイルはそもそもパースエラーになる
//!    （回帰検知は `legacy_width_infinity_is_a_parse_error`）。
//! 2. `width_px == 1.0`（シリアライザが書く既定値。UI から設定不能なので「未指定」）
//!    → `width_mm = None`（ByLayer 0.35mm）。未指定の既定値については、レイヤー幅へ
//!    追従することこそが意図した意味付けである。
//! 3. `width_px != 1.0`（手編集等の明示値）→ `width_mm = Some(0.35mm × max(width_px, 1.0))`。
//!    **ByLayer 化しない**（レイヤー編集の影響を受けない「エンティティ固有値」という
//!    旧の意味を保存する）。[`mcad_core::WidthMm::MAX_MM`] を超える分は上限へ
//!    クランプし、黙殺せず [`LoadSummary::clamped_widths`] に計上する。
//!
//! 保証するのは「実効表示幅の保存」であって生の格納値の保存ではない
//! （`zoom = 1/0.35` において、クランプされない全エンティティの画面幅が旧実装と
//! 厳密一致する）。詳細は DESIGN.md M8 設計判断6。
//!
//! # import の再構築
//!
//! import は [`Document::new`] から [`Command`] 列で内容を再構築する
//! （`Document` の内部フィールドへ直接触らない）。手順:
//!
//! 1. ファイル全体をバリデーション（バージョン・レイヤー参照・ジオメトリ）。
//! 2. ファイルの先頭レイヤーをデフォルトレイヤーのプロパティ差し替えに割り当て、
//!    残りを `AddLayer` で追加する（`Document` はデフォルトレイヤーを必ず 1 つ持ち、
//!    export はそれを先頭に列挙するため、この対応は往復で安定する）。
//! 3. エンティティを `AddEntity` で追加する。このとき **ロックは未適用** にして
//!    おく（ロック済みレイヤーへの `AddEntity` はコアが拒否するため、ロックの
//!    適用は全エンティティ投入後に行う）。
//! 4. ロックすべきレイヤーへ `SetLayerProps` でロックを適用する。
//! 5. [`Document::clear_history`] で履歴を空にする（読込直後の Ctrl+Z で再構築
//!    手順が巻き戻るのを防ぐ）。
//!
//! # ジオメトリのバリデーション
//!
//! 非有限座標（NaN/∞）・負や非有限の半径・空ポリラインは
//! [`IoError::InvalidGeometry`] として読込を拒否する。標準 JSON は NaN/∞ を表現
//! できないが、[`import_document`] は JSON を経由せず [`FileDocument`] を直接
//! 受け取る公開 API なので、io 境界としてここで検証する。
//!
//! v4 の尺度（[`Scale`]）・線幅（[`WidthMm`]）は core の検証済み型をそのまま
//! 使うため、不正値（0・負・範囲外）は serde の `try_from` が
//! [`IoError::Json`] として読込境界で弾く。**この拒否は v4 のみ**で、v1〜v3 の
//! 有限な旧線幅は拒否ではなく上記の規則で移行する。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use mcad_core::{
    Command, Document, Entity, EntityGeom, Layer, LayerId, Linetype, Rgb, SheetMeta, Style, WidthMm,
};
use mcad_geom::Shape;

use crate::IoError;

/// 現行のファイルフォーマットバージョン。
///
/// - v2（M6）で [`FileEntity::geom`] を [`EntityGeom`]（テキスト・寸法を含む）へ拡張。
/// - v3 でレイヤーに `order`（重ね順）を追加（[`FileLayer`]）。
/// - v4（M8）で図面メタデータ [`SheetMeta`]・レイヤーの線種/線幅・スタイルの
///   紙 mm 線幅（ByLayer モデル）を追加。
///
/// 書き出しは常に v4。v1〜v3 のファイルは [`from_json`] が後方互換で読み込む
/// （[`FileDocumentV1`] / [`FileDocumentV2`] / [`FileDocumentV3`] 参照）。
pub const FORMAT_VERSION: u32 = 4;

/// `.mcad` ファイル全体を表すポータブルな DTO（現行 v4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDocument {
    /// フォーマットバージョン。[`FORMAT_VERSION`] 以外は読込を拒否する。
    pub version: u32,
    /// 図面メタデータ（尺度・用紙・表題欄）。v4 で追加。
    ///
    /// レイヤーと違って io 専用 DTO を挟まず core の [`SheetMeta`] をそのまま書く。
    /// [`FileEntity::geom`]（[`EntityGeom`]）と同じ扱いで、理由も同じ:
    /// 型が深い木（ユーザー定義の表題欄様式）で、io 側に構造をそっくり複製すると
    /// 二重管理になるため。**core 側でこの木を変えたらフォーマットバージョンを
    /// 上げる**という対応は変わらない。
    pub sheet: SheetMeta,
    /// レイヤー一覧。先頭（インデックス 0）が `Document` のデフォルトレイヤーに
    /// 対応する。**配列の並びは重ね順ではない**（重ね順は [`FileLayer::order`]）。
    pub layers: Vec<FileLayer>,
    /// カレントレイヤー（`layers` へのインデックス）。
    pub current_layer: usize,
    /// エンティティ一覧。
    pub entities: Vec<FileEntity>,
}

/// `.mcad` ファイル内のレイヤー 1 枚（現行 v4）。
///
/// core の [`Layer`] とフィールドは一致するが、**ファイル形式を core の型定義から
/// 切り離すために別型として持つ**。core 側にフィールドが増えても、ここへ明示的に
/// 足さない限りファイル形式は変わらない（逆に、ここを変えるならフォーマット
/// バージョンを上げる、という対応が明確になる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileLayer {
    /// レイヤー名。
    pub name: String,
    /// レイヤー色。
    pub color: Rgb,
    /// レイヤー既定の線種。v4 で追加。
    pub linetype: Linetype,
    /// レイヤー既定の線幅（紙 mm）。v4 で追加。
    pub width_mm: WidthMm,
    /// 表示するか。
    pub visible: bool,
    /// ロックされているか。
    pub locked: bool,
    /// 重ね順（値が大きいほど手前）。v3 で追加。
    pub order: i32,
}

impl FileLayer {
    /// core の [`Layer`] から DTO を作る。
    fn from_core(layer: &Layer) -> Self {
        Self {
            name: layer.name.clone(),
            color: layer.color,
            linetype: layer.linetype,
            width_mm: layer.width_mm,
            visible: layer.visible,
            locked: layer.locked,
            order: layer.order,
        }
    }

    /// DTO から core の [`Layer`] を作る。
    fn to_core(&self) -> Layer {
        Layer {
            name: self.name.clone(),
            color: self.color,
            linetype: self.linetype,
            width_mm: self.width_mm,
            visible: self.visible,
            locked: self.locked,
            order: self.order,
        }
    }
}

/// `.mcad` ファイル内のエンティティ 1 件（v2 以降で共通）。
///
/// [`mcad_core::Entity`] と違い、所属レイヤーを `LayerId` ではなく
/// [`FileDocument::layers`] へのインデックスで参照する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntity {
    /// 所属レイヤー（[`FileDocument::layers`] へのインデックス）。
    pub layer: usize,
    /// 描画スタイル（v4 で線幅が紙 mm・線種つきの ByLayer モデルへ）。
    pub style: Style,
    /// 幾何形状（v2 でテキスト・寸法を含む [`EntityGeom`] へ拡張）。
    pub geom: EntityGeom,
}

/// v1〜v3 の読込結果（現行 DTO への移行で失われた情報の集計つき）。
///
/// v4 への移行で値を丸めたのは「読めなかった」ことではないので `Err` にはせず、
/// DXF import の `skipped_entities` と同じ流儀で件数を返し、呼び出し側が
/// ステータス表示できるようにする。
struct MigratedFile {
    /// 現行 v4 形式へ移行した内容。
    file: FileDocument,
    /// 旧線幅が上限 [`WidthMm::MAX_MM`] へクランプされたエンティティ数。
    clamped_widths: usize,
}

/// v2 以前のレイヤー 1 枚を表す凍結 DTO（後方互換読込専用）。
///
/// v1・v2 の時代のレイヤーは 4 フィールドで、`order` を持たなかった。この型は
/// **その当時の形のまま凍結する**（今後 core の [`Layer`] や [`FileLayer`] が
/// 変わっても追随しない）。v1 と v2 でレイヤー構造は同一だったため、両バージョンの
/// 読込がこの 1 つの型を共有する。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct FileLayerV2 {
    name: String,
    color: Rgb,
    visible: bool,
    locked: bool,
}

impl FileLayerV2 {
    /// 凍結 DTO を v3 のレイヤーへ変換する。
    ///
    /// `index` は `layers` 配列内の位置で、これをそのまま `order` に採用する
    /// （モジュール doc「後方互換の変換規則」参照）。
    fn into_v3(self, index: usize) -> FileLayerV3 {
        FileLayerV3 {
            name: self.name,
            color: self.color,
            visible: self.visible,
            locked: self.locked,
            order: i32::try_from(index).unwrap_or(i32::MAX),
        }
    }
}

/// v3 のレイヤー 1 枚を表す凍結 DTO（後方互換読込専用）。
///
/// v3 の時代のレイヤーは 5 フィールドで、線種・線幅を持たなかった。
/// [`FileLayerV2`] と同じく **その当時の形のまま凍結する**。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct FileLayerV3 {
    name: String,
    color: Rgb,
    visible: bool,
    locked: bool,
    order: i32,
}

impl FileLayerV3 {
    /// 凍結 DTO を現行の [`FileLayer`] へ変換する。
    ///
    /// 線種・線幅は v3 に存在しないので既定値（実線・0.35mm）を補う
    /// （DESIGN.md M8 設計判断6）。
    fn into_v4(self) -> FileLayer {
        FileLayer {
            name: self.name,
            color: self.color,
            linetype: Linetype::Continuous,
            width_mm: WidthMm::DEFAULT,
            visible: self.visible,
            locked: self.locked,
            order: self.order,
        }
    }
}

/// v1・v2 の `layers` 配列を v3 のレイヤー列へ変換する（`order = インデックス`）。
fn layers_v2_into_v3(layers: Vec<FileLayerV2>) -> Vec<FileLayerV3> {
    layers
        .into_iter()
        .enumerate()
        .map(|(index, layer)| layer.into_v3(index))
        .collect()
}

/// v3 以前のスタイルを表す凍結 DTO（後方互換読込専用）。
///
/// 当時の線幅は **画面 px**（`f32`）で、線種は存在しなかった。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
struct FileStyleV3 {
    color: Option<Rgb>,
    width: f32,
}

/// 旧 `Style.width` を紙 mm へ換算する係数。
///
/// 旧描画は `max(width_px, 1.0)` px をズーム非依存で使っていた。既定の線幅
/// 0.35mm がちょうど 1px に見えるズーム（`zoom = 1/0.35`）を基準に取ると、
/// 実効表示幅 `p` px は紙 `0.35 × p` mm に相当する（DESIGN.md M8 設計判断6）。
const LEGACY_PX_TO_MM: f64 = 0.35;

/// 旧 `Style.width`（px）を「未指定」とみなす値。
///
/// シリアライザが既定スタイルに対して書く値であり、旧 UI からは変更できなかった。
const LEGACY_DEFAULT_WIDTH_PX: f32 = 1.0;

impl FileStyleV3 {
    /// 凍結 DTO を現行の [`Style`] へ移行する。
    ///
    /// 戻り値の `bool` は「上限 [`WidthMm::MAX_MM`] へクランプしたか」で、
    /// 呼び出し側が [`LoadSummary::clamped_widths`] に計上する。規則の全文は
    /// モジュール doc「旧 `Style.width`（px）の移行」を参照。
    fn migrate(self) -> (Style, bool) {
        // 規則2: 既定値（未指定）は ByLayer 化する。
        if self.width == LEGACY_DEFAULT_WIDTH_PX {
            return (
                Style {
                    color: self.color,
                    width_mm: None,
                    linetype: None,
                },
                false,
            );
        }
        // 規則3: 明示値は「エンティティ固有値」として保存する。旧描画の下限 1px を
        // 先に効かせてから換算する（f64 で乗算してから f32 へ落とし、上限ちょうどの
        // 値が丸め誤差でクランプ扱いにならないようにする）。
        //
        // JSON は非有限値を表現できないので `self.width` は常に有限だが、
        // `f64::max` は NaN 側を無視するため、万一の NaN でも 1.0px として扱われる。
        let effective_px = f64::from(self.width).max(1.0);
        let mm = (LEGACY_PX_TO_MM * effective_px) as f32;
        let clamped = mm > WidthMm::MAX_MM;
        // `mm` は有限なので `WidthMm::clamped` は必ず `Some` を返す。
        // 万一の `None` は既定値へ倒す（防御的経路）。
        let width_mm = WidthMm::clamped(mm).unwrap_or(WidthMm::DEFAULT);
        (
            Style {
                color: self.color,
                width_mm: Some(width_mm),
                linetype: None,
            },
            clamped,
        )
    }
}

/// v2・v3 のエンティティ 1 件を表す凍結 DTO（後方互換読込専用）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileEntityV3 {
    layer: usize,
    style: FileStyleV3,
    geom: EntityGeom,
}

/// v2・v3 の `entities` 配列を現行の [`FileEntity`] 列へ移行し、線幅をクランプした
/// 件数を返す。
fn entities_v3_into_v4(entities: Vec<FileEntityV3>) -> (Vec<FileEntity>, usize) {
    let mut clamped_widths = 0usize;
    let migrated = entities
        .into_iter()
        .map(|e| {
            let (style, clamped) = e.style.migrate();
            if clamped {
                clamped_widths += 1;
            }
            FileEntity {
                layer: e.layer,
                style,
                geom: e.geom,
            }
        })
        .collect();
    (migrated, clamped_widths)
}

/// v3 ファイル全体を表す DTO（後方互換読込専用）。v4 との差は `sheet` が無く、
/// レイヤーが [`FileLayerV3`]（線種・線幅なし）、スタイルが [`FileStyleV3`]
/// （px 線幅）であること。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileDocumentV3 {
    version: u32,
    layers: Vec<FileLayerV3>,
    current_layer: usize,
    entities: Vec<FileEntityV3>,
}

impl FileDocumentV3 {
    /// v3 DTO を現行の v4 [`FileDocument`] へ変換する（バージョンは
    /// [`FORMAT_VERSION`] に更新、`sheet` は既定値）。
    fn into_v4(self) -> MigratedFile {
        let (entities, clamped_widths) = entities_v3_into_v4(self.entities);
        MigratedFile {
            file: FileDocument {
                version: FORMAT_VERSION,
                sheet: SheetMeta::default(),
                layers: self.layers.into_iter().map(FileLayerV3::into_v4).collect(),
                current_layer: self.current_layer,
                entities,
            },
            clamped_widths,
        }
    }
}

/// v2 ファイル全体を表す DTO（後方互換読込専用）。v3 との差はレイヤーが
/// [`FileLayerV2`]（`order` なし）であること。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileDocumentV2 {
    version: u32,
    layers: Vec<FileLayerV2>,
    current_layer: usize,
    entities: Vec<FileEntityV3>,
}

impl FileDocumentV2 {
    /// v2 DTO を v3 相当へ引き上げてから v4 へ変換する。
    fn into_v4(self) -> MigratedFile {
        FileDocumentV3 {
            version: 3,
            layers: layers_v2_into_v3(self.layers),
            current_layer: self.current_layer,
            entities: self.entities,
        }
        .into_v4()
    }
}

/// v1 ファイル全体を表す DTO（後方互換読込専用）。v2 との差は [`FileEntityV1`] の
/// `geom` が [`Shape`] であること。[`from_json`] が `version == 1` を検出したときのみ使う。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileDocumentV1 {
    version: u32,
    layers: Vec<FileLayerV2>,
    current_layer: usize,
    entities: Vec<FileEntityV1>,
}

/// v1 ファイル内のエンティティ 1 件（`geom` が [`Shape`]）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileEntityV1 {
    layer: usize,
    style: FileStyleV3,
    geom: Shape,
}

impl FileDocumentV1 {
    /// v1 DTO を v2 相当へ引き上げてから v4 へ変換する（各 `Shape` を
    /// [`EntityGeom::Shape`] で包む）。
    fn into_v4(self) -> MigratedFile {
        FileDocumentV2 {
            version: 2,
            layers: self.layers,
            current_layer: self.current_layer,
            entities: self
                .entities
                .into_iter()
                .map(|e| FileEntityV3 {
                    layer: e.layer,
                    style: e.style,
                    geom: EntityGeom::Shape(e.geom),
                })
                .collect(),
        }
        .into_v4()
    }
}

/// ドキュメントをポータブルな [`FileDocument`] へ変換する。
///
/// 生存中のレイヤー・エンティティのみを列挙する。undo/redo 履歴は含めない。
/// `Document` の不変条件（エンティティの所属レイヤーは必ず生存、カレントレイヤーは
/// 必ず生存）により、この変換は失敗しない。
#[must_use]
pub fn export_document(doc: &Document) -> FileDocument {
    let layers: Vec<(LayerId, Layer)> = doc.layers().map(|(id, l)| (id, l.clone())).collect();
    let layer_index = |id: LayerId| -> usize {
        layers
            .iter()
            .position(|(lid, _)| *lid == id)
            .expect("Document invariant: referenced layer must be alive")
    };

    let current_layer = layer_index(doc.current_layer());
    let entities = doc
        .entities()
        .map(|(_, e)| FileEntity {
            layer: layer_index(e.layer),
            style: e.style,
            geom: e.geom.clone(),
        })
        .collect();

    FileDocument {
        version: FORMAT_VERSION,
        sheet: doc.sheet().clone(),
        layers: layers
            .iter()
            .map(|(_, l)| FileLayer::from_core(l))
            .collect(),
        current_layer,
        entities,
    }
}

/// [`FileDocument`] から [`Document`] を再構築する。
///
/// 再構築の手順・レイヤー対応の規則はモジュール doc を参照。読込後の undo/redo
/// 履歴は空になる。
///
/// # Errors
///
/// 未知バージョン・レイヤーなし・範囲外のレイヤー参照・不正ジオメトリで
/// [`IoError`] を返す。図面メタデータ（ユーザー定義の表題欄様式の寸法）が不正な
/// 場合はコアが拒否し [`IoError::Core`] になる。
pub fn import_document(file: &FileDocument) -> Result<Document, IoError> {
    if file.version != FORMAT_VERSION {
        return Err(IoError::UnsupportedVersion(file.version));
    }
    if file.layers.is_empty() {
        return Err(IoError::NoLayers);
    }
    if file.current_layer >= file.layers.len() {
        return Err(IoError::BadCurrentLayer(file.current_layer));
    }
    for (index, entity) in file.entities.iter().enumerate() {
        if entity.layer >= file.layers.len() {
            return Err(IoError::BadLayerRef {
                index,
                layer: entity.layer,
            });
        }
        if let Err(reason) = entity.geom.validate() {
            return Err(IoError::InvalidGeometry { index, reason });
        }
    }

    let mut doc = Document::new();

    // 図面メタデータ。ユーザー定義の表題欄様式が不正ならコアが拒否する
    // （`IoError::Core` として伝播する）。
    doc.apply(Command::SetSheet(file.sheet.clone()))?;

    // レイヤー投入。ロックは全エンティティ投入後に適用するため、ここでは一旦
    // locked = false で作る。
    let mut layer_ids: Vec<LayerId> = Vec::with_capacity(file.layers.len());
    for (index, layer) in file.layers.iter().enumerate() {
        let unlocked = Layer {
            locked: false,
            ..layer.to_core()
        };
        if index == 0 {
            let id = doc.default_layer();
            doc.apply(Command::SetLayerProps {
                id,
                props: unlocked,
            })?;
            layer_ids.push(id);
        } else {
            let new_ids = doc.apply(Command::AddLayer(unlocked))?;
            layer_ids.push(new_ids.layers[0]);
        }
    }

    doc.apply(Command::SetCurrentLayer(layer_ids[file.current_layer]))?;

    for entity in &file.entities {
        doc.apply(Command::AddEntity(Entity::new(
            entity.geom.clone(),
            layer_ids[entity.layer],
            entity.style,
        )))?;
    }

    // ロックの適用（エンティティ投入後）。
    for (index, layer) in file.layers.iter().enumerate() {
        if layer.locked {
            doc.apply(Command::SetLayerProps {
                id: layer_ids[index],
                props: layer.to_core(),
            })?;
        }
    }

    doc.clear_history();
    Ok(doc)
}

/// ドキュメントを `.mcad` の JSON 文字列（整形済み）へシリアライズする。
///
/// # Errors
///
/// シリアライズ失敗時に [`IoError::Json`] を返す（通常は起きない）。
pub fn to_json(doc: &Document) -> Result<String, IoError> {
    Ok(serde_json::to_string_pretty(&export_document(doc))?)
}

/// `.mcad` 読込の結果。
///
/// 旧バージョンからの移行で値を丸めたことは失敗ではないため `Result` の `Err` には
/// せず、この構造体の件数として返す（DXF import の
/// [`ImportSummary`](crate::ImportSummary) と同じ流儀）。
///
/// `Document` が `Debug` を実装していないため、この構造体自体も `Debug` は
/// 導出しない。
pub struct LoadSummary {
    /// 再構築されたドキュメント。
    pub document: Document,
    /// 旧 `Style.width`（px）の移行で、線幅を上限 [`WidthMm::MAX_MM`] mm へ
    /// クランプしたエンティティ数（v4 ファイルでは常に 0）。
    ///
    /// 黙って値を変えたことに気づけるよう、呼び出し側はこれをステータス表示する
    /// （DESIGN.md M8 設計判断6 の規則3）。
    pub clamped_widths: usize,
}

/// `.mcad` の JSON 文字列からドキュメントを再構築する。
///
/// # バージョン互換
///
/// 先頭でフォーマットバージョンだけを読み、バージョンごとに DTO を選ぶ:
///
/// - `1`: [`FileDocumentV1`]（幾何は [`Shape`]、レイヤーは `order` なし）
/// - `2`: [`FileDocumentV2`]（レイヤーは `order` なし）
/// - `3`: [`FileDocumentV3`]（`sheet` なし、レイヤーは線種・線幅なし、線幅は px）
/// - `4`（[`FORMAT_VERSION`]）: 現行の [`FileDocument`]
///
/// それ以外は [`IoError::UnsupportedVersion`] を返す。書き出しは常に v4。
///
/// v3 以前の旧線幅はモジュール doc の規則で移行し、クランプ件数を
/// [`LoadSummary::clamped_widths`] で返す。
///
/// v4 として読むファイルのレイヤーに `order` が欠けていれば [`IoError::Json`] で
/// 失敗する（暗黙の既定値で埋めない = 壊れたファイルを検出できる）。同様に、v4 の
/// 不正な尺度・線幅（0・負・範囲外）も検証済み型の `try_from` が
/// [`IoError::Json`] として弾く。
///
/// # Errors
///
/// JSON 構文・型の不一致・検証済み型の範囲外は [`IoError::Json`]、未知バージョンは
/// [`IoError::UnsupportedVersion`]、その他フォーマット上の異常は
/// [`import_document`] と同じ [`IoError`] を返す。
pub fn from_json(json: &str) -> Result<LoadSummary, IoError> {
    /// バージョンだけを先読みするための最小 DTO。
    #[derive(Deserialize)]
    struct VersionProbe {
        version: u32,
    }
    let probe: VersionProbe = serde_json::from_str(json)?;
    let migrated = match probe.version {
        1 => serde_json::from_str::<FileDocumentV1>(json)?.into_v4(),
        2 => serde_json::from_str::<FileDocumentV2>(json)?.into_v4(),
        3 => serde_json::from_str::<FileDocumentV3>(json)?.into_v4(),
        FORMAT_VERSION => MigratedFile {
            file: serde_json::from_str::<FileDocument>(json)?,
            clamped_widths: 0,
        },
        other => return Err(IoError::UnsupportedVersion(other)),
    };
    Ok(LoadSummary {
        document: import_document(&migrated.file)?,
        clamped_widths: migrated.clamped_widths,
    })
}

/// ドキュメントを `.mcad` ファイルへ保存する。
///
/// # Errors
///
/// シリアライズ失敗（[`IoError::Json`]）・書き込み失敗（[`IoError::File`]）を返す。
pub fn save_mcad(doc: &Document, path: impl AsRef<Path>) -> Result<(), IoError> {
    fs::write(path, to_json(doc)?)?;
    Ok(())
}

/// `.mcad` ファイルからドキュメントを読み込む。
///
/// 戻り値は [`LoadSummary`]（ドキュメント + 旧線幅のクランプ件数）。
///
/// # Errors
///
/// 読み取り失敗（[`IoError::File`]）・JSON/フォーマット異常（[`from_json`] と同じ）を返す。
pub fn load_mcad(path: impl AsRef<Path>) -> Result<LoadSummary, IoError> {
    from_json(&fs::read_to_string(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::{
        MAX_TITLE_BLOCK_CELLS_PER_ROW, MAX_TITLE_BLOCK_ROWS, Orientation, PaperSize,
        ProjectionMethod, Rgb, Scale, TitleBlockFields, TitleBlockKind, TitleBlockTemplate,
    };
    use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline};

    /// [`from_json`] の戻り値からドキュメントだけを取り出す（クランプ件数を検証
    /// しないテスト用。件数を見るテストは [`from_json`] を直接呼ぶ）。
    fn load(json: &str) -> Result<Document, IoError> {
        from_json(json).map(|s| s.document)
    }

    /// レイヤー2枚（1枚はロック+非表示+色付き）・全 Shape 種・個別スタイル・
    /// 非デフォルトのカレントレイヤーを持つドキュメントを作る。
    fn full_document() -> Document {
        let mut doc = Document::new();
        let default = doc.default_layer();

        let second = doc
            .apply(Command::AddLayer(Layer::new(
                "second",
                Rgb::new(200, 40, 40),
            )))
            .unwrap()
            .layers[0];

        let shapes = [
            Shape::Point(Point2::new(1.0, 2.0)),
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(3.0, 4.0))),
            Shape::Circle(Circle::new(Point2::new(-1.0, 5.0), 2.5)),
            Shape::Arc(Arc::new(Point2::new(2.0, 2.0), 1.5, 0.3, 2.8)),
            Shape::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(1.0, 1.0),
                    Point2::new(2.0, 0.0),
                ],
                true,
            )),
        ];
        for (i, shape) in shapes.into_iter().enumerate() {
            let layer = if i % 2 == 0 { default } else { second };
            let style = if i == 0 {
                Style {
                    color: Some(Rgb::new(10, 20, 30)),
                    width_mm: Some(WidthMm::new(0.7).unwrap()),
                    linetype: Some(Linetype::DashDotDot),
                }
            } else {
                Style::inherited()
            };
            doc.apply(Command::AddEntity(Entity::new(shape, layer, style)))
                .unwrap();
        }

        // second をカレントにし、ロック+非表示にする。
        doc.apply(Command::SetCurrentLayer(second)).unwrap();
        let mut props = doc.layer(second).unwrap().clone();
        props.locked = true;
        props.visible = false;
        // 重ね順も既定値から動かしておき、往復テストが order を含めて検証するようにする。
        props.order = 7;
        // 線種・線幅も既定値から動かす（v4 の往復検証のため）。
        props.linetype = Linetype::DashDot;
        props.width_mm = WidthMm::new(0.13).unwrap();
        doc.apply(Command::SetLayerProps { id: second, props })
            .unwrap();

        // 図面メタデータも全項目を既定から動かす。
        doc.apply(Command::SetSheet(full_sheet())).unwrap();
        doc
    }

    /// 既定値と全項目が異なる図面メタデータ。
    fn full_sheet() -> SheetMeta {
        SheetMeta {
            unit: mcad_core::Unit::Millimeter,
            scale: Scale::new(2, 5).unwrap(),
            paper: PaperSize::A3,
            orientation: Orientation::Portrait,
            title_block: TitleBlockKind::C,
            fields: TitleBlockFields {
                drawing_number: "MCAD-001".to_owned(),
                drawing_title: "ブラケット".to_owned(),
                projection: ProjectionMethod::FirstAngle,
                author: "almaz".to_owned(),
                date: "2026-08-01".to_owned(),
                revision: "B".to_owned(),
            },
            frame_visible: true,
        }
    }

    #[test]
    fn round_trip_empty_document() {
        let doc = Document::new();
        let exported = export_document(&doc);
        assert_eq!(exported.version, FORMAT_VERSION);
        assert_eq!(exported.entities.len(), 0);
        assert_eq!(exported.layers.len(), 1);
        assert_eq!(exported.current_layer, 0);

        let imported = import_document(&exported).unwrap();
        assert_eq!(export_document(&imported), exported);
    }

    #[test]
    fn round_trip_preserves_semantic_content() {
        // export → import → export が意味的に一致する（SlotMap の実キー一致は
        // 要求しない。FileDocument 上の比較がその「意味的同値」にあたる）。
        let doc = full_document();
        let exported = export_document(&doc);
        let imported = import_document(&exported).unwrap();
        assert_eq!(export_document(&imported), exported);

        // 内容の抜き取り確認: エンティティ数・レイヤー数・カレントが保たれている。
        assert_eq!(imported.entity_count(), doc.entity_count());
        assert_eq!(imported.layer_count(), doc.layer_count());
        let cur = imported.layer(imported.current_layer()).unwrap();
        assert_eq!(cur.name, "second");
        assert!(cur.locked);
        assert!(!cur.visible);
    }

    #[test]
    fn json_string_round_trip() {
        let doc = full_document();
        let json = to_json(&doc).unwrap();
        let imported = load(&json).unwrap();
        assert_eq!(export_document(&imported), export_document(&doc));
    }

    #[test]
    fn import_leaves_history_empty() {
        let imported = import_document(&export_document(&full_document())).unwrap();
        assert!(!imported.can_undo(), "読込直後に undo できてはならない");
        assert!(!imported.can_redo());
    }

    #[test]
    fn locked_layer_entities_survive_import() {
        // ロック済みレイヤー上のエンティティも import で復元される
        // （ロックの適用がエンティティ投入より後であることの検証）。
        let doc = full_document();
        let locked_entities = doc
            .entities()
            .filter(|(_, e)| doc.layer(e.layer).is_some_and(|l| l.locked))
            .count();
        assert!(
            locked_entities > 0,
            "テスト前提: ロック層にエンティティがある"
        );

        let imported = import_document(&export_document(&doc)).unwrap();
        let imported_locked = imported
            .entities()
            .filter(|(_, e)| imported.layer(e.layer).is_some_and(|l| l.locked))
            .count();
        assert_eq!(imported_locked, locked_entities);
    }

    #[test]
    fn unsupported_version_fails() {
        // 現行は v4。未知の将来バージョン（5）は拒否する。
        let mut file = export_document(&Document::new());
        file.version = 5;
        assert!(matches!(
            import_document(&file),
            Err(IoError::UnsupportedVersion(5))
        ));
    }

    #[test]
    fn v3_round_trip_preserves_layer_order() {
        // v3 の往復で重ね順が保たれる（配列順ではなく order フィールドが根拠）。
        let mut doc = Document::new();
        let default = doc.default_layer();

        // デフォルトレイヤーを一番手前（order = 10）へ、後から足す 2 枚をその奥へ置く。
        // → 配列順（デフォルトが先頭）と重ね順が一致しない状態を作る。
        let mut props = doc.layer(default).unwrap().clone();
        props.order = 10;
        doc.apply(Command::SetLayerProps { id: default, props })
            .unwrap();
        for (name, order) in [("mid", 5), ("back", -3)] {
            let mut layer = Layer::new(name, Rgb::new(1, 2, 3));
            layer.order = order;
            doc.apply(Command::AddLayer(layer)).unwrap();
        }

        let expected: Vec<(String, i32)> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(
            expected,
            vec![
                ("back".to_owned(), -3),
                ("mid".to_owned(), 5),
                ("0".to_owned(), 10),
            ]
        );

        let loaded = load(&to_json(&doc).unwrap()).unwrap();
        let actual: Vec<(String, i32)> = loaded
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(actual, expected);
        // デフォルトレイヤーの対応（先頭 = デフォルト）も往復で保たれる。
        assert_eq!(loaded.layer(loaded.default_layer()).unwrap().name, "0");
        assert_eq!(export_document(&loaded), export_document(&doc));
    }

    #[test]
    fn v2_file_is_read_with_backward_compat() {
        // v0.7.x 以前の v2 ファイル。レイヤーに order キーが無い。
        // 幾何は v2 以降の EntityGeom（"Shape" ラッパーあり）。
        let v2_json = r#"{
          "version": 2,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false},
            {"name": "middle", "color": {"r": 10, "g": 20, "b": 30}, "visible": false, "locked": true},
            {"name": "top", "color": {"r": 40, "g": 50, "b": 60}, "visible": true, "locked": false}
          ],
          "current_layer": 2,
          "entities": [
            {"layer": 1, "style": {"color": null, "width": 1.0},
             "geom": {"Shape": {"Line": {"a": {"x": 0.0, "y": 0.0}, "b": {"x": 3.0, "y": 4.0}}}}}
          ]
        }"#;

        let doc = load(v2_json).expect("v2 ファイルは後方互換で読み込めるべき");

        // order は配列インデックスで復元される（並びはファイル内の元の順序）。
        let ordered: Vec<(String, i32)> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(
            ordered,
            vec![
                ("0".to_owned(), 0),
                ("middle".to_owned(), 1),
                ("top".to_owned(), 2),
            ]
        );

        // 他のプロパティ・カレントレイヤー・エンティティも従来どおり復元される。
        let (_, middle) = doc.layers().find(|(_, l)| l.name == "middle").unwrap();
        assert!(middle.locked && !middle.visible);
        assert_eq!(doc.layer(doc.current_layer()).unwrap().name, "top");
        assert_eq!(doc.entity_count(), 1);

        // 読込後の書き出しは常に v3。
        assert_eq!(export_document(&doc).version, FORMAT_VERSION);
    }

    #[test]
    fn v1_file_layer_order_falls_back_to_array_index() {
        // v1 もレイヤーは order なし。v2 と同じ「order = 配列インデックス」規則を通る。
        let v1_json = r#"{
          "version": 1,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false},
            {"name": "front", "color": {"r": 1, "g": 2, "b": 3}, "visible": true, "locked": false}
          ],
          "current_layer": 0,
          "entities": []
        }"#;

        let doc = load(v1_json).unwrap();
        let ordered: Vec<(String, i32)> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(ordered, vec![("0".to_owned(), 0), ("front".to_owned(), 1)]);
    }

    #[test]
    fn v3_file_without_layer_order_is_rejected() {
        // v3 と自称するファイルのレイヤーに order が無いのは壊れたファイル。
        // 暗黙の既定値で埋めず、JSON エラーとして弾く（core の Layer に
        // serde(default) を付けない理由そのもの）。
        let broken = r#"{
          "version": 3,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false}
          ],
          "current_layer": 0,
          "entities": []
        }"#;
        assert!(matches!(load(broken), Err(IoError::Json(_))));
    }

    #[test]
    fn v1_file_is_read_with_backward_compat() {
        // v0.5.0 以前の v1 ファイル。幾何は Shape が直下に来る（v2 の "Shape" ラッパーなし）。
        let v1_json = r#"{
          "version": 1,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false}
          ],
          "current_layer": 0,
          "entities": [
            {"layer": 0, "style": {"color": null, "width": 1.0},
             "geom": {"Line": {"a": {"x": 0.0, "y": 0.0}, "b": {"x": 3.0, "y": 4.0}}}}
          ]
        }"#;

        let doc = load(v1_json).expect("v1 ファイルは後方互換で読み込めるべき");
        assert_eq!(doc.entity_count(), 1);
        let (_, entity) = doc.entities().next().unwrap();
        // v1 の Shape は EntityGeom::Shape へ包まれて受理される。
        assert_eq!(
            entity.geom,
            EntityGeom::Shape(Shape::Line(LineSeg::new(
                Point2::new(0.0, 0.0),
                Point2::new(3.0, 4.0),
            )))
        );

        // 読込後の書き出しは常に現行バージョン。往復しても内容が保たれる。
        let reexported = export_document(&doc);
        assert_eq!(reexported.version, FORMAT_VERSION);
        let reimported = import_document(&reexported).unwrap();
        assert_eq!(export_document(&reimported), reexported);
    }

    // --- v4 移行（DESIGN.md M8 設計判断6、タスク35b）---

    /// 旧線幅 `width_px` を 1 件だけ持つ v3 ファイルの JSON。
    fn v3_json_with_width(width_px: &str) -> String {
        format!(
            r#"{{
              "version": 3,
              "layers": [
                {{"name": "0", "color": {{"r": 255, "g": 255, "b": 255}},
                 "visible": true, "locked": false, "order": 0}}
              ],
              "current_layer": 0,
              "entities": [
                {{"layer": 0, "style": {{"color": null, "width": {width_px}}},
                 "geom": {{"Shape": {{"Line": {{"a": {{"x": 0.0, "y": 0.0}}, "b": {{"x": 1.0, "y": 0.0}}}}}}}}}}
              ]
            }}"#
        )
    }

    /// v3 ファイルを読み、唯一のエンティティの線幅とクランプ件数を返す。
    fn migrated_width(width_px: &str) -> (Option<f32>, usize) {
        let summary = from_json(&v3_json_with_width(width_px)).expect("v3 は読めるべき");
        let (_, entity) = summary.document.entities().next().unwrap();
        (
            entity.style.width_mm.map(WidthMm::mm),
            summary.clamped_widths,
        )
    }

    #[test]
    fn legacy_default_width_becomes_bylayer() {
        // 規則2: 1.0px は「未指定」なので ByLayer（None）にする。
        assert_eq!(migrated_width("1.0"), (None, 0));
    }

    #[test]
    fn legacy_explicit_width_is_converted_to_paper_mm() {
        // 規則3: 明示値は 0.35mm × 実効表示幅 px（ByLayer 化しない）。
        assert_eq!(migrated_width("2.5"), (Some(0.875), 0));
    }

    #[test]
    fn legacy_sub_pixel_and_negative_widths_use_the_one_pixel_floor() {
        // 旧描画の `max(width_px, 1.0)` を先に効かせるので、どちらも実効 1px = 0.35mm。
        assert_eq!(migrated_width("0.5"), (Some(0.35), 0));
        assert_eq!(migrated_width("-3.0"), (Some(0.35), 0));
    }

    #[test]
    fn legacy_width_at_the_clamp_boundary_is_not_counted() {
        // 実効 5.0/0.35 px はちょうど上限 5.0mm。クランプ扱いにはしない
        // （境界が丸め誤差で計上側へ倒れないことの回帰テスト）。
        let boundary = (f64::from(WidthMm::MAX_MM) / LEGACY_PX_TO_MM).to_string();
        assert_eq!(migrated_width(&boundary), (Some(WidthMm::MAX_MM), 0));
    }

    #[test]
    fn legacy_width_above_the_limit_is_clamped_and_counted() {
        // 上限超えは 5.0mm へ丸め、黙殺せず件数に計上する。
        assert_eq!(migrated_width("20.0"), (Some(WidthMm::MAX_MM), 1));
    }

    #[test]
    fn legacy_width_infinity_is_a_parse_error() {
        // 規則1: JSON は非有限値を表現できない。`Infinity` リテラルは構文エラーとして
        // 弾かれ、移行規則の入力領域から外れる（DESIGN.md M8 設計判断6 の規則1）。
        assert!(matches!(
            from_json(&v3_json_with_width("Infinity")),
            Err(IoError::Json(_))
        ));
        assert!(matches!(
            from_json(&v3_json_with_width("NaN")),
            Err(IoError::Json(_))
        ));
    }

    #[test]
    fn legacy_clamped_widths_are_counted_per_entity() {
        // 複数エンティティのうち、クランプされたものだけが計上される。
        let json = r#"{
          "version": 2,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false}
          ],
          "current_layer": 0,
          "entities": [
            {"layer": 0, "style": {"color": null, "width": 1.0},
             "geom": {"Shape": {"Point": {"x": 0.0, "y": 0.0}}}},
            {"layer": 0, "style": {"color": null, "width": 30.0},
             "geom": {"Shape": {"Point": {"x": 1.0, "y": 0.0}}}},
            {"layer": 0, "style": {"color": null, "width": 100.0},
             "geom": {"Shape": {"Point": {"x": 2.0, "y": 0.0}}}}
          ]
        }"#;
        let summary = from_json(json).unwrap();
        assert_eq!(summary.clamped_widths, 2);
        assert_eq!(summary.document.entity_count(), 3);
    }

    #[test]
    fn v3_file_gets_default_sheet_and_layer_line_properties() {
        // v3 には sheet も線種・線幅も無いので既定値を補う
        // （A4 横・1:1・様式B・枠非表示 / 実線・0.35mm）。
        let doc = load(&v3_json_with_width("1.0")).unwrap();
        assert_eq!(*doc.sheet(), SheetMeta::default());
        assert!(!doc.sheet().frame_visible);
        let (_, layer) = doc.layers().next().unwrap();
        assert_eq!(layer.linetype, Linetype::Continuous);
        assert_eq!(layer.width_mm, WidthMm::DEFAULT);
        // 書き出しは常に v4。
        assert_eq!(export_document(&doc).version, FORMAT_VERSION);
    }

    #[test]
    fn v4_round_trip_is_lossless_including_sheet_and_line_properties() {
        // 図面メタデータ・レイヤーの線種/線幅・スタイルの上書きまで含めて無損失。
        let doc = full_document();
        let summary = from_json(&to_json(&doc).unwrap()).unwrap();
        assert_eq!(summary.clamped_widths, 0);
        let loaded = summary.document;
        assert_eq!(export_document(&loaded), export_document(&doc));

        // 抜き取り確認: sheet の各項目が保たれている。
        assert_eq!(*loaded.sheet(), full_sheet());
        assert_eq!(loaded.sheet().scale, Scale::new(2, 5).unwrap());
        assert_eq!(loaded.sheet().paper, PaperSize::A3);
        assert_eq!(loaded.sheet().orientation, Orientation::Portrait);
        assert_eq!(loaded.sheet().title_block, TitleBlockKind::C);
        assert!(loaded.sheet().frame_visible);

        // レイヤーの線種・線幅、エンティティ側の上書きも保たれている。
        let (_, second) = loaded.layers().find(|(_, l)| l.name == "second").unwrap();
        assert_eq!(second.linetype, Linetype::DashDot);
        assert_eq!(second.width_mm.mm(), 0.13);
        let overridden = loaded
            .entities()
            .find(|(_, e)| e.style.width_mm.is_some())
            .expect("線幅を個別指定したエンティティがあるはず");
        assert_eq!(overridden.1.style.width_mm.map(WidthMm::mm), Some(0.7));
        assert_eq!(overridden.1.style.linetype, Some(Linetype::DashDotDot));
    }

    #[test]
    fn v4_custom_title_block_round_trips() {
        // ユーザー定義様式（M8 では編集 UI を持たないがデータは往復する）。
        let mut template = TitleBlockTemplate::standard_a().clone();
        template.rows[0].cells[0].text_height_mm = 7.0;
        let mut doc = Document::new();
        doc.apply(Command::SetSheet(SheetMeta {
            title_block: TitleBlockKind::Custom(template.clone()),
            ..SheetMeta::default()
        }))
        .unwrap();

        let loaded = load(&to_json(&doc).unwrap()).unwrap();
        assert_eq!(
            loaded.sheet().title_block,
            TitleBlockKind::Custom(template.clone())
        );
        assert_eq!(loaded.sheet().title_block.template(), &template);
    }

    #[test]
    fn v4_file_missing_new_fields_is_rejected() {
        // v3 と同じ規律: v4 と自称するのに v4 のフィールドが欠けているのは壊れた
        // ファイル。暗黙の既定値で埋めず JSON エラーとして弾く（`sheet` を欠く、
        // レイヤーの `linetype`/`width_mm` を欠く、の両方）。
        let full = v4_json(r#"{"num": 1, "den": 1}"#, "0.35", "null");
        assert!(load(&full).is_ok(), "テスト前提: 完全な v4 は読める");

        // `sheet` を欠く v4。
        let without_sheet = r#"{
          "version": 4,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255},
             "linetype": "Continuous", "width_mm": 0.35,
             "visible": true, "locked": false, "order": 0}
          ],
          "current_layer": 0,
          "entities": []
        }"#;
        assert!(matches!(load(without_sheet), Err(IoError::Json(_))));

        // レイヤーの `linetype`/`width_mm` を欠く v4。
        let without_line_props = full.replace(r#""linetype": "Continuous", "width_mm": 0.35,"#, "");
        assert_ne!(without_line_props, full, "テスト前提: 置換が効いている");
        assert!(matches!(load(&without_line_props), Err(IoError::Json(_))));
    }

    #[test]
    fn v4_rejects_invalid_scale_at_the_read_boundary() {
        // 検証済み型の try_from が読込境界で弾く（core コマンドまで届かない）。
        for scale in [
            r#"{"num": 0, "den": 1}"#,
            r#"{"num": 1, "den": 0}"#,
            r#"{"num": 0, "den": 0}"#,
            r#"{"num": 2000000, "den": 1}"#,
            r#"{"num": -1, "den": 2}"#,
        ] {
            let json = v4_json(scale, "0.35", "null");
            assert!(
                matches!(from_json(&json), Err(IoError::Json(_))),
                "scale {scale} should be rejected"
            );
        }
        // 妥当な尺度は通る。
        let ok = v4_json(r#"{"num": 1, "den": 2}"#, "0.35", "null");
        assert_eq!(
            from_json(&ok).unwrap().document.sheet().scale,
            Scale::new(1, 2).unwrap()
        );
    }

    #[test]
    fn v4_rejects_invalid_line_width_at_the_read_boundary() {
        let scale = r#"{"num": 1, "den": 1}"#;
        // レイヤー線幅（必須値）とエンティティ線幅（上書き）の両方で拒否する。
        for width in ["0.0", "-0.5", "0.01", "9.0", "1e400"] {
            assert!(
                matches!(
                    from_json(&v4_json(scale, width, "null")),
                    Err(IoError::Json(_))
                ),
                "layer width {width} should be rejected"
            );
            assert!(
                matches!(
                    from_json(&v4_json(scale, "0.35", width)),
                    Err(IoError::Json(_))
                ),
                "entity width {width} should be rejected"
            );
        }
        // 非有限リテラルは JSON 構文としても不正。
        assert!(matches!(
            from_json(&v4_json(scale, "Infinity", "null")),
            Err(IoError::Json(_))
        ));
        // 範囲内の値は通り、ByLayer（null）も受理される。
        let ok = from_json(&v4_json(scale, "0.13", "2.0")).unwrap().document;
        let (_, layer) = ok.layers().next().unwrap();
        assert_eq!(layer.width_mm.mm(), 0.13);
        let (_, entity) = ok.entities().next().unwrap();
        assert_eq!(entity.style.width_mm.map(WidthMm::mm), Some(2.0));
    }

    #[test]
    fn v4_rejects_oversized_custom_title_block_dimensions() {
        // 手編集された v4 のユーザー定義様式は「個々には有限」な巨大値を持てる。
        // 検証済み型では防げない（`TitleBlockTemplate` のフィールドは素の f64）ので、
        // serde → Command::SetSheet の経路でコアが拒否することを固定する
        // （Codex M8 アドバーサリアルレビュー(2026-08-02) medium 指摘）。

        // (a) f64::MAX のセル幅（単一の巨大な有限値）。
        let huge_cell = custom(&row("15.0", &cell("1.7976931348623157e308")));
        assert!(
            matches!(
                load(&v4_json_with_title_block(&huge_cell)),
                Err(IoError::Core(mcad_core::CoreError::InvalidSheet(_)))
            ),
            "f64::MAX のセル幅は拒否されるべき"
        );

        // (b) 個々は上限内でも合算が上限を超える構成（セル幅の合計・段高さの合計）。
        let wide_row = custom(&row(
            "15.0",
            &format!("{}, {}", cell("9000.0"), cell("9000.0")),
        ));
        assert!(matches!(
            load(&v4_json_with_title_block(&wide_row)),
            Err(IoError::Core(mcad_core::CoreError::InvalidSheet(_)))
        ));
        let tall = custom(&format!(
            "{}, {}",
            row("9000.0", &cell("170.0")),
            row("9000.0", &cell("170.0"))
        ));
        assert!(matches!(
            load(&v4_json_with_title_block(&tall)),
            Err(IoError::Core(mcad_core::CoreError::InvalidSheet(_)))
        ));

        // 妥当なユーザー定義様式は通る（拒否が広すぎないことの確認）。
        let sane = custom(&row("15.0", &cell("170.0")));
        let doc = load(&v4_json_with_title_block(&sane)).expect("妥当な様式は読めるべき");
        assert_eq!(doc.sheet().title_block.template().height_mm(), 15.0);
    }

    #[test]
    fn v4_rejects_custom_title_block_with_too_many_elements() {
        // 寸法をすべて極小にすれば合計は上限内に収まるが、要素数が多いと表題欄の
        // 派生描画（毎フレーム全要素を走査）が持続的なコストになる。件数上限で
        // 読込境界から断つ（Codex M8 アドバーサリアルレビュー 2 周目(2026-08-02) medium 指摘）。
        let tiny_cell = cell("0.000001");
        let tiny_row = row("0.000001", &tiny_cell);

        let rows_at_limit = custom(&repeat_json(&tiny_row, MAX_TITLE_BLOCK_ROWS));
        assert!(
            load(&v4_json_with_title_block(&rows_at_limit)).is_ok(),
            "段数が上限ちょうどなら受理されるべき"
        );

        let rows_over_limit = custom(&repeat_json(&tiny_row, MAX_TITLE_BLOCK_ROWS + 1));
        assert!(matches!(
            load(&v4_json_with_title_block(&rows_over_limit)),
            Err(IoError::Core(mcad_core::CoreError::InvalidSheet(_)))
        ));

        let cells_at_limit = custom(&row(
            "15.0",
            &repeat_json(&tiny_cell, MAX_TITLE_BLOCK_CELLS_PER_ROW),
        ));
        assert!(
            load(&v4_json_with_title_block(&cells_at_limit)).is_ok(),
            "セル数が上限ちょうどなら受理されるべき"
        );

        let cells_over_limit = custom(&row(
            "15.0",
            &repeat_json(&tiny_cell, MAX_TITLE_BLOCK_CELLS_PER_ROW + 1),
        ));
        assert!(matches!(
            load(&v4_json_with_title_block(&cells_over_limit)),
            Err(IoError::Core(mcad_core::CoreError::InvalidSheet(_)))
        ));
    }

    #[test]
    fn v4_rejects_empty_custom_title_block() {
        // 構造の最小条件（段 1 つ以上・各段にセル 1 つ以上）も読込境界で効く。
        let no_rows = r#"{"Custom": {"width_mm": 170.0, "rows": []}}"#;
        assert!(matches!(
            load(&v4_json_with_title_block(no_rows)),
            Err(IoError::Core(mcad_core::CoreError::InvalidSheet(_)))
        ));

        let no_cells = custom(&row("15.0", ""));
        assert!(matches!(
            load(&v4_json_with_title_block(&no_cells)),
            Err(IoError::Core(mcad_core::CoreError::InvalidSheet(_)))
        ));
    }

    /// 表題欄セル 1 つの JSON。
    fn cell(width: &str) -> String {
        format!(r#"{{"width_mm": {width}, "bind": "DrawingTitle", "text_height_mm": 5.0}}"#)
    }

    /// 表題欄の段 1 つの JSON（`cells` は連結済みのセル列）。
    fn row(height: &str, cells: &str) -> String {
        format!(r#"{{"height_mm": {height}, "cells": [{cells}]}}"#)
    }

    /// ユーザー定義様式の `title_block` JSON（`rows` は連結済みの段列）。
    fn custom(rows: &str) -> String {
        format!(r#"{{"Custom": {{"width_mm": 170.0, "rows": [{rows}]}}}}"#)
    }

    /// JSON 要素を `count` 個カンマ区切りで並べる。
    fn repeat_json(element: &str, count: usize) -> String {
        vec![element; count].join(", ")
    }

    /// `title_block` の JSON だけを差し替えられる最小の v4 JSON。
    fn v4_json_with_title_block(title_block: &str) -> String {
        v4_json(r#"{"num": 1, "den": 1}"#, "0.35", "null").replace(
            r#""title_block": "B","#,
            &format!(r#""title_block": {title_block},"#),
        )
    }

    /// レイヤー線幅・エンティティ線幅・尺度を差し替えられる最小の v4 JSON。
    fn v4_json(scale: &str, layer_width: &str, entity_width: &str) -> String {
        format!(
            r#"{{
              "version": 4,
              "sheet": {{
                "unit": "Millimeter",
                "scale": {scale},
                "paper": "A4",
                "orientation": "Landscape",
                "title_block": "B",
                "fields": {{
                  "drawing_number": "", "drawing_title": "", "projection": "ThirdAngle",
                  "author": "", "date": "", "revision": ""
                }},
                "frame_visible": false
              }},
              "layers": [
                {{"name": "0", "color": {{"r": 255, "g": 255, "b": 255}},
                 "linetype": "Continuous", "width_mm": {layer_width},
                 "visible": true, "locked": false, "order": 0}}
              ],
              "current_layer": 0,
              "entities": [
                {{"layer": 0,
                 "style": {{"color": null, "width_mm": {entity_width}, "linetype": null}},
                 "geom": {{"Shape": {{"Point": {{"x": 0.0, "y": 0.0}}}}}}}}
              ]
            }}"#
        )
    }

    #[test]
    fn text_entity_round_trips_with_cjk_content() {
        use mcad_core::TextGeom;
        // M6: v2 フォーマットで Text エンティティ（CJK 文字列含む）が JSON 往復で保たれる。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let text = EntityGeom::Text(TextGeom {
            anchor: Point2::new(1.5, -2.5),
            content: "日本語ABC 123".to_owned(),
            height: 3.5,
            angle: 0.75,
        });
        doc.apply(Command::AddEntity(Entity::new(
            text.clone(),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        // JSON 文字列を経由しても意味的に一致する（UTF-8 の CJK がそのまま保たれる）。
        let json = to_json(&doc).unwrap();
        assert!(json.contains("日本語"), "JSON に CJK 文字列が含まれるべき");
        let loaded = load(&json).unwrap();
        assert_eq!(export_document(&loaded), export_document(&doc));

        // 幾何が Text として復元されている。
        let (_, entity) = loaded.entities().next().unwrap();
        assert_eq!(entity.geom, text);
    }

    #[test]
    fn unknown_version_via_json_is_rejected() {
        // from_json はバージョン先読みで未知バージョンを弾く（v1/v2 以外）。
        let json = r#"{"version": 99, "layers": [], "current_layer": 0, "entities": []}"#;
        assert!(matches!(load(json), Err(IoError::UnsupportedVersion(99))));
    }

    #[test]
    fn no_layers_fails() {
        let mut file = export_document(&Document::new());
        file.layers.clear();
        assert!(matches!(import_document(&file), Err(IoError::NoLayers)));
    }

    #[test]
    fn bad_current_layer_fails() {
        let mut file = export_document(&Document::new());
        file.current_layer = 5;
        assert!(matches!(
            import_document(&file),
            Err(IoError::BadCurrentLayer(5))
        ));
    }

    #[test]
    fn bad_layer_ref_fails() {
        let mut file = export_document(&Document::new());
        file.entities.push(FileEntity {
            layer: 9,
            style: Style::inherited(),
            geom: EntityGeom::Shape(Shape::Point(Point2::new(0.0, 0.0))),
        });
        assert!(matches!(
            import_document(&file),
            Err(IoError::BadLayerRef { index: 0, layer: 9 })
        ));
    }

    #[test]
    fn invalid_geometry_fails() {
        let cases: Vec<Shape> = vec![
            Shape::Point(Point2::new(f64::NAN, 0.0)),
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), -1.0)),
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), f64::INFINITY)),
            Shape::Arc(Arc::new(Point2::new(0.0, 0.0), 1.0, f64::NAN, 1.0)),
            Shape::Polyline(Polyline::new(vec![], false)),
        ];
        for geom in cases {
            let mut file = export_document(&Document::new());
            file.entities.push(FileEntity {
                layer: 0,
                style: Style::inherited(),
                geom: EntityGeom::Shape(geom.clone()),
            });
            assert!(
                matches!(
                    import_document(&file),
                    Err(IoError::InvalidGeometry { index: 0, .. })
                ),
                "should reject: {geom:?}"
            );
        }
    }

    #[test]
    fn malformed_json_fails() {
        assert!(matches!(load("{ not json"), Err(IoError::Json(_))));
        // 型は正しい JSON だが必須フィールドがない。
        assert!(matches!(load("{}"), Err(IoError::Json(_))));
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.mcad");

        let doc = full_document();
        save_mcad(&doc, &path).unwrap();
        let loaded = load_mcad(&path).unwrap().document;
        assert_eq!(export_document(&loaded), export_document(&doc));

        fs::remove_file(&path).ok();
    }
}
