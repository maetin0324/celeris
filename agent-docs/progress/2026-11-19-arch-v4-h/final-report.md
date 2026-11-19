---
title: アーキ v4 H final-report — A の基準からの before・after、検証の結果、未達・要判断、回帰条件
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
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

人が決めること（H は決めていない。H は起票もしていない）:

1. **未達の扱い**（close 段の決定 unmet）。共通受入条件 (1)・(2)・(4) は未達、(3) は一部だけ減った、(6) は増えた、(5) は health:S2 の悪化が残り、
   review の時間と費用と修正の波及（#13）は比べられない。これらを未達として記録して H を閉じるか、次の task を求めるかを人が選ぶ。
   一覧は「未達・要判断」の節（項目ごとに数値・理由・次の候補）。
2. **提案の起票の要否**。「提案」の節の 17 件（D・E・F の次の候補、F の提案 1・2、`Applied` の形の移行 3 領域、planner の prompt の受入条件ほか）を
   起票するかを人が選ぶ。H は 1 件も起票していない。
3. **2026-11-25T06:00Z の後の review の比較を誰が行うか**（[review-prod.md](review-prod.md) §7.1 の手順。決定 unmet で人が決める）。

数値の要点（before は B0、after は `git merge-base HEAD main`。どれも各 md・tsv の写しで、この file のために測り直していない）:

- (1) 未達: module の循環 17 → 17、task-core の SCC 55 単位・210 組、task-api の SCC 49 単位・261 組はどれも同じ（組の集合も同じ）。
  行の辺は task-api −10（b0 628 → 618）、task-core +4（617 → 621）。health の既存負債 94・210・31 も同じ。
- (2) 未達: `rusqlite` を書く production file 74 → 75（+1 は D の `org_write.rs`）、SQL 文字列の file 50 → 50。dispatcher で self.store を書く file 44 → 38・
  self.config 35 → 31 は参照を入口の file へ移した結果で（出現数 633 → 633）、成果に数えない。
- (3) 一部: `SqliteStore` の `pub` method 633 → 631・private 42 → 41（D が消した method。成果に数える）、`Fn(&Connection)` の closure 引数は task-ops で 5 → 4
  （F が消した）。handlers → cos 22 → 19 行、cos → handlers 48 → 46 行（`crate::`）だが、単位の循環は残る。task-api の `&Transaction` の引数は 14 → 15。
- (4) 未達: S1 で変える file・単位 3 → 4（D の `org_write.rs`。抽象の追加）、S2 は 4 file・2 単位のまま。import する単位（b0 で S1 92 / 369、S2 6 / 14）と
  test-affected の対象は同じ。修正の波及（#13）は B0 の後の窓に production を変えた commit が no-merges で 1 件・with-merges で 9 件しかなく、比べられない。
- (5) health:S2 は 96.09 s で A（81.92 s）の +17.3 %。A の +10 %（90.11 s）を超える。コード由来は +2.02 s で、人が決定 h1 で keep と決めた。
  ほかの 9 組は A の +10 % 以内。review の時間と費用は窓の長さが違い（A 7.625 日、after 1.875 日）比べられない。
- (6) 増えた: production +822 行（375099 → 375921、すべて Rust）、試験 +4743 行、own_total_with_tests +5829 行。依頼は一律の純減を求めていない。
- 判定経路: gate.tsv の 10 行すべて期待どおり（C の 6 ケース、`prompt_boundary_` 3 件、repo-health の試験 126 件、probe a は exit 1、probe b は exit 0）。
  本番でも C を含む release の昇格の後の設計 review の要求 7 件（3 task・$14.33）は、すべて C の形の理由で始まった。
- 実 prompt: worker 157 run と reviewer 20 run は、prompt が残る run のすべてで objective の末尾と受入条件が入った（cut 0）。worker の 4 run は prompt.txt が無く未確認。
  planner の 24 run は objective の末尾が全部入ったが、task の受入条件は 0/126 件。
- 安全不変条件: safety.tsv の 11 行すべて pass（延べ 563 件、fail 0）。D・E・F の反例試験は 81 passed。ただし userns の env が無いと検査の前に return する 9 件は実質未検査。
- `Applied` の形: 広い command の file は 19 → 18 で、回帰条件（18 以下）を満たす。
- リリースゲート: H を含む release の gate は未実施。いまの current の release（B〜G を含む）は gate ok・verify ok で昇格済み。

成果に数えたのは、消した method（`pub` −2・private −1）と消した closure（−1）、`Applied` の旧い形を消して org_write の形にした関数（−3）である。行の辺の減り（task-api −10、
handlers ⇄ cos −3・−2）は移動の分と削除の分を分けて測っていないので、成果として確定しない。dispatcher の file 数の減り（−6・−4）は移動なので数えない。
増えた項目（`rusqlite` の file +1、S1 の file・単位 +1、`&Transaction` +1）は D の共有 use-case という抽象の追加による。分母（dispatcher の 69・76）は増えていない。

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

判定: **10 行すべて期待どおり（未達 0）**。C の 6 ケース・prompt の 3 件・repo-health の python 試験 126 件が通り、本物の検査器では SCC の中の新しい組が止まり、
同じ file の普通の変更は通った。本番でも、C を含む release の昇格の後の設計 review の要求 7 件はすべて C の形の理由で始まった。

- 出所は [gate.md](gate.md)・[gate.tsv](gate.tsv)（列 `case result command note`）と [review-prod.md](review-prod.md) §5。gate の tree は gate の WorkUnit の HEAD
  （件名 `integrate wu/struct-store (phase measure)`）、health の base は `git merge-base HEAD main`（件名 `integrate wu/close (phase close)`）。
  両者の `scripts/dev/repo-health` の差分は 0 で、検査器は base と同じ内容。host は home-dev、2026-11-19T11:55Z〜12:05Z。
- result は「期待どおりの結果だったか」で、検査器の verdict ではない（危険な probe は検査器が fail を出せば pass）。

gate.tsv の行（case と result は tsv のまま）:

| case | result | 確かめたこと | 要点（gate.tsv の note・gate.md §1・§3） |
|---|---|---|---|
| case1 | pass | `boundary_case_1_authz_one_line_is_high` | 認可の条件 1 行の変更は、`new_*` の宣言が無くても High で、設計 review を求める |
| case2 | pass | `boundary_case_2_equivalent_plain_change_is_low` | 画面・class 外の handler の同等の変更は Low で、設計 review を求めない |
| case3 | pass | `boundary_case_3_move_cannot_escape` | file の移動・別名・`#[path]` の付け替えで class の判定を逃れられない |
| case4 | pass | `boundary_case_4_worsening_dependency_blocks` | 汎用 Rust の fixture で、既存の循環への新しい組は health の関門で止まる |
| case5 | pass | `boundary_case_5_policy_self_approval_impossible` | 候補自身の policy で class を外しても、base の policy で判定される |
| case6 | pass | `boundary_case_6_fail_closed` | 方針が読めない・差分が取れない・判定が欠けるときは合格にしない |
| prompt_boundary | pass | `prompt_boundary_` の 3 件 | 3 tests run: 3 passed, 1311 skipped、exit 0。3,000 文字の objective の末尾を確かめるのは worker の prompt だけ（gate.md §2） |
| repo_health_tests | pass | `test_repo_health_*.py` | Ran 126 tests in 51.6s OK、exit 0。B が足した SCC の内部辺の 5 件（`test_repo_health_scc_edges.py`）を含む |
| probe_scc_edge | pass | 本物の検査器に probe a | SCC の中にまだ無い組を 1 行足した。exit 1・verdict=fail、`deps.cycle` の new 1 件 |
| probe_plain | pass | 本物の検査器に probe b | 同じ file に comment を 1 行足した。exit 0・verdict=pass、`deps.*` の new・worsened 0 件 |

command（gate.tsv の command 列。exit はどれも 0、probe は期待どおりの exit）:

- case1〜case6: `cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)'` → 6 tests run: 6 passed, 1912 skipped
- prompt_boundary: `cargo nextest run -p task-worker -E 'test(/prompt_boundary_/)'` → 3 tests run: 3 passed, 1311 skipped
- repo_health_tests: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'` → Ran 126 tests、OK
- probe_scc_edge・probe_plain: `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$(git merge-base HEAD main)" --head worktree --policy-version 1 --out <report.json>`
  （dispatcher の health の関門 `health_review/gate.rs` の `run_report` と同じ引数の組で、head だけを `worktree` にした）

本物の検査器の probe の出力の要点（gate.md §3）。どちらも task-api の 49 単位の SCC の単位 `crates/task-api/src/lifecycle` を使った。この単位から
同じ SCC の `knowledge` への組は、probe の前には無い:

- probe a（probe_scc_edge）: `crates/task-api/src/lifecycle.rs` に `use crate::knowledge::KnowledgeItem;` を 1 行足した。
  - exit 1、verdict=fail。summary は findings 696・fail_grade 1・new 1・worsened 0・existing 695。
  - `deps.cycle` は fail（findings 18 = existing 17 ＋ new 1）。new の finding の symbol は `crates/task-api/src/lifecycle -> crates/task-api/src/knowledge`、
    evidence は `crates/task-api/src/lifecycle.rs:26`。fingerprint は gate.tsv の note にある。`deps.violation` は pass（0 件）。
  - 49 単位の SCC の finding 自身は existing のままで、単位の数は増えていない。単位の数が変わらない悪化を、B が足した組の finding（`cycle-edge:` の鍵）が止めた。
