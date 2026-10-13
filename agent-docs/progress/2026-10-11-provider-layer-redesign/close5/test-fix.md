---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: close5 test-fix — 統合検査で落ちた 2 試験の確認
completed: 2026-10-13
---

# close5 / test-fix: 統合検査で落ちた 2 試験

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。修正 commit は枝の `6ad6954d`（`close5: 統合検査の 2 件を直す`）。この leaf は修正を名前指定で流して通ることを確かめ、記録した。コードは変えていない。

## 2 試験の原因と修正

| 試験 | 原因 | 修正（6ad6954d） |
|---|---|---|
| `celeris` `config::tests::migration_runbook_toml_loads` | `docs/ops/provider-llm-source-migration.md` の runbook TOML は `[knowledge.langmem] model = "qwen/qwen3.8-27b"`（proxy を指す形）に直ったが、親進捗 `agent-docs/progress/2026-10-11-provider-layer-redesign.md` の写しが旧の `model = "qwen3.8-27b"` のままで、`progress.contains(toml)` が落ちた。 | 親進捗の写しを `qwen/qwen3.8-27b` に合わせた（1 行）。 |
| `task-core` `approval::tests::cancelling_a_task_withdraws_its_pending_approvals_in_the_same_transition` | 同じ `created_at` の 2 件の承認は id（ULID の乱数部）順に並ぶため、作った順 `vec![a.id, b.id]` と比べると並び順しだいで落ちた。 | 期待値と実際の withdrawn event の id 列をそれぞれ sort して集合で比べる（試験の 11 行）。 |

## 確認コマンドと結果

- `cargo test -p celeris --lib migration_runbook_toml_loads`
  - exit 0。`test config::tests::migration_runbook_toml_loads ... ok`、`1 passed; 0 failed; 387 filtered out`。
- `cargo test -p task-core --lib cancelling_a_task_withdraws_its_pending_approvals_in_the_same_transition`
  - exit 0。`... ok`、`1 passed; 0 failed; 906 filtered out`。
- 承認試験を同じコマンドで 5 回繰り返した。5 回とも `1 passed; 0 failed`（並び順依存が残っていないことの確認）。

`TMPDIR=/local/crc` で流した（unix socket の SUN_LEN 対策。この 2 試験は socket を使わないが、同じ手順で揃えた）。

## 残したもの・未解決

- なし。この 2 試験は枝の現在の HEAD で通る。
- test-parallel 全体の結果は closeout の `fix5.md` に書く。
