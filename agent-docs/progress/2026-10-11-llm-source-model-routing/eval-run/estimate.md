---
unit: estimate
task: 01M4VAAKFJHRSVKZ3996NFKSKS
status: blocked
updated: 2026-10-14
---
# estimate: 実行直前の見積り — 保留（pilot やり直し待ち）

## 人が見る要点
- **承認用の表はまだ出していない。** pilot（batch 01M4W35QEMQGVV5ZSD0JJ61ZDD）は評価の仕組みの不具合（result.json の置き場が prompt と adapter で食い違う）で全試行が error。人の指示（2026-10-14 コメント）により、この batch の結果を見積り・根拠表に使わず、estimate に進まない。
- 修正 task 01M4W3PTCHJD4XQPZGT81WBTRM は 2026-10-14 再確認時点でも running（本番未反映）。estimate の check は run-plan.json を要するので、plan_issue で replan を求めた（pilot 再実施の段を estimate の前に置く）。
- 再開の条件: 修正が本番に入る → pilot を新しい batch でやり直す（計画に pilot 再実施の葉が要る）→ その実測で measured.json・dryrun-1.json・run-plan.json と承認表を作る。

## pilot の合否
- 不合格（全 error、仕組みの不具合。codex 側は作業を終えている）。

## 未解決事項
- pilot 再実施の葉を estimate の前に置く replan（決定 pilot-rerun で人に確認中）。
