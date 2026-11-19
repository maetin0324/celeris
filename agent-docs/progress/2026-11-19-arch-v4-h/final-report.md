---
title: アーキ v4 H final-report — A の基準からの before・after、検証の結果、未達・要判断、回帰条件
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: running
updated: 2026-11-19
---
# アーキ v4 H final-report

アーキ v4（依頼キー celeris-architecture-scalability-v4-20261117）の H（task 01M7MZTKBPZPQE4XN451F8X84F）のまとめ。
before は A の基準 main `bc665168c9caf1898adc4c611404bbee056daf9a`（以下 B0。[A の進捗](../2026-11-17-arch-v4-a.md)）、after は H の measure 段が数えた
`git merge-base HEAD main`（件名 `integrate wu/close (phase close)`。アーキ v4 G までが入った main）。
この file のために新しく測ったものは無い。数は measure 段と verify 段の md・tsv（[struct-deps](struct-deps.md)・[struct-store](struct-store.md)・
[loc-cost](loc-cost.md)・[review-prod](review-prod.md)・[gate](gate.md)・[prompt-audit](prompt-audit.md)・[safety](safety.md)）から写した。

表の読み方:
- (1)〜(6) の節には表が 2 つある。「A の項目」の表は、A の監査根拠の 14 項目（A の表の行を項目に分けた順。A が「#13 修正の波及」と呼ぶ番号）と、
  A の共通受入条件 (1)〜(6) の before を条件ごとに振り分け、before・after・差・判定を書いたもの。
  「tsv の行」の表は [struct-deps.tsv](struct-deps.tsv)・[struct-store.tsv](struct-store.tsv)・[loc-cost.tsv](loc-cost.tsv) の全行で、
  metric・scope・checker・before・after は tsv の値をそのまま写した（桁区切りなし。同じ metric の別 scope・別 checker は別の行）。
- 14 項目の振り分け: (1) に #2 Cargo の依存の辺と crate 循環・#3 module 循環・#4 task-core の SCC・#5 task-api の SCC・#6〜#8 health の既存負債・#14 `#[path]`、
  (2) に #9 dispatcher の参照、(3) に #10〜#12 SqliteStore、(4) に #13 修正の波及、(6) に #1 LoC。#6〜#8 は A の (1)〜(6) のどれにも名前が無く、
  struct-deps が循環と同じ検査器で数えたので (1) に置いた。(5) に当たる項目は 14 項目に無く、A の代表シナリオの時間と review の実績を before にした。
- checker の欄: `b0` は B0 の `scripts/dev/repo-health`、`head` は after の tree の repo-health（アーキ v4 B の変更入り。`super::` の path と `#[path]` の論理 module を
  辺の単位に使う）、`a-c3`・`a-c4`・`a-c6`・`a-c7` は A の baseline.md §7 の snippet、`e-e1` は E の E1（改行をまたぐ参照も数える）、`d-narrow`・`d-wide` は
  D の ADR の付記の 2 本の command、`h-awk` は H の補助の awk、`ssr@bc665168` は B0 に固定した source-size-report、`-` は検査器の版によらない値
  （git grep・cargo metadata・G の値の写し）。
- 成果の数え方: 移動・分割・抽象の追加だけの変更と、分母の増加は成果に数えない。差がその種類のときは判定か理由の欄にそう書いた。

## 結論

<!-- fill: conclude -->

## (1) import・依存・循環の内部の辺

判定: **未達**。循環の単位と組は before と after で同じだった（module の循環 17、task-core の SCC 55 単位・210 組、task-api の SCC 49 単位・261 組。
組の集合も同じ。b0 と head のどちらの検査器でも同じ）。減ったのは task-api の SCC の行の辺だけ（b0 628 → 618、head 632 → 622、どちらも −10）で、
task-core の SCC の行の辺は 617 → 621（+4）に増えた。2 つの SCC の行の合計は −6。

- task-api の −10 の内訳: D の org の pilot が −12、F の knowledge-pin が +2（`cos/ops/decisions.rs → knowledge`）。D は重複していた書き込み 2 本と
  SqliteStore の wrapper 3 つを消し、org の処理を SCC の外の新しい単位 `org_write` に集めた。行の減りには新しい単位への移動の分が含まれ、
  移動の分と削除の分は分けて測っていない。
- task-core の +4 は `task-core/src/chat/attachments.rs → store`（F の provenance-read）。組は増えていないので repo-health の `deps.cycle` は fail にならない。
- health の既存負債（size.responsibility-growth 94、dup.contract 210、owner 不一致 31）は件数も finding の scope の集合も同じ。
- `#[path]` で物理と論理の単位が食い違う file は task-dispatch で 9 → 9、workspace で 15 → 15。A の未解決（`docs/architecture-map.md:67` が
  check_item・review_limits・inventory の 3 file を `#[path]` の説明に書いていない）も残る（[struct-deps.md](struct-deps.md) §3）。

A の項目:

| # | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| #2 | Cargo の依存の辺 / crate 循環 | 49（normal 35・dev 14）/ 0 | 49（normal 35・dev 14）/ 0 | 0 / 0 | 変化なし（Cargo.toml・Cargo.lock の差分 0） |
| #3 | 既存の module 循環 | 17 | 17 | 0 | 減っていない |
| #4 | task-core の SCC（単位・組・行） | 55・210・617 | 55・210・621 | 0・0・+4 | 行の辺が増えた（組は同じ） |
| #5 | task-api の SCC（単位・組・行。b0） | 49・261・628 | 49・261・618 | 0・0・−10 | 行の辺だけ減った（単位・組は同じ） |
| #6 | size.responsibility-growth | 94 | 94 | 0 | 変化なし |
| #7 | dup.contract | 210 | 210 | 0 | 変化なし |
| #8 | owner 不一致（arch-map.wrong_owner） | 31 | 31 | 0 | 変化なし |
| #14 | `#[path]`（task-dispatch） | 6 ＋ 2（逆向き 1） | 同じ宣言（H の数え方で 9 → 9） | 0 | 変化なし（地図の 3 file の抜けも残る） |