- probe b（probe_plain）: 同じ位置に `// probe: comment only (gate-cases)` を 1 行足した。
  - exit 0、verdict=pass。summary は findings 695・fail_grade 0・new 0・worsened 0・existing 695。
  - `deps.cycle` は pass（findings 17、すべて existing）、`deps.violation` は pass（0 件）。
- probe は検査器を走らせた直後に `git checkout -- crates/task-api/src/lifecycle.rs` で戻し、commit していない。戻した後の
  `git status --porcelain -- crates scripts web` は 2 回とも空だった。

本番の判定経路で設計 review が始まった証拠（[review-prod.md](review-prod.md) §0・§5。本番 DB は `mode=ro` で読んだだけ）:

- C を含む release `f85df0209504`（件名「integrate wu/loc-fix (phase fix)」）は 2026-11-18T12:18:40Z に本番の active になった。
  その後の窓 [2026-11-18T12:18:40Z, 2026-11-19T12:00Z)（0.987 日）に、設計 review の要求は 7 件・3 task（D・C の申し送り・F）で、level は 7 件とも high。
  設計 review の run は 7 件・費用の合計 $14.33（合格 4・差し戻し 3）。
- 理由の形: A の窓の 27 件の理由 75 個は、すべて旧い判定の `path_class:<class>+new_*` の形だった。昇格の後の 7 件は C の形
  （`path_class:<class>:<path>` と `invariant:<class>:<不変条件>`）で、7 件中 6 件は `declared:new_*` が無い。review-gate・transaction は C の profile で足した class。
- 「`new_*` の宣言が無い 6 件は旧い判定では High にならなかった」は推定で、旧い判定で再判定はしていない（review-prod.md §5）。

昇格の後の 7 件（review-prod.md §5 の表の写し）:

| event | task | `declared:new_*` | 結果 | cost | wall |
|---|---|---|---|---|---|
| 960582 | D | 無し | 差し戻し | $4.556 | 312.6 s |
| 960771 | D | 無し | 差し戻し | $2.301 | 298.4 s |
| 961175 | C の申し送り | 無し | 合格 | $0.117 | 14.0 s |
| 961342 | D | 無し | 合格 | $2.506 | 354.6 s |
| 962093 | C の申し送り | 無し | 合格 | $0.124 | 12.9 s |
| 964461 | F | new_branch | 差し戻し | $1.905 | 300.0 s |
| 964589 | F | 無し | 合格 | $2.823 | 514.4 s |

理由の例（3 件。`risk.reasons` の写し）:

- 960582（D、2026-11-18T12:38:58Z）: `path_class:store:crates/task-core/src/store/org.rs` ほか store の path 3 個、`invariant:store:task-state-in-sqlite`、`invariant:store:no-data-loss`
- 961175（C の申し送り 01M7Q70NDQAK1KKXHY6WT027QZ、2026-11-18T13:02:46Z）: `path_class:review-gate:crates/task-core/src/design_review.rs`、
  `invariant:review-gate:completion-not-self-reported`、`invariant:review-gate:policy-change-needs-human`、`invariant:review-gate:no-llm-in-dispatcher`
- 964589（F、2026-11-18T23:16:12Z）: `path_class:transaction:crates/task-api/src/knowledge.rs`、`path_class:transaction:crates/task-ops/src/lifecycle.rs`、
  `invariant:transaction:task-state-in-sqlite`、`invariant:transaction:lifecycle-human-epoch-same-tx`

注意（未達ではない）: `prompt_boundary_` の試験が 3,000 文字の objective の末尾まで確かめるのは worker の prompt だけで、planner・design review・final review の
prompt の objective の末尾は確かめていない（gate.md §2・§8）。実 prompt に載ったかは次の節の記録で確かめた。

## 実 prompt

判定: **worker と reviewer は、prompt が残る run のすべてで objective の末尾と受入条件が入っていた（cut 0 件）**。未確認は prompt の無い worker の 4 run。
planner は objective の末尾が 24/24 run で入ったが、task の受入条件は 0/126 件で 1 件も入っていない（planner の prompt に受入条件の節が無い）。
planner の欠けを直すかは人が決める（[prompt-audit.md](prompt-audit.md) の提案 1）。

- 出所は [prompt-audit.md](prompt-audit.md)・[prompt-audit.tsv](prompt-audit.tsv)（201 行と header）。本番 DB は `mode=ro` の URI で開いて読んだだけ（2026-11-19T11:58Z）。
- 範囲: 親 01M7MZT8KX547464YYS76QNP4G の子孫の 15 task（A〜H の 8 件と子 task の 7 件）の、role が worker・planner・reviewer の run 201 件
  （worker 157・planner 24・reviewer 20）。prompt は `<task の workspace>/runs/<run id>/prompt.txt`。
- 判定の規則: 両側の空白の連なりを 1 つにしてから比べる。`objective_tail` は objective の末尾 200 字が prompt に含まれれば found・含まれなければ cut・
  prompt.txt が無ければ missing。受入条件は text ごとに含まれるかを数える。reviewer は review する task、WorkUnit の run は run の開始時に有効だった
  計画の版（`execution_plans` の `units[]`）の objective と done_when と比べた。

kind ごとの集計（prompt-audit.md の表の写し）:

| kind | run | found | cut | missing | 受入条件（prompt のある run） | 受入条件の欠け | prompt の無い run の受入条件 |
|---|---:|---:|---:|---:|---:|---:|---:|
| worker | 157 | 153 | 0 | 4 | 651/651 | 0 | 18（未確認） |
| reviewer | 20 | 20 | 0 | 0 | 112/112 | 0 | 0 |
| planner | 24 | 24 | 0 | 0 | 0/126 | 126 | 0 |
| 計 | 201 | 197 | 0 | 4 | 763/889 | 126 | 18 |

- objective が 1,500 字を超えて found の run は worker 116・reviewer 19・planner 23。最長は A の 7,247 字で、A の worker 3 run と reviewer 2 run の全部で
  末尾が入っていた。以前の「親 task の objective が先頭 1,500 字で切れる」不具合は、この木の run では再現していない。
- 参考（tsv に無い数）: prompt のある WorkUnit の run 150 件で、「このタスク全体の目的（全文）」の節の親 task の objective（1,168〜7,247 字）の末尾 200 字は
  150/150 で見つかった。「最終レビューで確かめる Task の受け入れ条件（参考）」の節の task の受入条件は 849/849 件で見つかった。

切れた例: この audit の規則では cut は 0 件。WorkUnit の objective を今の `work_units` 行と比べた場合だけ、次の 7 run が cut に見える。
7 run とも prompt には計画 v1 の unit の objective と done_when が全部入っていた。計画の改訂が同じ id の WorkUnit の行を続きの run 用の文に書き換えたため、
今の行は run が受け取った文と違う。prompt が切れたのではない。

| task | WorkUnit | run | 計画の版（開始時 → 今） | 今の行と比べた done_when の欠け |
|---|---|---:|---|---|
| B（01M7MZTKAZQY72EX2R4RQ1WVWF） | checker（v1 の objective 1,632 字） | 4 | v1 → v2 | 各 4 件（計 16） |
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | risk-rule（v1 の objective 1,256 字） | 3 | v1 → v3 | 各 3 件（計 9） |

開始時の版と比べると欠けは 0 件。過去の run の監査は `work_units` 行でなく、run の開始時の `execution_plans` の `units[]` を引く必要がある
（prompt-audit.md の提案 2。run ごとの spec を残すかは人が決める）。

prompt の無い run（missing 4 件。どれも `runs/<run id>/` の dir が無く、request.json・stdout.jsonl も無い）:

| task | run | WorkUnit | 開始（UTC） | 終了までの秒 | 受入条件（done_when）の数 | 記録された終わり方 |
|---|---|---|---|---:|---:|---|
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | 01M7PBH4HWDFMXSEMC5AKVKJVK | risk-rule | 2026-11-18T03:34:41 | 2.7 | 5 | `interrupted: aborted (task no longer running under this lease)`、usage null |
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | 01M7PBPTFQQG9N8WDMRW8M5435 | risk-rule | 2026-11-18T03:37:48 | 2.7 | 5 | 同上 |
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | 01M7PBWQKNTNHKM4E3S00DJ1AV | risk-rule | 2026-11-18T03:41:01 | 2.7 | 5 | 同上 |
| D（01M7MZTKB4RYM80S6CT3RST2N9） | 01M7QC2E69H17AXEYD9AS8TK10 | adr-forms | 2026-11-18T13:03:22 | 2.5 | 3 | 同上 |

原因の候補は、worker が prompt を書く前に lease を失い、dispatcher が run を中止したこと（4 run とも計画の改訂の間に始まった）。LLM は呼ばれていない
（usage null）ので、作業に使われた prompt は無い。これらの run に何が渡るはずだったかは未確認。

planner の受入条件（0/126）:

