# 参考図枠(FreeCAD TechDraw テンプレート)

mcad の図面枠・表題欄(`crates/mcad-app/src/frame.rs` / `crates/mcad-core/src/title_block.rs`)を
ISO 準拠の実物と見比べるための参照用ファイル。**mcad のビルドには一切使わない**(コードから
参照していない、資料としての同梱)。

調査の経緯と mcad との差分の実測は [`../参考図面・図枠の入手先調査.md`](../参考図面・図枠の入手先調査.md) を参照。

## 出所とライセンス

- 出所: [FreeCAD](https://github.com/FreeCAD/FreeCAD) `src/Mod/TechDraw/Templates/ISO/`
  (取得日 2026-08-11、`main` ブランチ)
- ライセンス: **CC0 1.0 Universal(パブリックドメイン)**。各 SVG 内の RDF に
  `creativecommons.org/publicdomain/zero/1.0/` として記載されている。改変・再配布とも制約なし。

## ファイル

| ファイル | 内容 |
|---|---|
| `A4_Landscape_ISO5457_minimal.svg` | ISO 5457 準拠の A4 横図枠 + 最小構成の表題欄 |
| `A4_Landscape_ISO5457_advanced.svg` | 同上、表題欄が拡張版 |
| `ISO7200_titleblock_1_minimal.svg` | ISO 7200 表題欄(必須項目のみ、180 × 36mm) |
| `ISO7200_titleblock_3_advanced.svg` | 同(拡張、180 × 48mm) |
| `ISO7200_titleblock_5_maximal.svg` | 同(全項目) |

いずれも SVG なので、ブラウザで開けばそのまま表示できる。mcad の SVG 出力
(`Ctrl+Shift+E`、M8 タスク39)と並べて比較する用途を想定している。

FreeCAD 独自様式の `A4_Landscape_TD.svg` は ISO 準拠ではないため同梱していない
(必要なら同じディレクトリから取得できる)。各国語版(`ISO/localized/`)に日本語版は無い。

## 読み取るときの注意

- 欄の識別子は `freecad:editable="..."` 属性に入っている(例: `legal_owner_1`、`part_material`)。
  欄の位置・寸法は `<rect id="..._border">` の座標から読める。単位は mm(`viewBox` が実寸)。
- 規格票(ISO 5457 / ISO 7200 / JIS Z 8311)の**条文そのものは転記しない**。これらの
  CC0 テンプレートから読み取れる寸法・欄構成を根拠として引く。