tsv の行（struct-deps.tsv の循環・Cargo・health・`#[path]` の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `cargo_edges` | workspace の Cargo 宣言（normal+dev、cargo metadata） | - | 49 | 49 | 0 | A と同じ値（49 = normal 35 + dev 14）。Cargo.toml・Cargo.lock の B0 との差分 0 なので after の tree の cargo metadata を before にも使える |
| `crate_cycles` | workspace の通常依存の crate の SCC | - | 0 | 0 | 0 | A は数え方の snippet を書いていない。H は cargo metadata の normal の辺の強連結成分（2 crate 以上）で数えた |
| `module_cycles` | repo-health deps.cycle（C2 の SCC の数） | b0 | 17 | 17 | 0 | b0 は after の profile を読めない（profile_invalid）ので、profile を B0 から読み、測る tree を after にした（struct-deps.md §2 の 1） |
| `module_cycles` | repo-health deps.cycle（C1 の findings） | head | 17 | 17 | 0 | head の検査器（B の変更入り）。C2 の SCC の数も 17 → 17 |
| `tc_scc_units` | task-core の SCC（C2） | b0 | 55 | 55 | 0 | after の profile は B0 から読む（§2 の 1） |
| `tc_scc_units` | task-core の SCC（C2） | head | 55 | 55 | 0 | head の検査器でも task-core の値は A と同じ |
| `tc_scc_pairs` | task-core の SCC の単位の組（C2） | b0 | 210 | 210 | 0 | 組の集合も同じ。after の profile は B0 から読む |
| `tc_scc_pairs` | task-core の SCC の単位の組（C2） | head | 210 | 210 | 0 | 組の集合も同じ |
| `tc_scc_lines` | task-core の SCC の行の辺（C2） | b0 | 617 | 621 | +4 | A と同じ数え方。+4 は task-core/src/chat/attachments.rs → store（F の provenance-read） |
| `tc_scc_lines` | task-core の SCC の行の辺（C2） | head | 617 | 621 | +4 | b0 と同じ差（+4） |
| `ta_scc_units` | task-api の SCC（C2） | b0 | 49 | 49 | 0 | after の profile は B0 から読む |
| `ta_scc_units` | task-api の SCC（C2） | head | 49 | 49 | 0 | — |
| `ta_scc_pairs` | task-api の SCC の単位の組（C2） | b0 | 261 | 261 | 0 | 組の集合も同じ |
| `ta_scc_pairs` | task-api の SCC の単位の組（C2） | head | 261 | 261 | 0 | 組の集合も同じ |
| `ta_scc_lines` | task-api の SCC の行の辺（C2） | b0 | 628 | 618 | −10 | A と同じ数え方。−12 は D の org の pilot（org.rs→cos −3・admin.rs→handlers −2・admin.rs→skills −2・skills.rs→handlers −4・schema.rs→handlers −1）、+2 は F（cos/ops/decisions.rs→knowledge） |
| `ta_scc_lines` | task-api の SCC の行の辺（C2） | head | 632 | 622 | −10 | head の検査器は super:: の 4 行を辺にするので before が A より 4 多い（632）。差は b0 と同じ −10 |
| `responsibility_growth` | repo-health size.responsibility-growth（C1） | head | 94 | 94 | 0 | C1 の b0 × after は profile_invalid で測れないので head の行だけ（§2 の 2）。finding の scope の集合も同じ。b0 × B0 は 94 |
| `dup_contract` | repo-health dup.contract（C1） | head | 210 | 210 | 0 | 同上。b0 × B0 は 210 |
| `owner_mismatch` | repo-health arch-map.wrong_owner（C1） | head | 31 | 31 | 0 | 同上。b0 の arch-map は checker_missing で 0 になるので、比較は head × before と head × after（scope の集合も同じ） |
| `path_attr_files` | task-dispatch の production の #[path]（A §3.2） | - | 9 | 9 | 0 | A §3.2 の「8 ＋ 逆向き 1」を足して 9 とした |
| `path_attr_files` | workspace の production の #[path]（単位の食い違い、H の追加） | - | 15 | 15 | 0 | H の追加。workspace 全体で repo-health の単位が物理と論理で違う宣言（task-dispatch 9・task-worker 5・task-core 1） |

## (2) 共有状態・SQLite を直接参照する実装 file

判定: **未達**。`rusqlite` を書く production file（task-core/src/store/ の外）は 74 → 75（+1）、SQL 文字列を持つ file は 50 → 50（file の集合も同じ）。

- +1 は D が足した共有 use-case `crates/task-api/src/org_write.rs`（`use rusqlite::Transaction`。呼び手の transaction を受けて書く）で、抽象の追加に伴う増加。
- F は task-api の `knowledge.rs` と task-ops の `lifecycle.rs` の中の自前の接続と closure を消したが、A の正規表現は「接続を開く行」と
  「呼び手の `&Transaction` を受ける行」を区別しないので、file の数には出ない（[struct-store.md](struct-store.md) §7）。