- planner の 24 run で、task の受入条件の text は 1 件も prompt に無く、prompt の見出しにも受入条件の節が無い（B の planner run 01M7ND92G1Q4CVS3YEYKQ9B6SF で確かめた）。
- 原因: `crates/task-worker/src/claude_code/prompt.rs` の `build_plan_prompt`（:745）は `prompt_header`（objective の全文を含む）に計画の指示を足すだけで、
  `task.acceptance` を出さない。`task.acceptance` を出すのは worker 用の `build_execute_prompt`（:649〜:675）と review の prompt（:1685）だけ。
- 影響: planner は task の受入条件の text を見ずに WorkUnit の done_when を書く。受入条件の text にだけある条件は planner の prompt に入らない。
- H の task の受入条件 1 は worker・reviewer の prompt を対象にしているので、planner の欠けはそれとは別の観察として書く。H では直していない。
  今の planner の試験（`crates/task-worker/src/claude_code/tests.rs` の `planner_prompt_*`）は受入条件の有無を見ていないので、この欠けを検出できない。

## 安全不変条件

判定: **11 行すべて pass（fail 0・untested 0）**。passed は延べ 563 件（`cex_lease_restart` 12 件と `cex_same_tick` 3 件は lease_atomicity と
arch_v4_counterexamples の両方に数えた）。D・E・F の反例試験（arch_v4_counterexamples）は 81 passed で、3 つの command がどれも exit 0。

- 出所は [safety.md](safety.md)・[safety.tsv](safety.tsv)（列 `invariant result command passed note`）。試験は `docs/architecture-map.md` の試験の列と、
  `CLAUDE.md`・review 方針（`scripts/dev/repo-health/profiles/celeris.toml` の `[review]` 節）が挙げる根拠の file から選び、`cargo nextest list` で実在を
  確かめてから crate と filter を絞って走らせた。workspace 全体・doc test・e2e は走らせていない。各 filter の `--run-ignored only` の件数はどれも 0。
- 注意（実質は未検査の 9 件）: `CELERIS_USERNS_TESTS` などの env を付けていないので、sqlite_source の task-worker の `db_guard` 7 件と、credential の
  celeris-credentiald の `injection_peer_uid` 2 件は、userns の検査の前に return して pass に数えられた。release gate の gate.json は `userns: true` だが、
  この 9 件が release gate で検査まで進んだかは、gate log が試験を 1 件ずつ出さないので未確認（safety.md の「気づいた点」）。
- nextest の summary の `skipped` は filter の外の試験の数で、実行時の skip ではない。

safety.tsv の行（invariant・result・passed は tsv のまま）:

| invariant | result | passed | 守る条件（safety.md の要約） | 内訳（safety.tsv の note） |
|---|---|---:|---|---|
| sqlite_source | pass | 169 | task の状態の真実は SQLite。状態は `transition` が許す遷移でだけ変わる。worker の run は DB を読むだけ | task-core 140（store::tests 111・legacy::tests 7・client_open_tests 6・migration_harness_tests 2・transition::tests 14）＋ task-worker 29（db_guard::tests。うち 7 件は userns の env 無しで早期 return） |
| events_append_only | pass | 13 | events は追記専用。遷移と event は同じ transaction で書く | task-core 13（append_event_*・apply_transition_*appends* 4 件と create_task_inserts_task_and_events_atomically、test(append_only) の 8 件） |
| lease_atomicity | pass | 51 | lease は 1 つの持ち主だけが持つ。取得と `Transitioned` event は同じ transaction。回収は compare-and-release | task-core 12（lease_reclaim_tests 5・store::tests の lease 7）＋ task-dispatch 39（cex_lease_restart 12・cex_same_tick 3・ownerless_runs 11・orphan_takeover 6・run_supervisor::tests 7） |
| replay | pass | 14 | events を replay した結果は現在の投影と一致する | task-ops 14（replay::tests 全件） |
| project_controller_stop | pass | 14 | 案件の off・paused・cancelled・pause・cancel のとき controller の効果は実行されず suppressed になる | task-dispatch 14（project_wake_pending_* 4・project_wake_human_races_* 3・project_wake_off_is_inert 1・project_recovery_k37_* 3・project_controller_replay_* 3） |
| rust_ts_canary | pass | 16 | canary の対象は単位ごとに決定的に選ぶ。拒否・失敗・予算超過は Rust に倒れる | task-dispatch 16（script_canary 7・cos_digest_script::canary 3・cos_digest_behavior_change の canary 4・ほか 2） |
| acl | pass | 28 | API の主体は認証済みの Bearer token から決め、自己申告から決めない | task-api 28（auth_and_guards 11・principal::tests 3・middleware::tests 3・resolve_principal 1・api_principals 10） |
| credential | pass | 78 | credential の値は broker の外に出さず、相手 process の uid を確かめて渡す | celeris-credentiald 54（broker 17・injection_ipc 19・injection_peer_uid 4・prod_admission 14。うち 2 件は env 無しで早期 return）＋ task-worker 5 ＋ task-api 19 |
| cos_audit | pass | 16 | CoS の変更操作は監査付きの操作層だけを通り、監査と業務の書き込みは同じ transaction に入る | task-api 16（cos_operations 8・cos::operations::skill_table_tests 1・cos_delivery_suite_a の cos_auth 7） |
| selfdeploy_authority | pass | 83 | 昇格していない release の daemon は本番を引き継がない（exit 4）。昇格は verify 済みの release だけ | celeris 80（instance::tests 29・releases::tests 21・releases::audit::tests 2・unpromoted_release 10・instance_handoff 8・releases_api 10）＋ task-api 3 |
| arch_v4_counterexamples | pass | 81 | D・E・F の反例が再発しない（org の書き込みは plan と apply の 1 本、HTTP と CoS の書き込みは 1 つの transaction 等） | org_write_contract 6 ＋ tx_boundary_contract 8 ＋ task-dispatch 67（cex_ 40・structure_guard 14・write_set_ 13） |

command（safety.tsv の command 列の写し。repo の root で。exit はどれも 0）:

```sh
# sqlite_source（169）
cargo nextest run -p task-core -E 'test(/^store::(tests|migration_harness_tests|client_open_tests|legacy::tests)::/) | test(/^transition::tests::/)' && cargo nextest run -p task-worker -E 'test(/^db_guard::tests::/)'
# events_append_only（13）
cargo nextest run -p task-core -E 'test(/^store::tests::append_event_/) | test(/^store::tests::apply_transition_.*appends/) | test(/^store::tests::create_task_inserts_task_and_events_atomically$/) | test(append_only)'
# lease_atomicity（51）
cargo nextest run -p task-core -E 'test(/^store::lease_reclaim_tests::/) | test(/^store::tests::(acquire_lease_|acquire_work_unit_lease_|renew_lease_|apply_transition_releases_lease_)/)' && cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::(cex_lease_restart|cex_same_tick|ownerless_runs|orphan_takeover)::/) | test(/^run_supervisor::tests::/)'
# replay（14）
cargo nextest run -p task-ops -E 'test(/^replay::tests::/)'
# project_controller_stop（14）
cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::project_wake::effects::project_wake_pending_/) | test(/^dispatcher::tests::project_wake::project_wake_(human_races_|off_is_inert$)/) | test(/project_recovery_k37_/) | test(/^dispatcher::tests::project_wake::replay::project_controller_replay_(paused_project|cancelled_project|invalid_proposals_stop)$/)'
# rust_ts_canary（16）
cargo nextest run -p task-dispatch -E 'test(canary)'
# acl（28）
cargo nextest run -p task-api -E 'binary(auth_and_guards) | (kind(lib) & test(/^(principal|middleware)::tests::|^handlers::tests::resolve_principal_/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::/))'
# credential（78）
cargo nextest run -p celeris-credentiald -E 'binary(broker) | binary(injection_peer_uid) | binary(prod_admission) | binary(injection_ipc)' && cargo nextest run -p task-worker -E 'test(browser_credential)' && cargo nextest run -p task-api -E '(kind(lib) & test(/^secrets::tests::/)) | (binary(run_worker_suite) & test(/^secrets_admin::/))'
# cos_audit（16）
cargo nextest run -p task-api -E 'binary(cos_operations) | (kind(lib) & test(/^cos::operations::/)) | (binary(cos_delivery_suite_a) & test(/^cos_auth::/))'
# selfdeploy_authority（83）
cargo nextest run -p celeris -E '(kind(lib) & test(/^(instance|releases)::/)) | binary(unpromoted_release) | binary(instance_handoff) | (binary(cos_delivery_suite) & test(/^releases_api::/))' && cargo nextest run -p task-api -E '(kind(lib) & test(/^releases::tests::/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::promotion_audit_/))'
# arch_v4_counterexamples（81）
cargo nextest run -p task-api --test cos_delivery_suite org_write_contract && cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract && cargo nextest run -p task-dispatch -E 'test(store_access_guard) | test(structure_guard) | test(cex_) | test(write_set_)'
```

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

A の 14 項目と共通受入条件 (1)〜(6)、verify 段の md から、減らなかった・増えた・比べられない・確かめられなかった項目を拾った。
移動・分割・抽象の追加だけの変更と分母の増加は成果に数えていない（数値の欄の括弧にそう書いた）。次の候補の番号は「提案」の節の番号。
未達のまま閉じるか、次の task を求めるかは人が決める（close 段の決定 unmet）。

