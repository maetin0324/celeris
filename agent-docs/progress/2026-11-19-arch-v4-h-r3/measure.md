---
title: アーキ v4 H R3 after — dispatcher の self.store・self.config の出現の after の測定と反例試験
tasks: [01M7THYAPKESR1BG6BQZPJ749P]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# after（WorkUnit）— 測定の結果・再現コマンド・反例試験

完了日: 2026-11-19

## 0. 結論

- 4 scope（request/C3・request/E1・logical/C3・logical/E1）のどれでも、self.store の出現は **−3**、self.config の出現は **−8** 減った。
  主の数の request/C3 は store 633 → 630、config 220 → 212。分母（非試験 file 69・論理 76・crate 138）と、参照を持つ file の数は変わらない。
- 減りは [ADR dispatcher-narrow-ops-round2](../../adr/2026-11-19-dispatcher-narrow-ops-round2.md) の見込み（store −3・config −8）と同じ。
  減った file は `tree_units.rs`（store −1）・`cos_chat/root_done.rs`（store −2）・`cos_chat/triage.rs`（config −3）・`cos_chat/fallback.rs`（config −5）の 4 つだけで、
  他の file の出現は 1 つも増えていない。crate 全体（E2）も同じだけ減った（store 683 → 680、config 240 → 232）ので、dispatcher/ の外へ移した減りではない。
- production の行（A の C12、分類規則は base に固定）は 375921 → 375914（−7）。Rust の production も −7、試験を含む自前の総量も −7。
- `measure.sh verify`（before = [base.txt](base.txt) の sha、after = HEAD）・`measure.sh gain`・`measure.sh shrink` はどれも exit 0。
  反例試験と構造の guard（`cex_`・`replan`・`parallel_work_unit`・`structure_guard`・`store_access_guard`、task の受け入れ条件の `write_set_`・`ownerless_runs` を含む）は 144 本が全部 pass。
- この WorkUnit は `crates/` を変えていない。ADR は回帰の guard（`structure_guard.rs` の変更）を決めていないので、`structure_guard.rs` も変えていない。

## 1. 測定の結果（[metrics.tsv](metrics.tsv)。before = base.txt の sha、after = HEAD）

| metric | request/C3 | request/E1 | logical/C3 | logical/E1 |
|---|---|---|---|---|
| `dispatcher_files` | 69 → 69 | 69 → 69 | 76 → 76 | 76 → 76 |
| `dispatcher_store_files` | 38 → 38 | 40 → 40 | 42 → 42 | 44 → 44 |
| `dispatcher_store_hits` | 633 → **630**（−3） | 759 → **756**（−3） | 661 → **658**（−3） | 791 → **788**（−3） |
| `dispatcher_config_files` | 31 → 31 | 31 → 31 | 35 → 35 | 35 → 35 |
| `dispatcher_config_hits` | 220 → **212**（−8） | 227 → **219**（−8） | 226 → **218**（−8） | 233 → **225**（−8） |

| metric | scope | before | after | 差 |
|---|---|---|---|---|
| `dispatch_crate_files` | crate/E2 | 138 | 138 | 0 |
| `dispatch_crate_store_hits` | crate/E2 | 683 | 680 | −3 |
| `dispatch_crate_config_hits` | crate/E2 | 240 | 232 | −8 |
| `dispatcher_sqlitestore_files` | request/E2 | 26 | 26 | 0 |
| `dispatcher_sqlitestore_files` | logical/E2 | 28 | 28 | 0 |
| `loc_product_lines` | repo/C12 | 375921 | 375914 | −7 |
| `loc_product_rust_lines` | repo/C12 | 331230 | 331223 | −7 |
| `loc_own_total_with_tests` | repo/C12 | 1031506 | 1031499 | −7 |

## 2. file ごとの差（base.txt の sha と HEAD。crates/task-dispatch/src の非試験 138 file を全部比べた）

差のある file は 4 つだけ。行は file の改行の数。