- Dispatcher の参照: self.store を書く file は依頼 scope 44 → 38・論理 scope 48 → 42（C3。E1 でも −6）、self.config を書く file は
  依頼 35 → 31・論理 39 → 35（−4）。分母（依頼 69・論理 76）は変わらない。出現数は self.store が C3 633 → 633・E1 759 → 759、self.config が
  C3 218 → 220（+2 は rustfmt で割れていた 2 か所を 1 行にした分）・E1 227 → 227 で、参照の総量は減っていない。file 数の減りは E が対象 file の参照を
  入口の file へ移した結果（struct-store.tsv の note）なので、移動として成果に数えない（記録する）。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| (2) の before | `rusqlite` を書く production file（task-core/src/store/ の外） | 74 | 75 | +1 | 未達（org_write.rs の追加で増えた） |
| (2) の before | SQL 文字列を持つ production file | 50 | 50 | 0 | 未達 |
| #9 | dispatcher/ の非試験 file・self.store・self.config（依頼 scope、C3。file 数/出現数） | 69・44/633・35/218 | 69・38/633・31/220 | 0・−6/0・−4/+2 | file 数だけ減った（参照の移動。成果に数えない） |
| #9 | 同（論理 scope、C3） | 76・48/661・39/224 | 76・42/661・35/226 | 0・−6/0・−4/+2 | 同上 |

tsv の行（struct-store.tsv の rusqlite・SQL 文字列・dispatcher の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `rusqlite_prod_files` | prod-excl-task-core-store | a-c4 | 74 | 75 | +1 | A と同じ数え方（A §7 C4）。+1 は crates/task-api/src/org_write.rs（D の共有 use-case、use rusqlite::Transaction） |
| `sql_string_prod_files` | prod-excl-task-core-store | a-c4 | 50 | 50 | 0 | A と同じ数え方（C4）。file の集合も同じ |
| `dispatcher_files` | request/C3 | a-c3 | 69 | 69 | 0 | 分母。A §7 C3 |
| `dispatcher_files` | request/E1 | e-e1 | 69 | 69 | 0 | 分母。E1 は E の数え方（改行をまたぐ self\n.store も数える） |
| `dispatcher_files` | logical/C3 | a-c3 | 76 | 76 | 0 | 分母。69 − project_decider_ts.rs ＋ #[path] の 8 file（after で選び直して同じ） |
| `dispatcher_files` | logical/E1 | e-e1 | 76 | 76 | 0 | 分母（E1） |
| `dispatcher_store_files` | request/C3 | a-c3 | 44 | 38 | −6 | A と同じ数え方（C3）。−6 は E が対象 file の参照を入口の file に移した分（出現数は同じ） |
| `dispatcher_store_files` | request/E1 | e-e1 | 46 | 40 | −6 | E1 で数えても −6 |
| `dispatcher_store_files` | logical/C3 | a-c3 | 48 | 42 | −6 | 論理 scope でも −6 |
| `dispatcher_store_files` | logical/E1 | e-e1 | 50 | 44 | −6 | 論理 scope・E1 でも −6 |
| `dispatcher_store_hits` | request/C3 | a-c3 | 633 | 633 | 0 | A と同じ数え方（C3）。出現数は同じ |
| `dispatcher_store_hits` | request/E1 | e-e1 | 759 | 759 | 0 | E1 |
| `dispatcher_store_hits` | logical/C3 | a-c3 | 661 | 661 | 0 | 論理 scope |
| `dispatcher_store_hits` | logical/E1 | e-e1 | 791 | 791 | 0 | 論理 scope・E1 |
| `dispatcher_config_files` | request/C3 | a-c3 | 35 | 31 | −4 | A と同じ数え方（C3）。−4（E） |
| `dispatcher_config_files` | request/E1 | e-e1 | 35 | 31 | −4 | E1 でも −4 |
| `dispatcher_config_files` | logical/C3 | a-c3 | 39 | 35 | −4 | 論理 scope でも −4 |
| `dispatcher_config_files` | logical/E1 | e-e1 | 39 | 35 | −4 | 論理 scope・E1 でも −4 |
| `dispatcher_config_hits` | request/C3 | a-c3 | 218 | 220 | +2 | A と同じ数え方（C3）。+2 は rustfmt で割れていた self\n.config 2 か所を 1 行にした分（E） |
| `dispatcher_config_hits` | request/E1 | e-e1 | 227 | 227 | 0 | E1 では改行をまたぐ参照も数えるので差 0 |
| `dispatcher_config_hits` | logical/C3 | a-c3 | 224 | 226 | +2 | 論理 scope。+2 は上と同じ |
| `dispatcher_config_hits` | logical/E1 | e-e1 | 233 | 233 | 0 | 論理 scope・E1 |

## (3) HTTP と CoS の相互 import と SQL の公開面

判定: **一部だけ減った**。消した method の分（`SqliteStore` の `pub` −2・private −1）と handlers ⇄ cos の行の辺の減り（−3・−2）がある。
handlers ⇄ cos の単位の循環は解けておらず、task-api の `&Transaction` の引数は +1 になった。

- 公開面: `SqliteStore` の `pub` method 633 → 631（D の store-prune が `org_upsert_browser_settings`・`org_delete` を消した）、private 42 → 41
  （`org_upsert_with_actor`）。消した method なので成果に数える。impl block 102、`pub(crate)` 等 67、task-core の逆依存の crate 11 は同じ。
