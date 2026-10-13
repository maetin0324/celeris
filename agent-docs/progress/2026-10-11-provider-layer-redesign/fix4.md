---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: provider 層の再設計 — fix4（schema drift・文書参照の修正と全体検査）
completed: 2026-10-13
updated: 2026-10-13
---

# fix4: schema drift・文書参照の修正と全体検査

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。WU: schema-fix（API schema 再生成・gui-api.md 参照・architecture-map）→ close4（全体検査と記録）。

## 経緯

- 旧 branch（36c2a8e3）を main の上に直線履歴で載せ直した（rebase）際、merge commit に入っていた統合修正が消えた。`model.rs` の余分な閉じ括弧（`},`）と main 由来の試験（`task_waits.rs`・`stranded_*.rs`・`integration_api.rs`）が新しい型（`Task.harness_id`・`WorkerHint` の新形）に合わなかった。前回 close3 の失敗はこの compile error だった（`crates/task-core/src/model.rs:1906` の unexpected closing delimiter）。
- build-fix（da9771e6・58c449bf）で直した file:
  - `crates/task-core/src/model.rs`: `Event` enum 末尾の余分な `},`（1095 行付近）を削除。
  - `crates/celeris/tests/integration_api.rs`・`crates/celeris/tests/stranded_default_branch.rs`・`crates/celeris/tests/stranded_integration.rs`・`crates/task-dispatch/src/dispatcher/tests/task_waits.rs`: main 由来の試験を `Task.harness_id`・`WorkerHint` の新形に合わせた。
  - `crates/task-core/src/model_catalog/tests.rs`: 試験の migration 版数を 69 → 72 へ。`crates/task-core/src/store/migrations.rs`: 0072 の include_str! 行を rustfmt の折り返しに整えた（内容は同じ）。
  - `docs/api/v1/event.schema.json`: `harness_id` を持つ event 形の variant を追加（型の再生成結果）。
- 統合後の test-parallel で残った失敗は `task-api schema::tests::committed_schema_matches_generated` の 1 件だけだった（4994 passed）。schema drift。
- fix3（d32b7ecc）で `docs/api/v1/api-v1.schema.json` を `UPDATE_SCHEMA=1` で再生成。
- schema-fix（7e0fc18c）で直した file:
  - `crates/task-dispatch/src/dispatcher/route_failure.rs`: doc comment の参照を `docs/api/v1/gui-api.md` §3.29 から `docs/api/v1/api.md` §3.29 へ（`gui-api.md` は無い）。
  - `docs/architecture-map.md`: 2 行 3 path を直した。scratch 行から `tiered.rs`（無い file）を除いた。モデルの役割割り当て行は、無い `crates/task-api/src/model_assignments.rs`・`web/api/model-assignments.ts` と、存在しない HTTP API（`/api/v1/llm/models/assignments…`）の記述を外し、「保存と CoS 解決の型。HTTP API は無い」に書き換えた。
  - `web/api/generated/schema.json`・`web/api/generated/types.ts`: API schema に合わせて再生成。（schema-fix の変更は 4 file: route_failure.rs・architecture-map.md・web/api/generated の 2 file。）

## 証拠

すべて作業 branch の HEAD 7e0fc18c（schema-fix 後）で実行。

| 条件 | 実行したコマンド | 結果 |
|---|---|---|
| 全体試験 | `TMPDIR=/var/tmp/c4t bash scripts/dev/test-parallel.sh` | exit 0。nextest: passed 4995 / failed 0 / ignored 14（exit 0）。doctest: exit 0 |
| 静的検査 | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`、warning 0） |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | exit 0（ok） |
| 文書配置 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0（ok） |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（ok、198 files） |
| architecture-map | `python3 -I scripts/dev/check-architecture-map.py` | exit 0（353 件のパスを確認） |
| 進捗索引 | `sh scripts/dev/progress-index.sh --check` | exit 0（ok） |

注: test-parallel.sh は run の既定 TMPDIR（長く socket 試験が SUN_LEN で落ちる）を避けて `/var/tmp/c4t` で流した。試験の一時 file は `tmp_leftovers: 0`。

## 前回（close3）との差

- close3 の失敗は compile error（model.rs）と schema drift 1 件。build-fix と fix3 で compile error は解消、schema drift は fix3 の再生成と schema-fix の文書参照で解消。
- 今回の HEAD では test-parallel.sh・clippy・文書検査すべて exit 0。

## 未解決事項

- なし（この fix4 の範囲）。
- 前回の経緯（rebase で merge commit の統合修正が消えた）の再発防止: task branch は merge commit で main を取り込む形を保つ（直線履歴への載せ直しをしない）。これは運用の決まりで、この段では変えていない。
- 本番 config の移行（`[[providers]]` の書き換え）は migration-v2 の runbook にあり、本番は変えていない（この段の対象外）。
