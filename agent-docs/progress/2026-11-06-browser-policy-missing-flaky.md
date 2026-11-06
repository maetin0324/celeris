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
worker が tick に間に合わない状況を、待つ tick を 1 回（`d.tick()` のみ）に減らして作った。修正前の主張のまま
`left: ["dispatch"] right: ["dispatch", "browser_prerequisite"]` で落ちる（実 release gate と同じ失敗）。
tick 回数を 1 に戻した後は 5 tick で通る（負荷の無い環境では 5 tick で足りるため偶発）。

## 修正
`tick_until`（browser_ledger_gate.rs）を追加。pred が真になるまで tick を駆動し、上限は壁時計 60 秒。
試験は status == Blocked を待ち、その後 `tick_n(5)` で infra_requeue が続かないことを確かめる。主張（attempts 不変・
browser_prerequisite で止まる・adapter 未呼び出し）は弱めていない。他の `tick_n` 利用は gate が dispatch 前に同期で
止める試験で、worker の結果を待たないので対象外。

## 検証
提出時の HEAD で `cargo nextest run -p task-dispatch browser_policy_missing browser_ledger_gate` 9 件通過。
