---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: provider 層の再設計 — 全体検査（close5 後の HEAD）と旧い仕組みの残りの記録
completed: 2026-10-13
updated: 2026-10-13
---

# fix5: 全体検査と記録

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。段の leaf は `close5/<key>.md`（docs・scripts・web・test-fix）と `crates-residue.md`。この leaf はコードを変えていない（検査と記録だけ）。ログは `artifacts/fix5-*.log` に置いた。

## 検査の結果（統合後 HEAD `ba733af4` の作業ツリー）

| 検査 | 実行したコマンド | 結果 |
|---|---|---|
| 全体試験 | `TMPDIR=/local/crc bash scripts/dev/test-parallel.sh` | exit 0。`CELERIS_TEST_SUMMARY`: passed 4998 / failed 0 / ignored 14、nextest_exit 0、doctest_exit 0、binaries 177（nextest 167・doc 10）、nextest_secs 130.3・doctest_secs 10.0。`tmp_leftovers` 1（script の後片付け対象。失敗ではない。前回記録の 0 と違う）。ログ `artifacts/fix5-test-parallel.log` |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`、警告・エラー 0 件）。ログ `artifacts/fix5-clippy.log` |
| live fixture の試験 | `bash scripts/selfdeploy/tests/migrate_to_local_test.sh` | exit 0（`migrate_to_local_test: all ok`）。ログ `artifacts/fix5-migrate-test.log` |
| portable providers 試験 | `python3 -I scripts/test_portable_providers.py` | exit 0（`Ran 3 tests`、`OK`）。ログ `artifacts/fix5-portable.log` |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（`ok (198 files)`） |
| 文書の配置 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0（`check-doc-layout: ok`）。tsv 引数が要る。引数なしは usage で exit 1 |
| 進捗索引 | `sh scripts/dev/progress-index.sh --check` | exit 0（`progress-index --check: ok`） |
| architecture map | `python3 -I scripts/dev/check-architecture-map.py` | exit 0 |
| 空白の検査 | `git diff --check 61108a5d` | exit 0（出力なし） |
| 旧 alias の grep（受け入れ 0） | `grep -rnE 'celeris/(frontier\|standard\|cheap)' docs/guides docs/ops docs/api/v1/api.md scripts web/features` と `grep -rnE '\[\[providers\]\]\|providers_include' scripts` | 出力なし（0 件） |
| 旧 alias の grep（web 全体） | `grep -rnE 'celeris/(frontier\|standard\|cheap)' web`（node_modules・dist・schema.json を除く） | 0 件 |

`test-parallel` の sandbox 既知失敗（browser・launcher・credentiald・CDP 系）は今回 0 件。

## 消したもの（段ごと）

- **crates-residue**（task-worker・task-core・task-api）: tier alias 判定（OpenCode・Aider・Pi の routing context）、paperqa の `celeris` tier 例外、tier 語の入った試験名と fixture（具体的な `qwen/qwen3.8-27b`・`opencode-go/grok-4.6` に替えた）、`/providers` 注釈。acp fixture は proxy の base URL を足した（試験側の誤り）。
- **scripts**（close5/scripts.md）: live fixture の `[[providers]]` を `[llm_sources.*]`・`[harnesses.*]`・`[route_defaults.*]`・`[harness_selection]` に書き換えた（`cos-chat-live.sh`・`cos-chat-bench.sh`・`live-check-phase53.sh`・`migrate_to_local_test.sh`・`test_portable_providers.py`）。`cos-chat-bench.sh` の codex 引数を外した。
- **docs**（close5/docs.md）: `docs/guides/providers.md`・`docs/guides/llm-source.md`・`docs/ops/coding-harness-pi-hashline.md` の tier alias 名（`celeris/cheap` など）。`docs/api/v1/api.md` の改訂履歴に「この API は後に削除（§3.19）」。`docs/ops/provider-llm-source-migration.md` の langmem 例を `qwen/qwen3.8-27b`（proxy）に揃えた。
- **web**（close5/web.md）: routing catalog の lane 表示の `celeris/` 接頭辞と、試験の期待値。
- **確認のみで既に無かったもの**: `scripts/selfdeploy/migrate-to-celeris.sh`（追跡されていない）、`docs/api/v1/gui-api.md`、`gui/docs/`、`docs/guides/knowledge.md` の langmem `[[providers]]` 例、`docs/ops/selfdeploy.md` の `providers.d/` 行、`docs/ops/opencode-go-and-model-catalog.md` §3a の旧 POST 手順（「廃止された」と書き換え済み）。`gui/` に `[[providers]]`・`providers_include` は無い。

## 意図的に残したもの

- **移行 runbook と読み込みエラーの説明**: `docs/ops/provider-llm-source-migration.md`・`docs/ops/model-routing-migration.md`・`docs/guides/providers.md` の `[[providers]]`・`providers_include`・`[llm_proxy.models]`。「新 binary は読み込みエラーにする」「移行先はこれ」を示す文。
- **廃止を示す説明**: `docs/guides/llm-source.md`・`docs/guides/providers.md` の「tier alias は廃止」。`docs/ops/opencode-go-and-model-catalog.md` §71 の「廃止された」。
- **lane 名**: `frontier` / `standard` / `cheap` は lane 名として残る（alias 名ではない）。
- **API 欄名**: `celeris_tiers`（JSON 欄名。常に空配列）、`RunSummary`・`LaneResolution`・`InFlight`・`RunRoutingAudit` の `provider`（中身は source id）。
- **生成 schema**: `web/api/generated/schema.json` の `celeris/<tier>` 説明文（crates の doc 由来。docs/web の grep から除外）。
- **旧 provider の pin を書く script**: `scripts/portable-providers.py`（と `test_portable_providers.py`）。書き換える `[[harnesses]] adapter`・`[[roles]] adapter` は現行の config が今も読む欄なので消していない（close5/scripts.md の判断）。
- **crates の内部名**: `reload_providers`（`crates/celeris/src/daemon/admin.rs`）、`ProviderSelection` / `ProviderCandidate`（`crates/task-core/src/model_routing.rs`）。挙動には影響しない。
- **migration SQL の歴史的注釈**。
- **本番**: 本番 config の移行は人が `docs/ops/provider-llm-source-migration.md` の手順で行う。この run では本番を触っていない。

## 残した未解決（親の未解決事項に残るもの）

- `scratch.sccache`・cache server の説明文: `crates/task-ops/src/daemon.rs` の doc comment と `web/api/generated/schema.json`・`gui/app/celeris/types.ts`。直すには schema 再生成（`UPDATE_SCHEMA=1` と gui の gen:types）が要る。
- 内部名の整理（上の「意図的に残したもの」の crates の内部名）。
- 本番 config の移行（人の作業）。