- handlers ⇄ cos: handlers → cos 22 → 19 行（13 → 12 file）、cos → handlers は `crate::` 48 → 46 行（9 file のまま）、`super::` 4 → 4 行、
  A の C5 の式 52 → 50 行。git grep と両方の検査器の辺で同じ差。task-api の SCC の単位 49・組 261 は同じ（(1)）。この減りは D が org の処理を
  `org_write` に集めた変更による。
- `&Connection`・`&Transaction` の引数: task-api 14 → 15（+1 は `org_write.rs:77` の `tx: &Transaction`）、task-core 320（うち store/ 205）と
  task-ops 7 は同じ。`Fn(&Connection)` の closure 引数は task-ops 5 → 4（F が `apply_project_action` の closure を消した）。
- A の数え方の外（F の進捗の値。H は再計測していない）: task-core に `&SqliteStore` を取る `pub` の自由関数が F で 7 → 8 に増えた
  （method を増やさない代わりに関数にした。[struct-store.md](struct-store.md) §1）。
- `Applied` の形の残数（applied_narrow・applied_wide・applied_functions）も下の表に置いた。領域ごとの内訳と移行の提案は「Applied の形の残数と領域ごとの移行の提案」の節。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| #10 | `SqliteStore` の impl（plain・path 付き込み・trait） | 102・105・6 | 102・105・6 | 0 | 変化なし |
| #11 | `SqliteStore` の pub method / pub(crate) 等 | 633 / 67 | 631 / 67 | −2 / 0 | 減った（D が消した 2 method） |
| #12 | task-core に依存する crate（通常依存の直接・間接） | 11 | 11 | 0 | 変化なし |
| (3) の before | handlers → cos の行（git grep） | 22 | 19 | −3 | 行は減った。単位の循環は残る |
| (3) の before | cos → handlers の行（`crate::` ＋ `super::`） | 48 ＋ 4 | 46 ＋ 4 | −2 ＋ 0 | 同上 |
| (3) の before | `&Connection` 型の引数（task-core（うち store/）・task-api・task-ops） | 320（205）・14・7 | 320（205）・15・7 | 0・+1・0 | task-api で増えた |

tsv の行（struct-deps.tsv の handlers ⇄ cos、struct-store.tsv の引数・SqliteStore・Applied の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `handlers_to_cos_lines` | git grep crate::cos::（C5、handlers/ と handlers.rs） | - | 22 | 19 | −3 | A §7 C5 の git grep と同じ。13 → 12 file |
| `handlers_to_cos_lines` | repo-health の辺（C2 の task-api の SCC、handlers→cos） | b0 | 22 | 19 | −3 | A §6 の 2 に従い repo-health の辺も並べた（b0） |
| `handlers_to_cos_lines` | repo-health の辺（C2 の task-api の SCC、handlers→cos） | head | 22 | 19 | −3 | 同（head） |
| `cos_to_handlers_lines` | git grep crate::handlers（C5、cos/） | - | 48 | 46 | −2 | A の C5 の式から crate:: の行だけを分けた（H の分け方）。9 file のまま |
| `cos_to_handlers_lines` | git grep (crate\|super(::super)*)::handlers（A の C5 の式） | - | 52 | 50 | −2 | A の C5 の式（crate:: と super:: の合計） |
| `cos_to_handlers_lines` | repo-health の辺（C2 の task-api の SCC、cos→handlers） | b0 | 48 | 46 | −2 | b0 の検査器は super:: を辺にしない（A の 48 と同じ数え方） |
| `cos_to_handlers_lines` | repo-health の辺（C2 の task-api の SCC、cos→handlers） | head | 52 | 50 | −2 | head の検査器は super:: の 4 行を辺にする |
| `cos_to_handlers_super_lines` | git grep super(::super)*::handlers（C5、cos/） | - | 4 | 4 | 0 | super:: の行だけを分けた（H の分け方）。cos/ops/admin.rs の accounts の 4 行 |
| `conn_arg_task_core` | crates/task-core/occ | a-c6 | 320 | 320 | 0 | A と同じ数え方（C6） |
| `conn_arg_task_core` | crates/task-core/files | a-c6 | 76 | 76 | 0 | 同じ grep の file 数（補助） |
| `conn_arg_task_core_store` | crates/task-core/src/store/occ | a-c6 | 205 | 205 | 0 | A と同じ数え方（task-core の内数） |
| `conn_arg_task_core_store` | crates/task-core/src/store/files | a-c6 | 49 | 49 | 0 | A は file 数を書いていない。H が同じ grep の出力の file 数を数えた |
| `conn_arg_task_api` | crates/task-api/occ | a-c6 | 14 | 15 | +1 | A と同じ数え方。+1 は org_write.rs:77 の tx: &Transaction（D） |
| `conn_arg_task_api` | crates/task-api/files | a-c6 | 9 | 10 | +1 | 同じ grep の file 数。+1 は org_write.rs |
| `conn_arg_task_ops` | crates/task-ops/occ | a-c6 | 7 | 7 | 0 | A と同じ数え方。lifecycle.rs の apply_project_change_tx の引数の型が &Connection → &Transaction（F）。数は同じ |
| `conn_arg_task_ops` | crates/task-ops/files | a-c6 | 4 | 4 | 0 | 同じ grep の file 数 |
| `conn_fn_closure` | crates/task-core | a-c6 | 13 | 13 | 0 | 補助（A §4 の Fn(&Connection) の closure 引数） |
| `conn_fn_closure` | crates/task-api | a-c6 | 4 | 4 | 0 | 補助 |
| `conn_fn_closure` | crates/task-ops | a-c6 | 5 | 4 | −1 | 補助。−1 は task-ops/src/lifecycle.rs の apply_project_action（F が消した） |
| `sqlitestore_impl` | plain | a-c6 | 102 | 102 | 0 | A と同じ数え方（C6）。88 file のまま |
| `sqlitestore_impl` | path | a-c6 | 105 | 105 | 0 | path 付きの impl crate::store::SqliteStore 3 を含む |
| `sqlitestore_impl` | trait | a-c6 | 6 | 6 | 0 | 補助。impl … for SqliteStore の 6 件は同じ |
| `sqlitestore_pub` | plain | a-c6 | 633 | 631 | −2 | A と同じ数え方（括弧を数える近似）。−2 は org_upsert_browser_settings・org_delete（D の store-prune が消した） |
| `sqlitestore_pub` | path | a-c6 | 636 | 634 | −2 | path 付きの 3 block を含む |
| `sqlitestore_pub_crate` | plain | a-c6 | 67 | 67 | 0 | pub(crate)・pub(super)・pub(in …) |
| `sqlitestore_pub_crate` | path | a-c6 | 67 | 67 | 0 | path 付きを含む |
| `sqlitestore_private` | plain | a-c6 | 42 | 41 | −1 | 補助。−1 は org_upsert_with_actor（D） |
| `sqlitestore_rdeps` | normal-transitive | a-c7 | 11 | 11 | 0 | A と同じ数え方（C7）。before は git archive した B0 の tree に cargo metadata --offline を当てた（作業ツリーを切り替えないため） |
| `applied_narrow` | crates/task-api/src | d-narrow | 14 | 14 | 0 | A に無い指標（D の付記の狭い command）。file の集合も同じ |
| `applied_wide` | crates/task-api/src | d-wide | 19 | 18 | −1 | A に無い指標（D の付記の広い command）。−1 は handlers/org.rs（D）。回帰条件 18 以下を満たす |
| `applied_functions` | crates/task-api/src | h-awk | 58 | 55 | −3 | H の補助（awk）。−3 は org_commit（D）・record_op・project_action_op（F） |

