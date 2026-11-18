---
title: アーキ v4 E — 同じ規則で測った before/after と共通受入条件 (1)〜(6)
tasks: [01M7R60D0K6ZF1KCNG2QYHZEV8]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# アーキ v4 E — 同じ規則で測った before/after（measure）

[measure-commands.md](measure-commands.md) の C1・C2・C3・C7b・C12 と E1〜E3 を、次の 3 つの rev に当てた（WorkUnit counts）。
(5)（test-affected の時間）と反例試験の結果は WorkUnit tests が足した（節「(5) test-affected の時間」と「反例試験」）。

| 記号 | rev | 内容 |
|---|---|---|
| B0 | main `bc665168c9caf1898adc4c611404bbee056daf9a` | A の基準（A の baseline.md §7） |
| B1 | `git merge-base HEAD main`（件名 `integrate wu/loc-fix (phase fix)`） | E の分岐点。B0 との差は B・C の変更 |
| HEAD | 提出時の HEAD（作業ツリーは clean） | E の wu-region・cos-region の後 |

source（`crates/`・`scripts/`・`web/`・`docs/`）は変えていない。この WorkUnit の変更はこの file だけ。

## 結論

- 目安に届いた。dispatcher/ の self.store を書く file は 44 → 38（−6。目安 −5）、self.config は 35 → 31（−4。目安 −3）。B0 と B1 の値は同じなので、
  B0 比と B1 比の差は同じ。分母（scope 69・論理 76・crate 138）は変わらず、crate 全体も 53 → 47・40 → 36 で同じ数だけ減った。
  SqliteStore を書く file は crate 50 → 49（不増）。閾値の count check は B0・B1 の両方で exit 0。
- (1) dispatcher を含む SCC は B1 → HEAD で変わらない（今の checker で 4 単位・7 組・51 本、辺の file 単位の組も同じ）。A の before
  「3 単位・4 組・22 本」は B0 の checker の値で、B0 に B0 の checker を当てると同じ 3・4・22 が出た。単位数の差は arch-v4 B の checker の変更
  （Rust の単位を `#[path]`・`super::` の論理 module にした。health_review が SCC に入る）による。E の変更による悪化は無い。
- (3) B1..HEAD で task-api・task-core・task-ops の差分は 0。crates/ の差分は task-dispatch の 13 file（全部 M）だけ。
- (4) S2 は direct 6・transitive 14・package 4 のまま（3 rev とも）。
- (6) production は B1 → HEAD で +61 行（全言語・Rust とも）、own_total_with_tests は +195。ADR の目安 +80 の中、replan の閾値 +200 の下。
- (5) S2 の comment probe での test-affected は 1 回で 769.32 s（exit 0、nextest 3,936 passed・11 skipped）。A の S2 before の中央値 655.59 s
  （3,917 passed）より 113.73 s 長い。1 回だけの値なので、悪化かどうかは判定しない。3 回の比較は G の担当で「要判断」。
- 反例試験（lease・restart・stale review・並列 WU・replan）は ADR の 12 本がすべて pass。filter に当たった 121 本も全部 pass（2,772 skipped）。

## (2) C3・E1・E2・E3

C3 は A の snippet（1 行の中の `self.store`・`self.config`）。E1 は改行をまたぐ `self\s*\.\s*(store|config)\b`。値は「file 数 / 出現数」。