| file | C3 store | C3 config | E1 store | E1 config | 行 | 減った置き換え |
|---|---|---|---|---|---|---|
| `dispatcher/tree_units.rs` | 56 → 55 | 19 → 19 | 63 → 62 | 20 → 20 | 1353 → 1351 | `reconcile_tree_units` の鏡の後の `self.store.work_units_for` を 3 か所から 2 か所に（`if changed { 読む; if hold.is_none() { promote_newly_ready; 読む } }`） |
| `dispatcher/cos_chat/root_done.rs` | 16 → 14 | 5 → 5 | 22 → 20 | 6 → 6 | 477 → 472 | `root_done_advance`（`Queued`）の `collect_facts(&self.store, …)` を 3 つの分岐から分岐の後の 1 回に |
| `dispatcher/cos_chat/triage.rs` | 13 → 13 | 4 → 1 | 24 → 24 | 4 → 1 | 760 → 760 | `self.config.triage.policy_version` を 4 つの private 関数から `triage_ingest` の 1 回にし、`policy: &str` で渡す |
| `dispatcher/cos_chat/fallback.rs` | 13 → 13 | 9 → 4 | 14 → 14 | 9 → 4 | 709 → 709 | `triage_fallback` の `enabled`・`unavailable_reason` の読みを先頭の `enabled`・`summary_on` の 1 回に（−3）。`handoff_message` の `summary_requested` の読みを `fallback_one` → `handoff_message` の `bool` 引数に（−2） |

`review_verdict.rs`（C3 store 32・config 3）と `work_units.rs`（store 58・config 14、行 2053）は ADR の D6 どおり変わっていない。

## 3. ADR の「成果に数える条件」1〜5 の確認

| 条件 | 結果 |
|---|---|
| 1. request/C3 で store・config の出現が減り、E1 でも同じだけ減る | 満たす。store −3 / −3、config −8 / −8（論理 scope も同じ） |
| 2. crate 全体（E2）が dispatcher/ と同じだけ減り、SqliteStore を書く file が増えない | 満たす。crate store −3・config −8、SqliteStore の file 26 → 26・28 → 28 |
| 3. 4 file 以外の dispatcher/ の file の出現が増えない | 満たす。2. の表のとおり差のある file は 4 つだけ。`measure.sh shrink crates/task-dispatch/src` も `increased` を出さない |
| 4. 分母が増えない | 満たす。69 → 69・76 → 76・138 → 138。base と HEAD の `crates/` の差分は 4 file の変更だけで、file の追加・削除・移動は無い |
| 5. production の行が増えない | 満たす。375921 → 375914（−7） |

数えないもの（ADR の節）に当たる変更は無い。足したのは同じ file の private 関数の値の引数（`policy: &str` を 4 つ、`summary_requested: bool` を 2 つ）と
局所の変数（`policy`・`summary_on`、`enabled` の移動）だけで、getter・別名・新しい型・trait・helper・`task_ops` の関数・既存の写し（`let store = self.store.clone()`）は増えていない。

## 4. 反例試験と構造の guard（提出時の HEAD で実行）

| command | 結果 |
|---|---|
| `cargo nextest run -p task-dispatch -E 'test(cex_) \| test(replan) \| test(parallel_work_unit) \| test(structure_guard) \| test(store_access_guard)'`（この WorkUnit の filter） | 120 tests run: 120 passed |
| `cargo nextest run -p task-dispatch -E 'test(cex_) \| test(write_set_) \| test(ownerless_runs) \| test(store_access_guard) \| test(structure_guard)'`（task の受け入れ条件 2 の command） | 78 tests run: 78 passed |
| 上の 2 つの和（`replan`・`parallel_work_unit`・`write_set_`・`ownerless_runs` を全部含む） | 144 tests run: 144 passed |

- `cex_` の試験は task-dispatch に 40 本あり、全部 pass。ADR の表の 12 本（lease 2・restart 3・stale review 2・並列 WU 2・replan 3）が
  それぞれ `cargo nextest list` で 1 本ずつ見つかり、上の run に入っている。反例試験を足していない・消していない・弱めていない。
- 構造の guard `store_access_guard_work_units`・`store_access_guard_cos_chat`・`store_access_guard_detects_each_violation` と `structure_guard_*` は pass。

## 5. 再現コマンド

repo の root で実行する。`M=agent-docs/progress/2026-11-19-arch-v4-h-r3`、`B=$(cat $M/base.txt)`（開始時の `git merge-base HEAD main`）。

measure.sh の 4 mode（[tool.md](tool.md) §1）:

```sh
M=agent-docs/progress/2026-11-19-arch-v4-h-r3
sh $M/measure.sh rows HEAD                                       # 28 行（metric scope value checker）。約 22 秒
sh $M/measure.sh verify $M/metrics.tsv "$(cat $M/base.txt)" HEAD # before と after を数え直して tsv と比べる → exit 0（verify: ok rows 28）
sh $M/measure.sh gain $M/metrics.tsv                             # 成果の条件 → exit 0（gain: ok）
sh $M/measure.sh shrink crates/task-dispatch/src                 # 作業ツリーと base の file ごとの E1 → exit 0（files 138 -> 138、store+config 1065 -> 1054）
sh $M/measure.sh shrink crates/task-dispatch/src/dispatcher/tree_units.rs crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs \
  crates/task-dispatch/src/dispatcher/cos_chat/triage.rs crates/task-dispatch/src/dispatcher/cos_chat/fallback.rs    # → exit 0（162 -> 151）
sh $M/measure.sh shrink --allow-same crates/task-dispatch/src/dispatcher/review_verdict.rs crates/task-dispatch/src/dispatcher/work_units.rs  # → exit 0（116 -> 116）
```

