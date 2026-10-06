---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: ops-doc
status: done
completed: 2026-10-05
---

# Phase 4 ops-doc: shadow opt-in・上限・export/evaluate・戻し方の移行文書

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §7.1・§7.2・§10 Phase 4 に対応する
運用手順を `docs/ops/model-routing-migration.md` に §9（9.1〜9.5）として追加した。crates/ は変更していない。
本番 host の変更は全て人が実行する手順として書いた。

- 冒頭に「Phase 4（§9）で変わること」を 3 行追加（decision shadow / 実行 shadow の opt-in / export・evaluate / migration 0049）。
- §9.1: `mode = "shadow"` の有効化。dispatcher は reload 直後、llm-proxy の decision shadow は再起動で効く
  （daemon ログの warning 文言は `crates/celeris/src/daemon/routing_shadow.rs` の実装から）。
  確認は `GET /api/v1/tasks/<task id>/routing` の `runs[].routing_shadow`（kind `decision`・status `completed`）。
- §9.2: `[model_routing.shadow]` の opt-in-capped 設定（`execute`・`candidate_policy`・`sample_rate`・
  allowlist 4 次元・日次上限 3 種・queue 上限・timeout）。欠けると `Config::load` が拒否する検証規則
  （`ShadowPolicy::validate` と `crates/celeris/src/config/model_routing.rs` の `routing_runtime` から）。
  allowlist の value の読み方（lane / source id、`*` と空の違い）、openai-compatible 以外は
  `failed/unsupported_source`、primary 優先の resource group、`dropped/unknown_cost`、確定の扱い
  （completed 実測 / failed 大きい方 / timeout 予約額、`crates/task-core/src/store/routing_shadow.rs` の
  付記から）、停止して既定 off に戻す手順（`dropped/off`・detail `reconfigured_off`）。
- §9.3: 戻し方（`mode = "legacy"` + `execute = false` の reload / binary rollback は旧 config と組）。
- §9.4: migration 0049（additive、新しい表と索引だけ）と「戻すときは設定を戻すだけ」。
- §9.5: `celerisctl routing export --db <path> --out <dir>` / `celerisctl routing evaluate --dataset <dir> --out <report.json>`
  の実行例。DB path の確認（`GET /api/v1/config` の `db` / `[db].path`）、本番 DB は読み取り専用で開けるが
  写しを使う推奨、`--policy-hash` 等の呼び手指定、dataset（`celeris.routing.dataset.v1`）・manifest・
  task 単位の split（seed 固定で再現）・report（`celeris.routing.report.v1`）の欄、paired metrics は
  paired outcome が無いと undefined、RouterBench 型の baseline（`BenchmarkBaselineV1` の `name` +
  `models.<id>.{quality,cost_usd}`）は `external_benchmark_baseline` に別欄。

flag・欄名・reason code・warning 文言は実装（`crates/celerisctl/src/commands/routing.rs`・
`crates/task-ops/src/routing_replay.rs`・`crates/celeris/src/config/model_routing.rs`・
`crates/task-core/src/model_router/shadow.rs`・`crates/celeris/src/daemon/routing_shadow.rs`・
`crates/llm-proxy/src/shadow.rs` / `shadow_budget.rs`・`crates/task-core/migrations/0049_routing_shadow_budget.sql`）
と突き合わせた。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `sh scripts/dev/check-doc-links.sh` | exit 0（壊れた参照 0 件） |
| `sh scripts/dev/check-doc-links.sh docs/ops/model-routing-migration.md` | exit 0 |
| `git diff --name-only "${CELERIS_WU_BASE:-HEAD}" -- . ':!artifacts'` + `git ls-files --others --exclude-standard` | `docs/ops/model-routing-migration.md` と `agent-docs/progress/2026-10-04-multi-objective-routing/p4-ops-doc.md` の 2 件のみ（範囲外 0 件） |
| `git diff --check` | exit 0 |

## 未解決事項

- なし（paired metrics を CLI から渡す `--paired` の欠如は p4-cli の未解決事項として記録済み。本文書ではその旨を注記した）

## 提案

- なし