| 計測 | 範囲 | B0 | B1 | HEAD | B1 → HEAD の file の差 |
|---|---|---|---|---|---|
| 非試験 file 数（分母） | 依頼 scope（dispatcher/） | 69 | 69 | 69 | 0 |
| C3 self.store | 依頼 scope 69 file | 44 / 633 | 44 / 633 | 38 / 633 | −6 |
| C3 self.config | 依頼 scope 69 file | 35 / 218 | 35 / 218 | 31 / 220 | −4 |
| 論理 scope の file 数（分母） | 69 − project_decider_ts.rs ＋ `#[path]` の 8 | 76 | 76 | 76 | 0 |
| C3 self.store | 論理 scope 76 file | 48 / 661 | 48 / 661 | 42 / 661 | −6 |
| C3 self.config | 論理 scope 76 file | 39 / 224 | 39 / 224 | 35 / 226 | −4 |
| E1 self.store（改行込み） | 依頼 scope | 46 / 759 | 46 / 759 | 40 / 759 | −6 |
| E1 self.config（改行込み） | 依頼 scope | 35 / 227 | 35 / 227 | 31 / 227 | −4 |
| E1 self.store（改行込み） | 論理 scope | 50 / 791 | 50 / 791 | 44 / 791 | −6 |
| E1 self.config（改行込み） | 論理 scope | 39 / 233 | 39 / 233 | 35 / 233 | −4 |
| E2 SqliteStore を書く file | 依頼 scope | 27 | 27 | 26 | −1 |
| E2 SqliteStore を書く file | 論理 scope | 29 | 29 | 28 | −1 |
| E2 self.store | crate 全体（task-dispatch/src の非試験） | 53 / 683 | 53 / 683 | 47 / 683 | −6 |
| E2 self.config | crate 全体 | 40 / 238 | 40 / 238 | 36 / 240 | −4 |
| E2 SqliteStore | crate 全体 | 50 / 195 | 50 / 195 | 49 / 195 | −1 |
| E2 非試験 file 数（分母） | crate 全体 | 138 | 138 | 138 | 0 |

論理 scope は HEAD で `git grep -n '#\[path' HEAD -- crates/task-dispatch/src` を引いて選び直した。dispatcher から `health_review/` を読む非試験の
`#[path]` は B0・B1・HEAD とも design_gate（dispatcher.rs）・design_verdict（design_gate.rs）・policy_review・inventory・check_item・plan_adoption・
review_limits・worker_profile の 8 file で、選び直しても同じ 76 file になった。

self.config の出現の +2（C3 218 → 220、crate 238 → 240）は wu-region が `work_units.rs` の rustfmt で `self\n.config` に割れていた 2 か所を
1 行に書き直した分で、E1（改行込み）では 227 → 227 と変わらない。参照の数は増えていない。self.store の出現は C3・E1 とも同じ（対象 file から消えた
参照が入口の file に同じ数だけ移った）。

E3（対象 6 file の token。列は self.store self.config SqliteStore Dispatcher CosChatLaunch(Config) DispatchConfig `use super::*`。comment の行も数える）:

| file | B0 | B1 | HEAD |
|---|---|---|---|
| `dispatcher/behind_target.rs` | `2 1 0 2 0 0 0`（計 5） | `2 1 0 2 0 0 0`（計 5） | `0 0 0 0 0 0 0`（計 0） |
| `dispatcher/auto_leaf.rs` | `1 1 0 1 0 0 1`（計 4） | `1 1 0 1 0 0 1`（計 4） | `0 0 0 0 0 0 0`（計 0） |
| `dispatcher/write_set_gate.rs` | `2 0 0 1 0 0 1`（計 4） | `2 0 0 1 0 0 1`（計 4） | `0 0 0 0 0 0 0`（計 0） |
| `dispatcher/write_set_record.rs` | `2 0 0 2 0 0 1`（計 5） | `2 0 0 2 0 0 1`（計 5） | `0 0 0 0 0 0 0`（計 0） |
| `dispatcher/cos_chat/relay_rejudge.rs` | `5 1 1 4 3 0 0`（計 14） | `5 1 1 4 3 0 0`（計 14） | `0 0 0 0 0 0 0`（計 0） |
| `dispatcher/cos_chat/slot_wait.rs` | `8 1 0 2 2 0 0`（計 13） | `8 1 0 2 2 0 0`（計 13） | `0 0 0 0 0 0 0`（計 0） |

閾値の count check（Task の受け入れ条件 0 の snippet。`B` を B0 と B1 にして実行）:

