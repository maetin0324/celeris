---
title: "健全性v2 R6 e2e-scope: 決定 r6-plan-e2e に従い R6 の write set に e2e の plan_scenarios.rs を足す"
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 e2e-scope

完了日: 2026-11-15

結論: `agent-docs/progress/2026-11-07-health-v2-b0/slices.json` の R6 の `write_set` に正規表現
`^tests/e2e/tests/plan_scenarios\.rs$` を 1 つだけ足した。slices.json の差分は追加の 1 行だけで、R6 の他の欄・
他の slice・write-set unit が足した 4 つの正規表現は変えていない。理由と決定の経緯は
[ADR 2026-11-07 の付記 r6-plan-e2e](../../adr/2026-11-07-health-v2-baseline.md) に書いた。

## 決定
- 決定 r6-plan-e2e の答えは a（推奨どおり）: R6 の write set に `tests/e2e/tests/plan_scenarios.rs` を足し、
  e2e の 2 つの Plan task の作成を `env.store.create_task` に替えて、`celerisctl plan` と `task_ops::plan::create_plan` を消す。
- 答えたのは運用者で、人ではない。人が 2026-11-15 に運用者の Claude Code の対話 session で
  「R6が止まっているのにも対処してください」と指示し、運用者がそれを受けて a を選んだ。
- e2e の書き換えと `celerisctl plan` の撤去は legacy unit が行う（この unit は範囲の規則だけを変えた）。

## commit（件名）
- 「health-v2 R6 e2e-scope: 決定 r6-plan-e2e（a）で R6 の write_set に tests/e2e/tests/plan_scenarios.rs を足す」
  — slices.json と ADR の付記だけの 1 commit。他の変更を混ぜていない。
- この進捗 file は別の commit にした。

## 証拠（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py agent-docs/progress/2026-11-07-health-v2-b0/slices.json`
  → exit 0、「slices-check: ok（工程 11、capability 41、base 5923a76c9aa0）」。
- `echo tests/e2e/tests/plan_scenarios.rs | python3 …/slices-check.py --write-set R6 …/slices.json`:
  変更後 → exit 0（「write-set R6: ok」）。変更前の slices.json → exit 1（1 件範囲外）。
- 意図しない path（`tests/e2e/tests/other.rs`・`tests/e2e/tests/plan_scenarios_rs`・`tests/e2e/tests/plan_scenarios.rs.bak`・
  `tests/common/mod.rs`）は変更後も範囲外（exit 1、4 件）。
- 変更前と変更後の slices.json を JSON として読み比べた: R6 の `write_set` 以外の全欄と他の 10 工程が一致し、
  変更前の `write_set` の 13 個は同じ順で残る。追加は上の 1 つだけ。
- 全 11 工程の `write_set` に `tests/` で始まる正規表現は変更前に無かった（兄弟と同じ file を同時に触らない）。
- `tests/e2e/tests/plan_scenarios.rs` の 2 つの試験（`celerisctl_plan_generates_children_that_complete_after_human_approval`・
  `invalid_plan_is_retried_once_with_prior_review`）が `celerisctl plan` を呼ぶことを grep で確かめた。
- `sh scripts/dev/check-doc-links.sh` → exit 0、`sh scripts/dev/check-adr-numbers.sh` → exit 0（447 files）。

## 未解決
- 無し。

## 提案
- 無し。
