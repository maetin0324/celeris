---
title: 健全性v2 G2 adr — ADR health-review-integration
tasks: [01M6VBSKZQG2YWF7NWYH6GP9P7]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 adr — ADR health-review-integration

完了日: 2026-11-07

WorkUnit `adr`（G2 の工程「ADR health-review-integration を書く」）。基準は `$CELERIS_WU_BASE`。根拠は [survey](adr-survey.md)。コードは変えていない。

## 変更
- 件名「G2 adr: ADR health-review-integration と進捗」の commit — [ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md) を追加した。8 見出し（根本原因・責務 owner・既存の再利用先・選択肢・新しい状態・分岐・fallback・port・撤去対象・不変条件・決定）と、決定の下の (1)〜(7)、後続 unit U1〜U6 の path・型名・試験名 prefix（`change_rules_`・`design_brief_`・`health_review_`）を書いた。

## 決めたこと（要約）
- (1) 汎用規約は `crates/task-worker/src/change_rules/generic.md`、`prompt_header` から全 harness へ 1 回。CoS chat には入れない。
- (2) profile の `[review]` 節（`owner_map`・`high_risk`・`invariants`）。dispatcher は `repo_health.py --print-review-policy --base <target_sha>` で base 側を読む。
- (3) `ExecutionPlanSpec.design_brief: Option<DesignBrief>`、`validate_v3` で形の検証、`task_core::design_review::assess_risk`、High/Unknown は採用の前に既存の reviewer run で設計 review（`DesignReviewRequested`・`DesignReviewed`）。
- (4) `ReviewRequest.health: Option<HealthReviewInput>` を `(repo_id, target_sha, reviewed_sha, definition_sha256)` ごとに 1 回（`HealthReviewInputBuilt`）。
- (5) `ReviewOutput.health` の 7 種の finding。blocking は `Verdict` に足して `classify_review_failure` に載せる。局所修復を許すのは `removal_missing`・`test_change_unjustified` だけ。
- (6) policy の file だけの branch に分け、reviewer の policy verdict と人の承認（`Check::Human`）で通す。`check-health.sh` は policy だけの branch で exit 0 にして review に回す。
- (7) reviewer prompt の hash 規則の写し・Result 節の旧い形、planner prompt の `CLAUDE.md` と同じ文、`source-health.md` の重複説明を撤去する。

## 証拠（提出時の HEAD で実行）
- 受け入れ条件の見出し検査（`for h in 根本原因 責務 再利用 選択肢 fallback 撤去 不変条件 決定; do grep -q …`）→ exit 0
- `grep -c '^### ([1-7])'` → 7
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ scripts/` → exit 0
- 文書だけの変更なので cargo の試験・clippy は走らせていない。

## 未解決
- (6) の D6.4 で `Check::Human` の経路に「policy の変更の承認」をどう載せるか（暗黙の criterion を足すか、受信箱の項目だけにするか）は U5 で既存の human check の実装を読んで決める。
- (3) の設計 review の reviewer run が既存の run の行（task の review run）とどう区別されるか（run の kind・reason）は U3 で決める。
- D2.2 の `high_risk` の class と path の具体値は U2 が決める。

## 提案
- Aider だけが `CLAUDE.md` の前置き（`required_rules`）を受け取らない（survey (a)）。別 task で `aider.rs` に `required_rules::preamble_for_run` を足す。
- G2 の実装計画では U2（`scripts/dev/repo-health/`・`check-health.sh` を変える policy の branch）を独立の task にし、受け入れ条件に `{"type":"human"}` を置いて最初に通す（ADR D6.6）。
