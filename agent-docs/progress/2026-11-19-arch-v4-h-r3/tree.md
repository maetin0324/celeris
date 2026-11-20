---
title: アーキ v4 H R3 tree — tree_units.rs の reconcile_tree_units の鏡の後の unit の読みを 1 回化
tasks: [01M7THYAPKESR1BG6BQZPJ749P]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# tree（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- `crates/task-dispatch/src/dispatcher/tree_units.rs` の self.store を 1 つ減らした（C3 56 → 55、E1 63 → 62）。self.config は 19（E1 20）のまま。
  減らしたのは [ADR dispatcher-narrow-ops-round2](../../adr/2026-11-19-dispatcher-narrow-ops-round2.md) の表が「1 回化」とした
  `reconcile_tree_units` の 3 か所（基準の 996・998・1001 行）だけ。ADR の見込み（store −1 / −1）と同じ。
- file の行は 1353 → 1351（−2）。新しい関数・型・helper・`task_ops` の関数・引数は足していない。他の file は変えていない。
- `sh agent-docs/progress/2026-11-19-arch-v4-h-r3/measure.sh shrink crates/task-dispatch/src/dispatcher/tree_units.rs` は exit 0
  （出力は 4.）。

## 1. 変えた行

`reconcile_tree_units`（開いた task unit を持つ task ごとの照合）の「1. 子の状態の写し」の後。

- 基準: 鏡（`work_units_apply`）の後に `let units = if changed && hold.is_some() { 読む } else if changed { 読む; promote_newly_ready; 読む } else { units };`
  の if の連鎖で、`self.store.work_units_for(task_id)` を 3 回書いていた。
- 変更後: 鏡の前の読み（`// 1. 子の状態の写し。` の直後）を `let mut units` にし、鏡の後を
  `if changed { units = 読む; if hold.is_none() { promote_newly_ready; units = 読む; } }` にした。書く参照は 2 つ。コメント 2 行は残した。

道筋ごとの store の読みと書き込みの前後（D4）:

| 道筋 | 基準の読み | 変更後の読み | `promote_newly_ready` との前後 |
|---|---|---|---|
| `changed` かつ人の gate が止めている（`hold` が Some） | 1 回 | 1 回 | 呼ばない（同じ） |
| `changed` かつ止めていない | 2 回（読む → promote → 読む） | 2 回（同じ順） | 同じ |
| `changed` でない | 0 回（鏡の前の `units` を使う） | 0 回（同じ） | 呼ばない（同じ） |

transaction・event・lease・書き込みの回数と順序・失敗時の扱い（`?`）は変えていない。後の「2. 子 task の生成」が使う `units` の値は
どの道筋でも基準と同じ（鏡の前の値か、同じ時点で読み直した値）。

## 2. 使った操作

- store の既存の読み `work_units_for` と、同じ file から呼ぶ既存の method `promote_newly_ready`（`work_units.rs`）。どちらも基準のまま。
- ADR D1 (b) の「排他の分岐に並ぶ同じ読みを分岐の後の 1 か所にする」形。D1 (a)（既存の owner の操作への置き換え）で減る参照は、
  ADR D2 のとおりこの file に無かった。

## 3. 残した参照と理由

WorkUnit の objective が挙げた関数について、ADR の表（「4 file の参照の表」の tree_units.rs）の判断を確かめた。どれも残した。

| 関数 | 残した理由 |
|---|---|
| `check_tree_liveness` | `tree`（enabled・`liveness_timeout_secs`）は既に先頭の 1 回の読み。`execution_plan_active` は `is_tree_node` と重なるが、1 回にすると `is_tree_node` の判定を複製する（規約 2）。events・`decision_path`・追記・通知は節点ごとの別の読み書き |
| `worker_decisions` | `limits` の読み・`work_units_for`（WU のときだけ）・`decisions_list` はどれも既に 1 回。呼び出し元は高 risk の `worker_finish.rs` で、引数を変えない（D3） |
| `effective_tree_limits` | `task_ops::decision::limit_allowances` の 1 回と設定の 1 回。owner の操作 1 回で、置き換えても数は変わらない |
| `replan_limits` | events の 1 回の読み |
| `effective_max_replans` | `max_replans` と `tree.enabled` は別の値。`get` と `limit_allowances` は別の読み。呼び出し元（`replan_limits`・`raise_node_replan_limit`・`replan_exhausted_ask`）が task id しか持たないので task を渡せない（D3） |
| `raise_node_replan_limit` | `work_units.rs` の `replan_gate` が確認なしで呼ぶので、中の enabled・`is_tree_node` の確認は外せない。未回答の確認は節点（task id）で絞る。`task_ops::tree::open_decision` は節点で絞らないので置き換えられない。中の `effective_max_replans` の読み直しを避けるには上限の計算を複製するか引数を変える（規約 2・D3） |
| `tree_limit_hold_for` | `tree.enabled` を `effective_tree_limits` の結果（`TreeLimits.enabled`）で見ると、木が無効の道筋でも `limit_allowances` を読み、読みの回数と前後が変わる（D4）。`tree_counters`・`open_decision`・`decision_path`・追記は別の owner の操作と書き込み |
| `root_plan_approval` | 上と同じ。enabled の確認を `effective_tree_limits` の後へ動かすと、無効の道筋に store の読みが増える（D4） |
| `tree_plan_checks` | `check_task_unit_repos`・仮の子の `build_child_task`（roles・genres）・`cross_department_questions`。`task_ops::delegate::plan_children` は delegate の spec を受け、unit の spec を受けない（入力が違う） |
| `handle_child_infra_failure` | `reconcile_tree_units` の子の生成と同じ並び（`answered_decisions` → `build_child_task`）だが、まとめるには新しい private の helper が要る（D5。ADR の「提案」）。events の読みは鏡の書き込みの後で、呼び出し元の読みと時点が違う |

`reconcile_tree_units` の残り（`events_for` の 925 は鏡の書き込みの前、1081 は子を作る書き込みの後の読み。`max_parallel_child_tasks` は
1 回の読み）も ADR の表のとおり残した。

## 4. 検査の結果（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、measure.sh の base は [base.txt](base.txt)）

- `sh agent-docs/progress/2026-11-19-arch-v4-h-r3/measure.sh shrink crates/task-dispatch/src/dispatcher/tree_units.rs` → exit 0。出力:
  `shrink: ok files 138 -> 138 paths 1 file store+config 83 -> 82`（crate の非試験 file の集合は同じ、どの file の E1 も増えていない）。
- file ごとの数（base.txt の commit と作業ツリー）: C3 store 56 → 55、C3 config 19 → 19、E1 store 63 → 62、E1 config 20 → 20、行 1353 → 1351。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  nextest 3938 tests run: 3938 passed、11 skipped。repo-health の verdict=pass、new=0・worsened=0）。
- `cargo nextest run -p task-dispatch -E 'test(/cex_|tree|structure_guard|store_access_guard/)'` → 163 passed。名前に `cex_` を含む反例の試験 40 本
  （ADR の 12 本を含む）と `store_access_guard_work_units`・`store_access_guard_cos_chat`・`store_access_guard_detects_each_violation` を含む。
- 変えた path は `crates/task-dispatch/src/dispatcher/tree_units.rs` とこの記録だけ（`sh "$CELERIS_WU_SCOPE_PATHS"`）。
  `structure_guard.rs`・`cex_*.rs`・試験の file は変えていない。

## 5. 未解決

- 無し。4 scope の metric と E2・C12 の after は WorkUnit `after` が測る。
