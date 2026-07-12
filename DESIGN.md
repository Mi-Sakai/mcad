# mcad — Rust製 2D CAD 設計書

## 1. 概要

Rust + egui で作る2D CAD。MVPの機能範囲は以下のとおり。

- 基本作図: 線分・円・円弧・ポリライン・点
- 選択・移動・削除、ズーム・パン
- スナップ(端点・中点・中心・交点・グリッド)とグリッド表示
- レイヤー管理(表示/非表示・色・ロック)
- ファイル保存/読込(独自形式)+ DXF入出力
- アンドゥ/リドゥ

**スコープ外(MVP後)**: 幾何拘束ソルバー、寸法・注記、トリム/フィレット等の編集系、印刷。

## 2. 技術スタック

| 用途 | 採用 | 備考 |
|---|---|---|
| GUI・描画 | `eframe` / `egui` | Painter APIで2D描画。MVPはCPUテッセレーションで十分 |
| 幾何計算 | 自前実装(f64) | ワールド座標はf64。描画時のみf32へ変換 |
| ID管理 | `slotmap` | エンティティ・レイヤーのキー |
| シリアライズ | `serde` + `serde_json` | 独自形式 `.mcad`(JSON)。将来バイナリ化可 |
| DXF | `dxf` クレート | R2000相当のASCII DXFを対象 |
| エラー | `thiserror` / `anyhow` | core側はthiserror、app側はanyhow |

## 3. アーキテクチャ

Cargoワークスペース構成。依存方向は一方向のみ(app → io → core → geom)。

```
mcad/
├── Cargo.toml            # workspace
├── crates/
│   ├── mcad-geom/        # 幾何プリミティブと計算(GUI非依存・純関数)
│   ├── mcad-core/        # ドキュメントモデル・レイヤー・undo/redo
│   ├── mcad-io/          # .mcad保存/読込、DXF入出力
│   └── mcad-app/         # eguiアプリ(バイナリ)
```

### 3.1 mcad-geom

- 型: `Point2`(f64)、`Vec2`、`Aabb`、`LineSeg`、`Circle`、`Arc`(中心+半径+開始/終了角、CCW)、`Polyline`
- アルゴリズム:
  - 最近点計算 `closest_point(shape, p) -> Point2`
  - ヒットテスト `distance_to(shape, p) -> f64`(ピック許容量との比較用)
  - 交点計算 `intersect(a, b) -> Vec<Point2>`(線分×線分、線分×円/弧、円×円 の全組合せ)
  - AABB算出(ビューポートカリング・矩形選択用)
- 全関数は純関数。プロパティテスト(`proptest`)を書く。

### 3.2 mcad-core

- `Document`: `SlotMap<EntityId, Entity>` + `SlotMap<LayerId, Layer>` + カレントレイヤー
- `Entity` = `{ geom: Shape, layer: LayerId, style: Style }`(`Shape`はgeomの列挙型)
- `Layer` = `{ name, color, visible, locked }`。レイヤー0は削除不可
- **undo/redo**: コマンドパターン。`enum Command { Add, Remove, Modify{before, after}, … }` を逆操作可能な形で履歴スタックに積む。ツール操作1回=1コマンド(ドラッグ中の中間状態は積まない)
- ドキュメント変更はすべて `Document::apply(cmd)` 経由に強制し、直接フィールドを触らせない

### 3.3 mcad-io

- `.mcad`: Documentのserde JSON。バージョンフィールド必須(`"version": 1`)
- DXF: `dxf`クレートで LINE / CIRCLE / ARC / LWPOLYLINE / POINT とレイヤーテーブルを相互変換。未対応エンティティは読み飛ばして件数を警告として返す

### 3.4 mcad-app

- **Viewport**: ワールド(f64)↔スクリーン(f32)変換。`zoom: f64` と `center: Point2` を保持。ホイールでカーソル中心ズーム、中ボタン/Spaceドラッグでパン
- **Tool状態機械**: 
  ```rust
  trait Tool {
      fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult;
      fn draw_preview(&self, painter: &Painter, vp: &Viewport);
  }
  ```
  `ToolResult` は `{ Continue, Commit(Command), Cancel }`。ツール: Select(単選択+矩形選択)/ Line / Circle / Arc(3点)/ Polyline / Move / Delete
- **スナップエンジン**: カーソル半径(px)内の候補点を列挙し優先度順(端点>交点>中点>中心>グリッド)で1点返す。交点は画面内エンティティのみ対象にAABBで事前絞り込み。スナップ位置はマーカー表示
- **UIレイアウト**: 左=ツールバー、右=レイヤーパネル、下=ステータスバー(座標・スナップ状態)、中央=キャンバス
- **描画**: 毎フレーム、ビューポートAABBと交差するエンティティのみPainterへ。選択中はハイライト色。グリッドはズームに応じて間引き

## 4. 主要な設計判断の理由

1. **f64ワールド座標**: CADは座標精度が命。f32は大きい図面で破綻する。egui描画境界でのみ変換
2. **コマンドパターンundo**: スナップショット方式より省メモリで、DXFのような大図面でも成立する
3. **geomをGUI非依存に分離**: 幾何計算のテストをheadlessで回せる。将来の拘束ソルバー追加もこの層の上に載る
4. **空間インデックスはMVPでは省略**: 数千エンティティまでは全走査+AABBカリングで足りる。ボトルネック実測後にrstar導入を検討

## 5. タスク分割(実装フェーズ用)

依存順。各タスクに担当subagentを指定。

### M1: 土台

