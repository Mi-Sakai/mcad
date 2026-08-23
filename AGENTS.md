# AGENTS.md — AI開発エージェント向けガイド

このリポジトリで開発を引き継ぐAIエージェント(および人間)向けの規約集。
全体設計と現在のタスク分割は [`DESIGN.md`](./DESIGN.md)、変更履歴は [`CHANGELOG.md`](./CHANGELOG.md) を参照。
このリポジトリで図面を作成する際の製図規約は `製図規定.md` を参照
(mcad の機能要件の源泉としての扱いは [`M8を始める前に.md`](./M8を始める前に.md) 第7章)。
**`製図規定.md` は策定中のため未公開**(ローカルのみ・gitignore 対象)。完成後に追跡へ戻して公開する。
それまでは DESIGN.md や `M8を始める前に.md` 内の同ファイルへのリンクも解決しない。

## プロジェクト概要

Rust + egui の 2D CAD。現在 **v0.8.1**(M8「出力と設定永続化」完了)。M9「寸法」を実装中。
各バージョンの実装内容は CHANGELOG.md、マイルストーンとタスクの現況は DESIGN.md を参照。

## ビルド・検証

```bash
cargo build                                  # ワークスペース全体
cargo test --workspace                       # 全テスト
cargo clippy --workspace --all-targets       # 警告ゼロを維持
cargo fmt --all --check                      # 整形チェック
```

**すべての変更はこの3チェック(fmt / clippy / test)が通ってからコミットする。** CI(`.github/workflows/ci.yml`)も同じ内容を実行する。

GUI に関わる変更(ダイアログ・ビューポート・パネル等)は自動テストできないため、実ウィンドウでの手動スモークテストをユーザーへ依頼し、結果を記録する。

## アーキテクチャ不変条件(破らないこと)

- **依存方向は一方向のみ**: レイヤー順は `mcad-app` → `mcad-io` → `mcad-core` → `mcad-geom`。上位クレートは下位クレートへ直接依存してよい(実際に `mcad-app` は `mcad-core`・`mcad-geom` にも依存)が、下位→上位の逆方向の依存は足さない。
- **ワールド座標は f64**。egui へ描画する境界でのみ f32 へ変換する。viewport は y 軸を反転する(ワールドは y-up、スクリーンは y-down)。
- **ドキュメントの変更は必ず `Document::apply(Command)` 経由**。フィールドの public 化や直接変更で回避しない。import のためだけに内部を公開しない(既存の DTO + Command 再構築を維持)。
- **削除済み entity/layer は墓標として残る**(`SlotMap` の `Option<T>`)。undo/redo をまたいで `EntityId` / `LayerId` が安定するための設計。
- **`Command::Batch` は原子的**: 途中で失敗したら全体をロールバックする。UI の一操作 = undo 1単位。
- **ファイル読込は Command で再構築し、最後に `clear_history()`**。読込操作自体を Ctrl+Z で巻き戻せてはいけない。読込・新規作成後は選択集合・作図ツール・スナップ表示をリセットする(`reset_transient_ui_state()`)。
- レイヤーロックで失敗しうる操作の `Err` は捨てず、ステータスバーへ表示する。

## コーディング規約

- **egui のユーザー可視文字列に日本語を使ってよい**(M8 で ASCII 限定を撤廃)。M6 タスク23 で Noto Sans JP を `Proportional`/`Monospace` 両ファミリの fallback として登録済みのため、egui はグリフ単位でフォールバックし CJK も正しく描画される(`fonts.rs`)。**ただし日本語化は領域単位で完結させること** — 1つのパネルやダイアログの中で英語と日本語を混在させない(DESIGN.md M6 設計判断3 が挙げる「UI の一貫性」はこの意味で維持する)。移行途中のため、未着手の領域は英語のまま残っている。
  - 現在日本語化済み: 右パネル(レイヤーパネル・選択中のスタイル・図面セクション)と表題欄編集ダイアログ。それ以外(上部パネル・確認モーダル等)は英語。
  - **日本語は「見た目の幅」が英字と異なる**ため、ラベルを変えたら実ウィンドウでレイアウト崩れがないか確認する。
  - レイヤー名の既定値(`Layer {n}` 等)は UI ラベルではなく**ドキュメントに保存されるデータ**なので、この規約の対象外(変更すると `.mcad`/DXF の中身が変わる)。