| 項目 | 数値 | 理由 | 次の候補 |
|---|---|---|---|
| (1) #3〜#5 module の循環と SCC の単位・組 | module の循環 17 → 17。task-core の SCC 55 単位・210 組、task-api の SCC 49 単位・261 組（before と after で同じ、組の集合も同じ）。行の辺は task-api −10（b0 628 → 618。移動の分を含む）、task-core +4（617 → 621） | D の org の pilot は行の辺を減らしたが組を消していない。task-core の +4 は F の `chat/attachments.rs → store`（[struct-deps.md](struct-deps.md) §0） | 提案 1・2（D の次の候補。`cos/ops/admin.rs` の accounts の 9 行と `cos/operations.rs` の HTTP の道具）。組を消すには残る辺のある領域をまとめて移す（D の提案） |
| (1) #6〜#8 health の既存負債 | `size.responsibility-growth` 94 → 94、`dup.contract` 210 → 210、owner 不一致 31 → 31（finding の scope の集合も同じ） | B〜G はこの 3 rule の finding を減らす変更をしていない（struct-deps.md §0） | 減らす候補は材料の md に無い。増えないことを回帰条件で見る。減らす task を起こすかは人が決める |
| (1) #14 `#[path]` と地図の抜け | 物理と論理が食い違う宣言は task-dispatch 9 → 9、workspace 15 → 15。`docs/architecture-map.md:67` は 3 file（check_item・review_limits・inventory）を書いていない | B〜G は `#[path]` の宣言を変えず、B0..after の地図の差分に 3 file の名の行が無い（struct-deps.md §3） | 提案 11（地図の `#[path]` の説明に 3 file を足す。文書だけ） |
| (2) `rusqlite`・SQL 文字列の production file | `rusqlite` 74 → 75（+1 は抽象の追加）、SQL 文字列 50 → 50（file の集合も同じ） | +1 は D の共有 use-case `org_write.rs`。F は file の中の自前の接続を消したが、A の正規表現は「接続を開く行」と「呼び手の `&Transaction` を受ける行」を区別しない（[struct-store.md](struct-store.md) §7） | 提案 4・5（F の提案 1・2）と提案 12（C4 に「接続を開く行」の file 数を補助の指標として並べる） |
| (2) #9 dispatcher の参照 | self.store を書く file 44 → 38、self.config 35 → 31（依頼 scope・C3。移動なので成果に数えない）。出現数 633 → 633・218 → 220、分母 69 → 69 | E が対象 file の参照を入口の file へ移した結果で、参照の総量は減っていない（struct-store.tsv の note） | 提案 3（E の次の候補。`review_verdict.rs`・`tree_units.rs`・cos_chat の 3 file。`work_units.rs` は先に設計） |
| (3) handlers ⇄ cos の単位の循環 | handlers → cos 22 → 19 行、cos → handlers 48 → 46 行（`crate::`）と `super::` 4 → 4 行。task-api の SCC の組 261 は同じ | 減った行は D が org の処理を `org_write` に集めた変更による。残る辺のある領域（accounts・HTTP の道具ほか）は移していない | 提案 1・2 と同じ。比較には `super::` を辺にする head の検査器の値を使う（struct-deps.md の提案 2） |
| (3) task-api の `&Connection`・`&Transaction` の引数 | 14 → 15（+1 は抽象の追加） | +1 は `org_write.rs:77` の `tx: &Transaction`。`org_write::apply` が CoS の `apply_checked` と同じ transaction に入るために要る引数で、接続を開く file は増えていない（D の未達・要判断、struct-store.md §7） | 減らす候補は無い（意図した増加として記録する）。接続を開く行の数は提案 12 の補助の指標で見る |
| (3) #10・#12 と #2 SqliteStore の impl・逆依存・Cargo の辺 | impl block 102 → 102、task-core に依存する crate 11 → 11、Cargo の依存の辺 49 → 49 | B〜G は store の impl の分け方と crate の構成を変えていない | 材料に候補は無い。G は task-core の crate 分離の利益を S1・S2 とも 0 % と見積もり、推さなかった（[G の進捗](../2026-11-19-arch-v4-g.md)） |
| (4) 変更波及（S1・S2） | S1 で変える file・単位 3 → 4（抽象の追加）、crate 2 → 2。S2 は 4 file・2 単位・1 crate のまま。import する単位は b0 で S1 92 / 369・S2 6 / 14 のまま。test-affected の対象も同じ集合 | D が plan と apply を別 file（`org_write.rs`）に分けたので、S1 の新しい操作は 1 file 多く触る。波及を減らす変更は B〜G に無い（[loc-cost.md](loc-cost.md) §0） | 材料に候補は無い（G の cause は crate 分離を推さなかった）。提案 1・2 で handlers ⇄ cos の組が減ったときに、同じ手順で数え直す |
| (4) #13 修正の波及 | A の窓 194 件・複数 region 46.9 %・複数 module 69.1 %。B0 の後の窓は no-merges 1 件（0.0 %・0.0 %）、with-merges 9 件（55.6 %・55.6 %）。比較不能 | B0 の後の窓は件数が少なく、9 件はアーキ v4 の commit と WorkUnit の統合の merge で、通常の修正 1 件と粒度が違う（loc-cost.md §3） | 提案 13（同じ snippet を、通常の修正が数十件たまった窓、例えば B0 の後の 2 週間に当てる） |
| (5) health:S2 の悪化 | 96.09 s（A 81.92 s の +17.3 %、上限 90.11 s を超える）。G が同じ host で測り直した before は 87.26 s、コード由来は `deps.violation` の CPU +2.02 s（+2.3 %） | 規則の上では code だが、人が決定 h1 で「直さない（keep）」と決めた。残りは測った時間帯の host の差（[G の進捗](../2026-11-19-arch-v4-g.md)） | 回帰条件を超えたら `lang_deps.py` の `mod` 宣言の走査を 1 回にまとめる（policy の別 review。G の次の候補） |
| (5) review の時間と費用（C11） | A の窓 7.625 日・165 run・中央値 1137.5 s・$2.047。after の窓 1.875 日・30 run・514.4 s・$2.167。promo の窓 0.987 日・17 run。比較不能 | 窓の長さが違い、仕事の量も違う（[review-prod.md](review-prod.md) §0） | 提案 14（2026-11-25T06:00Z の後に review-prod.md §7.1 の手順で A と同じ長さの窓を比べる。誰が比べるかは決定 unmet） |
| (5) 昇格後の設計 review と C の見積もり | promo の窓で要求 7 件・3 task・$14.33。見方 1 は +3.551 回/日・+$8.65/日（見積もり +0.670〜+4.498 回/日・+$1.37〜+$9.21/日の範囲の中）、見方 2 は 6.08 回/日・$12.59/日（上限を超える） | 窓が 1 日で、task がアーキ v4 の 3 件に偏る。見積もりは commit の単位、実績は計画の単位の要求で単位が違う（review-prod.md §6） | 提案 14 と同じ手順。見積もりを見直すかは人が決める（決定 unmet） |
| (6) LoC | production +822 行（375099 → 375921、すべて Rust）、試験 +4743 行、own_total_with_tests +5829 行 | B〜F が足した code。アーキ v4 の依頼は一律の純減を求めていない（loc-cost.md §0） | 純減は求めない。production が 375921 行を超えて増えたら区分と file を調べる（回帰条件） |
| 実 prompt: planner の受入条件 | planner の 24 run で task の受入条件 0/126 件（objective の末尾は 24/24） | `crates/task-worker/src/claude_code/prompt.rs` の `build_plan_prompt` が `task.acceptance` を出さない。今の `planner_prompt_*` の試験は受入条件の有無を見ない（[prompt-audit.md](prompt-audit.md)） | 提案 6（planner の prompt に受入条件の節を足すか決める） |
| 実 prompt: prompt の無い worker の run | 4 run（C の risk-rule 3・D の adr-forms 1）。開始から 2.5〜2.7 秒で lease 喪失により中止、usage null。何が渡るはずだったかは未確認 | run の dir が作られる前に中止された。LLM は呼ばれていない（prompt-audit.md） | 材料に直す候補は無い。run の開始時の spec を残すかは提案 7 と合わせて人が決める |
| 実 prompt: 過去の WorkUnit の run の監査 | 今の `work_units` 行と比べると 7 run が cut に見える（B の checker 4 run・C の risk-rule 3 run）。開始時の版と比べると欠け 0 | 計画の改訂が同じ id の WorkUnit の行を続きの run 用の文に書き換える（prompt-audit.md） | 提案 7（run の開始時の `execution_plans` の `units[]` で監査する。run ごとの spec を残すか決める） |
| 判定経路: prompt の試験の範囲 | `prompt_boundary_` が 3,000 文字の objective の末尾を確かめるのは worker の prompt だけ（未達ではない） | planner・design review・final review の prompt の試験は objective の末尾を見ていない（[gate.md](gate.md) §2・§8） | 提案 8（planner と reviewer の prompt に長い objective を入れて末尾を確かめる試験を足すか決める） |
| 安全不変条件: userns の 9 件 | sqlite_source の `db_guard` 7 件と credential の `injection_peer_uid` 2 件は、env が無いと検査の前に return して pass に数えられた（実質は未検査） | `CELERIS_USERNS_TESTS` などの env を付けていない。release gate（`userns: true`）で検査まで進んだかは、gate log が試験を 1 件ずつ出さないので未確認（[safety.md](safety.md)） | H の統合の後の release gate の log で確かめるか、人が決める。提案 9（gate.json に nextest の件数を入れる）は件数だけで、9 件の個別の結果は出さない |
| リリースゲート: B・C・D の最初の release | 3 release（B・C・D を最初に含んだもの）の gate・verify は未確認。昇格は promote ログで確かめた | release の dir と `.build` の dir が `release.sh` の掃除で消えた（safety.md の「release gate」節）。B・C・D は後の 4 release にも含まれ、その gate・verify は ok | 提案 10（gate・verify・昇格の要約を消えない場所へ 1 行ずつ残す） |
| リリースゲート: H | H を含む release は無い（gate・verify・昇格とも未実施） | H の commit は docs だけで、いまの current の release より後 | H の統合の後に人が `release.sh` を回す |

