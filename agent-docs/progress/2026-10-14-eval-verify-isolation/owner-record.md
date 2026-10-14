---
title: "handoff-eval / owner-record: eval_trials.instance_id と回復の持ち主判定"
tasks: [01M4WYKAQTQSKBNB2P341MMHCY]
status: done
updated: 2026-10-14
---
# owner-record（ADR eval-verify-isolation D3.1・D3.4・D3.5 前半）

実装は commit `383f6f41`: migration `0078_eval_trials_instance_id.sql`、claim で instance id を記録、
`reconcile_eval_trials` は持ち主の生死（`orphan::holder_gone` と同じ定義）で判定し、回復で error にした試行の
`runs` 行を `run_index_finish` で cancelled（`interrupted: eval recovery: …`）に閉じる。旧い無条件 error の経路と試験は撤去。

## 未解決事項
- なし（drain 側は drain-run 葉）

## 提案
- `docs/architecture-map.md` の評価 run の試行の行に migration 0078（`0078_eval_trials_instance_id.sql`）と
  ADR 2026-10-14 eval-verify-isolation D3 を足す。この葉の許可範囲外のため closeout 葉で行う。
