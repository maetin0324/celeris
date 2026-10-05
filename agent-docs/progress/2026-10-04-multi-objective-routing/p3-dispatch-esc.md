---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: dispatch-esc
status: done-in-branch
completed: 2026-10-05
---

# Phase 3 dispatch-esc: retry lane の選択に軌跡 escalation を配線し audit に残す

`dispatch_run::decide_lane` の retry 判定を `EscalationPolicy::decide`（直前の失敗だけ見る）から `decide_trajectory`（events の区間内の軌跡を見る）に替えた。上げた/上げなかった決定は `EscalationAudit` として `RoutingRecord.escalation` に載る（task の run、`attempts > 0`）。WU の run と計画 run は `None` のまま（WU の lane は task の lane を上限にする既存規則）。

試験 `routing_trajectory_escalates_one_lane_and_respects_caps`（dispatcher/tests/routing_and_quota.rs）で次を固定した。

- 同 lane の品質失敗（review・検査）2 回で cheap → standard の 1 段だけ。
- 供給失敗（requeue）は数えず据え置く。
- 上げた lane では数え直す（1 回の失敗は据え置き）。
- 総試行 4 回で止まる（max_retries 3 の task）。
- reopen で区間を切り、区間 ID `reopen:<index>` の下で数え直す。
- 組織の天井（cheap まで）を超えて上げない。決定の理由は `ceiling` を含む。
- 人の明示 lane は上げず、理由 `explicit lane` を audit に残す。
- tick で実際に走らせると `RoutingDecided` の record の `escalation` が `requested=Cheap, selected=Standard, counted=2` になる。

## 検証

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p task-dispatch -E 'test(routing_trajectory) \| test(repeated_review_failures) \| test(lane_policy_decides) \| test(routing_context)'` | 5 passed（新試験 1、既存の回帰 `repeated_review_failures_escalate_the_retry_lane_one_step` は変更なしで通過） |
| `bash scripts/dev/test-parallel.sh`（整形前に実行） | exit 0。nextest Summary 3979 tests run: 3979 passed (1 slow), 12 skipped。doc test 0 failed、1 ignored |
| `cargo nextest run -p task-dispatch`（整形後） | 688 tests run: 688 passed, 0 skipped |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo clippy -p task-dispatch --all-targets -- -D warnings` | exit 0（試験コードも検査） |
| `cargo fmt --all -- --check` | exit 0（整形は `dispatch_run.rs` と `routing_and_quota.rs` のみ） |
| `git diff --check` | exit 0 |
| `sh scripts/dev/check-doc-links.sh` | exit 0 |

## 未解決

- 閾値（品質失敗回数・総試行上限・天井）は既定値のまま。設定ファイルからの読み込みは daemon-wire の範囲。
- `BudgetState` は `Ok` 固定。quota 層の defer を軌跡に入れる経路は無い（予算切れは履歴の `BudgetExhausted` で止まる）。
- WU の retry では上げない（E3 の方針を維持）。WU 単位の軌跡が必要になったら別 unit で決める。
- reward/outcome の追記（`routing_outcome_recorded`）と audit API の投影は dispatch-outcome・audit-api の範囲。

## 提案

- WU の軌跡 escalation を入れるなら、`task.attempts` ではなく WU の `runs` と `WorkUnitChecksFailed` を events から数える（`attempt_history` の WU 版）。task の天井と WU の天井を `EscalationThresholds` の `task_ceiling` / `work_unit_ceiling` に渡す形にそろえる。