| 基準 | 出力 | exit |
|---|---|---|
| B0 | `files 69->69 store 44->38 occ 633->633 config 35->31 occ 218->220 crate-store 53->47 crate-config 40->36 SqliteStore-files 50->49` | 0 |
| B1 | `files 69->69 store 44->38 occ 633->633 config 35->31 occ 218->220 crate-store 53->47 crate-config 40->36 SqliteStore-files 50->49` | 0 |

## file ごとの増減

file ごとの出現数は C3 と同じ `git grep -o`（非試験）で数えた。B0 と B1 の file ごとの値は self.store・self.config とも全 file で同じなので、B1 → HEAD だけを書く。
dispatcher/ の外（crate の残り。self.store 9 file、self.config 5 file）は B1 と HEAD で file の集合も出現数も同じなので、crate 全体の増減は下の表と同じ。

self.store を書く file の集合の差:

- 減った file（6）: `auto_leaf.rs`・`behind_target.rs`・`write_set_gate.rs`・`write_set_record.rs`・`cos_chat/relay_rejudge.rs`・`cos_chat/slot_wait.rs`
- 増えた file: 無し

self.config を書く file の集合の差:

- 減った file（4）: `auto_leaf.rs`・`behind_target.rs`・`cos_chat/relay_rejudge.rs`・`cos_chat/slot_wait.rs`
- 増えた file: 無し

file ごとの出現数（C3。dispatcher/ の下。B1 → HEAD）:

| file | 役割 | self.store | self.config |
|---|---|---|---|
| `auto_leaf.rs` | 対象 | 1 → 0（−1） | 1 → 0（−1） |
| `behind_target.rs` | 対象 | 2 → 0（−2） | 1 → 0（−1） |
| `write_set_gate.rs` | 対象 | 2 → 0（−2） | 0 → 0 |
| `write_set_record.rs` | 対象 | 2 → 0（−2） | 0 → 0 |
| `cos_chat/relay_rejudge.rs` | 対象 | 2 → 0（−2） | 1 → 0（−1） |
| `cos_chat/slot_wait.rs` | 対象 | 6 → 0（−6） | 1 → 0（−1） |
| `dispatch_run.rs` | 入口（前から書く） | 30 → 34（+4） | 変化なし |
| `review_spawn.rs` | 入口（前から書く） | 47 → 49（+2） | 14 → 15（+1） |
| `work_units.rs` | 入口（前から書く） | 57 → 58（+1） | 11 → 14（+3） |
| `cos_chat/launch.rs` | 入口（前から書く） | 15 → 19（+4） | 3 → 5（+2） |
| `cos_chat/routes.rs` | 入口（前から書く） | 10 → 11（+1） | 変化なし |
| `cos_chat/control.rs` | 入口（前から書く） | 10 → 13（+3） | 変化なし |
| 合計 | | 対象 −15、入口 +15（計 0） | 対象 −4、入口 +6（計 +2。上の rustfmt の 2 か所） |

出現が増えた 6 file は B1 で既に self.store を書いていた file で、増えた file の集合には入らない（self.config も同じ。`dispatch_run.rs`・`routes.rs`・
`control.rs` の self.config は数が変わらない）。B1..HEAD で変わった crates/ の file は上の 12 file と試験の `dispatcher/tests/structure_guard.rs` だけ
（`git diff --numstat`: 非試験の 12 file で +718 / −657）。

## (1) 循環

C1（`repo_health.py --base <rev> --head <rev>`。今の worktree の checker）の deps.cycle の finding は 3 rev とも 17 件で、dispatcher を含むものの rationale は
3 rev とも同じ:

- 「4 単位の循環（辺 7 組）: crates/task-dispatch/src/dispatcher, crates/task-dispatch/src/health_review, crates/task-dispatch/src/review, crates/task-dispatch/src/undeclared_artifacts。」
- （別の SCC）「2 単位の循環（辺 2 組）: crates/task-dispatch/src/project_controller, crates/task-dispatch/src/project_decider_ts。」

