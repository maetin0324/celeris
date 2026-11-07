---
title: browser_policy_missing_worker_rejection_is_not_infra_requeue の flaky 修正
tasks: [01M6S55NVP6BAKA3CNKP19H4QS]
status: done
updated: 2026-11-06
---
# browser_policy_missing_worker_rejection_is_not_infra_requeue の flaky 修正

完了日: 2026-11-06

## 原因
worker の拒否（`browser policy rejected`）は実 scheduler 上の worker task が返す。試験は `tick_n(&mut d, 5)`（約 50ms）だけ待って
transition を検査していたため、高負荷で worker の終了が 5 tick に間に合わないと
`["dispatch"]` != `["dispatch","browser_prerequisite"]` で落ちる。

## 決定的な再現（負荷なし）
試験専用フック `Dispatcher::test_worker_start_delay`（cfg(test)）を追加した。spawn した worker task が `run_worker` に入る前に
指定時間だけ sleep する。試験で 300ms を入れると、旧形（`tick_n(5)` ≒ 50ms 待って検査）では worker の拒否が届かず
`left: ["dispatch"] right: ["dispatch", "browser_prerequisite"]` で必ず落ちる。試験はこれを事前確認として
明示している（5 tick 後の transition が `["dispatch"]` のまま）。修正後の形は `tick_until`（Blocked まで）で待ち、遅延 300ms でも通る。
CPU 負荷は使っていない。

## 修正
- `tick_until`（browser_ledger_gate.rs）: pred が真になるまで tick を駆動、上限は壁時計 60 秒。
- 試験は status == Blocked を待ち、その時点で検査する。Blocked の後にさらに tick を回すと gate が解けて
  `browser_prerequisite_resolved` と再 dispatch が続く（遅延を入れて判明）ので、tick を足さない。attempts 不変・
  browser_prerequisite で止まる・adapter 未呼び出しの主張は弱めていない。
- 他の `tick_n` 利用は gate が dispatch 前に同期で止める試験で、worker の結果を待たないので対象外。

## 検証
提出時の HEAD で `cargo nextest run -p task-dispatch browser_policy_missing browser_ledger_gate` 9 件通過（遅延 300ms 入り）。test-affected・clippy・fmt は exit 0。

### 修正前に落ち修正後に通ることの確認（提出時の HEAD、attempt 3）
- 修正前の形: 遅延フック 300ms を残したまま、待ちを旧形（`tick_n(&mut d, 5)` の直後に検査）へ一時的に戻して
  `cargo nextest run -p task-dispatch browser_policy_missing_worker_rejection` → exit 100、
  `browser_policy_missing.rs:223` で `left: ["dispatch"] right: ["dispatch", "browser_prerequisite"]`（release gate と同じ失敗）。一時変更は戻した。
- 修正後の形: 同じコマンドを 5 回 → 5 回とも 1 passed。`browser_policy_missing browser_ledger_gate` は 9 passed。
- `bash scripts/dev/test-affected.sh` exit 0（3461 passed、11 skipped）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
