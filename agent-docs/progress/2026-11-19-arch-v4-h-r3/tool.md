---
title: アーキ v4 H R3 tool — 測定の台本 measure.sh と metrics.tsv の before（E の C3・E1・E2 と A の C12）
tasks: [01M7THYAPKESR1BG6BQZPJ749P]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# tool（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- base は開始時の `git merge-base HEAD main` = `1680de5cd1b4c57d5df3ceeab59d03a3783a0f6c`（main の件名 `integrate wu/close (phase close)`）。
  [base.txt](base.txt) に完全な sha を 1 行で書いた。H の after（H の struct-store の `git merge-base HEAD main`）と同じ commit。
- [measure.sh](measure.sh)（`/bin/sh`、集計と比較だけ `python3 -I`）を書いた。mode は `rows`・`verify`・`shrink`・`gain` の 4 つ。
  数え方は [E の measure-commands.md](../2026-11-18-arch-v4-e/measure-commands.md) の C3・E1・E2 と A の C12 の command をそのまま使い、
  規則（除外の正規表現 X3、C3 の git grep の除外、論理 scope の 8 file、E1 の perl の正規表現）を変えていない。
  C12 の `source-size-report.py`・`.toml` は base.txt の commit から `git show` で出す。
- [metrics.tsv](metrics.tsv) を header `metric scope before after checker note` で作り、before を `rows <base>` の 28 行、after を `-` にした。
  **request/C3 は H の値（非試験 69 file・self.store 633・self.config 220）と同じ。** 他の 25 行も H・E の記録と全部同じで、違う行は無い。

## 1. measure.sh の mode

repo のどこから呼んでもよい（台本が `git rev-parse --show-toplevel` に移る）。一時 file は `$TMPDIR` の下に作り、終わると消す。

| mode | 形 | 動き |
|---|---|---|
| rows | `sh measure.sh rows <rev\|.>` | `metric<TAB>scope<TAB>value<TAB>checker` を 28 行出す。`.` は作業ツリー（25 行。C12 の 3 行は rev のときだけ） |
| verify | `sh measure.sh verify <tsv> <before-rev> <after-rev\|->` | rows を数え直し、tsv の before（と after）と違う行（`before`・`after`・`checker`）・tsv に無い行（`missing`）・数え直しに無い tsv の行（`extra`）を出して exit 1。header と列の数も見る。after に `-` を渡すと after の列を見ない |
| shrink | `sh measure.sh shrink [--allow-same] <path>...` | base.txt の sha と作業ツリーを比べる。`crates/task-dispatch/src` の非試験 file の集合が変わった（`added`・`removed`）、どれかの file の E1 の self.store か self.config が増えた（`increased`）なら exit 1。`--allow-same` が無ければ、与えた path（file か dir。base の非試験 file に当てる）の E1 の store＋config の合計が減っていないときも exit 1（`not-decreased`）。path が当たらなければ exit 2 |
| gain | `sh measure.sh gain <tsv>` | dispatcher_files の after＝before（4 scope）、dispatcher_store_hits・dispatcher_config_hits の after＜before（4 scope）、`*_files` と dispatcher_sqlitestore_files の after≦before、dispatch_crate_store_hits・dispatch_crate_config_hits の減り ≧ dispatcher_store_hits・dispatcher_config_hits（request/C3）の減り、loc_product_lines の after≦before を満たせば exit 0。満たさない行を出して exit 1。after が数でない行も落とす |

exit の値: 0 = 条件を満たす、1 = 満たさない、2 = 使い方・引数の誤り、3 = 論理 scope を選び直す必要がある。

行と checker（H の struct-store.tsv と同じ key・scope・checker の文字列）:

- `dispatcher_files`・`dispatcher_store_files`・`dispatcher_store_hits`・`dispatcher_config_files`・`dispatcher_config_hits` ×
  `request/C3`（`a-c3`）・`request/E1`（`e-e1`）・`logical/C3`（`a-c3`）・`logical/E1`（`e-e1`）の 20 行
- `dispatch_crate_files`・`dispatch_crate_store_hits`・`dispatch_crate_config_hits`（`crate/E2`、`e-e2`）
- `dispatcher_sqlitestore_files`（`request/E2`・`logical/E2`、`e-e2`）
- `loc_product_lines`（views.product_only.lines）・`loc_product_rust_lines`（views.product_only.rust_lines）・
  `loc_own_total_with_tests`（views.own_total_with_tests.lines）（`repo/C12`、`a-c12`）

実装の注:

- 作業ツリー（`.`）は、本物の index の写しを `$TMPDIR` に置き、`GIT_INDEX_FILE` で `crates/task-dispatch` を `git add -A` して
  `git write-tree` した tree に、rev と同じ command を当てる。本物の index と作業ツリーは変えない（blob は git の object store に書く）。
- 論理 scope は E と同じ固定の list（dispatcher/ の非試験 file − `project_decider_ts.rs` ＋ `health_review/` の
  design_gate・design_verdict・plan_adoption・policy_review・worker_profile・check_item・review_limits・inventory）。
  E の「after で場所が変わったら `git grep -n '#\[path'` で選び直す」を guard にした: 測る rev の非試験の `#[path = "…"]` の
  属性（file と中身。行番号は除く）の集合が base と違うか、list の file が rev に無ければ、差を出して exit 3 で止まる（数を出さない）。
- 論理 scope の C3・E1・E2 は E の `git show <rev>:<path> | …` と同じ内容を file に写してから grep・perl を当てる。
  空の file は E1 の perl が何も出さないので 0 0 とした（合計と file 数は E の awk と同じ）。
- C12 は `python3 -I` で base の `source-size-report.py` を `--root . --config <base の toml> --exclusive-classes --rev <rev> --format json` で走らせる。
  base と `bc665168c9caf1898adc4c611404bbee056daf9a`（A・H の B0）で script と toml の差分は 0 行（`git diff --stat` が空）。