## 提案

どれも提案で、H は起票していない。起票するかは人が決める。出所の D・E・F は [D の進捗](../2026-11-18-arch-v4-d.md)・[E の進捗](../2026-11-18-arch-v4-e.md)・
[F の進捗](../2026-11-18-arch-v4-f.md) の「提案」の節。struct-deps の提案 3 と prompt-audit の提案 3 は回帰条件なので「回帰条件」の節に入れた。
F の提案 4（org-write の ADR の付記の表を今の関数に合わせる）は H の struct-store が済ませた（[struct-store.md](struct-store.md) の「4. ADR の付記の表の修正」）。

| # | 提案 | 出所 | 関係する項目 |
|---|---|---|---|
| 1 | `cos/ops/admin.rs` → `handlers::accounts` の 9 行（`crate::` 5 行と `super::` 4 行。`pool_adapter`・`create_account_op` ほか）を、org と同じ形（plan と transaction の中の apply を SCC の外の module に置く）へ移す | D の次の候補、struct-deps.md の提案 2 | (1) task-api の SCC の組、(3) handlers ⇄ cos |
| 2 | `cos/operations.rs` → handlers の HTTP の道具（`ApiResult`・`Params`・`json_response`・`no_query`・`read_json`・`cluster_ids`）を SCC に入らない module に置く | D の次の候補、struct-deps.md の提案 2 | 同上 |
| 3 | dispatcher の参照を同じ手で減らす次の file: `review_verdict.rs`（store 32・config 3。先に review の順序の設計が要る）、`tree_units.rs`（store 56・config 19）、cos_chat の `root_done.rs`・`triage.rs`・`fallback.rs`（13〜24 の参照）。`work_units.rs` は本体の行が閾値 2000 を超えるので、先に WorkUnit の遷移と commit の副作用を分ける設計が要る | E の次の候補 | (2) #9 dispatcher の参照 |
| 4 | `chat/attachments.rs` と `cos/inbox.rs`（`triage_db`、`cos/triage_view.rs` の `Connection::open`）の自前の接続を、`SqliteStore` の `write_tx`・読み取りの接続へ移す（`ChatAttachmentStore` の設計と合わせる） | F の提案 1、struct-store.md の提案 | (2) `rusqlite` を書く file |
| 5 | 4 が済んだ時点で、`knowledge.rs` の有無の旗の用途と合わせて `state.chat.attachment_db_path` を `ChatState` の欄ごと消す。有無の旗は添付の保存の設定（`attachment_data_dir`）から導く | F の提案 2 | 同じ DB の file を指す値が 2 つある状態をなくす |
| 6 | planner の prompt に task の受入条件の節を足すか決める。owner は `crates/task-worker/src/claude_code/prompt.rs` の `build_plan_prompt`。守る不変条件は「planner の prompt に task の `acceptance[].text` が全部入る」 | prompt-audit.md の提案 1 | 実 prompt: planner の受入条件 0/126 |
| 7 | 過去の WorkUnit の run の監査は、run の開始時に有効だった `execution_plans` の `units[]` で行う。run ごとの spec を残すか（例: request.json の work_unit の写しを正本にする）を決める | prompt-audit.md の提案 2 | 実 prompt: 7 run の見かけの cut、prompt の無い 4 run |
| 8 | planner と reviewer（design review・final review）の prompt に長い objective を入れ、末尾まで載ることを確かめる試験を足す | gate.md の提案 | 判定経路: prompt の試験の範囲 |
| 9 | `release.sh` の gate.json の `cargo_test` に nextest の passed・skipped を入れる（いまの `passed`・`ignored` は doc test の値と nextest の skipped の和に見える） | safety.md の提案 | リリースゲートの記録の読み取り |
| 10 | release の掃除で消える gate.json・verify.json・promoted.json の要約を、promote ログのように消えない場所へ 1 行ずつ残す | safety.md の提案 | リリースゲート: B・C・D の最初の release |
| 11 | `docs/architecture-map.md:67` の `#[path]` の説明に check_item・review_limits・inventory の 3 file を足す（文書だけ） | struct-deps.md の提案 1 | (1) #14 `#[path]` と地図の抜け |
| 12 | A の C4 の正規表現に、「接続を開く行」（`Connection::open`・`TransactionBehavior`・`busy_timeout`）を持つ file の数を補助の指標として並べる（次の計測の規則の変更。H は規則を変えていない） | struct-store.md の提案 | (2) `rusqlite` を書く file |
| 13 | 修正の波及（A §7 C10）の snippet を、通常の修正が数十件たまった窓（例: B0 の後の 2 週間）に当てる | loc-cost.md §3 の次の候補 | (4) #13 修正の波及 |
| 14 | 2026-11-25T06:00Z の後（その窓に始まった reviewer run が全部終わった後）に、[review-prod.md](review-prod.md) §7.1 の手順で A と同じ長さの窓の reviewer run と設計 review を比べ、C の見積もりと並べる。誰が比べるかは決定 unmet で人が決める | review-prod.md §7 | (5) review の時間と費用、昇格後の設計 review |
| 15 | 知識の `knowledge.rs` の `accept_op`・`reject_op`・`put_page_op` を org_write の形へ移す | 「Applied の形の残数と領域ごとの移行の提案」の節、F の提案 3 | `Applied` の形の残数 |
| 16 | task-api の `lifecycle.rs` の `task_pause_op` を org_write の形へ移す（知識と別の 1 件） | 同上、F の提案 3 | 同上 |
| 17 | `skills.rs` の `put_skill_op`・`delete_skill_op` を org_write の形へ移す | 同上 | 同上 |

## 回帰条件

後の変更で次の条件が崩れたら、原因を調べる。数値が変わったら、各 md の再現コマンドを新しい `git merge-base HEAD main` と HEAD に当て直す。

`Applied` の形（D の ADR の付記、[struct-store.md](struct-store.md) §3）:

- 広い command `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src` の file の数（applied_wide）が **18 以下**であること（いまは 18）。
  新しい操作をこの形で足さない。狭い command `git grep -l 'Option<&OperationAudit>' crates/task-api/src` は いま 14。
- その領域の共有の操作を変える task は、同じ変更で org_write の形へ移す（ADR の付記の撤去の条件）。

G の回帰条件（health・build・test-affected の上限。[G の進捗](../2026-11-19-arch-v4-g.md)「回帰条件」、上限は [G の compare.md](../2026-11-19-arch-v4-g/compare.md) §1 の「A の +10 %」の列）:

- build・test-affected: A の baseline.md §7 の手順（G の [rules.md](../2026-11-19-arch-v4-g/rules.md)）で測った中央値が下の上限を超えたら、before（B0）を同じ host で
  3 回測り直して原因を絞る（rules.md の ## BEFORE）。
- health: 同時に並べた before と after の health の wall の差が before の +10 % を超えたら、rule ごとの計時（G の cause の手順、cause.md §5）で rule を絞る。
  `deps.violation` が原因なら cause.md §3 の修正案を policy の別 review で入れる。health:S2 はいま A の上限を超えている（96.09 s。人の決定 h1 で keep）ので、
  health は A の上限でなく、この同時に並べた差で見る。
- 変更波及: S1・S2 の file を選び直したとき、変える crate・再 build される crate・`test-affected.sh --list` の対象が (4) の表より増えたら原因を調べる
  （import する単位は b0 で S1 92 / 369・S2 6 / 14、head で S1 93 / 365・S2 6 / 14。[loc-cost.md](loc-cost.md) §7）。
- HEAD の crates/・scripts/・web/・Cargo.lock が G の測定の tree と変わったら、(5) の after は G の値として写せない。health の S1・S2 を G の rules.md の手順で
  3 回ずつ測り直す（loc-cost.md §7）。

| metric | A の中央値（s） | after の中央値（s） | 上限 = A の +10 %（s） |
|---|---|---|---|
| `check_cold` | 53.65 | 53.90 | 59.02 |
| `check_warm_s1` | 42.47 | 42.64 | 46.72 |
| `check_warm_s2` | 14.67 | 15.56 | 16.14 |
| `test_build_cold` | 142.39 | 147.03 | 156.63 |
| `test_build_warm_s1` | 138.69 | 138.10 | 152.56 |
| `test_build_warm_s2` | 77.64 | 81.60 | 85.40 |
| `health_s1` | 86.56 | 93.58 | 95.22 |
| `health_s2` | 81.92 | 96.09 | 90.11（いま超えている。上の health の条件で見る） |
| `test_affected_s1` | 893.41 | 906.03 | 982.75 |
| `test_affected_s2` | 655.59 | 707.76 | 721.15 |