## (4) 変更波及

判定: **未達**（減っていない）。修正の波及（#13）は **比較不能**。

- S1（新しい CoS 業務操作）: 変える production file 3 → 4、単位 3 → 4、crate 2 → 2。+1 は D が足した `crates/task-api/src/org_write.rs`
  （G の scenarios.env の選び直し）で、抽象の追加による増加。
- S2（WorkUnit の終了処理）: 4 file・2 単位・1 crate のまま。
- import する単位（C7b）は検査器の版をそろえると同じ: b0 で S1 92 / 369（12 package）、S2 6 / 14（4 package）。head で S1 93 / 365（before' は
  head の検査器 × B0）。`test-affected.sh --list` の対象も A と同じ集合（S1 14 crate ＋ web ＋ ts-verify、S2 5 crate）。
- 修正の波及（#13）: A の窓の値は再現した（no-merges 194 件・複数 region 46.9 %・複数 module 69.1 %）。B0 の後の窓
  （`bc665168..HEAD`、SINCE 2026-11-17T00:00Z）で production を変えた commit は no-merges で 1 件、merge を含めて 9 件しかない。9 件はアーキ v4 の C・E・F の
  commit と WorkUnit の統合、main の変更を E・F の branch に取り込んだ merge で、通常の修正 1 件と粒度が違う。割合は A の窓と比べられない
  （[loc-cost.md](loc-cost.md) §3）。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| (4) の before | S1 で変える file・単位・crate | 3・3・2 | 4・4・2 | +1・+1・0 | 増えた（抽象の追加） |
| (4) の before | S2 で変える file・単位・crate | 4・2・1 | 4・2・1 | 0 | 変化なし |
| (4) の before | S1 の単位を直接・推移的に import する単位（b0） | 92・369 | 92・369 | 0 | 変化なし |
| (4) の before | S2 の単位を直接・推移的に import する単位（b0） | 6・14 | 6・14 | 0 | 変化なし |
| (4) の before | test-affected の対象（S1・S2） | 14 crate ＋ web ＋ ts-verify・5 crate | 同じ集合 | 0 | 変化なし |
| #13 | 修正の波及（no-merges。件数・複数 region・複数 module） | 194・46.9 %・69.1 % | 1・0.0 %・0.0 % | 比較不能 | 比較不能（件数が少ない） |
| #13 | 修正の波及（with-merges） | 246・49.6 %・71.1 % | 9・55.6 %・55.6 % | 比較不能 | 比較不能（1 件が WorkUnit か task の束） |

