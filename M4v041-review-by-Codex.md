# mcad v0.4.1 review / Claude 引き継ぎ

レビュー日: 2026-07-19 (JST)  
レビュー基準: `3499a28 feat: ファイルダイアログがセッション内で最後に使ったディレクトリを記憶`  
対象バージョン: Cargo workspace `0.4.1`

## 結論

現時点の `mcad` は、M4「入出力の一貫性」を完了した安定した基盤の上で、M5「編集操作の充実」のタスク17・18まで実装済みである。レビューでリリースを止めるべきコード不具合は見つからなかった。アーキテクチャ不変条件、Command 経由の変更、Batch の原子性、世代ベース dirty 判定は維持されている。

検証結果は次のとおり。

```text
cargo fmt --all --check                         PASS
cargo clippy --workspace --all-targets -- -D warnings  PASS
cargo test --workspace                          PASS (238 tests)
```

テスト内訳は app 113、core 41、geom unit 51、geom property 10、io unit 19、io integration 4。さらに `mcad-geom` / `mcad-core` を path 依存する `../tcad` について、元の dirty worktree を変更しない一時コピー上で workspace test と clippy を実行し、v0.4.1 の公開 API との互換性を確認した。

次に Claude が着手すべき本流は M5 タスク19（回転・ミラー）である。ただし実装前に、移動が `M` キーを使用するようになったためミラーの起動キーを確定し、`DESIGN.md` 内の古い記述を揃えること。

## 作業ツリーの注意

レビュー開始時点で次の未追跡ファイルが存在した。

```text
?? M5v050-review-by-Codex.md
```

これは既存のユーザー成果物として扱い、変更していない。本ファイル `M4v041-review-by-Codex.md` だけが今回追加したファイルである。`../tcad` 側には既存の `Cargo.lock` 変更があり、これも変更していない。

## 現在地

コミットの流れは以下。

```text
v0.4.0 tag
  -> M5 task 17: Shape::rotated / mirrored
  -> status message visibility fix
  -> drawing UX improvements
  -> M5 task 18: duplicate + shared two-click placement + M-key move
  -> v0.4.1 release/document alignment
  -> remember last file-dialog directory (current HEAD, post-release commit)
```

M4 の検収項目は実装・文書上とも完了している。

- GUI から `.mcad` open/save と DXF import/export が可能。
- DXF import 後は `current_path = None`、強制 dirty となり、Ctrl+S は `.mcad` の名前付き保存へ進む。
- 新規文書と起動直後は空。読込後は document AABB にズームフィットする。
- `Document::generation()` と保存世代の比較で dirty を判定し、保存後の変更を undo すると clean に戻る。
- `Shape::validate()` を core の Add/Modify 境界でも適用し、NaN、無限値、負半径、空 polyline を拒否する。
- 読込は DTO から Command で再構築し、最後に `clear_history()` する。
- GUI の一時状態は新規作成・読込・履歴操作など適切な境界でリセットされる。

v0.4.1 では M5 の一部を先行して取り込んでいる。

- タスク17: `Shape` と全プリミティブに `rotated` / `mirrored` を追加。円弧の鏡映では端角を交換して CCW 表現を維持する。
- タスク18: Ctrl+D の2クリック複製。`Batch(AddEntity)` で undo 1単位、新規 ID を新しい選択集合にする。
- 移動も同じ配置機構へ統一。`M` → 基準点 → 配置先で `Batch(ModifyEntity)` を確定し、ID と選択を維持する。
- 通常ドラッグは矩形選択専用。クリック選択は累積式。
- Line の連続作図、polyline の始点クリックによる close、作図途中の頂点スナップを追加。
- HEAD では4種のファイルダイアログがセッション中の最終ディレクトリを共有する。

## アーキテクチャと変更時の境界

依存方向は次の一方向だけを維持する。

```text
mcad-app -> mcad-io -> mcad-core -> mcad-geom
```

### `mcad-geom`

GUI 非依存の f64 幾何層。`Point2`, `Vec2`, `Aabb`, `LineSeg`, `Circle`, `Arc`, `Polyline`, `Shape` と、最近点・距離・交点・AABB・検証・変換を持つ。

- 座標変換を app 側で個別実装しない。回転・鏡映は既存の `Shape::rotated` / `Shape::mirrored` を使う。
- `Arc` は start から end への CCW sweep。鏡映は向きを反転するため、反射した終点を新 start、反射した始点を新 end にする現行実装が正しい。
- ゼロ長ミラー軸に対する低水準の `Vec2::reflected` は入力をそのまま返す。UI は同一点2クリックを拒否し、見かけ上成功した no-op を作らないこと。
- 公開 API を変更した場合は mcad だけでなく `../tcad` も検証する。

