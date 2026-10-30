---
title: plan_conforms の人へ回す決定的な条件と危うい段の種類
tasks: [01M655VRMSE9TV5FHWXJFKSP77]
status: blocked
updated: 2026-10-30
---
# plan_conforms の人へ回す決定的な条件と危うい段の種類（WU guards）

完了日: 2026-10-30

## 変更
- ADR `agent-docs/adr/2026-10-30-plan-conforms-human-guards.md`（2026-10-30-cos-plan-conforms-approve の付記）。
  plan_approval を捨てて plan_conforms に一本化した経緯、G1 kind の 6 種、G2 条件 (1)〜(4)。
- `crates/task-api/src/cos/basis/plan_conforms.rs`: `StageRiskKind`（billing・node_hours・production・external_publish・destructive・secrets_permissions）を
  `GuardedStage.kind`（必須）に足した。`needs_human` が (1) acceptance の human check → 403 `cos_human_required`、
  (2) `review_human:`・`decisions:` 以外の承認理由 → 422 `cos_basis_plan_needs_human`、
  (4) 前の版の承認（最後の `PlanGateDecided{approve}`、無ければ task 作成）以降の objective・acceptance の `Edited` → 422 `cos_basis_plan_needs_human` を拒む。
  (3) 未回答の決定の 422 `cos_basis_invalid` の detail に `key (kind)` を出す。
- `crates/task-api/src/cos/basis.rs`: `within_approved_frame` から `plan_conforms::needs_human` を 1 行で呼ぶ。`BASIS_PLAN_NEEDS_HUMAN` を足した。
- 試験 `crates/task-api/tests/cos_plan_conforms_guards.rs`（7 件）。`cos_plan_conforms.rs`・`cos_chat_triage_scenarios.rs` は basis に kind を足し、task の acceptance を reviewer にした。
- API schema・web の生成型を作り直した。`docs/api/v1/api.md` の (f) に kind と新しい拒否を書いた（policy_version は変えない）。
- `config/skills/cos-inbox-triage/SKILL.md` と `crates/celeris/tests/credential_wait_notify.rs` の変更は、この WU の範囲 check（crates/task-api・agent-docs・docs/api/v1・web/api/generated だけ）に掛かるので打ち消した。SKILL.md の (f) の行は WU policy-close で書く。

## commit
- phase 1: plan_conforms に人へ回す決定的な条件と危うい段の種類（kind）を足す
- phase 1: credential_wait_notify の試験が本番 host の release 置き場を読まないようにし、進捗を書く
- phase 1: 範囲外の SKILL.md と credential_wait_notify の変更を打ち消す（範囲 check）

## 検証（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p task-api --test cos_plan_conforms_guards --test cos_plan_conforms --test cos_chat_triage_scenarios`: 7 + 4 + 3 passed
- `bash scripts/dev/test-affected.sh`: exit 100。nextest 4788 件中 4787 passed・1 failed（`celeris::credential_wait_notify` の通知数 2≠1。本番 host の release 置き場に verify 済みで未昇格の release があり、通知が 1 件混ざる）。vitest 101 files・697 passed
- 範囲 check（`$CELERIS_WU_SCOPE_PATHS`）: 範囲外の path なし
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0（差分なし）
- `sh scripts/dev/progress-index.sh --check`: ok

## 未解決事項
- 範囲 check と test-affected が両立しない。credential_wait_notify の試験の直し（config に `[selfdeploy] releases_dir`・`repo`・`detach=inline`）は crates/celeris にあり、この WU の範囲外。直さないと本番 host の状態次第で test-affected が落ちる。plan_issue で計画に申告した。
- 前回の task review で `cargo test --workspace` の `cap_probe_real_daemon`（probe の task が 404）が落ちていた。この WU の変更とは別の箇所で、未調査。

## 提案
- 計画の段に構造化した危うさの欄が無いので、kind は CoS の申告のまま。planner の出力に kind を持たせると API が申告漏れを検査できる（別 ADR）。