C1 の finding の rule ごとの件数（`deps.cycle` 17・`size.responsibility-growth` 94・`dup.contract` 210 など 12 種）も 3 rev で同じ。C1 の時間は
B0 102.8 s・B1 113.9 s・HEAD 92.3 s（load average 16〜21 の中。参考値）。

C2（全部の辺）の dispatcher を含む SCC:

| checker | rev | 単位数 | 組の数 | 行の本数 | 組ごとの行の本数 |
|---|---|---|---|---|---|
| 今の checker（worktree の scripts/dev/repo-health） | B0 | 4 | 7 | 51 | dispatcher→review 19、dispatcher→health_review 16、dispatcher→undeclared_artifacts 4、review→dispatcher 4、health_review→dispatcher 3、health_review→review 3、undeclared_artifacts→review 2 |
| 今の checker | B1 | 4 | 7 | 51 | 同じ |
| 今の checker | HEAD | 4 | 7 | 51 | 同じ（辺の (src の file, dst の単位) の組ごとの本数の差 0） |
| B0 の checker（`git archive` で B0 から取り出した） | B0 | 3 | 4 | 22 | dispatcher→review 12、dispatcher→undeclared_artifacts 4、review→dispatcher 4、undeclared_artifacts→review 2（A の before と同じ） |

A の before（3 単位・4 組・22 本）と今の checker の値（4・7・51）の差は、B0..B1 の arch-v4 B の checker の変更（件名 `arch-v4 B checker: deps.cycle の
循環の内部の新しい組を finding にし、Rust の単位を論理 module（#[path]・super::）にする`。`rules/deps.py` +51・`health/lang_deps.py` +182 行）による。
同じ checker で B0・B1・HEAD を比べると変わらないので、E の変更は循環を広げていない。B0 の checker は B1・HEAD の profile を読めない
（C で `review.high_risk` の欄が増え、`profile_invalid: … review.high_risk[0] は class・paths だけ` で止まる）ので、B0 の checker の値は B0 だけ。

## (3) handlers⇄cos と SQL の公開面

- `git diff --quiet <B1> HEAD -- crates/task-api crates/task-core crates/task-ops` → exit 0（差分 0）。
- `git diff --name-only <B1> HEAD -- crates/ | cut -d/ -f2 | sort -u` → `task-dispatch` だけ。`--name-status` は 13 file とも `M`（追加・削除・rename 無し）。
- `git diff --stat <B1> HEAD -- scripts web docs` → 差分なし。
- B0..B1 の crates/ の差分は 36 file（task-core 3・task-dispatch 23・task-worker 10）。task-core の 3 file（`design_review.rs`・`design_review/tests.rs`・
  `execution_plan/design_brief.rs`）は B・C（設計 review・risk の規則）の変更で、E の外。
- handlers⇄cos の循環と SQL の公開面は D の範囲で、E はどちらも変えていない。

## (4) C7b S2

S2 = dispatcher・run_settlement を import する単位（cargo・cargo-dev の辺を除いた use・import の逆向き）。

| checker | rev | direct | transitive | package |
|---|---|---|---|---|
| 今の checker | B0 | 6 | 14 | 4 |
| 今の checker | B1 | 6 | 14 | 4 |
| 今の checker | HEAD | 6 | 14 | 4 |
| B0 の checker | B0 | 6 | 14 | 4 |

A の before（6・14・4）と同じ。package は campaign・celeris・celerisctl・task-dispatch。参考の S1（handlers・cos・store）は今の checker で 3 rev とも
direct 93・transitive 365・package 12、B0 の checker で B0 に当てると A の before と同じ 92・369・12（差は (1) と同じ checker の変更）。

## (6) C12 LoC

script と toml は B0 から `git show` で取り出して 3 rev に当てた（`--exclusive-classes --rev <rev> --format json`）。B0・B1・HEAD の
`scripts/dev/source-size-report.py`・`.toml` は同じ（`git diff --quiet` が exit 0）。

