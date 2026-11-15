# mcad — Rust製 2D CAD

*[English README](./README.en.md)*

Rust + egui で開発している2D CADソフトウェアです。現在 **v1.0.0**(初の安定版)。
機械製図の図面を1枚、作図から PDF 出力まで mcad だけで仕上げられることを目標にしています。

Claude Fable 5 の発表を機に、AI 支援開発でどこまで実用に耐える 2D CAD を構築できるかを検証する目的で、
2026年7月に開始した個人プロジェクトです。全体設計・アーキテクチャ方針・製図要件の策定・レビュー時の採否判断は
著者が行い、実装には Claude Code を利用しています。

> 本プロジェクトは Anthropic 社とは無関係の個人プロジェクトです。

## 特徴

- **作図と編集**: 線分・円・円弧・ポリライン・矩形・正多角形・構築線・文字(日本語対応)。移動・複製・回転・ミラー・
  オフセット・トリム・延長・フィレット・面取り・分割・結合・グリップ編集。数値入力・スナップ・直交・極トラッキング・
  等測作図の補助
- **寸法**: 長さ・半径・直径・角度・座標。寸法補助記号・サイズ公差・参考寸法などの表記、文書単位の寸法スタイル
- **図面**: レイヤー(色・線種・線幅・ロック・重ね順)、図面枠と表題欄、表と部品欄、線分・円・円弧の幾何拘束
- **入出力**: `.mcad`(JSON、1.0 以降の読込互換を約束)、DXF の読み書き、SVG/PDF への実寸出力

機能・操作・ファイル形式の詳細は [**仕様書**](./docs/mcad仕様書.md) にあります。よく引く節:
[キーの一覧](./docs/mcad仕様書.md#a-キーの一覧)・
[新規図面のレイヤー](./docs/mcad仕様書.md#43-新規図面のレイヤーと自動割り当て)・
[`.mcad` 形式](./docs/mcad仕様書.md#11-mcad-形式)と[互換性の約束](./docs/mcad仕様書.md#115-互換性の約束)・
[DXF の対応と制約](./docs/mcad仕様書.md#12-dxf)・
[設定ファイル](./docs/mcad仕様書.md#13-設定configjson)。

## ビルドと実行

```bash
cargo run -p mcad-app             # 起動
cargo run --release -p mcad-app   # 通常利用ではリリースビルドを推奨
```

テスト・lint・整形チェックなど開発時の手順は [`AGENTS.md`](./AGENTS.md) を参照してください。

## ドキュメント

| 文書 | 内容 |
|---|---|
| [`docs/mcad仕様書.md`](./docs/mcad仕様書.md) | 仕様書。利用者から見た振る舞い、キーの一覧、`.mcad` の形式と[互換性の約束](./docs/mcad仕様書.md#115-互換性の約束)、DXF・SVG/PDF の出力、設定ファイル |
| [`DESIGN.md`](./DESIGN.md) | 設計書。設計の方針・構造・設計判断・ロードマップ |
| [`CHANGELOG.md`](./CHANGELOG.md) | バージョンごとの変更履歴 |
| [`AGENTS.md`](./AGENTS.md) | 開発規約(アーキテクチャ不変条件・ビルドと検証・コーディング規約) |

## ライセンス

本リポジトリは以下のいずれかを選択して利用できます（デュアルライセンス）。

- MIT License（[`LICENSE-MIT`](./LICENSE-MIT)）
- Apache License 2.0（[`LICENSE-APACHE`](./LICENSE-APACHE)）

Rust エコシステムの慣習に倣った構成です。利用者はどちらか一方の条件を満たせば
足ります。特段の記載なく本リポジトリへ寄稿された内容は、Apache-2.0 の第5条に従い
同じデュアルライセンスの下で提供されたものとみなします。

**同梱フォントは対象外です**: `crates/mcad-app/assets/fonts/` 配下のフォントは
第三者の著作物であり、SIL Open Font License 1.1 が適用されます（次節を参照）。

## フォント / ライセンス帰属

文書内の文字や UI の日本語を表示するため、[Noto Sans JP](https://fonts.google.com/noto/specimen/Noto+Sans+JP)
Regular をバイナリへ埋め込んでいます。Noto Sans JP は SIL Open Font License 1.1 で提供されています。
ライセンス全文は
[`crates/mcad-app/assets/fonts/LICENSE-OFL.txt`](./crates/mcad-app/assets/fonts/LICENSE-OFL.txt)
に同梱しています。

> Noto Sans JP is licensed under the SIL Open Font License, Version 1.1.
> © 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'.
> "Noto" is a trademark of Google LLC.