- 時間: `rows <rev>` は約 14 秒（ほとんどが C12）、`rows .` は約 2 秒、`shrink` は約 3 秒。

## 2. before（rows <base>）と H の値の比較

値は「file 数 / 出現数」。H の値は [H の struct-store.tsv](../2026-11-19-arch-v4-h/struct-store.tsv) の after（H の after は同じ commit）。

| scope / 数え方 | 分母 before（H） | self.store before（H） | self.config before（H） |
|---|---|---|---|
| request / C3 | 69（69） | 38 / 633（38 / 633） | 31 / 220（31 / 220） |
| request / E1 | 69（69） | 40 / 759（40 / 759） | 31 / 227（31 / 227） |
| logical / C3 | 76（76） | 42 / 661（42 / 661） | 35 / 226（35 / 226） |
| logical / E1 | 76（76） | 44 / 791（44 / 791） | 35 / 233（35 / 233） |

| metric | scope | before | 比べた記録 |
|---|---|---|---|
| dispatch_crate_files | crate/E2 | 138 | E の metrics.md の HEAD の値 138 と同じ |
| dispatch_crate_store_hits | crate/E2 | 683 | E の metrics.md の HEAD の値 683 と同じ |
| dispatch_crate_config_hits | crate/E2 | 240 | E の metrics.md の HEAD の値 240 と同じ |
| dispatcher_sqlitestore_files | request/E2 | 26 | E の metrics.md の HEAD の値 26 と同じ |
| dispatcher_sqlitestore_files | logical/E2 | 28 | E の metrics.md の HEAD の値 28 と同じ |
| loc_product_lines | repo/C12 | 375921 | H の loc-cost.tsv の loc_prod_all の after 375921 と同じ |
| loc_product_rust_lines | repo/C12 | 331230 | H の loc-cost.tsv の loc_prod_rust の after 331230 と同じ |
| loc_own_total_with_tests | repo/C12 | 1031506 | H の loc-cost.tsv の loc_own_total の after 1031506 と同じ |

request/C3 は H の値（69・633・220）と違わないので、note に違いの理由を書く行は無い。

台本が規則を変えていないことの確かめ: `sh measure.sh rows bc665168c9caf1898adc4c611404bbee056daf9a`（A・E の before の commit）は
A・E の記録の before と全部同じ値を出した（request/C3 69・44/633・35/218、request/E1 46/759・35/227、logical/C3 76・48/661・39/224、
logical/E1 50/791・39/233、crate 138・683・238、SqliteStore 27・29、C12 375099・330408・1025677）。

## 3. 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

| 確かめたこと | command | 結果 |
|---|---|---|
| before が rows と一致 | `sh measure.sh verify metrics.tsv $(cat base.txt) -` | exit 0（`verify: ok rows 28`） |
| 作業ツリーの数が base と同じ | `sh measure.sh rows .` と `rows <base>` の先頭 25 行の diff | 差分 0 |
| crates/ の変更が無いとき | `sh measure.sh shrink --allow-same` | exit 0（`files 138 -> 138`） |
| 同上 | `sh measure.sh shrink crates/task-dispatch/src/dispatcher/tree_units.rs` | exit 1（`not-decreased … 83 -> 83`） |
| 同上 | `sh measure.sh shrink`（path なし） | exit 1（`not-decreased 0 file`） |
| 当たらない path | `sh measure.sh shrink crates/nope.rs` | exit 2 |
| 一時の改変で減る | tree_units.rs の self.store 1 個を消して `shrink <tree_units.rs>`・`shrink <dispatcher/>` | 両方 exit 0（83 → 82、986 → 985） |
| 参照を別の file へ移す | 上に加え root_done.rs に `// self.store` を 1 行足して `shrink <tree_units.rs>` | exit 1（`increased root_done.rs self.store 22 -> 23`） |
| 非試験 file を足す | `dispatcher/new_owner.rs` を足して `shrink --allow-same` | exit 1（`added`） |
| 試験 file を足す | `dispatcher/tests/zz_probe.rs` を足して `shrink --allow-same` | exit 0（試験は数えない） |
| verify の検出 | 値を 1 変えて 1 行消した tsv で `verify … -` | exit 1（`before … tsv=634 count=633`・`missing loc_own_total_with_tests`） |
| gain の after 未記入 | `sh measure.sh gain metrics.tsv` | exit 1（`not-number`） |
| gain の合格 | after を store −10・config −5（4 scope と crate）・loc −30 にした tsv | exit 0 |
| gain の不合格 | 分母 −1・config 不変・files +1・crate の減り 4 < 10・loc +5 の tsv | exit 1（5 行とも出た） |

一時の改変は `git checkout --` と `rm` で戻した。終わりに `git diff --quiet -- crates` が exit 0（crates/ の差分 0）。
cargo の検査（test-affected・clippy・fmt）は走らせていない。この WorkUnit は `agent-docs/progress/2026-11-19-arch-v4-h-r3/` の
4 file（base.txt・measure.sh・metrics.tsv・tool.md）だけを足し、crates/ を変えていない。

## 4. 後の unit の使い方

- cos・tree・verdict の check: `sh agent-docs/progress/2026-11-19-arch-v4-h-r3/measure.sh shrink <変えた file>`（参照を減らさない unit は `--allow-same`）。
- after: `rows HEAD` の値を metrics.tsv の after に写し、`verify metrics.tsv $(cat base.txt) HEAD` と `gain metrics.tsv` を exit 0 にする。
- main の同期で `git merge-base HEAD main` が動いても、base は base.txt の sha のまま使う。

## 未解決

- 無し。

## 提案

- 無し。