| 値 | B0 | B1 | HEAD | B1 → HEAD | B0 → HEAD |
|---|---|---|---|---|---|
| production 全言語（`views.product_only.lines`） | 375099 | 375763 | 375824 | +61 | +725 |
| production Rust（`views.product_only.rust_lines`） | 330408 | 331072 | 331133 | +61 | +725 |
| own_total_with_tests（`views.own_total_with_tests`） | 1025677 | 1028966 | 1029161 | +195 | +3484 |
| external_test（`class_totals`） | 455363 | 457713 | 457847 | +134 | +2484 |
| inline_test（`class_totals`） | 21408 | 21419 | 21419 | 0 | +11 |

- E の 2 領域の production の純増は B1 → HEAD の +61 行（全言語・Rust とも。B1..HEAD の production の変更は dispatcher/ の 12 file だけ）。
  ADR の目安 +80 の中、replan の閾値 +200 の下。試験の +134 は `structure_guard.rs`（guard の試験）。
- B0 → HEAD の +725 のうち +664 は B・C の変更（B0..B1）で、E の外。
- この task は行数の純減を目的にしていないので、純減の判定は無い（ADR の上限 +80 だけを見る）。

## (5) test-affected の時間

A の baseline.md §7 C8・C9 と同じ手順で 1 回だけ測った（WorkUnit tests、提出時の HEAD〈件名 `arch-v4 E counts: metrics.md に C3・E1〜E3・…`〉の作業ツリー）。

1. S2 の comment probe を入れた: `crates/task-dispatch/src/run_settlement/work_unit.rs`・`run_settlement.rs`・`dispatcher/worker_finish.rs`・
   `dispatcher/work_units.rs` の各 file の最初の `//` の行（4 file とも 1 行目の `//!`）の末尾に ` probe-$(date +%s%N)` を足した。`git diff --numstat` は
   4 file とも `1 1`（行数も名も増えない）。`test-affected.sh --list` の対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
2. 計る前に反例試験の nextest（下の節）を 1 回走らせ、target を warm にした（A の計測も warm の target で、probe で再 build されるのは
   task-dispatch とその逆依存だけ）。
3. load を待った: `s=0; while awk '{exit !($1>4)}' /proc/loadavg && [ $s -lt 1200 ]; do sleep 10; s=$((s+10)); done`。20 s 待って 1 分 load average が
   3.91 になった（条件内）。
4. `/usr/bin/time -v bash scripts/dev/test-affected.sh` を foreground で 1 回（2026-11-18T21:09:19Z 開始）。
5. `git checkout --` で 4 file を戻した。`git status --porcelain crates/` は 0 行、`git diff --quiet -- crates/` は exit 0（crates/ は clean）。

| | E（1 回） | A の S2 before（run 1 / 2 / 3、中央値） |
|---|---|---|
| 開始時の 1 分 load average | 3.91（5 分 5.98・15 分 9.85） | 4 以下（§7 C8 の waitload） |
| 経過時間（`Elapsed`） | 769.32 s（12:49.32） | 695.36 / 655.59 / 647.32 s、中央値 655.59 s |
| exit | 0 | 0 |
| nextest | 3,936 run・3,936 passed（9 slow）・11 skipped、181 binary、443.874 s | 3,917 passed・11 skipped（406.6 / 418.0 / 406.1 s） |
| test の build（cargo の `Finished` 行） | 2m 19s | baseline.md の表に無い |
| user の CPU 時間 | 3,629.05 s（system 811.90 s、CPU 577%） | 3,705 / 3,104 / 2,961 s |
| repo-health（test-affected の先頭） | pass（new 0・worsened 0、base は `git merge-base HEAD main`） | pass |
| doc test・web | doc test が通った（log に `Doc-tests` が 14 節。失敗 0）。web は対象外 | doc test、web は対象外 |

