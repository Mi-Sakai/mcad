# AGENTS.md — AI開発エージェント向けガイド

このリポジトリで開発を引き継ぐAIエージェント(および人間)向けの規約集。
全体設計と現在のタスク分割は [`DESIGN.md`](./DESIGN.md)、変更履歴は [`CHANGELOG.md`](./CHANGELOG.md) を参照。

## プロジェクト概要

Rust + egui の 2D CAD。現在 **v0.7.0(M7「修正系ジオメトリ演算」完了)**。
M7 は トリム・延長・フィレット・分割の4つの修正ツールを実装。次は M8「出力と設定永続化」を予定。

## ビルド・検証

```bash
cargo build                                  # ワークスペース全体
cargo test --workspace                       # 全テスト(v0.7.0時点で446本)
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

- **egui のユーザー可視文字列は ASCII 限定**(ステータスメッセージ・パネルラベル等)。egui の既定フォントは CJK 非対応で、日本語は tofu(□)になる。コード内コメントは日本語でよい。
- **バージョン更新はルート `Cargo.toml` の2箇所のみ**: `[workspace.package].version` と `[workspace.dependencies]` の内部クレートのversion。各クレートは `{ workspace = true }` 参照なので触らない。
- コミットメッセージは日本語(`feat:` / `fix:` / `chore:` プレフィックス)。既存の `git log` の流儀に合わせる。

## 外部クレートの既知の制約

- **`dxf` クレート(0.6.1)**: Color は ACI インデックス(1〜255)のみで RGB 直接指定不可(9色パレットで近似)。`dxf::tables::Layer` にロックフィールドがない(往復でロック消失)。同様に重ね順(z-index)フィールドも無いため、mcad の `Layer.order` は DXF に保存できない(export はデフォルトレイヤーを LAYER テーブルの先頭に固定し、残りを `layers_in_order()` の順で続けて書く。先頭固定は import 側の「テーブル先頭 = デフォルトレイヤー」規則を壊さないためで、デフォルトレイヤー自身の重ね順は往復で失われる。デフォルト以外の並びは他CADがテーブル出現順を尊重した場合のみ意味を持つbest-effort。import はテーブル出現順を `order` として採用する)。`Style::width` も変換していない(仕様として非保存)。**ヘッダは R2007 で、上下どちらにも動かせない**: R2004 以下だと文字列 codec(`\U+XXXX` エスケープ)が往復でデータを壊す(非BMP文字が化ける / 末尾バックスラッシュが消える / リテラル `\U+XXXX` が 1 文字に潰れる。3 件とも実測済み)、R14 未満だと LWPOLYLINE が黙って落ちる。R2007 以上は UTF-8 でそのまま書かれる。詳細は `export_dxf` のコメント、再発検知は `round_trip_preserves_pathological_text`。`Drawing::new()` はレイヤー "0" を自動追加するので export 時に除去している。**位置基準(justification)が「水平 Left かつ垂直 Baseline」以外の `TEXT` は import せずスキップする**(それ以外では文字位置を alignment point (group code 11) が持ち `location` (10) が無意味になるが、`TextGeom` への逆算にはフォントメトリクスが必要で、依存方向から io 層では実装できない)。alignment point を使う「改善」を入れないこと — 理由は `is_text_justification_supported` の doc にある。
- **`rfd`(ネイティブダイアログ)**: フレームコールバック内で同期(ブロッキング)呼び出し。MVP としては許容だがプラットフォーム依存の癖があるため、変更時は手動確認する。

## リポジトリ運用

- ルートの `A-*.md`(外部レビュー等)は**ユーザー管理のメモで、コミット・変更・削除の対象外**(.gitignore 済み)。
- タグは `v0.X.0` 形式でマイルストーン完了時に付ける(ローカルのみ、リモートなし)。
- ドキュメント(README / DESIGN / CHANGELOG)は実装と同じコミットで更新し、乖離させない。
