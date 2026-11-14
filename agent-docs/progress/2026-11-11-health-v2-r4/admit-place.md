---
title: "健全性v2 R4 admit-place: write-set・cluster・作業場所・disk の起動可否の順序と組み合わせを dispatch_admission::admit へ移す"
tasks: [01M7BYQNFCE4Z8PFD95T6VQ37J]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 admit-place

完了日: 2026-11-14

## 概要

- `dispatch_admission::admit` を、admit-gate が移した browser の前提の後ろへ広げた。予算の上書きを task に当てる位置・write-set の
  gate・クラスタの解決・cooldown・並列度・接続の確認と自動接続・作業場所の dir・WU の用意の backoff・WU の worktree・disk の見込み・
  scope env の順序と組み合わせと止め方は admit にある（ADR 2026-11-14 D5 の 17〜24）。
- `dispatch_one` は admit が求めた fact を既存の関数で集め、止め方を適用する。`warn_unroutable_once`・`mark_unroutable`・
  `block_task_missing_cluster_tool`・`note_cluster_waiting`・`set_cluster_connected`・`mark_cluster_unavailable`・
  `prepare_work_unit_workspace`・`on_work_unit_prepare_failed`・`note_dispatch_wait` は dispatch_one に残り、admit が求めたときだけ
  移行前と同じ順と条件で呼ぶ。
- `dispatch_run.rs` に `>= spec.concurrency`・`until > now`・`Admission::Wait(` の字面は無い（コメントにも無い）。
  `second_pass` は task の lease の引数（admit-launch の範囲）にだけ残る。
- `dispatch_one` の行数: 移行前 1022 行（34〜1055 行）→ 移行後 1018 行（34〜1051 行）。`dispatch_run.rs` は 1653 行 → 1652 行。
  `dispatcher` module の本体の行（repo health の size.responsibility-growth が数える行）は `dispatch_run.rs` で 1428 → 1428。
- `dispatch_admission/tests.rs` の試験は 8 本 → 13 本。ADR 2026-11-11 と `cex_` の試験と dispatcher の既存の試験は変えていない。

## 実装前 brief

1. 根本原因: `dispatch_one` の write-set から scope env までの部分は、gate を見る順序（write-set → クラスタ → dir → WU の用意 → disk →
   scope env）と止め方の組み合わせ（どの止め方でクラスタを使えないと記録し、どれでクラスタの待ちに数えるか）を、副作用と交互に
   書いていた。順序の入れ替えを検出する試験が無い（既存の試験は 1 つの gate だけを止める）。
2. 責務 owner: 順序と組み合わせは `dispatch_admission`（ADR 2026-11-11 D3・ADR 2026-11-14 D1）。個々の判定と副作用は既存の owner
   （`ExecutionCoordination`・`resolve_cluster`・`ClusterRuntime`・`HostMaintenance` の disk の gate・WU の用意の関数）と、
   それを呼ぶ `dispatch_one`。
3. 既存の再利用先: `admit_write_set`（`WriteSetAdmission`）、`resolve_cluster`（`ClusterResolution`）、`cooldown_until`・
   `cluster_in_use`・`connected`・`try_auto_connect_cluster`・`note_key_auth_attempt`・`set_cluster_connected`・
   `mark_cluster_unavailable`・`note_cluster_waiting`、`pin_task_root`・`task_workspaces_for`・`task_dir`、`wu_prepare_backoff`・
   `prepare_work_unit_workspace`・`on_work_unit_prepare_failed`・`clear_wu_prepare_failures`・`wu_base::scope_env`、`judge_run`。
   試験は admit-gate の `drive`（admit が求めた fact だけを写す）を広げて使う。
4. 選択肢: (a) admit-gate と同じく fact を 1 つずつ求める（採用。今は呼ばれない接続・作業場所の用意を起こさない）。
   (b) 予算の上書きを `Launch` で当てる（write-set 以降の関数が tier と予算を読まないことを全部の呼び先で確かめる必要があり、
   `block_task_missing_cluster_tool` のように task を渡して記録する経路もあるので不採用。`RunOverride` の要求で今と同じ位置に当てる）。
   (c) クラスタの待ちの文字列と理由を dispatch_one が作る（dispatcher module の本体の行が base を超えるので不採用。admit-gate の
   `TaskWait::Capacity` と同じく、待ちの値は admit が fact から作る）。
5. 新しい状態・分岐・fallback・port: 状態と fallback は無し。新しい型は入出力の値（`AdmissionPlace`・`AdmissionCluster`・
   `ClusterCooldown`・`ClusterFact`・`WorkUnitFact`、`AdmissionFact` と `AdmissionStop` の variant）だけ。`AdmissionCluster::of` は
   `ClusterResolution` を id・host・並列度に写すだけの constructor（`AdmissionTaskValues::read` と同じ扱い）。dispatch_one の
   loop には「admit が先に求めた fact が揃っていなければ起こさない」`return Ok(false)` が 3 か所ある（クラスタの fact でクラスタの
   設定が無い、WU の fact で WU か dir が無い、起動の前に dir が無い）。admit の順（試験で固定）では起きない。
6. 撤去対象: dispatch_one の中のクラスタの比較（cooldown の期限・並列度・接続）、止め方ごとの分岐、`run_key` の変数と
   `cluster_wait` の関数、予算の上書きの `Launch` の欄（`tier`・`budget`）。`on_work_unit_prepare_failed` の `second_pass` は
   admit の止め方の `forced` に置き換えた。