### `mcad-core`

`Document` が entities/layers、undo/redo、generation を所有する。フィールドは private で、変更は必ず `Document::apply(Command)` を通す。

- 複数選択への操作は1個の `Command::Batch` にする。
- Batch は途中失敗時に逆順 rollback される。ロックレイヤー混在時も部分更新しない。
- Add 系の採番結果は `NewIds` の適用順リストから受け取る。iterator 差分などで ID を推測しない。
- 削除は tombstone であり、undo/redo をまたいで ID が安定する。
- no-op は履歴にも generation にも影響しない。app 側で dirty を手動操作しない。
- core の `Err`、特にレイヤーロック失敗を app で捨てずステータスバーへ表示する。

### `mcad-io`

`.mcad` は version 1 の JSON DTO、DXF は R2000 の交換形式。

- `.mcad` は layer 参照を DTO index に変換し、Command で Document を再構築する。
- DXF 対応は LINE / CIRCLE / ARC / LWPOLYLINE / POINT。
- DXF 色は ACI 9色へ近似。未知 ACI は中間グレー。
- layer lock と `Style::width` は DXF round-trip で保持されない。
- 未対応 entity と不正 geometry はファイル全体を失敗させず skip 件数に加算する。
- DXF は作業ファイルではない。import 後の Ctrl+S で元 DXF を上書きしない。

### `mcad-app`

egui UI、viewport、snap、作図 tool、selection/placement、ファイル操作を担当する。world は f64、egui 境界だけ f32、screen の y 軸は反転。

- `main.rs` がアプリ結線とファイル/UIイベント、`tool.rs` が作図と Select/Placement 状態、`snap.rs` が候補選択、`viewport.rs` が座標変換を担当する。
- egui に表示する文字列は ASCII のみ。
- placement preview は Document を変更せず、確定時だけ Command を apply する。
- Esc、ツール切替、ファイル操作、modal、undo/redo で進行中 placement を解除する。
- snap 候補の優先度は endpoint > intersection > midpoint > center > grid。
- GUI変更後は自動テストだけで完了扱いにせず、実ウィンドウ smoke test をユーザーへ依頼して結果を記録する。

## レビュー所見

### 1. 重大なコード不具合は確認されなかった

配置処理は preview と commit が分離され、複製は Add、移動は Modify、複数対象は Batch になっている。ゼロ変位、空選択、ロックレイヤー混在、cancel、undo/redo、選択維持/更新のテストが揃っている。M4 の dirty・import・zoom-fit 経路にも回帰は見られない。

### 2. `DESIGN.md` の M5 記述に実装前の表現が残る（Medium、タスク19着手前に修正推奨）

次の表現は現行コードとずれている。

- 設計判断1は `rotate` / `mirror` と書くが、公開 API は既存 `translated` に合わせた `rotated` / `mirrored`。
- 設計判断4は回転・ミラーを `R / M` 起動と書くが、`M` はタスク18で移動へ割り当て済み。
- タスク19の行にはミラーキーが「変更予定」とだけあり未確定。

実装前にミラーキーを決めること。`Shift+M` は既に候補として記録されているが、egui の shortcut 判定、README の keybinding、画面下部 help と同時に更新する必要がある。

### 3. HEAD が v0.4.1 リリースコミットより1コミット先行している（Medium、次の文書更新で整理）

`34b68ec` が v0.4.1 リリース/文書整合コミットで、その後の `3499a28` がファイルダイアログの最終ディレクトリ記憶を追加している。現在の `CHANGELOG.md` の 0.4.1 節にはこの変更がなく、README にも明記されていない。

次回のリリース整理では、後発変更を `[Unreleased]` または適切なリリース節へ記載すること。既存の 0.4.1 節を「リリース済みの固定スナップショット」と扱うなら、後から混ぜず Unreleased に置く方が履歴として明快である。

### 4. テスト本数の文書値が古い（Low）

`AGENTS.md` は「v0.4.1時点で236本」と書くが、現 HEAD は238本である。最終ディレクトリ機能の unit test 2本がリリースコミット後に増えたためで、コード上の問題ではない。次回ドキュメント整合時に更新するか、変動しやすい総数を削除してもよい。

