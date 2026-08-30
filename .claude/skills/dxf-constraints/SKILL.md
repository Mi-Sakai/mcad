---
name: dxf-constraints
description: dxfクレート(0.6.1)の既知の制約と、踏んではいけない地雷。mcad-io の DXF import/export(`crates/mcad-io/src/dxf_file.rs`)を読む・直す・テストするとき、またはDXF往復・レイヤー・線幅・線種・TEXTの不具合を調べるときに必ず読む。
---

# dxf クレート(0.6.1)の既知の制約

`crates/mcad-io/src/dxf_file.rs` を触る前に読むこと。
詳細と実測根拠は同ファイルのモジュール doc にある。

**ここに書かれている制約は、すべて実測で確かめた結果として現在の実装になっている。**
「改善できそう」に見えても、まず理由を読むこと。

## 触ってはいけないもの

- **ヘッダは R2007 固定。上下どちらにも動かさない。**
  R2004 以下は文字列 codec が往復でデータを壊す。R14 未満は LWPOLYLINE が黙って落ちる。
  再発検知テストは `round_trip_preserves_pathological_text`。

- **位置基準が「水平 Left かつ垂直 Baseline」以外の `TEXT` は import せずスキップする。**
  alignment point (group code 11) を使う「改善」を入れないこと。
  理由は `is_text_justification_supported` の doc を読む。

- **`DASHDOT2` を使わない。**
  同ライブラリでは半スケールの一点鎖線を指す既存名。

## 仕様上できないこと

- **Color は ACI インデックス(1〜255)のみ。** RGB 直接指定は不可なので9色パレットで近似する。

- **`dxf::tables::Layer` にロック・重ね順フィールドがない。**
  レイヤーロックは往復で消失し、`Layer.order` も DXF に保存できない。
  export はデフォルトレイヤーを LAYER テーブル先頭に固定し、残りを順序どおり書く。
  import はテーブル出現順を `order` として採用(best-effort)。

- **レイヤー線幅は export 不可。**
  `LineWeight` に任意 raw 値を作る公開コンストラクタがない。
  エンティティ線幅(group code 370)は読み書き可。
  mcad からの export は常に raw 0 で、re-import では「未指定」として既定 0.35mm に戻る
  (クランプとしては計上しない)。import 側は読み取れる。

## 契約として決めていること

- **単位**: `$INSUNITS = 4`(mm)を export ヘッダへ書く。

- **TEXT 高さの尺度契約**: `TextGeom::height` は紙 mm。
  export は `SheetMeta::scale` で換算し、import は 1:1 とみなす。
  紙 mm の意味は尺度をまたいで往復しないが、**モデル空間の幾何サイズは保存される**。
  テストは `text_height_scales_by_sheet_scale_on_export` と
  `text_geometric_height_survives_round_trip_across_scale_reinterpretation`(`mcad-io`)。

- **線種**: mcad の4種を `CONTINUOUS` / `DASHED` / `DASHDOT` / `DIVIDE` で LTYPE 登録する。
  未知の線種名は `Continuous` へフォールバックする。

- `Drawing::new()` が自動追加するレイヤー "0" は export 時に除去している。