E の [measure-commands.md](../2026-11-18-arch-v4-e/measure-commands.md) の C3 と E1（`R` に base.txt の sha と HEAD の sha を順に入れる。measure.sh を通さない独立の数え直し）:

```sh
X3='(^|/)(tests?)(/|\.rs$)|_tests?\.rs$'
R=$(git rev-parse HEAD)   # before は R=$(cat agent-docs/progress/2026-11-19-arch-v4-h-r3/base.txt)
# C3 依頼 scope
git ls-tree -r --name-only $R crates/task-dispatch/src/dispatcher/ | grep -vcE "$X3"
for p in 'self\.store' 'self\.config'; do
  git grep -o "$p" $R -- crates/task-dispatch/src/dispatcher/ | grep -vE ':[^:]*((^|/)(tests?)(/|\.rs:)|_tests?\.rs:)' > $TMPDIR/m
  echo "$p files=$(cut -d: -f2 $TMPDIR/m | sort -u | wc -l) occ=$(wc -l < $TMPDIR/m)"; done
# C3 論理 scope（69 − project_decider_ts.rs ＋ #[path] の 8 file）
{ git ls-tree -r --name-only $R crates/task-dispatch/src/dispatcher/ | grep -vE "$X3" | grep -v '^crates/task-dispatch/src/dispatcher/project_decider_ts\.rs$'
  for f in design_gate design_verdict plan_adoption policy_review worker_profile check_item review_limits inventory; do echo crates/task-dispatch/src/health_review/$f.rs; done; } > $TMPDIR/logical.txt
for p in 'self\.store' 'self\.config'; do xargs -a $TMPDIR/logical.txt -I{} sh -c "git show $R:{} | grep -o '$p' | wc -l" |
  awk -v p="$p" '{o+=$1; if($1>0)f++} END{print p, "files="f, "occ="o}'; done
# E1（改行をまたぐ参照も数える）
git ls-tree -r --name-only $R crates/task-dispatch/src/dispatcher/ | grep -vE "$X3" > $TMPDIR/scope.txt
for L in scope logical; do xargs -a $TMPDIR/$L.txt -I{} sh -c "git show $R:{} | perl -0777 -ne '\$s=()=/self\s*\.\s*store\b/g; \$c=()=/self\s*\.\s*config\b/g; print \"\$s \$c\n\"'" |
  awk -v l=$L '{if($1>0)sf++; so+=$1; if($2>0)cf++; co+=$2} END{print l, "store files="sf, "occ="so, "config files="cf, "occ="co}'; done
```

この数え直しの出力（2026-11-19、提出時の HEAD）:

| 数 | base.txt の sha | HEAD |
|---|---|---|
| C3 依頼 scope の非試験 file | 69 | 69 |
| C3 依頼 scope self.store | files=38 occ=633 | files=38 occ=630 |
| C3 依頼 scope self.config | files=31 occ=220 | files=31 occ=212 |
| C3 論理 scope self.store | files=42 occ=661 | files=42 occ=658 |
| C3 論理 scope self.config | files=35 occ=226 | files=35 occ=218 |
| E1 scope | store 40/759、config 31/227 | store 40/756、config 31/219 |
| E1 logical | store 44/791、config 35/233 | store 44/788、config 35/225 |

measure.sh の `rows` と同じ値。

反例試験:

```sh
cargo nextest run -p task-dispatch -E 'test(cex_) | test(replan) | test(parallel_work_unit) | test(structure_guard) | test(store_access_guard)'
cargo nextest run -p task-dispatch -E 'test(cex_) | test(write_set_) | test(ownerless_runs) | test(store_access_guard) | test(structure_guard)'
```

## 6. 未解決

- 無し（この WorkUnit の範囲）。減りが小さいこと・`review_verdict.rs`・`work_units.rs` を実装していないことは、
  task の進捗 [2026-11-19-arch-v4-h-r3.md](../2026-11-19-arch-v4-h-r3.md) の「未解決事項」と「提案」に書いた。
