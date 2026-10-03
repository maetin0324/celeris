---
title: 日次整理の worker が daemon の検証を通る curation-plan.json を確実に出す（ADR-0131 付記 D12）
tasks: [01M411BGJ7EQJ56ATFPFTK5E76]
status: done
updated: 2026-10-03
---
# 日次整理の worker が daemon の検証を通る curation-plan.json を確実に出す

完了日: 2026-10-03。設計判断は [ADR-0131 付記 D12](../adr/0131-cron-jobs.md)。

## 状況

2026-10-03 の本番初回 dry-run（task `01M410C9X7TGMRBJ6NYXDSW07P`）で worker は `curation-plan.json` を
独自の形（`task_id`・`operations`・`manual_review` …）で出し、daemon の `CurationPlan`（`deny_unknown_fields`）が
`unknown field task_id` で拒否して計画は使われなかった。`_inbox` には候補が 283 件あった。

## やったこと

- `celerisctl curation validate [PLAN] [KB] [--diff] [--inbox] [--no-diff] [--json]`
  （`crates/celerisctl/src/commands/curation.rs`）。DB・ネットワークに触れず、daemon の `check_plan` と同じ順・同じ関数・
  同じ文言で検証する。引数省略時は cwd から上へ `artifacts/curation-plan.json` と `inputs/kb` を持つ作業場所を探す
  （検査コマンドの cwd は `repos/<name>`）。失敗は stderr `error: <理由>` と exit 1。
- `task_ops::knowledge_curation`: `parse_plan`（daemon も同じ関数を呼ぶ）、`known_task_ids_from_inbox_json`、
  `MAX_INBOX_CANDIDATES_PER_RUN = 40` と `validate_with_inbox` での上限検査。
- 本番 worker の実出力を試験 fixture に（`crates/task-ops/src/knowledge_curation/fixtures/worker-plan-2026-10-03.json`。
  配列は先頭 2 件に刈り込み）。
- `config/celeris.example.toml`: harness `knowledge-curation` の `instructions` に計画全体の形・最小の例・
  merge/delete の例・`celerisctl curation validate`・40 件の上限を書き、`[[cron.seed]]` の `objective` と
  `acceptance`（`{ type = "command", cmd = "celerisctl curation validate", expect_exit = 0 }`）を更新。
- `docs/ops/cron-jobs.md`: §2 の雛形に同じ acceptance、§2.1 に形・最小の例・検証コマンド、§4 に検証の使い方。
- `docs/architecture-map.md` の cron 行に `curation.rs` を追加。
- 試験: task-ops 4 件、celerisctl 6 件、config の seed 試験に acceptance と instructions の語の検査を追加。

## 受け入れ条件と証拠

| # | 条件 | 証拠 |
| --- | --- | --- |
| 0 | 2026-10-03 の形で非 0 と理由、最小の計画で 0 | `cargo test -p celerisctl curation`（6 passed: `worker_shape_of_2026_10_03_is_rejected_with_reason`、`minimal_plan_passes` ほか）、`cargo test -p task-ops knowledge_curation`（14 passed）。実機: `celerisctl curation validate <本番 run の artifacts/curation-plan.json> <inputs/kb>` → `error: curation-plan.json の形が違う: unknown field `task_id`, expected one of `version`, `kb`, `inbox`, `human_decisions` at line 2 column 11`、exit 1。最小計画 → `ok: kb merge=0 … _inbox candidates=0/40`、exit 0 |
| 1 | instructions と docs/ops に形・最小の例・事前検証 | `config/celeris.example.toml`（instructions 2,370 文字、tomllib で parse 可）、`docs/ops/cron-jobs.md` §2.1。`cron_seed_example_toml_has_daily_curation_and_knowledge_curation_harness` が語を検査 |
| 2 | cron 雛形の acceptance に形の決定的検査 | `docs/ops/cron-jobs.md` §2 と `[[cron.seed]]` の acceptance 4 件目（command）。同上の試験が `celerisctl curation validate` の command check 1 件と `expect_exit = 0` を検査 |
| 3 | `_inbox` の 1 回上限と持ち越し規則 | ADR-0131 付記 D12 (3)(4): 40 件/run、ファイル名昇順（古い順）、残りは計画に載せず次回へ、上限超過は計画全体を拒否 |
| 4 | cargo test / clippy の記録 | 下記 |

## 検査コマンドと結果（2026-10-03）

- `cargo test --workspace` → exit 0。passed=3618 failed=0 ignored=13。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh`・`check-doc-links.sh`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・
  `python3 scripts/dev/check-architecture-map.py` → すべて ok。

## 未解決事項

- 本番の検査は daemon の PATH（`~/.local/bin` 等）の `celerisctl` に `curation` がある release を昇格してからでないと
  非 0 になる（古い release の `celerisctl` には無い）。昇格と cron 雛形の PATCH（acceptance の追加）は人の操作。
- 本番 cron job `daily-curation` の雛形は DB が正なので、`docs/ops/cron-jobs.md` §6 と同じ要領で `template.acceptance` に
  command check を足す PATCH が要る（この run では本番 DB に書いていない）。
- `dry_run` のあいだは本番 `_inbox` が減らず毎回同じ先頭 40 件を扱う（付記 D12 (4) に明記）。

## 提案

- 上限 40 を `apply` の実績（1 run の turn 数・拒否率）を見てから設定値にするかを決める（今は定数）。
- `merge` の統合先を 1 計画で 1 回しか使えない制約は、同じ趣旨の候補が複数ある `_inbox` では毎回「1 merge + n delete」になる。
  候補をまとめて 1 件に統合する `sources` 形を D11 の形に足すかは別 task で検討する。
