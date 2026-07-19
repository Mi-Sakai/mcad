# AGENTS.md — AI開発エージェント向けガイド

このリポジトリで開発を引き継ぐAIエージェント(および人間)向けの規約集。
全体設計と現在のタスク分割は [`DESIGN.md`](./DESIGN.md)、変更履歴は [`CHANGELOG.md`](./CHANGELOG.md) を参照。

## プロジェクト概要

Rust + egui の 2D CAD。現在 **v0.4.1(操作性改善)**。
M5「編集操作の充実」(DESIGN.md 第7章)が進行中で、タスク17・18が完了済み。

## ビルド・検証

```bash
cargo build                                  # ワークスペース全体
cargo test --workspace                       # 全テスト(v0.4.1時点で238本)
cargo clippy --workspace --all-targets       # 警告ゼロを維持
cargo fmt --all --check                      # 整形チェック
```

**すべての変更はこの3チェック(fmt / clippy / test)が通ってからコミットする。** CI(`.github/workflows/ci.yml`)も同じ内容を実行する。

GUI に関わる変更(ダイアログ・ビューポート・パネル等)は自動テストできないため、実ウィンドウでの手動スモークテストをユーザーへ依頼し、結果を記録する。

## アーキテクチャ不変条件(破らないこと)

- **依存方向は一方向のみ**: `mcad-app` → `mcad-io` → `mcad-core` → `mcad-geom`。逆方向・飛び越しの依存を足さない。
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

- **`dxf` クレート(0.6.1)**: Color は ACI インデックス(1〜255)のみで RGB 直接指定不可(9色パレットで近似)。`dxf::tables::Layer` にロックフィールドがない(往復でロック消失)。`Style::width` も変換していない(仕様として非保存)。ヘッダは R2000 — R12 だと LWPOLYLINE が黙って落ちる。`Drawing::new()` はレイヤー "0" を自動追加するので export 時に除去している。
- **`rfd`(ネイティブダイアログ)**: フレームコールバック内で同期(ブロッキング)呼び出し。MVP としては許容だがプラットフォーム依存の癖があるため、変更時は手動確認する。

## リポジトリ運用

- ルートの `A-*.md`(外部レビュー等)は**ユーザー管理のメモで、コミット・変更・削除の対象外**(.gitignore 済み)。
- タグは `v0.X.0` 形式でマイルストーン完了時に付ける(ローカルのみ、リモートなし)。
- ドキュメント(README / DESIGN / CHANGELOG)は実装と同じコミットで更新し、乖離させない。