構造（(1)〜(4)。head の検査器で数える）:

- module の循環 17、task-core の SCC の組 210、task-api の SCC の組 261 を超えないこと（struct-deps.md の提案 3）。repo-health の `deps.cycle` は SCC の中の新しい組を
  fail にする（判定経路の probe a）。
- handlers → cos の 19 行と、A の C5 の式の cos → handlers の 50 行を超えないこと。`git grep -nE 'handlers::org|handlers::load_org_node|org_commit' -- crates/task-api/src` が
  空のこと（D の回帰条件）。
- health の既存負債（`size.responsibility-growth` 94・`dup.contract` 210・owner 不一致 31）が増えないこと。
- F の境界: `git grep -nE 'Connection::open|TransactionBehavior|busy_timeout' -- crates/task-api/src/knowledge.rs` と
  `git grep -nE 'Fn(Once|Mut)?\(&(rusqlite::)?(Connection|Transaction)' -- crates/task-ops/src/lifecycle.rs` が 0 件のこと（1 件でも出たら F の境界が戻った印。F の回帰条件）。
- LoC: 同じ規則（B0 の source-size-report）で production が 375921 行を超えて増えたら、増えた区分と file を調べる（loc-cost.md §7）。

判定経路・実 prompt・安全不変条件:

- `boundary_case_` の 6 件、`prompt_boundary_` の 3 件、`test_repo_health_*.py` の 126 件が通り続けること。本物の検査器に gate.md §3.1 の probe を当てると
  `deps.cycle` の new の組の finding が出て exit 1、§3.2 の probe（comment 1 行）は exit 0 のままであること（main で `lifecycle -> knowledge` の組が SCC の中にできたら、
  §3 の手順で、まだ無い組を選び直す。[gate.md](gate.md) §6）。
- prompt-audit を H と同じ規則で再実行し、worker・reviewer の `objective_tail` の cut が 0 件、prompt のある run の受入条件の欠けが 0 件であること（prompt-audit.md の提案 3）。
- safety.tsv の 11 の command が pass のままであること（D・E・F の反例試験 `org_write_contract`・`tx_boundary_contract`・`cex_`・`structure_guard`・`write_set_` を含む）。
  F の `tx_boundary_contract` の 8 件ほかが落ちたら、落ちた経路の commit を git の revert で戻す（F の回帰条件）。
- review の query: A の窓の再現が 165 run・wall の中央値 1137.5 s・cost の中央値 $2.047・合計 $280.17 に一致すること（query の条件が A と同じことの確認。review-prod.md）。

## リリースゲート

判定: **H の変更を含む release gate は未実施**。H の measure・verify・report の commit（docs だけ）は、いまの current の release `1680de5cd1b4`
（G の最後の commit「integrate wu/close (phase close)」）より後で、H を含む release はまだ作られていない。release gate は H の統合の後に `release.sh` が走らせる。
B〜G の変更は、いまの current の release にすべて含まれ、その release は gate ok・verify ok で 2026-11-19T11:30:18Z に昇格した。

- 出所は [safety.md](safety.md) の「release gate」節（unit record）。H は release の記録を読んだだけで、`release.sh`・`verify.sh`・`promote.sh`・`status.sh` は
  走らせていない。本番の file・DB・systemd には触れていない（[review-prod.md](review-prod.md) の「再現と検査」も、統合後の release の gate は未実施と書く）。
- 読んだ記録: `~/.local/celeris/releases/<sha12>/` の manifest.json・gate.json・verify.json・promoted.json・promote_failed.json、`releases/.build/<sha12>/gate.json`、
  current と previous の symlink、`/local/celeris/state/logs/promote-*.log`（2026-11-19 に読んだ）。promote_failed.json と rollback のログは無かった。

B〜G ごとの結果（safety.md の表の写し）:

| task | 最初に含んだ release | その release の gate | verify | 昇格 | いまの current（1680de5cd1b4） |
|---|---|---|---|---|---|
| B | 48b700ede57f（「integrate wu/closeout (phase closeout)」2026-11-17） | 未確認 | 未確認 | した（2026-11-17T22:28:42Z） | 含む。gate ok・verify ok・昇格済み |
| C | f85df0209504（「integrate wu/loc-fix (phase fix)」2026-11-18） | 未確認 | 未確認 | した（2026-11-18T12:18:41Z） | 含む。同上 |
| D | eb1244bb5e8d（D の branch への merge、2026-11-18） | 未確認 | 未確認 | した（2026-11-18T21:10:04Z） | 含む。同上 |
| E | 1c69c955f0fb（E の branch への merge、2026-11-18） | ok | ok | した（2026-11-18T23:25:35Z） | 含む。同上 |
| F | 7014d24222c4（F の branch への merge、2026-11-19） | ok | ok | した（2026-11-19T05:35:55Z） | 含む。同上 |
| G | 1680de5cd1b4（「integrate wu/close (phase close)」2026-11-19） | ok | ok | した（2026-11-19T11:30:18Z） | この release 自身 |
| H | 無し | 未実施 | 未実施 | 未実施 | 含まない |

昇格の連鎖（2026-11-17〜11-19。safety.md の表の写し。「含む」は `git merge-base --is-ancestor <B〜G の最後の commit> <release の commit>` が 0）:

| release | release の commit（件名・日時 UTC） | 昇格（UTC、from） | 含む task | gate | verify |
|---|---|---|---|---|---|
| 48b700ede57f | 「integrate wu/closeout (phase closeout)」2026-11-17T21:45:07Z | 2026-11-17T22:28:42Z、from f549b50b7ccc | B | 未確認 | 未確認（promote ログに `verify.live_ok=true`） |
| b2fe0a5e920f | 「repair: PathClass.invariants の進捗に test-affected の結果を書く」2026-11-18T02:25:45Z | 2026-11-18T03:16:41Z | B | 未確認 | 未確認（同） |
| f85df0209504 | 「integrate wu/loc-fix (phase fix)」2026-11-18T10:12:48Z | 2026-11-18T12:18:41Z | B・C | 未確認 | 未確認（同） |
| ed43a4abb7bf | 「integrate wu/fix-test (phase fix)」2026-11-18T13:42:24Z | 2026-11-18T15:25:24Z | B・C | 未確認 | 未確認（同） |
| 3033e0228853 | 「Merge commit '…' into celeris/01M7Q70NDQAK1KKXHY6WT027QZ」2026-11-18T16:18:10Z | 2026-11-18T17:17:11Z | B・C | 未確認 | 未確認（同） |
| eb1244bb5e8d | 「Merge commit '…' into celeris/01M7MZTKB4RYM80S6CT3RST2N9」2026-11-18T20:08:59Z | 2026-11-18T21:10:04Z | B・C・D | 未確認 | 未確認（同） |
| 1c69c955f0fb | 「Merge commit '…' into celeris/01M7MZTKB6GQ241F6JJYSPJNS1」2026-11-18T22:31:07Z | 2026-11-18T23:25:35Z、from eb1244bb5e8d | B〜E | 記録 ok（nextest 8515 passed） | 記録 ok |
| bb772ffca8ec | 「進捗: rejected-worker-decisions」2026-11-19T00:08:00Z | 2026-11-19T01:25:29Z、from 1c69c955f0fb | B〜E | 記録 ok（nextest 8517 passed） | 記録 ok |
| 7014d24222c4 | 「Merge commit '…' into celeris/01M7MZTKB8A4NJQEEA35Q4G5JV」2026-11-19T04:30:48Z | 2026-11-19T05:35:55Z、from bb772ffca8ec | B〜F | 記録 ok（nextest 8525 passed） | 記録 ok |
| 1680de5cd1b4 | 「integrate wu/close (phase close)」2026-11-19T10:37:13Z | 2026-11-19T11:30:18Z、from 7014d24222c4。いまの current | B〜G | 記録 ok（nextest 8525 passed） | 記録 ok |

残っている 4 release（1c69c955f0fb・bb772ffca8ec・7014d24222c4・1680de5cd1b4）の記録の中身（4 つとも同じ形）:

- manifest.json: `gate_ok: true`、`schema_version: 126`、web の 7 段が ok、browser_ledger ok。
- gate.json: `ok: true`・`failed_step` 空。17 段（cargo-workspace-clean・cargo-fmt-check・control-ts-install・control-ts-bundle・script-bundle-verify・cargo-test・
  script-bundle-tests・cargo-clippy・source-size-report・cargo-build・web の 7 段）がどれも exit 0。cargo_test は `nextest_exit 0`・`doctest_exit 0`・`failed 0`・`userns: true`。
  nextest は gate-logs/cargo-test.log の Summary 行で 8515〜8525 passed・22 skipped・fail 0。
- verify.json: `ok: true`・`live_ok: true`。検査 1（start-and-migrate）・2（counts-match）・3（main-gets）・5（n-1-compat）・6（smoke）・4d（web-app-start）の 6 件がどれも ok。
- promoted.json: `status: succeeded`・`exit_code 0`・`mode: live`。actor は 4 件とも `external_operator:claude-code-ops-session`。