- **バージョン更新はルート `Cargo.toml` の2箇所のみ**: `[workspace.package].version` と `[workspace.dependencies]` の内部クレートのversion。各クレートは `{ workspace = true }` 参照なので触らない。
- コミットメッセージは日本語(`feat:` / `fix:` / `chore:` プレフィックス)。既存の `git log` の流儀に合わせる。

## 外部クレートの既知の制約

- **`dxf` クレート(0.6.1)**: 既知の制約は以下。詳細と実測根拠は `crates/mcad-io/src/dxf_file.rs` のモジュール doc を参照。
  - Color は ACI インデックス(1〜255)のみで RGB 直接指定不可(9色パレットで近似)。
  - `dxf::tables::Layer` にロック・重ね順フィールドがない → レイヤーロックは往復で消失し、`Layer.order` も DXF に保存できない。export はデフォルトレイヤーを LAYER テーブル先頭に固定し、残りを順序どおり書く。import はテーブル出現順を `order` として採用(best-effort)。
  - **ヘッダは R2007 固定で、上下どちらにも動かさないこと**(R2004 以下は文字列 codec が往復でデータを壊す、R14 未満は LWPOLYLINE が黙って落ちる)。再発検知テストは `round_trip_preserves_pathological_text`。
  - `Drawing::new()` が自動追加するレイヤー "0" は export 時に除去している。
  - **位置基準が「水平 Left かつ垂直 Baseline」以外の `TEXT` は import せずスキップ**。alignment point (group code 11) を使う「改善」を入れないこと(理由は `is_text_justification_supported` の doc)。
  - 単位は `$INSUNITS = 4`(mm)を export ヘッダへ書く。
  - **TEXT 高さの尺度契約**: `TextGeom::height` は紙 mm。export は `SheetMeta::scale` で換算し、import は 1:1 とみなす。紙 mm の意味は尺度をまたいで往復しないが、モデル空間の幾何サイズは保存される。テストは `text_height_scales_by_sheet_scale_on_export`・`text_geometric_height_survives_round_trip_across_scale_reinterpretation`(`mcad-io`)。
  - 線幅は best-effort: エンティティ線幅(group code 370)は読み書き可。**レイヤー線幅は export 不可**(`LineWeight` に任意 raw 値を作る公開コンストラクタがない)。import は読み取れる。mcad からの export は常に raw 0 で、re-import では「未指定」として既定 0.35mm に戻る(クランプとして計上しない)。
  - 線種は mcad の4種を `CONTINUOUS`/`DASHED`/`DASHDOT`/`DIVIDE` で LTYPE 登録。**`DASHDOT2` を使わないこと**(同ライブラリでは半スケールの一点鎖線を指す既存名)。未知の線種名は `Continuous` へフォールバック。
- **`rfd`(ネイティブダイアログ)**: フレームコールバック内で同期(ブロッキング)呼び出し。MVP としては許容だがプラットフォーム依存の癖があるため、変更時は手動確認する。

## リポジトリ運用

- `A-*.md`(外部レビュー等)は**ユーザー管理のメモで、コミット・変更・削除の対象外**(ルート・`資料/` 配下とも .gitignore 済み)。
- 過去のマイルストーンの Codex レビュー記録は `資料/` に置く(追跡対象)。
- タグは `v0.X.0` 形式でマイルストーン完了時に付ける。GitHub(`origin`)へ公開しており、タグも `git push --tags` でリモートへ反映する。
- コミットの author は GitHub の noreply アドレスを使う(リポジトリローカルの `user.email` に設定済み)。
- ドキュメント(README / DESIGN / CHANGELOG)は実装と同じコミットで更新し、乖離させない。