tsv の行（loc-cost.tsv の S1・S2・修正の波及の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `s1_files` | S1 で変える production file | - | 3 | 4 | +1 | after は G の scenarios.env の選び直し。+1 は D の org_write.rs（抽象の追加による増加） |
| `s1_units` | S1 で変える単位（repo-health の単位 crates/<c>/src/<第 1 要素>） | - | 3 | 4 | +1 | +1 は task-api の org_write |
| `s1_crates` | S1 で変える crate | - | 2 | 2 | 0 | A と同じ数え方 |
| `s2_files` | S2 で変える production file | - | 4 | 4 | 0 | A と同じ数え方（A と同じ 4 file） |
| `s2_units` | S2 で変える単位 | - | 2 | 2 | 0 | A と同じ数え方 |
| `s2_crates` | S2 で変える crate | - | 1 | 1 | 0 | A と同じ数え方 |
| `s1_direct_importers` | S1 の単位を直接 import する単位（C7b） | b0 | 92 | 92 | 0 | b0 の検査器で HEAD を数えた（profile は B0 から読む）。after は 4 単位。b0 × B0 は A の 92 を再現 |
| `s1_direct_importers` | S1 の単位を直接 import する単位（C7b） | head | 93 | 93 | 0 | head の検査器。before は before'（head の検査器 × B0。A の値ではない）。A との差は B の検査器の変更による |
| `s1_transitive_importers` | S1 の単位を推移的に import する単位（C7b） | b0 | 369 | 369 | 0 | b0。package 12 → 12。b0 × B0 は A の 369 を再現 |
| `s1_transitive_importers` | S1 の単位を推移的に import する単位（C7b） | head | 365 | 365 | 0 | head。before は before'。package 12 → 12 |
| `s2_direct_importers` | S2 の単位を直接 import する単位（C7b） | b0 | 6 | 6 | 0 | b0。head の検査器でも 6 → 6 |
| `s2_transitive_importers` | S2 の単位を推移的に import する単位（C7b） | b0 | 14 | 14 | 0 | b0。package 4 → 4。head の検査器でも 14 → 14 |
| `s1_ta_crates` | S1 の test-affected --list の crate（comment の probe） | - | 14 | 14 | 0 | A と同じ数え方（comment の probe）。＋ web ＋ ts-verify。A と同じ集合 |
| `s2_ta_crates` | S2 の test-affected --list の crate（comment の probe） | - | 5 | 5 | 0 | A と同じ数え方。A と同じ集合 |
| `fix_commits` | 修正の波及 C10 no-merges（production を変えた first-parent の非 merge commit） | ssr@bc665168 | 194 | 1 | 比較不能 | after の窓は bc665168..HEAD（SINCE 2026-11-17T00:00Z）。production を変えたのは 12 commit のうち 1 件 |
| `fix_commits` | 修正の波及 C10 with-merges（merge commit を first-parent の差分で含める） | ssr@bc665168 | 246 | 9 | 比較不能 | after は 41 commit のうち 9 件。すべてアーキ v4 の変更か WU の統合の merge で、通常の修正と粒度が違う |
| `fix_region_pct` | C10 no-merges の複数 region（crate か top-level の dir）の割合 % | ssr@bc665168 | 46.9 | 0.0 | 比較不能 | after は 1 件中 0 件。1 件では割合にならない |
| `fix_region_pct` | C10 with-merges の複数 region の割合 % | ssr@bc665168 | 49.6 | 55.6 | 比較不能 | after は 9 件中 5 件。merge 1 件が WorkUnit か task の束 |
| `fix_module_pct` | C10 no-merges の複数 module の割合 % | ssr@bc665168 | 69.1 | 0.0 | 比較不能 | after は 1 件中 0 件 |
| `fix_module_pct` | C10 with-merges の複数 module の割合 % | ssr@bc665168 | 71.1 | 55.6 | 比較不能 | after は 9 件中 5 件 |

## (5) build・test-affected・review・health の時間と費用

判定: **health の S2 は A の +17.3 % で悪化が残る。review の時間と費用は比較不能**。ほかの 9 組は A の +10 % 以内。

- 10 組の after は G の 3 回の中央値の写し。G の測定の tree と HEAD で crates/・scripts/・web/・Cargo.lock・Cargo.toml の差分は 0 で、H は測り直していない
  （[loc-cost.md](loc-cost.md) §4）。
- health_s2 の 96.09 s は A の +10 %（90.11 s）を超える。G が同じ host で before を測り直した中央値は 87.26 s、コード由来は `deps.violation` の CPU +2.02 s
  （+2.3 %）で、人が決定 h1 で「直さない（keep）」と決めた（[G の進捗](../2026-11-19-arch-v4-g.md)）。
- review の窓: A は [2026-11-10T00:00Z, 2026-11-17T15:00Z)（7.625 日）、after は [2026-11-17T15:00Z, 2026-11-19T12:00Z)（1.875 日）、promo は C を含む release の
  昇格の後 [2026-11-18T12:18:40Z, 2026-11-19T12:00Z)（0.987 日）。日あたりの reviewer run と費用は A の窓より少ないが、窓が短く仕事の量も違うので
  増減とは書かない（[review-prod.md](review-prod.md) §0）。A と同じ長さの窓は 2026-11-25T06:00Z の後に揃う（同 §7.1）。
- 設計 review（review-prod.md §4・§6）: promo の窓で要求 7 件・3 task（D・C の申し送り・F）、run の費用の合計 $14.33、日あたり 7.092 件・$14.52
  （A の窓は 3.541 件・$5.87）。C の見積もり（日あたり +0.670〜+4.498 回・+$1.37〜+$9.21）と比べると、A の窓との差の数え方では +3.551 回/日・+$8.65/日
  （範囲の中）、`new_*` の宣言の無い要求の数え方では 6.08 回/日・$12.59/日（上限を超える）で、数え方で結論が変わる。