B・C・D の最初の release の gate・verify が未確認の理由: release の dir と `releases/.build/<sha12>/` がどちらも無く、gate.json・verify.json を読めない。
`release.sh` の掃除が current・previous・検証済みの新しい 3 件だけを残すので、後の release を作ったときに消えたとみられる。
promote ログに 3 件とも `promoted <sha12> (mode=live)` があり、`promote.sh` は verify.json の `ok` が真でなければ止まり、`verify.sh` は gate.json の `ok` が
真でなければ止まるので、gate と verify は ok だったと推定できる（記録そのものは読めていない）。B・C・D は後の 4 release にも含まれ、その gate・verify・昇格は記録で確かめた。

気づいた点（safety.md。直していない）: gate.json の `cargo_test.passed` は 4 release とも 3、`ignored` は 24 で、nextest の通過数を含まない
（doc test の通過数 3 と、nextest の skipped 22 と doc test の ignored 2 の和に一致する）。nextest の件数は gate log の Summary 行にだけある。

## 再現コマンド

7 つの WorkUnit の md の再現の節の command を写した。この file のためには実行していない（数は各 md の実行の記録から写した）。
repo の root で実行する。`$TMPDIR` は run の一時 dir、生の出力は各 WorkUnit の run の成果物 dir（repo の外）にある。
本番 DB を読む prompt-audit と review-prod の command は、DB を `mode=ro` の URI でだけ開く。release の記録を読む safety の command は読むだけで、
`release.sh`・`verify.sh`・`promote.sh` は含まない。

### struct-deps

[struct-deps.md](struct-deps.md) §5。`ART` はその run の成果物 dir、`G` は G の scope の成果物の `raw/`（`c2.py` の元）。

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a; H=$(git merge-base HEAD main); O=$TMPDIR/out; mkdir -p $O
R=$TMPDIR/rh-B0; mkdir -p $R
git archive $B0 scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $R
# C1（A §7 C1。約 1 分 36 秒ずつ。3 本を同時に走らせた）
python3 scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-head-B0.json
python3 scripts/dev/repo-health/repo_health.py --repo . --base $H --head $H --policy-version 1 --out $O/c1-head-H.json
python3 $R/scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-b0-B0.json   # arch-map.* は checker_missing
python3 -c "import json,collections,sys;r=json.load(open(sys.argv[1]));c=collections.Counter(f['rule_id'] for f in r['findings']);print({k:c[k] for k in ('deps.cycle','size.responsibility-growth','dup.contract','arch-map.wrong_owner')})" $O/c1-head-H.json
#   scope の集合の比較: findings を rule_id ごとに json.dumps(f['scope'],sort_keys=True) の Counter にして before と after の差を取る（差 0）
# C2（A §7 C2 の snippet = G の raw/c2.py。第 4 引数で profile を読む rev を指定できるようにした写し）
sed "s/^snap=snapshot.resolve('.',B,B)/PB=sys.argv[4] if len(sys.argv)>4 else B; snap=snapshot.resolve('.',PB,B)/" $G/c2.py > $TMPDIR/c2.py
python3 $TMPDIR/c2.py scripts/dev/repo-health $B0 $O/edges-head-B0.json
python3 $TMPDIR/c2.py scripts/dev/repo-health $H  $O/edges-head-H.json
python3 $TMPDIR/c2.py $R/scripts/dev/repo-health $B0 $O/edges-b0-B0.json
python3 $TMPDIR/c2.py $R/scripts/dev/repo-health $H  $O/edges-b0-H.json $B0      # profile は B0 から読む（§2 の 1）
#   出力の 'cycles' の数 = module_cycles、'crates/task-core/src/store' を含む SCC = tc_scc_*、'crates/task-api/src/handlers' を含む SCC = ta_scc_*、
#   'handlers->cos'・'cos->handlers' の行 = repo-health の辺。行の辺の差は (src, dst_unit) ごとの Counter の差で見た
# C5（A §7 C5）
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'
for B in $B0 $H; do
  git grep -nE 'crate::cos::' $B -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | grep -vE "$X" | wc -l        # 22 / 19
  git grep -nE '(crate|super(::super)*)::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                         # 52 / 50
  git grep -nE 'crate::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                                         # 48 / 46
  git grep -nE 'super(::super)*::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                               # 4 / 4
  git grep -n '#\[path' $B -- crates | grep -vE "$X"     # path_attr_files の元の一覧（§2 の 4 の規則で分ける）
done
# C7（cargo metadata。workspace_members の dependencies のうち path のある workspace の crate を kind ごとに数え、normal の辺で強連結成分を取る）
git diff --stat $B0 $H -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'        # 差分 0
cargo metadata --format-version 1 --offline > $O/meta-H.json                   # members 15、edges 49（normal 35・dev 14）、crate_cycles 0
# 地図の確認（§3）
for f in design_gate design_verdict plan_adoption policy_review worker_profile check_item review_limits inventory; do
  echo "$f $(git show $H:docs/architecture-map.md | grep '#\[path\]' | grep -c $f)"; done                      # 後ろの 3 件が 0
# profile の差（§2 の 1）
python3 -c "import subprocess,tomllib; ..."   # B0 と after の celeris.toml を tomllib で読み、差のある key を出す（review.* だけ）
```

### struct-store

[struct-store.md](struct-store.md) §6。`B` を before（B0）と after（`git merge-base HEAD main`）の sha に替えて 2 回走らせる。

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a   # after は B=$(git merge-base HEAD main)
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'; P='^crates/(campaign|routing-eval|jev-classifier)/'
# C4 rusqlite・SQL 文字列（task-core/src/store/ を除く production file）
git grep -l 'rusqlite' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l
git grep -lE '"[^"]*\b(SELECT .* FROM|INSERT (OR [A-Z]+ )?INTO|UPDATE [a-z_]+ SET|DELETE FROM)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l
# C6 &Connection の引数（crate ごと。store/ は ^crates/task-core/src/store/ で絞る）と Fn の closure
git grep -nE '\b[a-z_]+: &(mut )?(rusqlite::)?(Connection|Transaction)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" > $TMPDIR/ca
for c in task-core task-api task-ops task-core/src/store; do echo "$c files=$(grep "^crates/$c/" $TMPDIR/ca | cut -d: -f1 | sort -u | wc -l) occ=$(grep -c "^crates/$c/" $TMPDIR/ca)"; done
git grep -nE 'Fn(Once|Mut)?\(&(mut )?(rusqlite::)?(Connection|Transaction)' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | cut -d/ -f2 | sort | uniq -c
# C6 SqliteStore: impl block と pub method は A の baseline.md §7 C6 の git grep と python の snippet をそのまま使う（path 付きは同 §7 の注の正規表現）
# C7 逆依存: cargo metadata --format-version 1 --offline の workspace_members の normal 依存を逆にたどる（before は git archive した tree で実行）
# C3・E1 dispatcher: E の measure-commands.md の C3（request と logical.txt）と E1 の snippet をそのまま使う
# Applied の形（D の付記）
git grep -l 'Option<&OperationAudit>' $B -- crates/task-api/src | wc -l
git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | wc -l
# 補助: file ごとの関数（type Audit の行は除く）
for f in $(git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | sed "s/^$B://"); do
  git show $B:$f | awk -v f=$f '/^[[:space:]]*(pub(\([a-z]+\))? )?(async )?fn [a-z_0-9]+/ {match($0,/fn [a-z_0-9]+/); fn=substr($0,RSTART+3,RLENGTH-3)}
    /^type Audit</ {next}
    /Option<&('"'"'[a-z_]+ )?(crate::cos::operations::)?OperationAudit>/ || /: Audit<'"'"'_>/ {print f, fn}'; done | sort -u | wc -l
```

### loc-cost

[loc-cost.md](loc-cost.md) §5。`B=bc665168c9caf1898adc4c611404bbee056daf9a`、`H=HEAD`。`c7b.py`・`c10.py` は A の baseline.md §7 の snippet の写し。

