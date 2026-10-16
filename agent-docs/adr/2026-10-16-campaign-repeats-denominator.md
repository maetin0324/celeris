---
title: "campaign: 独立反復と内部再試行の分離、分母は計画された試行総数"
tasks: [01M526E98AFXB02ZG468J789FB]
status: accepted
date: 2026-10-16
---
# ADR: campaign の反復（repetition）と分母

## 背景
`trial_no` は「同じ試行の内部再試行（attempt）」と「独立反復」を兼ねており、集計は各 problem の `trial_no` 最小の記録だけを見ていた（runner の summary は `trial_no == 1` だけ数えていた）。反復を足すと、同じ ID・同じ out を共有して再試行と混ざるうえ、失敗した反復が分母から落ちうる。

## 決定
- D1: manifest に `repeats`（u32、既定 1）を足す。1 のときは直列化しない（既存 manifest の hash は不変）。`plan()` は (arm, problem, rep) を `repeats` 回ぶん作る。新しいスケジューラは作らず、既存の plan → executor → ledger 経路にそのまま載せる。
- D2: 反復は `rep`（1 始まり）という別 ID。`TrialRecord.rep`・`TrialKey.rep`・`TrialRequest.rep` に持つ（欠けた古い記録は 1）。out は `trials/<arm>/<problem>/rep<k>`、台帳の冪等キーも rep ごとに別（rep 1 のキーは従来と同じ文字列）。`trial_no` は内部再試行の番号に専念する。
- D3: `validate` は `max_attempts_per_trial != 1` を Err にする（内部再試行を持たない。失敗した試行は再試行で救わず、そのまま分母に残す）。`repeats == 0` も Err。
- D4: 依存（前提 problem）は同じ rep どうしで判定する（rep k の依存先は前提の rep k）。
- D5: 集計の分母は計画された試行総数。各 cell は計画された (problem, rep) ごとに `trial_no` 最小の記録で判定し、failed / error / timeout / cancelled / blocked は当然、記録の無い (problem, rep) も `missing` として分母に残す。`n = problems × repeats`（C は subset × repeats）。aggregate・checker の再計算・runner の summary の 3 箇所とも同じ規則で、`trial_no == 1` 決め打ちを rep 対応にする。pass 率 = passed / n。
- D6: checker は rep が 1..=repeats の範囲外・(arm, problem, rep, trial_no) の重複を違反にする。
