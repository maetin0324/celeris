---
title: 健全性v2 G2 limits-doc — limits.rs の doc comment の旧い関数名を直す
tasks: [01M6TYVJSTYGRPHERB93MM7J59]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 limits-doc — limits.rs の doc comment の旧い関数名を直す

完了日: 2026-11-08

WorkUnit `limits-doc`。task 01M6TYVJSTYGRPHERB93MM7J59 の三度目の final review の差し戻し (3) を直した。`crates/task-core/src/tree/limits.rs:86` の `TreeLimitKind::is_run_time` の doc comment は、この task の branch で `Dispatcher::tree_limit_hold_for`（`crates/task-dispatch/src/dispatcher/tree_units.rs`）に置き換えた旧い関数名 `tree_run_limit_hold` を指していた。基準は `$CELERIS_WU_BASE`。

## 変更
- commit「G2 limits-doc: TreeLimitKind::is_run_time の doc comment の旧い関数名 tree_run_limit_hold を Dispatcher::tree_limit_hold_for に直す」
  - doc comment の `` `tree_run_limit_hold` `` を `` `Dispatcher::tree_limit_hold_for` `` にした。task-core から task-dispatch の非公開の method（`pub(super)`）へは rustdoc の link を作れないので、backtick の名前で書いた。
  - 変えたのは doc comment の 1 行だけで、コードの振る舞いは変えていない。
- commit「G2 limits-doc: 進捗に変更・検査の結果・残る旧い名前を書く」（この file）

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）
- 条件 0（limits.rs の doc comment が `tree_limit_hold_for` を指し、crates/ に `tree_run_limit_hold` が無い）
  - `grep -n tree_limit_hold_for crates/task-core/src/tree/limits.rs` → 86 行に 1 件。
  - `grep -rn tree_run_limit_hold crates/ | wc -l` → 0。
- 条件 1（test-affected・clippy・fmt が exit 0）: 下の「検査の結果」。
- 条件 2（変更は limits.rs と自分の進捗 file だけ）
  - `git diff --name-only "$CELERIS_WU_BASE"..HEAD` → `crates/task-core/src/tree/limits.rs` と `agent-docs/progress/2026-11-07-health-v2-g2/limits-doc.md` の 2 件。

## 検査の結果（提出時の HEAD で実行。check-health の基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh` → exit 0（11 分 22 秒）。task-core を変えたので、逆依存の crate 全部（celeris・celeris-campaign・celeris-credentiald・celeris-mcp・celeris-script・celerisctl・e2e・llm-proxy・routing-eval・task-api・task-core・task-dispatch・task-ops・task-worker）と web・ts-verify が対象になった。
  - check-health: `verdict=pass new=0 worsened=0 existing=717`、`repo_health_gate: all checks passed`。
  - nextest: `7981 tests run: 7981 passed (11 slow), 23 skipped`。
  - web: tsc -b は合格、vitest は `Test Files 104 passed`・`Tests 711 passed`。
  - ts-verify: `20 passed, 0 failed`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決事項
- crates/ の外には旧い名前 `tree_run_limit_hold` が残っている。この WorkUnit は ADR と docs を編集しない範囲なので、変えていない。
  - `agent-docs/adr/2026-11-07-health-review-integration.md` の 296 行・333 行（付記の方針の本文。394 行・400 行は改名とこの残りを書いた記録）。
  - 過去の ADR: `agent-docs/adr/0079-recursive-task-decomposition.md`（908・1015・1034・1708 行）、`agent-docs/adr/2026-10-22-plan-issue-answer-replans.md`（21 行）、`agent-docs/adr/2026-10-11-task-wait-reasons.md`（41 行）。当時の名前を書いた決定の記録。
  - 過去の進捗 file（`agent-docs/progress/phase-R.md`・`2026-11-06-budget-usd-caps/dispatch.md`・この task の `2026-11-07-health-v2-g2.md`・`amend-adr.md`・`design-budget.md`）。作業当時の記録で、改名を書いた箇所も含む。
- ADR の付記に「limits.rs の旧い名前を直した」ことを写すのは unit `reclose-docs` が行う。

## 提案
- unit `reclose-docs` は、ADR 2026-11-07 health-review-integration の 400 行（limits.rs:86 の旧い名前の残り）を「直した」に改め、296 行・333 行の旧い名前を残すか（方針を書いた時点の名前として）を付記で明示する。
