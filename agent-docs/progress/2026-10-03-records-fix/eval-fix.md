---
task: records-fix
wu: eval-fix
status: done
completed: 2026-10-03
---
# eval-fix: merge-train-evaluation の表名・migration 番号を直し、Phase 5 の A/B と修正 4 件を反映する

WorkUnit `eval-fix`（親: records-fix）。コードは変えていない。対象は `agent-docs/progress/2026-10-03-merge-train-evaluation.md` だけ。

## 変更
- 表名・migration 番号を `crates/task-core/migrations/0043_work_unit_sessions.sql`・`0044_write_sets.sql`・`0045_behind_targets.sql` に合わせた: 表は `task_behind_targets`・`run_write_sets`・`work_unit_write_sets`、session は `node_sessions` の `task_id`・`work_unit_id`・`provider`・`cwd` 列（0043 は表を作らない）。migration 番号は 0042〜0045。閾値表の列名も実在の `behind_commits` に直した。
- '## 境界' の本番 DB の問い合わせを実在の表名・列名で書き直し、書き直したことを 1 行で書いた（旧名は書いていない）。
- '## 前後比較' に Phase 5 と Phase 2 の off/on の表（`write_set`・`stale_priority`・`review_sync_phase2`。出典 `review-sync-fix/ab-phase5.md`、`2f5ca69a`）を足し、「Phase 5 は run 数に出ない」の箇条を表への参照に置き換えた。
- 再計測の手順に node_sessions（continuation 行）・run_write_sets/work_unit_write_sets・task_behind_targets（behind_commits と behind_since からの age）の SQL を足した。
- '## 境界' に Phase 統合後の修正を 1 行ずつ: sync-history（`21ba1b45`・`9a27539e`、ADR-0118 付記）、fallback-delivery（`0f9089f9`・`17c81961`、ADR-0120 付記）、session-container（`774a71dd`・`1d645021`、ADR-0140 付記）、starve-fix（`dd44dd28`、`2026-10-03-write-set-no-starvation.md`）。

## 検査
- 受け入れ条件 1 の command（task_behind_targets・run_write_sets・work_unit_write_sets・stale_priority があり、`'behind_targets'`・`0040_behind_targets` が無い）→ exit 0。
- `grep -nE "0037_review|0038|0039|0040|'write_sets'|behind_target_commits"` → 0 件。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → ok / ok (132 files) / ok、exit 0。
- 本番 DB（/var/lib/celeris）はこの run の環境から見えず、書き直した問い合わせは流していない。値 0 は schema_migrations が 41 で止まっている記録からの帰結。

## 未解決・提案
- 文書だけの変更のため `cargo test --workspace` / `cargo clippy` は回していない（crates/ 差分ゼロ）。