時間（tsv の行。loc-cost.tsv の 10 組。単位は wall の秒、差の括弧は A 比）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `check_cold` | cargo check --workspace の cold の wall 中央値 s | - | 53.65 | 53.90 | +0.25（+0.5 %） | after は G の値（build-check.tsv）。H は測り直していない（G の測定の tree と source の差分 0） |
| `check_warm_s1` | cargo check の warm S1（const の probe）の wall 中央値 s | - | 42.47 | 42.64 | +0.17（+0.4 %） | after は G の値（build-check.tsv） |
| `check_warm_s2` | cargo check の warm S2 の wall 中央値 s | - | 14.67 | 15.56 | +0.89（+6.1 %） | after は G の値（build-check.tsv） |
| `test_build_cold` | cargo nextest run --workspace --no-run の cold の wall 中央値 s | - | 142.39 | 147.03 | +4.64（+3.3 %） | after は G の値（build-test.tsv） |
| `test_build_warm_s1` | test build の warm S1 の wall 中央値 s | - | 138.69 | 138.10 | −0.59（−0.4 %） | after は G の値（build-test.tsv） |
| `test_build_warm_s2` | test build の warm S2 の wall 中央値 s | - | 77.64 | 81.60 | +3.96（+5.1 %） | after は G の値（build-test.tsv） |
| `health_s1` | repo-health S1（const の probe、--head worktree）の wall 中央値 s | head | 86.56 | 93.58 | +7.02（+8.1 %） | after は G の値（scope.tsv） |
| `health_s2` | repo-health S2 の wall 中央値 s | head | 81.92 | 96.09 | +14.17（+17.3 %） | after は G の値（scope.tsv）。A の +10 %（90.11）を超える。G が同じ host で測り直した before は 87.26、コード由来 +2.02（人の決定 h1=keep） |
| `test_affected_s1` | bash scripts/dev/test-affected.sh S1（comment の probe）の wall 中央値 s | - | 893.41 | 906.03 | +12.62（+1.4 %） | after は G の値（ta-s1.tsv・ta-mix.tsv）。規則により測り直さない |
| `test_affected_s2` | test-affected S2 の wall 中央値 s | - | 655.59 | 707.76 | +52.17（+8.0 %） | after は G の値（ta-s2.tsv・ta-mix.tsv）。規則により測り直さない |

review の時間と費用（[review-prod.tsv](review-prod.tsv)。A §7 C11 の query。wall は s、cost は USD）:

| metric | A の窓 | after の窓 | promo の窓 | 注 |
|---|---|---|---|---|
| `review_runs` | 165 | 30 | 17 | 窓の日数が違う（7.625・1.875・0.987 日）ので件数は比べない。promo の 17 run のうち 7 run は設計 review |
| `review_wall_median` | 1137.5 | 514.4 | 434.5 | wall_ms のある run（A 130・after 27・promo 16）の中央値。A の値は A の進捗と同じ |
| `review_cost_median` | 2.047 | 2.167 | 2.036 | cost_usd のある run（A 127・after 27・promo 16）の中央値。A の値は A の進捗と同じ |
| `review_runs_per_day` | 21.64 | 16.00 | 17.22 | 窓が短く、仕事の量も違うので増減とは書かない |
| `review_cost_per_day` | 36.74 | 28.59 | 26.92 | 同上 |

## (6) production と試験の LoC

判定: **増えた**（production は純減していない。アーキ v4 の依頼は一律の純減を求めていない）。分類規則は B0 の source-size-report に固定した
（before と after の `config_sha256` が同じ）。

- production は全言語・Rust とも +822 行（差はすべて Rust）。試験 +4743 行、own_total_with_tests +5829 行、tooling +264 行。
- before は A の (6) と一致し、after は G の進捗の (6) と一致した（[loc-cost.md](loc-cost.md) §1）。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| #1 | production 全言語 / Rust | 375099 / 330408 | 375921 / 331230 | +822 / +822 | 増えた |
| (6) の before | 試験 / own_total_with_tests | 476771 / 1025677 | 481514 / 1031506 | +4743 / +5829 | 増えた |

tsv の行（loc-cost.tsv の LoC の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `loc_prod_all` | production 全言語（views.product_only.lines） | ssr@bc665168 | 375099 | 375921 | +822 | A と同じ数え方（A §7 C12、分類規則は B0 に固定）。差はすべて Rust |
| `loc_prod_rust` | production Rust（views.product_only.rust_lines） | ssr@bc665168 | 330408 | 331230 | +822 | A と同じ数え方 |
| `loc_test` | 試験 = class_totals.external_test + inline_test | ssr@bc665168 | 476771 | 481514 | +4743 | A と同じ数え方（external_test + inline_test）。Rust +4265、Python +478 |
| `loc_own_total` | own_total_with_tests 全言語（views.own_total_with_tests.lines） | ssr@bc665168 | 1025677 | 1031506 | +5829 | A と同じ数え方。tooling を含む |
| `loc_own_total_rust` | own_total_with_tests Rust（views.own_total_with_tests.rust_lines） | ssr@bc665168 | 879344 | 884431 | +5087 | A と同じ数え方 |
| `loc_tooling` | tooling（class_totals.tooling） | ssr@bc665168 | 25253 | 25517 | +264 | A の (6) に無い区分（H の補助）。Python +190、TOML +74 |

## 判定経路

<!-- fill: verify -->

## 実 prompt

<!-- fill: verify -->

## 安全不変条件

<!-- fill: verify -->

## Applied の形の残数と領域ごとの移行の提案

`Applied` の形（`Option<&OperationAudit>` を受けて `Applied` を返す操作）の残数は、D の ADR の付記の 2 本の command と H の補助の awk で数えた
（[struct-store.md](struct-store.md) §3）。広い command の file は 19 → 18 で、回帰条件（18 以下）を満たす。

