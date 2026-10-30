---
title: おかしなところの無い計画の承認を CoS が定型の代答で approve する
tasks: [01M655SMDYDX81M6QF2N4YP5C1]
status: done
updated: 2026-10-30
---
# おかしなところの無い計画の承認を CoS が定型の代答で approve する

完了日: 2026-10-30

ADR: [2026-10-30-cos-plan-conforms-approve](../adr/2026-10-30-cos-plan-conforms-approve.md)
（[2026-10-28-cos-routine-delegated-answers](../adr/2026-10-28-cos-routine-delegated-answers.md) に付記 A3）

## 変更

- 設計: 計画の中身が目的のとおりか・どの段が課金・node-h・外部公開・破壊的・本番の変更を含むかは計画の本文を読まないと決まらないので、
  CoS の判定（危うい段とその守りを `basis.plan.guarded_stages` に申告）と API の決定的な検査に分けた（ADR D1）。
- `crates/task-api/src/cos/basis/plan_conforms.rs`（新規）: 推奨・答えとも `approve` の plan gate だけ、`basis.plan.plan_id` が承認を待つ版、
  未回答の決定なし、危うい段の守りはその段より前の `review: human` の段か store にある人の承認（422 `cos_basis_plan_unguarded`）、
  24 時間に 3 版以上の採用は 422 `cos_basis_plan_churn`。
- `crates/task-api/src/cos/basis.rs`: `BasisCategory::PlanConforms`、`DelegatedBasis.plan`、plan gate の `approve` は `plan_conforms` か
  `within_approved_frame` 必須（`documented_default` で計画を読まずに承認できた穴を塞いだ。ADR D3）。
- `cos-inbox-triage` skill を version 8 に（§2 の表と追加の規則、§3 の (f) の条件と (f) にしない条件）。`[cos.triage] policy_version` の既定 "8"。
  `config/celeris.example.toml`・`docs/ops/cos-chat.md`・`docs/api/v1/api.md`・`docs/ops/mlsys-main-4-ops-r1.md` §8・SOURCE.md を更新。API schema と web の生成型を再生成。
- 試験: `crates/task-api/tests/cos_plan_conforms.rs`（新規 4 件、fake harness の CoS run credential、LLM なし）と plan_conforms.rs の単体試験 1 件。
  `cos_chat_triage_scenarios` の plan gate の代答を `documented_default` から `plan_conforms` に替え、fixture に計画の版を足した。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `cargo test -p task-api --test cos_plan_conforms --test cos_chat_triage_scenarios --test cos_routine_answers --test cos_review_rework`: 全件 ok（4・3・5・4 件）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-api --all-targets -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 4775 件 run・4775 passed・20 skipped、web の tsc -b と vitest 101 files・697 tests passed、doc test ok）。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## 本番への移行

- 本番 config に `[cos.triage] policy_version = "7"` を明示していれば "8" に書き換える（未設定なら既定の "8"）。
- 本番の KB に `cos-inbox-triage` skill の写しがあれば、この版の `config/skills/cos-inbox-triage/` に差し替える。

## 未解決事項

- 危うい段の申告は CoS の判定に依る（計画に構造化した欄が無い）。CoS が申告し漏らした段は API では検出できない。代答は監査とチャットのカードに残り、人が override で取り消せる。

## 提案

- planner の計画（`celeris.execution-plan/3`）の段に、課金・外部公開・本番の変更を表す欄（例 `effects: [billing, publish, production]`）を足せば、
  (2) を CoS の申告ではなく API が計画から決定的に照合できる。申告漏れの事例が出たら別 ADR で検討する。