| # | タスク | 内容 | 担当 |
|---|---|---|---|
| 1 | ワークスペース雛形 | 4クレート作成、依存設定、CI(fmt/clippy/test)、空のeframeウィンドウ起動まで | implement-sonnet |
| 2 | mcad-geom | 全プリミティブ型+最近点・ヒットテスト・交点計算+proptest。交点計算の数値安定性に注意 | implement-opus |
| 3 | mcad-core | Document/Entity/Layer、コマンドパターンundo/redo、単体テスト | implement-opus |

### M2: 描けるCAD

| # | タスク | 内容 | 担当 | 依存 |
|---|---|---|---|---|
| 4 | Viewport+描画 | 座標変換、ズーム/パン、エンティティ描画、グリッド表示、カリング | implement-sonnet | 1,3 |
| 5 | Toolフレームワーク+作図ツール | Toolトレイト、Line/Circle/Arc/Polyline/Pointツール、プレビュー描画 | implement-sonnet | 4 |
| 6 | 選択・編集ツール | Select(単/矩形)、Move、Delete、Escキャンセル、undo/redo結線 | implement-sonnet | 5 |
| 7 | スナップエンジン | 候補点列挙・優先度・マーカー表示・ツールへの統合 | implement-opus | 5 |

### M3: 実用化

| # | タスク | 内容 | 担当 | 依存 |
|---|---|---|---|---|
| 8 | レイヤーパネル | 一覧・追加/削除・色・表示/ロック切替、カレントレイヤー | implement-sonnet | 6 |
| 9 | .mcad保存/読込 | serde化、ファイルダイアログ(rfd)、未保存確認 | implement-sonnet | 3 |
| 10 | DXF入出力 | dxfクレート連携、レイヤー対応、往復テスト(export→import一致) | implement-sonnet | 9 |
| 11 | 仕上げ | README、キーバインド一覧、全体の統合テスト | haiku-assistant(ドキュメント)+ implement-sonnet(テスト) | 全部 |

### 検収基準(MVP完了の定義)

- 線分・円・円弧・ポリラインを描き、スナップを使って正確に接続できる
- 矩形選択→移動→undo→redoが正しく動く
- レイヤーを分けて非表示・ロックが効く
- .mcadで保存→再起動→読込で完全復元される
- 描いた図面をDXF出力し、再インポートして同一内容になる(往復テストがCIで通る)

**→ 全項目 v0.3.0 で達成済み(2026-07-12)。**

## 6. M4: 入出力の一貫性(v0.4.0)

2026-07-12 設計。v0.3.0 の外部レビュー(Codex)と内部レビューで挙がった、
**実ユーザーに見える入出力の一貫性と初期状態**の課題を解消する。
新しい作図・編集機能は足さない(トリム/フィレット・寸法・拘束ソルバーは引き続きスコープ外)。

### 設計判断

1. **DXF importは`.mcad`と混同しない**: DXF を開いたら `current_path = None`・`dirty = true` とし、
   Ctrl+S では元の DXF を上書きせず「名前を付けて `.mcad` 保存」へ誘導する。DXF は交換用の
   import/export 形式であり、作業ファイル形式ではない
2. **起動は空文書**: 作図ツールが揃った今、起動時サンプルの役目は終わった。サンプル生成は
   テスト用ヘルパーへ移す(feature flag より単純)
3. **dirtyは履歴世代ベースへ**: `Document` に世代カウンタを設け、保存成功時の世代を記録して
   現在世代と比較する。undo 後の再 apply による履歴分岐・no-op・`clear_history()` との
   相互作用に注意
4. **ジオメトリ検証をcoreへ引き上げ**: `Shape::validate()` を mcad-geom に置き、
   `Document::apply` の AddEntity / ModifyEntity でも検証する。ゼロ半径・ゼロ長線分は
   現行どおり許容(UI と整合)。mcad-io の `validate_shape` はこれへ委譲して重複排除
5. **DXF lineweight(線幅)対応はスコープ外**: 「width は保存されない」と仕様明記のみ行う

### タスク分割

| # | タスク | 内容 | 担当 | 依存 |
|---|---|---|---|---|
| 12 | DXFのGUI結線 | Open DXF / Export DXF(キーバインド例: Ctrl+Shift+O / Ctrl+E)。import成功時に `skipped_entities` 件数とロック非復元をステータス表示。`current_path = None`・`dirty = true` | implement-sonnet | — |
| 13 | 起動状態とズームフィット | 起動を空文書化(サンプルはテストヘルパーへ)。ファイル読込後(.mcad/DXF共通)に図面全体のAABBへズームフィット、空文書は既定ビューへリセット | implement-sonnet | 12 |
| 14 | dirtyの世代管理 | Documentに世代カウンタ、保存時世代との比較でdirty判定。undo分岐・no-op・clear_historyの各分岐をテストで固定 | implement-opus | — |
| 15 | ジオメトリ不変条件 | `Shape::validate()` をmcad-geomへ追加、`Document::apply` で検証、mcad-ioは委譲 | implement-sonnet | — |
| 16 | ドキュメント整合 | README・モジュールdocをM4の変更(DXF操作・キーバインド・dirty仕様)に合わせて更新 | haiku-assistant | 12-15 |

### 検収基準(M4完了の定義)

- GUIからDXFを開き・書き出しでき、importのスキップ件数がステータスに表示される
- DXFを開いた直後にCtrl+Sを押すと「名前を付けて`.mcad`保存」ダイアログが開く(元DXFを上書きしない)
- 起動直後は空文書。ファイルを開くと図面全体が視界に収まる
- 保存→1操作→undoで保存時内容へ戻ると、タイトルの`*`が消え未保存確認も出ない
- NaN/∞を含むShapeは`Command::AddEntity`/`ModifyEntity`で拒否される
- 全段で fmt / clippy / workspace test が通り、GUI変更(12・13)は手動スモークテストを記録する
