---
task: 01M54HYGQNYDP2MGXXS7AY7164
wu: e2e
status: done
completed: 2026-10-17
---
# e2e: evidence_mode の結合試験

- 追加: `crates/celeris/tests/evidence_mode_e2e.rs`（3 試験）。実 task-api router（HTTP）＋実 `Dispatcher`（fake harness、LLM なし）。
  - `none_survives_post_child_grandchild_retry_and_ignores_table_updates`: POST → 子 → 孫 → cancel+retry が none を継ぎ、v2 差し替え後も判断不変・lookup 0 回、latest は v2 に追従、pinned は v1。
  - `ticked_runs_record_the_evidence_mode_and_the_table_version_actually_used`: 実 tick の `routing_decided` に mode と evidence_version（none は null）が残る。
  - `contradictory_evidence_mode_is_rejected_before_any_task_exists`: 矛盾 4 種は 4xx、task 数不変。
- 変更（試験のため）: `task-dispatch` が `RouteContext` を公開 re-export。
- 区分は `../2026-10-17-evidence-mode.md` 参照。
