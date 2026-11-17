---
title: アーキ v4 C risk-rule — 高 risk の class に合う write path を new_* の宣言によらず High にする（ADR D1）
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
updated: 2026-11-17
completed: 2026-11-17
---
# risk-rule（WorkUnit）

ADR: [2026-11-17-boundary-change-risk-review.md](../../adr/2026-11-17-boundary-change-risk-review.md) の D1（と D6 の `PathClass.invariants`）。
基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。commit は件名で挙げる。

## 変更（commit `arch-v4 C risk-rule: …`）
- `crates/task-core/src/design_review.rs`（owner。純粋関数のまま、LLM・I/O なし）
  - `assess_risk` の規則を D1 にした: write path が高 risk の class に合えば brief の `new_*` によらず `High`。
    理由は class（方針の順）ごとに `path_class:<class>:<path>`（path は辞書順・重複なし）と `invariant:<class>:<名前>`、
    その後に宣言した欄の `declared:<欄>`（level に効かない）、brief が無ければ `brief_absent_human_plan`。
    `new_fallback` は path に依らず `High`（`new_fallback`）。それ以外は理由なしの `Low`。`Absent` → `None`、`Unavailable`・読めない正規表現 → `Unknown` は今のまま。
  - `PathClass` に `invariants: Vec<String>`（`#[serde(default, skip_serializing_if = "Vec::is_empty")]`）を足した（D2・D6）。
  - class と合った path を集める private の `high_risk_matches` を足し、`high_risk_classes`（`brief_required_classes` が使う）はそれから導く（判定は 1 か所）。
  - doc comment（`DesignBrief`・`RiskAssessment`・`assess_risk`）を新しい規則と理由の語に改めた。
- 撤去（brief 6）: 積の規則と理由の語 `path_class:<class>+<欄>`、「class に合い brief が無い → `Low`（`brief_absent_human_plan`）」の規則、
  それを固定した試験 `health_review_risk_high_risk_path_with_new_state_is_high`・`health_review_risk_human_plan_without_brief_is_low_with_reason`・
  `health_review_risk_other_plans_are_low` の「宣言の無い brief は Low」の部分、planner 向けの説明（`crates/task-worker/src/change_rules.rs` の
  `design_brief_section`）の「class の path を変えて `new_*` のどれかを宣言した計画は High」、`docs/protocol/worker-protocol.md` の `path_class:<class>+new_state`。
- 呼び出し側の文字列: `crates/task-core/src/execution_plan/design_brief.rs`（`require_design_brief` の doc）、
  `crates/task-dispatch/src/health_review/design.rs`（`rank` の doc）、試験の理由の文字列（`health_review/design/tests.rs`・
  `dispatcher/tests/health_review_design_gate.rs`・`health_review_design_budget.rs`・`task-worker` の `change_rules/review/tests.rs`・`protocol/design_review/tests.rs`）。
  `PathClass` の構造体の literal に `invariants: Vec::new()` を足した（task-dispatch・task-worker の試験 6 file）。
- 生成 schema（`UPDATE_SCHEMA=1` の schema 試験だけで再生成）: `docs/protocol/worker-protocol.schema.json`（`PathClass.invariants` と description）、
  `docs/protocol/execution-plan.schema.json`・`docs/api/v1/{api-v1,event}.schema.json`・`web/api/generated/schema.json`（description だけ）。`web/api/generated/types.ts` は変わらない。

## 試験（`crates/task-core/src/design_review/tests.rs`。注釈に守る不変条件と、既存の試験で検出できない理由を書いた）
| 試験 | 入力 | 期待 |
|---|---|---|
| `health_review_risk_class_path_is_high_without_new_declarations` | `crates/task-api/src/middleware.rs` 1 file、brief は `removals` だけ（`new_*` 無し） | `High`、理由 `path_class:authz:crates/task-api/src/middleware.rs`・`invariant:authz:api-principal-from-token`。複数 class・path の順、`declared:<欄>`、`re.search` の意味も確かめる |
| `health_review_risk_equivalent_change_outside_classes_is_low` | class 外の handler 1 file と画面 1 file（brief 有り・無し・`new_state` などの宣言有り）、似た名前の別 crate の `middleware.rs`、class の無い方針 | すべて理由なしの `Low` |
| `health_review_risk_new_fallback_is_high_on_any_path` | `new_fallback` の宣言（class 外・class 内） | `High`（`new_fallback`。class 内は path と `declared:new_fallback` も） |
| `health_review_risk_plan_without_brief_on_class_path_is_high` | brief 無しで `events` の class | `High`、理由に `brief_absent_human_plan` |
| `health_review_risk_unavailable_policy_is_unknown`（既存） | 方針 `Unavailable`・Rust で読めない正規表現 | `Unknown` |

