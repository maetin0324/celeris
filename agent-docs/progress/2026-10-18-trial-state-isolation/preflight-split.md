---
status: done
completed: 2026-10-18
---
# preflight-split WU
- `account_dir_residue` 削除。`preflight::check_trial_state`（台帳で開始済みか判定 → 再開検査 / 新規検査）。ADR 付記参照。
- `harness_exec::trial_state_dir` を公開し Sandbox と preflight が同じ path 規則を使う。
- 試験: tests/preflight.rs に新規・再開・不一致（rep/case/arm/campaign/sha/account）・混入・lock 不一致・gate 前拒否。
- 既存 campaign_lock_e2e（非 sandbox の A が台帳行だけ持つ）は「state 不在なら可」の規則で通る。
- 未解決: 実 CLI 停止・再起動 e2e は cli-e2e WU。
