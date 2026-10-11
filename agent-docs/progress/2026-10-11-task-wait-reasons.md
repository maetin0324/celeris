---
title: "web のボード・一覧・詳細の木で task の待ちの理由（子・依存・WorkUnit の依存）を出す"
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: running
updated: 2026-10-11
---

# 待ちの理由（task-wait-reasons）

- 決定: [ADR 2026-10-11-task-wait-reasons](../adr/2026-10-11-task-wait-reasons.md)
- 共通規則: `/local/celeris/data/workspaces/01M4NAKDCCS5XYAAJ518MVZ134/artifacts/plan-common.md`
- 葉ごとの記録: `2026-10-11-task-wait-reasons/<unit key>.md`

## 段と状態

| 段 | 葉 | 状態 |
|---|---|---|
| design | adr | 済（ADR を書いた。コードは無し） |
| api | core → api | 未 |
| web | web-list ∥ web-tree | 未 |
| close | verify | 未 |

## 未解決事項

- 一覧の応答で非終端 task の `work_units_for` を読む負荷（ADR Consequences）。api 葉で計測する。

## 提案

- （なし）