7. 不変条件: 権限と予算の判定を減らさず順序を変えない。並列 WU の 2 本目以降も置き場所の gate をすべて見る。確かめる試験は
   `dispatch_admission/tests.rs` の 13 本、`cex_` の試験、既存の dispatcher の試験（test-affected）。

## 順序の等価性で確かめたこと

- 移行前の 262〜448 行と同じ順で fact を求め、先の gate が止めたら後ろの関数を呼ばない。cooldown → 並列度 → 接続の確認 →
  自動接続 → 接続済みにする、の順。cooldown と接続の失敗は `note_cluster_waiting` を呼び、並列度は呼ばない。接続の失敗は
  `mark_cluster_unavailable` → `note_cluster_waiting` → 待ちの記録の順。
- 予算の上書きは browser の前提の後、write-set の gate の前（移行前と同じ位置）に `RunOverride` の要求で当てる。上書きの前の予算
  （`original_task_budget`）はその時点で控える。
- dispatch_one は loop の中で task の worktree を持ち、WU の worktree への差し替えは loop の後で行う（移行前と同じ値）。scope env は
  用意した WU の worktree があればそれで確かめる（移行前の差し替え後の worktree と同じ）。`task_worktree` は差し替え前の値。
- disk の見込みは planner run でないときだけ求め、`Go` の見込みは起動の後で `started` に渡す（admit の入力の欄から取り出す）。
  write-set の予約も同じく入力の欄から取り出して `reserve_write_set` に渡す。
- 設定に無いクラスタの warn: `resolve_cluster` は remote の作業場所にだけ `NotConfigured` を返すので、Local の作業場所の空の
  cluster id で warn する枝は消した（`warn_unroutable_once` は今と同じく必ず呼ぶ）。
- admit-gate の試験の直し方: 求める順を固定する 2 本（`admit_first_pass_asks_facts_in_the_dispatch_order`・
  `admit_second_parallel_unit_skips_only_the_first_pass_gates`）は、期待する列の後ろに置き場所の fact を足した。予算の上書きの値は
  `Launch` の欄の代わりに `RunOverride` の要求の値で確かめる（`admit_overrides_the_run_budget_by_run_kind` と 2 本目の WU の試験。
  確かめる tier と予算の値は同じ）。WU の値は `AdmissionUnit::WorkUnit { budget, phased }` に書き換えた。どれも弱めていない。

## 試験が順の入れ替えを検出すること（一時的に admit の本体を書き換えて確かめ、元に戻した）

- cooldown と並列度の順を入れ替える: 2 本が落ちた（`admit_place_facts_follow_the_dispatch_order`・
  `admit_cluster_cooldown_beats_concurrency_and_connection`）。
- disk の見込みを WU の worktree の前に置く: 2 本が落ちた（`admit_place_facts_follow_the_dispatch_order`・
  `admit_workspace_and_disk_stops_keep_the_dispatch_order`）。
- write-set の gate をクラスタの解決の後に置く: 4 本が落ちた。
- 自動接続の失敗をクラスタの待ちに数えない: 1 本が落ちた（`admit_cluster_connection_failure_stops_before_the_workspace`）。

## 変更（件名）

- `phase admit: write-set・cluster・作業場所・disk の起動可否の順序と組み合わせを dispatch_admission::admit へ移す`
- `phase admit: admit-place の進捗に検査の結果を書く`（文書だけ）

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- WU の check 4（純粋さと同名の関数の定義なしの grep、`admit`/`AdmissionInput`/`AdmissionProposal` が根の file にあること、
  `dispatch_admission::admit(` の呼び出し、lease・`spawn_worker(`・`WorkerStarted` が `dispatch_run.rs` に残ること、`second_pass` の
  条件式と planner の組み合わせの変数が無いこと、`>= spec.concurrency`・`until > now`・`Admission::Wait(` が無いこと、`#[test]` が
  9 本以上、`cargo nextest run -p task-dispatch -E 'test(cex_) | test(/^dispatch_admission::/) | test(structure_guard_)'`）:
  exit 0、49 passed、1809 skipped（`#[test]` は 13 本）。
- `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  3913 passed、11 skipped。
- WU の check 2（`cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check && bash scripts/dev/check-health.sh &&
  test -f <進捗> && sh scripts/dev/progress-index.sh --check`）: exit 0。check-health は verdict=pass、fail_grade 0、worsened 0。
  new 1 件は warning の `migration.remnant`（`host_maintenance.rs` の `legacy` の語。この unit は触っていない）。
- repo health を WU base と比べる（celeris の `celeris:repo-health/1` と同じ基準）:
  `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$CELERIS_WU_BASE" --head HEAD --policy-version 1`: exit 0、
  new 0、worsened 0。`dispatch_one` の関数の行 1022 → 1018、`dispatcher` module の本体の行 31662 → 31662（どちらも existing）。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を計画の許す path と比べる）: exit 0、範囲外 0。変えた path は
  ADR 2026-11-14・`dispatch_admission.rs`・`dispatch_admission/tests.rs`・`dispatch_run.rs`・`docs/architecture-map.md`・この進捗。
- `git diff --name-only "$CELERIS_WU_BASE" -- agent-docs/adr/2026-11-11-dispatcher-state-ownership.md crates/task-dispatch/src/dispatcher/tests crates/task-dispatch/tests`:
  0 件（ADR 2026-11-11 と `cex_` の試験と既存の dispatcher の試験は変えていない）。

## 未解決事項

- 無し。

## 提案

- 無し。