| metric | 数え方 | before | after | 差 | 注 |
|---|---|---|---|---|---|
| `applied_narrow` | `git grep -l 'Option<&OperationAudit>' crates/task-api/src` | 14 | 14 | 0 | file の集合も同じ（`handlers/org.rs` はもともと入らない） |
| `applied_wide` | 広い command（`'name` の寿命と `crate::cos::operations::` の完全修飾も当てる） | 19 | 18 | −1 | `handlers/org.rs`（D） |
| `applied_functions` | 広い command の file で該当の引数を受ける関数（H の補助） | 58 | 55 | −3 | `org_commit`（D）・`record_op`・`project_action_op`（F） |

- 広い 18 file の内訳: 型の定義側 `cos/operations.rs`（`file_op`）の 1 file・1 関数と、操作の側の 17 file・54 関数。
  狭い command は広い 18 file のうち ★ の 4 file に当たらない（18 − 4 = 14）。
- 撤去の条件（[ADR 2026-11-18 org-write-shared-use-case](../../adr/2026-11-18-org-write-shared-use-case.md) の付記）: その領域の共有の操作を変える task が、
  同じ変更で org_write の形へ移す。
- 下の表の「移行の task を提案する」は提案で、H は起票しない（起票は人が決める）。path は `crates/task-api/src/` からの相対。

| 領域 | file | 残る関数 | 残数（file / 関数） | 提案 | 理由 |
|---|---|---|---|---|---|
| 知識 | `knowledge.rs` | `accept_op`・`reject_op`・`put_page_op` | 1 / 3 | 移行の task を提案する | F が同じ file の `record_op` を移し、HTTP の `write_tx`・CoS の `reject`・`apply_checked` の手順と契約試験が既にある。`knowledge.rs` は `transaction` class の高 risk の path で、2 つの形が同じ file に並ぶ（F の提案 3） |
| task（lifecycle） | `lifecycle.rs` | `task_pause_op` | 1 / 1 | 移行の task を提案する（知識と別の 1 件） | F が同じ file の `project_action_op` を移した。1 関数で、F の lifecycle-tx と同じ手順を当てられる見込み（H は確かめていない） |
| skills | `skills.rs` | `put_skill_op`・`delete_skill_op` | 1 / 2 | 移行の task を提案する | D で org の書き込みを org_write に移した HTTP adapter で、同じ file に 2 つの形が並ぶ |
| task（その他） | `handlers/task_actions.rs`・`handlers/tasks.rs`・`integrate_task.rs` | `retry_op`・`patch_task_op`・`create_comment_op`・`reopen_op`・`rereview_op`・`create_task_op`・`create_task_linked_op`・`integrate_task_op` | 3 / 8 | 意図して残す | 下の共通の理由 |
| 承認 | `approvals.rs` | `decide_op`・`create_standing_rule_op`・`delete_standing_rule_op` | 1 / 3 | 意図して残す | 同上 |
| 変更 | `changes.rs` | `changes_effect`・`integrate_op`・`pr_merge_op` | 1 / 3 | 意図して残す | 同上 |
| 判断 | `decisions.rs` | `answer_op`・`withdraw_op`・`revise_op`・`resolve_op` | 1 / 4 | 意図して残す | 同上 |
| 文書 | `docs.rs` | `docs_effect`・`reject_for`・`docs_init_op`・`put_page_op`・`delete_page_op`・`promote_op`・`maintenance_op` | 1 / 7 | 意図して残す | 同上 |
| 案件 | `handlers/projects.rs` | `create_project_op`・`put_project_brief_op` | 1 / 2 | 意図して残す | 同上 |
| 実行計画 | `execution.rs` ★ | `execution_plan_adopt_op`・`tree_adopt_op`・`phase_gate_op`・`decompose_op` | 1 / 4 | 意図して残す | 同上 |
| cron | `cron_jobs.rs` ★ | `create_job_op`・`update_job_op`・`delete_job_op`・`pause_job_op`・`resume_job_op`・`run_job_op`・補助の `reject_with`・`resolve_for`（`type Audit<'a> = Option<&'a OperationAudit>` を受ける） | 1 / 8 | 意図して残す | 同上 |
| clusters | `handlers/clusters.rs` ★ | `put_local_settings_op`・`put_cluster_settings_op` | 1 / 2 | 意図して残す | 同上 |
| inbox の通知 | `inbox_notifications.rs` ★ | `notice_read_op`・`notice_read_all_op` | 1 / 2 | 意図して残す | 同上 |
| repos | `repos.rs` | `create_repo_op`・`patch_repo_op`・`delete_repo_op` | 1 / 3 | 意図して残す | 同上 |
| model_catalog | `model_catalog.rs` | `put_override_op`・`delete_override_op` | 1 / 2 | 意図して残す | 同上 |
| （型の定義側） | `cos/operations.rs` | `file_op` | 1 / 1 | 残す | 操作の側の file が残る間は要る |

操作の側の合計は 17 file・54 関数（移行の提案 3 領域で 3 file・6 関数、意図して残す領域で 14 file・48 関数）。

意図して残す共通の理由: 移すには領域ごとに HTTP と CoS の契約試験を旧コードで先に固定する作業が要る（D の ADR の付記の「D がこの task で全部を移さない理由」と同じ）。
その領域を次に変える task が、撤去の条件に従って同じ変更で移す。新しい操作をこの形で足さないことは、広い command の回帰条件（18 以下）で見る。

## 未達・要判断

<!-- fill: conclude -->

## 提案

<!-- fill: conclude -->

## 回帰条件

<!-- fill: conclude -->

## リリースゲート

<!-- fill: verify -->

## 再現コマンド

<!-- fill: verify -->