- 差: 経過時間は A の中央値より +113.73 s（+17.3%）、nextest の部分は A の 3 回の最大 418.0 s より +25.9 s。試験の数は +19（B・C・E が足した試験）。
- 1 回だけの値で、3 回の比較をしていないので、悪化かどうかは判定しない。**要判断**: 3 回の比較（同じ host・同じ load 条件で E と A の before を並べるか）は
  G の担当として判断を残す。
- 終了時の load average は 7.11 / 10.90 / 10.75 で、計測中に load が上がった（test-affected 自身の CPU 577% を含む。他の process の分は未確認）。
- log の末尾に `rm: cannot remove '/local/celeris/data/scratch/tr-1001/.trash-…/own-eval': Permission denied` が 3 行出たが、exit は 0（一時 dir の後始末の警告）。
- 生の log は run の artifacts の `ta.log`・`ta-time.txt`・`ta-start.txt`（run 終了後は残らない前提で、上の表に要点を写した）。

## 反例試験

command（提出時の HEAD、probe を入れる前に実行。2 回目は 1 試験ずつの結果を出すため `--status-level all --final-status-level none` を足した）:

```sh
cargo nextest run -p task-dispatch -p celeris -p celerisctl \
  -E 'test(cex_) | test(replan) | test(parallel_work_unit) | test(structure_guard) | test(store_access_guard)'
```

結果: exit 0。`Starting 121 tests across 65 binaries (2772 tests skipped)`、`121 tests run: 121 passed, 2772 skipped`（2 回とも同じ。build 込みの 1 回目は
116.45 s、試験の実行は 8.09 s）。内訳は task-dispatch 119 本、celeris 2 本（`project_controller_core_s6_review_failed_replans_once`・
`max_replans_changes_without_restart_and_is_written_back_with_comments`）、celerisctl 0 本。名に `cex_` を持つもの 40 本、`replan` 66 本、
`parallel_work_unit` 4 本、structure_guard・store_access_guard 14 本（`store_access_guard_work_units`・`store_access_guard_cos_chat`・
`store_access_guard_detects_each_violation` を含む。名の重なりがあるので合計は 121 にならない）。

ADR [2026-11-18 dispatcher-store-access-pilot](../../adr/2026-11-18-dispatcher-store-access-pilot.md) の「反例試験」の表の 12 本（すべて
`crates/task-dispatch/src/dispatcher/tests/` の下。名は変えていない）:

| 範囲 | 試験（module::名） | 結果 |
|---|---|---|
| lease | `cex_lease_restart::cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run` | PASS（0.656 s） |
| lease | `cex_lease_restart::cex_lease_renew_after_expired_snapshot_keeps_the_holder` | PASS（0.665 s） |
| restart | `cex_lease_restart::cex_draining_instance_reclaims_its_expired_lease_without_redispatching` | PASS（0.601 s） |
| restart | `cex_lease_restart::cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent` | PASS（0.566 s） |
| restart | `cex_lease_restart::cex_restart_after_wu_done_promotes_dependents_once` | PASS（0.638 s） |
| stale review | `cex_lease_restart::cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` | PASS（0.528 s） |
| stale review | `cex_review_authority::cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate` | PASS（1.163 s） |
| 並列 WU | `cex_lease_restart::cex_two_parallel_work_unit_runs_finishing_in_one_tick_lose_no_update` | PASS（1.325 s） |
| 並列 WU | `cex_lease_restart::cex_run_finish_and_work_unit_check_finish_in_one_tick` | PASS（1.403 s） |
| replan | `cex_late_completion::cex_late_review_and_integration_after_replan_are_dropped` | PASS（1.701 s） |
| replan | `cex_late_completion::cex_late_review_pass_after_replan_is_dropped` | PASS（2.000 s） |
| replan | `cex_late_completion::cex_late_review_pass_after_repair_is_dropped` | PASS（2.003 s） |

