---
title: R5 bundle-api-a — task-api の CoS・案件試験の束ね
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# R5 bundle-api-a — task-api の CoS・案件試験の束ね

完了日: 2026-11-10

WorkUnit `bundle-api-a`（段 bundle）。task-api の CoS・chat・案件の結合試験のうち、`inventory.json` で leaf が `bundle-api-a` の 18 target を 3 つの束（`crates/task-api/tests/<bundle>/main.rs`）にした。試験関数の中身は変えていない。基準は `$CELERIS_WU_BASE`。

## 変更
- 件名「phase 1: bundle task-api CoS and project integration targets」の commit — 前回の試行（reclass の前）の commit を cherry-pick した。このときは 21 target を 4 束（`cos_delivery_suite_a`・`_b`・`_c`・`routing_budget_suite`）にしていた。
- 件名「R5 bundle-api-a: reclass で r6 になった chat_stream・cos_chat_attach_pin・cos_operations を元の path に戻し、束を inventory の cos_delivery_suite_a・_b・routing_budget_suite に組み直す」の commit — reclass で r6 になった 3 target を `git mv` で元の path に戻し、中身を base と同じにした（`mod common;` に戻した）。束は reclass 後の `inventory.json` と ADR の表のとおりに組み直した。`cos_delivery_suite_c` は bundle-api-b の束になったので、この branch では作らない（`main.rs` を消した）。
- この file（前回の記録を下敷きにした）。

束（ADR [2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) の表と同じ）:

| 束 | 件数 | target |
|---|---|---|
| `cos_delivery_suite_a` | 8 | chat_attachments・cos_auth・cos_chat_attach_handoff_kb・cos_chat_triage_scenarios・cos_job_wait_extend・cos_operations_review_report・cos_ops_mutations・cos_ops_projects_cron_changes |
| `cos_delivery_suite_b` | 7 | cos_ops_projects_cron_docs・cos_ops_registry・cos_plan_conforms_guards・cos_review_rework・cos_routine_answers・cos_triage_override・general_cos_consult |
| `routing_budget_suite` | 3 | project_brief・project_wake_human・project_wake_human_chat |

動かしていない target: r6 の chat_api・chat_stream・cos_chat_attach_pin・cos_integrate_task・cos_operations・cos_triage・project_controller（地図が path で参照する）、keep の cos_live_fix_d1・cos_live_fix_d2（file 名に数字）・cos_ops_ops_surface・cos_plan_conforms（1 件だけの組）・project_cos_guard・project_plan・project_roadmap（名前で呼ばれる suite）、adjust-api の cos_operations_domains・cos_ops_admin_config・cos_ops_projects_cron・project_auto_integrate（R3 と競合）。

## 実装前 brief
1. 根本原因: 同じ fixture（`tests/common/mod.rs`）を使う試験が別々の target になり、target ごとに common と依存の link が走る。
2. 責務 owner: `crates/task-api/tests` の target 構成。地図の task-api HTTP API・CoS チャットの行が指す target は r6 として動かさない。
3. 既存の再利用先: `tests/common/mod.rs`、`inventory.json` の leaf と束、前回の試行の commit。
4. 選択肢: (a) 前回の 4 束のまま — r6 の 3 target を動かし `arch-map.wrong_path` で落ちるので採らない。(b) reclass 後の inventory の 3 束に組み直す — 採った。全件を 1 binary にまとめる案は ADR D1・D2 の上限に反するので採らない。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 担当 target の旧い path と、file ごとの `mod common;` 宣言（束の `main.rs` に 1 回だけ置く）。
7. 不変条件: 試験関数・表の case・不変条件の試験が base と同じ。keep・r6 と数字を含む file は元の path に残る。`cases.py`・`classify.py --verify-leaf`・`cargo nextest list` で確かめた。

## 証拠（提出時の HEAD で実行）
- 条件 0: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf bundle-api-a agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0。「18 moved target(s) of the leaf, 262 keep/r6 target(s) checked in place, 0 error(s)」。
- 条件 1: `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$CELERIS_WU_BASE"` → exit 0。試験関数 855 → 855、表の行 757 → 757、名前で追った移動 102、不変条件の試験 277（42 file）、removals 0 件。
- 移動した 18 file を base の中身と比べた（Python の difflib）→ 各 file の差は `mod common;` → `use crate::common;` の 1 行だけ。戻した r6 の 3 file は base と同じ。
- `cargo nextest list -p task-api --message-format json` → base・HEAD とも exit 0。試験 878 → 878、binary 114 → 99。(target 名::試験名) の組は base と HEAD で一致した。base は同じ worktree で `crates/task-api/tests` を base の tree に戻して取った（取った後に `git reset --hard HEAD` で戻した）。
- `cargo nextest run -p task-api -E 'binary_id(task-api::cos_delivery_suite_a) | binary_id(task-api::cos_delivery_suite_b) | binary_id(task-api::routing_budget_suite)'` → exit 0、102 passed、0 skipped。
- `cargo test -p task-api --test cos_delivery_suite_a --test cos_delivery_suite_b --test routing_budget_suite`（1 process の中で並行に実行）→ exit 0、42・36・24 passed。D3 で束から外す target は無かった。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（「repo_health_gate: all checks passed」）。前回の `arch-map.wrong_path` 3 件は出ない。
- 条件 2: `bash scripts/dev/test-affected.sh` → exit 0（18 分 38 秒）。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web。nextest 5860 tests run: 5860 passed、22 skipped。web は tsc -b が通り、vitest 104 file・712 tests passed。health gate は verdict=pass。
- 条件 2: `cargo clippy --workspace -- -D warnings` → exit 0（1 分 18 秒）。`cargo fmt --all -- --check` → exit 0（出力なし）。
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`（`git diff --no-renames --name-only "$CELERIS_WU_BASE" HEAD` の 40 path と未追跡 path を標準入力）→ exit 0、「write-set R5: ok」。`sh "$CELERIS_WU_SCOPE_PATHS"` の出力は束の 18 file・3 つの `main.rs`・この file だけ。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 targets、20 bundles、0 error）。inventory は変えていない。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 各 exit 0。

## 未解決
無し。前回の plan_issue（r6 の 3 target を動かして `arch-map.wrong_path` で落ちた件）は reclass で解け、この leaf では 3 target を元の path に戻した。

## 提案
無し。ADR の束の表と実装は同じなので、ADR の付記は足していない。