```sh
# (6) LoC（A §7 C12。分類規則は base に固定）
git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
for r in $B HEAD; do python3 $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev $r --format json --out $TMPDIR/ssr-$r.json; done
#   views.product_only.lines / rust_lines、class_totals.external_test + inline_test、views.own_total_with_tests.lines / rust_lines、
#   言語別は matrix.<class>.<lang>.lines。config_sha256 が 2 つで同じことを確かめる

# (4) 変える file・単位・crate
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env   # S1_FILES（4 file）・S2_FILES（4 file）
#   単位 = crates/<c>/src/<第 1 要素>（.rs を除く）、crate = crates/<c>

# (4) C7b（A §7 C7b の snippet を $TMPDIR/c7b.py に写したもの。head の検査器）
python3 $TMPDIR/c7b.py HEAD S1h:crates/task-api/src/org_write,crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store \
  S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement
python3 $TMPDIR/c7b.py $B  <同じ引数>          # before'（head の検査器 × B0）
# b0 の検査器: B0 の検査器と source-size-report を出し、sys.path を差し替え、snapshot.resolve('.', B0, 測る rev) にした写し（c7b-b0.py）
mkdir -p $TMPDIR/rh-B0 && git archive $B scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $TMPDIR/rh-B0
sed "s#sys.path.insert(0,'scripts/dev/repo-health')#sys.path.insert(0,'$TMPDIR/rh-B0/scripts/dev/repo-health')#; s#snap=snapshot.resolve('.',B,B)#snap=snapshot.resolve('.',B.split('@')[-1],B.split('@')[0])#" $TMPDIR/c7b.py > $TMPDIR/c7b-b0.py
python3 $TMPDIR/c7b-b0.py $B <同じ引数>; python3 $TMPDIR/c7b-b0.py HEAD@$B <同じ引数>   # 引数の「測る rev@profile の rev」

# (4) test-affected の対象（comment の probe。G の probe-comment.sh の写し）
bash probe-comment.sh $S1_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S1_FILES
bash probe-comment.sh $S2_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S2_FILES
git status --porcelain -- crates                              # 空であること

# 修正の波及（A §7 C10 の snippet を baseline.md から $TMPDIR/c10.py に写したもの。約 60〜85 s と 1〜2 s）
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-10-28T00:00:00Z $m $B; done          # A の窓
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z $m "$B..HEAD"; done   # B0 の後
#   commit ごとの内訳は c10.py の集計の直前に print を 1 行足した写しで出した

# (5) G の測定の tree と HEAD の source の差分（0 行なら G の値を写す）
G=$(git log --format=%H --grep='^arch-v4 G scope' -1 main)^
git diff --name-only $G HEAD -- crates scripts web Cargo.lock Cargo.toml | wc -l      # 0
git rev-parse $G:crates HEAD:crates $G:scripts HEAD:scripts $G:web HEAD:web           # 組ごとに同じ値
# G の TSV の after の中央値
python3 -I -c "import csv,statistics,collections;d=collections.defaultdict(list)
for f in ['scope','build-check','build-test','ta-s1','ta-mix','ta-s2']:
  for r in csv.DictReader(open(f'agent-docs/progress/2026-11-19-arch-v4-g/{f}.tsv'),delimiter='\t'):
    if r['rev']=='after' and r['valid']=='yes': d[(r['metric'],r['scenario'])].append(float(r['seconds']))
[print(k,statistics.median(v),v) for k,v in sorted(d.items())]"
```

### gate

[gate.md](gate.md) §7。probe はそれぞれ検査器を走らせた直後に `git checkout --` で戻す（commit しない）。

```sh
cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)' --status-level pass --final-status-level pass
cargo nextest run -p task-worker -E 'test(/prompt_boundary_/)' --status-level pass --final-status-level pass
python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'
B=$(git merge-base HEAD main)
F=crates/task-api/src/lifecycle.rs
# probe a
sed -i 's/^use crate::human_epoch::HumanEpoch;$/&\nuse crate::knowledge::KnowledgeItem;/' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/a.json; echo $?   # 1
git checkout -- $F
# probe b
sed -i 's|^use crate::human_epoch::HumanEpoch;$|&\n// probe: comment only (gate-cases)|' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/b.json; echo $?   # 0
git checkout -- $F
```

### prompt-audit

[prompt-audit.md](prompt-audit.md) の「再現」。`audit.py` の本文（66 行の python）は同じ節にあり、作業場所の外（`$TMPDIR`）に写して使う。

```sh
PA_DETAIL=$TMPDIR/detail.json python3 -I "$TMPDIR/audit.py" > agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.tsv   # exit 0、201 行と header
# kind ごとの集計
awk -F'\t' 'NR>1{k[$3" "$5]++; if($5!="missing"){t[$3]+=$6; f[$3]+=$7}} END{for(x in k) print x, k[x]; for(x in t) print x, "acceptance", f[x]"/"t[x]}' \
  agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.tsv | sort
```

### safety

[safety.md](safety.md) の不変条件ごとの節と「release gate」節。安全不変条件の 11 行（safety.tsv の command 列。§安全不変条件 の code block と同じ）:

```sh
# sqlite_source（169）
cargo nextest run -p task-core -E 'test(/^store::(tests|migration_harness_tests|client_open_tests|legacy::tests)::/) | test(/^transition::tests::/)' && cargo nextest run -p task-worker -E 'test(/^db_guard::tests::/)'
# events_append_only（13）
cargo nextest run -p task-core -E 'test(/^store::tests::append_event_/) | test(/^store::tests::apply_transition_.*appends/) | test(/^store::tests::create_task_inserts_task_and_events_atomically$/) | test(append_only)'
# lease_atomicity（51）
cargo nextest run -p task-core -E 'test(/^store::lease_reclaim_tests::/) | test(/^store::tests::(acquire_lease_|acquire_work_unit_lease_|renew_lease_|apply_transition_releases_lease_)/)' && cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::(cex_lease_restart|cex_same_tick|ownerless_runs|orphan_takeover)::/) | test(/^run_supervisor::tests::/)'
# replay（14）
cargo nextest run -p task-ops -E 'test(/^replay::tests::/)'
# project_controller_stop（14）
cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::project_wake::effects::project_wake_pending_/) | test(/^dispatcher::tests::project_wake::project_wake_(human_races_|off_is_inert$)/) | test(/project_recovery_k37_/) | test(/^dispatcher::tests::project_wake::replay::project_controller_replay_(paused_project|cancelled_project|invalid_proposals_stop)$/)'
# rust_ts_canary（16）
cargo nextest run -p task-dispatch -E 'test(canary)'
# acl（28）
cargo nextest run -p task-api -E 'binary(auth_and_guards) | (kind(lib) & test(/^(principal|middleware)::tests::|^handlers::tests::resolve_principal_/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::/))'
# credential（78）
cargo nextest run -p celeris-credentiald -E 'binary(broker) | binary(injection_peer_uid) | binary(prod_admission) | binary(injection_ipc)' && cargo nextest run -p task-worker -E 'test(browser_credential)' && cargo nextest run -p task-api -E '(kind(lib) & test(/^secrets::tests::/)) | (binary(run_worker_suite) & test(/^secrets_admin::/))'
# cos_audit（16）
cargo nextest run -p task-api -E 'binary(cos_operations) | (kind(lib) & test(/^cos::operations::/)) | (binary(cos_delivery_suite_a) & test(/^cos_auth::/))'
# selfdeploy_authority（83）
cargo nextest run -p celeris -E '(kind(lib) & test(/^(instance|releases)::/)) | binary(unpromoted_release) | binary(instance_handoff) | (binary(cos_delivery_suite) & test(/^releases_api::/))' && cargo nextest run -p task-api -E '(kind(lib) & test(/^releases::tests::/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::promotion_audit_/))'
# arch_v4_counterexamples（81）
cargo nextest run -p task-api --test cos_delivery_suite org_write_contract && cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract && cargo nextest run -p task-dispatch -E 'test(store_access_guard) | test(structure_guard) | test(cex_) | test(write_set_)'
```

release の記録（読むだけ。safety.md の「読んだ command と出力の要点」と「人が確かめる読み取りだけの command」）:

```sh
ls -la ~/.local/celeris/releases/
readlink /local/celeris/state/current /local/celeris/state/previous
python3 -I -m json.tool ~/.local/celeris/releases/<sha12>/{manifest,gate,verify,promoted}.json
ls ~/.local/celeris/releases/*/promote_failed.json
ls -lt ~/.local/celeris/releases/.build/
grep -E 'release=|current  ->|promoted [0-9a-f]{12}' /local/celeris/state/logs/promote-2026111{7,8,9}-*.log
ls /local/celeris/state/logs | grep rollback
grep -E '^\s+Summary \[' ~/.local/celeris/releases/<sha12>/gate-logs/cargo-test.log
git merge-base --is-ancestor <B〜G の最後の commit> <release の commit>; echo $?   # 0 なら含む
# B・C・D の最初の release（人が確かめる）
ls -la ~/.local/celeris/releases/48b700ede57f ~/.local/celeris/releases/f85df0209504 ~/.local/celeris/releases/eb1244bb5e8d ~/.local/celeris/releases/.build/
grep -vE 'preview:' /local/celeris/state/logs/promote-20261117-222401.log /local/celeris/state/logs/promote-20261118-121753.log /local/celeris/state/logs/promote-20261118-210838.log
git -C ~/workspace/agent-platform show 48b700ede57f:scripts/selfdeploy/promote.sh | grep -n 'refusing to promote'
```

### review-prod

[review-prod.md](review-prod.md) §2。道具（`review_prod.py`・`tsv_gen.py`）はその run の成果物 dir の `raw/` にある使い捨てで、query の SQL は同じ節にある。
A と同じ長さの窓での比べ直しは、2026-11-25T06:00Z の後に第 2 引数を `2026-11-25T06:00` にして走らせる（同 §7.1）。

```sh
ART=/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/review-prod/artifacts
python3 -I $ART/raw/review_prod.py /local/celeris/data/db/celeris.sqlite3 2026-11-19T12:00 2026-11-18T12:18:40 $ART/raw/review-prod.json
python3 -I $ART/raw/tsv_gen.py $ART/raw/review-prod.json > agent-docs/progress/2026-11-19-arch-v4-h/review-prod.tsv
```