範囲外: task-api・task-core・task-ops・task-worker にも名に replan・parallel_work_unit を含む関数がある（`git grep -lE 'fn [a-z_]*(replan|parallel_work_unit)'` で 24 file。試験の本数は数えていない）。この 4 crate は E で変えておらず
（`git diff --quiet $(git merge-base HEAD main) HEAD -- crates/task-api crates/task-core crates/task-ops crates/task-worker` が exit 0）、
task-dispatch に依存しない（4 crate の `Cargo.toml` に `task-dispatch` の行が 0。task-dispatch に依存するのは campaign・celeris・celerisctl・llm-proxy だけ）。
E の変更はこの 4 crate の試験の結果を変えないので、走らせていない。

## 数えないものの確認

| 項目 | command（要点） | 結果 |
|---|---|---|
| 分母の増加 | 非試験 file 数（scope・論理・crate） | 69・76・138（B0・B1・HEAD で同じ） |
| file 名の集合 | `diff <(git ls-tree -r --name-only <B1> crates/task-dispatch/src) <(… HEAD …)` | 差なし（344 file）。B0 と HEAD の dispatcher/ の非試験 file 名の集合も同じ |
| file の併合・削除 | `git diff --name-status <B1> HEAD -- crates/` | 13 file とも `M`。追加・削除・rename 無し |
| `#[path]` の付け替え | `git grep -c '#\[path' <rev> -- crates/task-dispatch/src` の合計、非試験の `#[path]` の mod 行 | 合計 B0 86・B1 94・HEAD 94（B1 と HEAD は全行同じ。B0 → B1 の +8 は C の試験 file `dispatcher/tests/boundary_cases.rs`・`health_review/input/tests/risk_tests.rs` の注釈と文字列で、E の外）。非試験の mod 行は 3 rev とも 16 |
| scope の外への移動 | dispatcher/ の外の crate の file の self.store・self.config の file ごとの出現 | B1 と HEAD で同じ（self.store 9 file、self.config 5 file）。crate 全体の減りは dispatcher/ の減りと同じ −6・−4 |
| 別名 | `git grep -nE 'let [a-z_]+ *= *&?(self\.(store\|config)\|Arc::clone\(&self\.store\))\s*;\|let (Dispatcher\|CosChatLaunch) *\{\|fn (store\|config)\(&self\)'`（非試験） | 3 rev とも 8 行。B1 と HEAD で file と行の中身が同じ（差は試験 `structure_guard.rs` の guard の負の試験の文字列 `("let s = &self.store;", "self.store")` の 1 行だけ） |
| 引数で store を渡す | E3 の SqliteStore・Dispatcher・CosChatLaunch(Config)・DispatchConfig の列 | 対象 6 file とも HEAD で 0（store・設定・`Dispatcher` を受ける引数が無い） |
| 新しい trait・Manager・Port・Facade・Service | Task の受け入れ条件 3 の grep、`git diff <B1> HEAD -- crates/task-dispatch/src`（試験を除く）の `+` 行の `struct`・`enum`・`trait`・`type` の宣言 | 受け入れ条件 3 の grep は該当なし。宣言は `type RelayPick`（純粋関数 `pick` の戻り値の別名）・`enum SlotWaitStep`（`step` の戻り値）・`struct GitSite`（B1 から在る struct の可視性を `pub(super)` にした。欄は repo_id・name・dir・worktree）の 3 つだけ。trait の追加と、store・config を包む struct の追加は無い |
| field の梱包 | 上の宣言の欄 | store・config を欄に持つ新しい struct は無い |

## 未達・未解決

- 未達の項目は無い（store −6・config −4、分母不減、crate 全体で同数、SqliteStore 不増、production +61 ≤ +80）。目安に対する余裕は store・config とも 1 file。
- (5) は 1 回だけの値（769.32 s。A の S2 中央値 655.59 s より +113.73 s）。悪化かどうかは判定していない。**要判断**: 3 回の比較は G の担当。
- 反例試験は ADR の 12 本と filter の 121 本がすべて pass。未達は無い。
- (1) の A の before（3・4・22）は B0 の checker でしか出ない。H の最終の集計で checker の版をそろえて比べるなら、今の checker の値（3 rev とも 4・7・51）を基準にする。

