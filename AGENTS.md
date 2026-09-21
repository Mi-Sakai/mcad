# AGENTS.md — AI開発エージェント向けガイド

このリポジトリで開発を引き継ぐAIエージェント(および人間)向けの規約集。
全体設計と現在のタスク分割は [`DESIGN.md`](./DESIGN.md)、変更履歴は [`CHANGELOG.md`](./CHANGELOG.md) を参照。
このリポジトリで図面を作成する際の製図規約は `製図規定.md` を参照
(mcad の機能要件の源泉としての扱いは [`M8を始める前に.md`](./M8を始める前に.md) 第7章)。
**`製図規定.md` は策定中のため未公開**(ローカルのみ・gitignore 対象)。完成後に追跡へ戻して公開する。
それまでは DESIGN.md や `M8を始める前に.md` 内の同ファイルへのリンクも解決しない。

## プロジェクト概要

Rust + egui の 2D CAD。現在 **v0.10.0**(M10「表・部品表」完了)。次は M11「性能と互換性」(空間インデックス、DXF DIMENSION import と寸法種別の拡張。詳細設計は着手時に策定)。
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

- **egui のユーザー可視文字列に日本語を使ってよい。** Noto Sans JP が `Proportional`/`Monospace` 両ファミリの fallback として登録済みで、egui はグリフ単位でフォールバックし CJK も正しく描画される(`fonts.rs`)。**ただし日本語化は領域単位で完結させること** — 1つのパネルやダイアログの中で英語と日本語を混在させない(DESIGN.md M6 設計判断3 が挙げる「UI の一貫性」はこの意味で維持する)。移行途中のため、未着手の領域は英語のまま残っている。
  - 現在日本語化済み: 右パネル(レイヤーパネル・選択中のスタイル・図面・寸法・表セクション)と表題欄編集ダイアログ・寸法スタイルダイアログ・表編集ダイアログ。それ以外(上部パネル・確認モーダル等)は英語。
  - **日本語は「見た目の幅」が英字と異なる**ため、ラベルを変えたら実ウィンドウでレイアウト崩れがないか確認する。
  - レイヤー名の既定値(`Layer {n}` 等)は UI ラベルではなく**ドキュメントに保存されるデータ**なので、この規約の対象外(変更すると `.mcad`/DXF の中身が変わる)。
- **バージョン更新はルート `Cargo.toml` の2箇所のみ**: `[workspace.package].version` と `[workspace.dependencies]` の内部クレートのversion。各クレートは `{ workspace = true }` 参照なので触らない。
- コミットメッセージは日本語(`feat:` / `fix:` / `chore:` プレフィックス)。既存の `git log` の流儀に合わせる。

## 外部クレートの既知の制約

- **`dxf` クレート(0.6.1)**: R2007 ヘッダ固定、ACI 色のみ、レイヤーのロック・重ね順・線幅が保存できない等、**踏んではいけない制約が多数ある**。`crates/mcad-io/src/dxf_file.rs` を触る前に `dxf-constraints` skill を読むこと(実測根拠は同ファイルのモジュール doc)。
  - **非対称往復を許している経路**: 表(`EntityGeom::Table`)は export 時に罫線 `LINE` とセル `TEXT` へ分解し、import では表に戻さない。`ACAD_TABLE` はクレートに対応する型が無くパーサ側で読み飛ばされるためスキップ件数にも出ない。寸法(`DimLinear`/`DimRadial`/`DimDiameter`/`DimAngular`/`DimOrdinate`)も M11 タスク72 で同じ非対称往復になった: export は寸法線・補助線・矢先・文字を `LINE`/`SOLID`/`ARC`/`TEXT` へ分解して書き、import では寸法エンティティへ戻さない。表と違い、矢先の塗りに使う `SOLID` はクレートの `EntityType::Solid` に対応するが `dxf_entity_to_geom` 側が対応しないため、re-import 時に `ImportSummary::skipped_entities` へ計上される(`LINE`/`ARC`/`TEXT` は通常の Shape/Text として取り込まれる)。他 CAD が書いた `DIMENSION` は M11 タスク67・69 で import 済みなので、「mcad が書いた寸法は読み戻せないが、他 CAD が書いた `DIMENSION` は読める」という二重の非対称になる。図面枠・表題欄は用紙メタを DXF へ載せられないため出力しない。
- **`rfd`(ネイティブダイアログ)**: フレームコールバック内で同期(ブロッキング)呼び出し。MVP としては許容だがプラットフォーム依存の癖があるため、変更時は手動確認する。

## リポジトリ運用

- `A-*.md`(外部レビュー等)は**ユーザー管理のメモで、コミット・変更・削除の対象外**(ルート・`資料/` 配下とも .gitignore 済み)。
- 過去のマイルストーンの Codex レビュー記録は `資料/` に置く(追跡対象)。
- タグは `v0.X.0` 形式でマイルストーン完了時に付ける。GitHub(`origin`)へ公開しており、タグも `git push --tags` でリモートへ反映する。リリース時の手順(バージョン更新箇所・CHANGELOG・タグ)は `release` skill を参照。
- コミットの author は GitHub の noreply アドレスを使う(リポジトリローカルの `user.email` に設定済み)。
- ドキュメント(README / DESIGN / CHANGELOG)は実装と同じコミットで更新し、乖離させない。
