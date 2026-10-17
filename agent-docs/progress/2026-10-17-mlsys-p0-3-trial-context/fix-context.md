---
title: "MLSys P0-3 fix-context: dispatcher の文脈生成を試行 task で試行用 KB に固定"
tasks: [01M54HYGQPFNRX6PNPJJG475EB]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# fix-context（WU）

ADR: `agent-docs/adr/2026-10-17-mlsys-trial-context-isolation.md`。棚卸し（audit-paths）の修正リスト fix-context 1〜6 に対応。

## 変更
- `crates/task-dispatch/src/dispatcher/run_context.rs`: `trial_scope`（campaign 識別の解決 → 試行の根の作業場所の `kb/`）。
  試行 task は knowledge を `dir:<ws>/kb` だけ、memory・conversation・standing_rules・skills を空、profile から knowledge/
  skills_mounts を外す。`knowledge_context` を `mounted_knowledge` に分け、`kb` mount が無ければ通常 KB の索引を読まない。
- `dispatcher.rs` / `dispatch_run.rs`: `RunExtras.trial_env`（`CELERIS_KNOWLEDGE_ROOT=<ws>/kb`）を `work_unit_env` に重ねる。
- `review_spawn.rs`: 試行 task の review に skills を渡さない（判定に失敗しても渡さない側）。
- 試験 `crates/task-dispatch/src/dispatcher/tests/trial_context.rs`（2 件）。

## 証拠
- `cargo nextest run -p task-dispatch --lib trial_context` → 2 passed
- `bash scripts/dev/test-parallel.sh` → exit 0、5616 tests run: 5616 passed, 14 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 状態の区別
- 実装済み・mock 検証済み（実 Dispatcher の tick ＋ 捕捉 adapter）: worker・planner・子 task・reviewer の RunContext と env。
- 未検証: 実 task-api router 経由の起票（canary-e2e の範囲）、実 harness（Claude Code）が env と索引をどう使うか（実 LLM）。

## 未解決事項
- 試行内の `celerisctl knowledge search/get` は `<ws>/kb` に `.git` が無いので「KB が無い」で止まる（通常 KB には行かない）。
  前置きの案内文は search を勧めるので、試行で KB 検索を使わせるなら seed の kb を `knowledge init` 相当にするか案内文を
  dir mount 向けに変える必要がある（preflight-kb / canary-e2e で判断）。
- worker の filesystem 直読み（R8）と Claude account dir（R10/R11）は本 WU の範囲外（人の決定 D1/D2）。

## 提案
- canary-e2e では実 `POST /tasks`（`campaign` 欄）で起票し、本 WU と同じ canary を使う。