## 提案

- measure-commands.md の C1・C2・C7b の snippet は worktree の checker を使うので、checker が変わると同じ rev でも値が変わる（今回は B の変更で
  3・4・22 が 4・7・51 になった）。後の計測では checker を B0 から `git archive` で取り出す形か、checker の版を値と並べて書く形にするとよい
  （C12 の script と toml を base から取り出すのと同じ扱い）。

## 再現コマンド

repo の root で実行した（`$TMPDIR` は run の一時 dir）。snippet の本体は [measure-commands.md](measure-commands.md) の写しで、`B` を 3 rev にして
1 つの script にまとめた（run の artifacts の `measure.sh`。中身は下の通り measure-commands.md の C3・E1・E2・E3・C1・C2・C7b・C12 と、file ごとの数・
数えないものの数）。

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a; B1=$(git merge-base HEAD main)
bash measure.sh B0 $B0; bash measure.sh B1 $B1; bash measure.sh HEAD HEAD      # 各 3 分ほど
# measure.sh <name> <rev> の中身（要点。B=$(git rev-parse <rev>)、T=$TMPDIR/m-<name>）
#   C3・E1・E2・E3: measure-commands.md の snippet の $B を <rev>、$TMPDIR を $T にしたもの
#   file ごと:  git grep -o "self\.$p" $B -- crates/task-dispatch/src/ | grep -vE ':[^:]*((^|/)(tests?)(/|\.rs:)|_tests?\.rs:)' | cut -d: -f2 | sort | uniq -c
#   #[path]:    git grep -c '#\[path' $B -- crates/task-dispatch/src | awk -F: '{s+=$NF} END{print s}'
#   別名:       git grep -nE '<上の表の式>' $B -- crates/task-dispatch/src/ | grep -vE '<試験の除外>' | wc -l
#   C1:         /usr/bin/time python3 scripts/dev/repo-health/repo_health.py --repo . --base $B --head $B --policy-version 1 --out $T/health.json
#               と deps.cycle の rationale の取り出し（measure-commands.md の「数」の python に rationale の print を足したもの）
#   C2・C7b:    measure-commands.md の python をそのまま（引数 $B）
#   C12:        git show $B0:scripts/dev/source-size-report.py > $T/ssr.py; git show $B0:scripts/dev/source-size-report.toml > $T/ssr.toml
#               python3 $T/ssr.py --root . --config $T/ssr.toml --exclusive-classes --rev $B --format json --out $T/ssr.json

# B0 の checker で C2・C7b（A の before と比べる）
R=$TMPDIR/rh-B0; mkdir -p $R
git archive $B0 scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $R
#   measure-commands.md の C2・C7b の python の 'scripts/dev/repo-health' を $R/scripts/dev/repo-health にして、引数 $B0 で実行
#   （B1・HEAD に当てると profile_invalid で止まる）

# 閾値の count check（Task の受け入れ条件 0 の snippet。B= の所を B0 と B1 にして 2 回）
# (3)
git diff --quiet $B1 HEAD -- crates/task-api crates/task-core crates/task-ops; echo $?
git diff --name-only $B1 HEAD -- crates/ | cut -d/ -f2 | sort -u
git diff --name-status $B1 HEAD -- crates/
git diff --name-only $B0 $B1 -- crates/task-core crates/task-api crates/task-ops
# 数えないもの
diff <(git ls-tree -r --name-only $B1 crates/task-dispatch/src) <(git ls-tree -r --name-only HEAD crates/task-dispatch/src)
git diff $B1 HEAD -- crates/task-dispatch/src ':!crates/task-dispatch/src/dispatcher/tests' | grep -E '^\+\s*(pub(\([a-z]+\))? )?(struct|enum|trait|type) '
# (6) の script の同一性
git diff --quiet $B0 $B1 -- scripts/dev/source-size-report.py scripts/dev/source-size-report.toml; echo $?
```
