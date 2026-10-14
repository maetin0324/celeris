---
title: "live 昇格で走行中の評価試行を error にしない（handoff-eval）"
tasks: [01M4WYKAQTQSKBNB2P341MMHCY]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# live 昇格で走行中の評価試行を error にしない

設計は [ADR 2026-10-14-eval-verify-isolation](../adr/2026-10-14-eval-verify-isolation.md) D3。実装の突き合わせは
同 ADR §5 付記。葉のメモは [owner-record](2026-10-14-eval-verify-isolation/owner-record.md)・
[drain-run](2026-10-14-eval-verify-isolation/drain-run.md)。

| 葉 | 中身 | commit |
|---|---|---|
| owner-record | migration 0078 `eval_trials.instance_id`、claim で持ち主を記録、`reconcile_eval_trials` の持ち主判定（`eval_running_recovery`）、回復 error の `runs` 行を cancelled | `383f6f41` |
| drain-run | `in_flight` に評価 run、draining は採点しない、`reconcile_ownerless_eval_run`、`abort_all_runs` が `abort_eval_runs`、名前固定の試験 2 件 | `95ef97d3` |
| closeout | 全体検査、ADR 付記、architecture-map の行（migration 0078・ADR D3） | この commit |

## 証拠（closeout、HEAD `eb463ffe` + 文書のみ）

- `bash scripts/dev/test-parallel.sh`: exit 0。`passed 5103, failed 0, ignored 13`（nextest 170 binaries、doctest exit 0、userns null）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0
- `cargo test -p task-dispatch live_handoff_keeps_running_eval_trial`: `test result: ok. 1 passed`
- `cargo test -p task-dispatch eval_run_row_not_left_running_after_handoff`: `test result: ok. 1 passed`
- `sh scripts/dev/check-adr-numbers.sh`: exit 0

## 本番への移行

- schema: migration `0078_eval_trials_instance_id.sql` は昇格で自動に当たる（既存行は `instance_id` NULL）。
- config の変更・手での DB 書き換えは無い。既に残っている running の `eval` の `runs` 行（試行は `error`）は、
  この release が active になった最初の `reconcile_ownerless_runs` が `cancelled` で閉じる。

## 未解決事項

- なし。08:52Z に error になった試行の再実行は評価の運用（batch の再投入）で行い、この task の範囲外。

## 提案

- なし。