## D1 の未確認（呼び出し元）の確認
- `assess_risk` を production で呼ぶのは `health_review::design::merge_risk` だけで、`merge_risk` は `Dispatcher::design_gate`
  （`dispatcher/planner_flow.rs` の planner の計画の採用）だけが呼ぶ。`assess_plan_risk` は試験だけが使う。
- 人の計画（`PlanOrigin::Human`。`task-api/src/execution.rs`・`celerisctl`・`task-ops/src/execution.rs`）は採用前の risk の判定を通らない。
  採用前の判定を通らない計画の変更は D3（WorkUnit `review-gate`）が final review で覆う。
- planner の計画は class に合えば brief が必須（`require_design_brief`）なので、`design_gate` で brief 無し ∧ class 一致は起きない。
  `brief_absent_human_plan` の理由は D1 のとおり `High` の補足として残した。

## 検査（提出時の HEAD で実行）
- `cargo nextest run -p task-core -E 'test(/health_review_risk|design_brief/)'`: 16 passed。
- `cargo nextest run -p task-worker -p task-dispatch -p task-api -E 'test(/change_rules|design_review|health_review_design|health_review_policy|health_review_risk|schema|protocol::health/)'`: 94 passed。
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base $CELERIS_WU_BASE --head worktree --policy-version 1`: verdict=pass exit=0、new=0・worsened=0
  （`size.responsibility-growth` pass。`design_review.rs` は既存の単位の超過に入らない）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（14 分 52 秒）。nextest 8,436 passed・22 skipped、web の tsc -b と vitest 101 file・705 passed、test-ts-verify 20 passed。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）
- production Rust: +72 −43（純増 29。ADR の目安 400 行以内）。試験: +151 −59（純増 92）。生成 schema: +16 −9。文書（worker-protocol.md）: +3 −2。

## 後続の WorkUnit への引き継ぎ
- `prompts`（D4）: 理由の語は `path_class:<class>:<path>`・`invariant:<class>:<名前>`・`declared:<欄>`。class の不変条件は `PathClass.invariants`
  （この WorkUnit で足した）から引ける。reviewer の `risk_lines` は今は理由を 1 行ずつそのまま出す（`invariant:` の語も出る）。
- `review-gate`（D3）: 差分の path に `assess_risk(policy, paths, None)` を当てると、class に合えば `High`（`brief_absent_human_plan` 付き）。
  覆う class は `path_class:<class>:` の語から読める。
- `cases`（D5）: ケース 1・2 は上の 2 試験と同じ入力を実際の判定経路で通す。失敗注入の「積の規則に戻す」は `assess_risk` の
  `if !matches.is_empty()` の下で `declared` が空なら理由を足さない形に戻せばよい。

## 範囲の check の不合格（run 01M7NW71ACNKFG51NQGJNHNFFX、plan_issue を申告）
- 前回の run の後の範囲の check が exit 1 で落ちた。範囲外と出たのは次の 5 file で、どれも ADR の決定から機械的に生じる変更。
  - `crates/task-worker/src/protocol/design_review/tests.rs`・`crates/task-worker/src/protocol/health/tests.rs`: `PathClass` の struct literal。
    ADR D6 が決めた `PathClass.invariants` を足すと、この 2 file は欄を書かなければ compile できない。
  - `docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json`・`web/api/generated/schema.json`（api-v1 の写しで同一）: `DesignBrief` と
    `RiskAssessment` の doc comment から生成される。ADR の撤去対象（brief 6）が旧い規則の記述の書き換えを求め、一致は
    `task-api/src/schema.rs` の `committed_schema_matches_generated` と `task-core/src/store/tests.rs` の event schema の試験が確かめる。
- 範囲の中だけで通す方法（欄を足さない・旧い doc comment を残す）は ADR と受け入れ条件 2 に反するので取らなかった。
  check の許可の正規表現に `crates/task-worker/src/protocol/(design_review|health)/tests\.rs$`・`docs/api/v1/(api-v1|event)\.schema\.json$`・
  `web/api/generated/schema\.json$` を足す修正を plan_issue で申告した。足した式は提出時の HEAD で exit 0 を確かめた。

## 未解決
- 範囲の check の修正（上の節。replan 待ち）。

## 提案
- `execution_plan::assess_plan_risk` は試験だけが使う。D3 の後も production の呼び出しが無ければ、closeout か H で消すかを決める。