### 5. app のファイルダイアログ記憶は妥当だが、失敗パスも記憶する仕様（Low、現状維持可）

open/import/save/export のいずれも、ユーザーがダイアログで確定した直後、実際の load/save より前に親ディレクトリを保存する。そのため読込失敗や保存失敗でも次回の開始ディレクトリは更新される。「最後に確定した場所」を覚える仕様として自然で、コミットメッセージとも整合する。もし将来「成功した操作の場所だけ」に意味を変更するなら `remember_dialog_dir` の呼出位置と test を揃えること。

### 6. ファイル肥大化は将来の保守リスク（Low、M5中に無理な全面分割は不要）

`main.rs` は約2300行、`tool.rs` は約2100行、`document.rs` は約1600行。現状は関心ごとの helper/type と単体テストがあり読めるが、回転・ミラー・オフセットを同じファイルへ平坦に追加すると状態遷移の見通しが落ちる。タスク19では既存 `PlacementKind` の拡張可能性を活かしつつ、変換ごとの command 構築と preview を小さな helper に分けること。大規模リファクタは機能変更と混ぜない。

## Claude が次に行うべきこと

### 最優先: M5 タスク19（回転・ミラー）

1. `DESIGN.md` でミラーキーと API 名を確定・整合する。
2. 選択が空なら R/ミラー shortcut を開始せず ASCII status を出す。
3. 回転は pivot と方向指定、ミラーは axis A/B の2クリック状態として実装する。両クリックに snap を適用する。
4. preview は `Shape::rotated` / `Shape::mirrored` を使い Document を変更しない。
5. 確定は選択中 entity ごとの `ModifyEntity` を1個の Batch にする。ID と選択集合を維持する。
6. 同一点によるゼロ方向/ゼロ長軸を拒否し、履歴を作らない。
7. Esc、modal、file op、tool switch、undo/redo で状態を解除する。
8. ロックレイヤー混在の原子的失敗と status 表示を確認する。
9. 状態遷移、command 内容、preview、zero input、cancel、locked Batch を unit test で固定する。
10. README、画面 help、CHANGELOG を同じ変更で揃え、実ウィンドウ smoke test を記録する。

回転角の UX は設計文言だけではまだ曖昧である。「pivot クリック後、ワールド +X を基準に2点目への絶対角で回す」のか、「pivot→参照点→target の3点で相対角を指定する」のかを実装前に明文化すること。現在の「2段階ステートマシン」を文字どおり取るなら前者だが、一般的な CAD の基準角指定とは操作感が異なる。ここは Claude が勝手に拡張せず、DESIGN を更新できる粒度でユーザーと決めるべき事項である。

### その次: M5 タスク20（オフセット）

オフセットは M5 で最も仕様リスクが高い。実装前に少なくとも以下を `DESIGN.md` へ追加する。

- クリックから符号付き距離をどう決めるか。
- offset は元 shape を変更するのか、新 entity を追加するのか。
- Point、ゼロ距離、source 上のクリックをどう扱うか。
- 円/円弧の内側 offset で半径が負またはゼロになる場合。
- open/closed polyline の端点、平行な隣接 segment、180度折返しの join fallback。
- 複数選択の一部に unsupported/degenerate shape がある場合、全体を失敗させるか対象だけ skip するか。

演算本体は必ず `mcad-geom` の純粋 API とし、app は入力・preview・Command 化だけを担当する。

## 検証手順

mcad のすべての変更前コミットで以下を通す。

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`mcad-geom` / `mcad-core` の公開 API を触った場合は `../tcad` でも同じ3チェックを行う。tcad の worktree が dirty なら変更を消さず、今回のレビューのように一時コピーまたはユーザーと合意した安全な方法で検証する。

GUI変更ではさらに実ウィンドウで、通常操作、Esc、undo/redo、locked layer、file/modal による進行状態解除、snap marker、status message を smoke test する。結果は CHANGELOG またはリリース記録へ残す。

## 最終評価

コードベースは小規模 CAD としてよく境界が守られ、特に Command/Batch、tombstone ID、generation dirty、DTO import の設計は次の編集機能を安全に増やせる状態にある。M5 の主要リスクは基盤の欠陥ではなく、回転入力とオフセット端ケースの仕様未確定、および実装進行に対する文書の軽微な遅れである。Claude は既存基盤を作り直さず、`DESIGN.md` の未確定点を狭く決めてからタスク19、20を順に積み上げるのがよい。
