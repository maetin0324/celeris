---
tasks: [01M4MAYXS5C7438RAB6TRZK60W]
status: done
title: LLM source・harness 実装の進捗
updated: 2026-10-12
---

# LLM source・harness 実装の進捗

## 全体検査（closeout、HEAD `674fb840` + 本 unit の文書のみ）

crates/ は変えていない。実装は runtime / api の各 unit（`2026-10-11-llm-source-harness-runtime/`、`2026-10-11-llm-source-harness-api.md`）で入った。

- `cargo clippy --workspace -- -D warnings`: exit 0。`Finished \`dev\` profile`、警告 0。ログ `artifacts/clippy.log`。
- `TMPDIR=/local/x/cl bash scripts/dev/test-parallel.sh`: exit 0、`test-parallel: ok`。
  - nextest: `4900 tests run: 4900 passed, 13 skipped`（約 280 秒）。
  - doc-test: exit 0。
  - `CELERIS_TEST_SUMMARY`: `passed 4903, failed 0, ignored 14, nextest_exit 0, doctest_exit 0, summary_parsed true, tmp_leftovers 0, userns null`。ログ `artifacts/test-parallel.log`。
- 既知の sandbox 失敗（browser・launcher・credentiald・CDP 系）: この run では 1 件も出なかった。`CELERIS_USERNS_TESTS` / `CELERIS_ISOLATION_TESTS` を立てていないので userns 依存の試験は opt-in されず、summary の `userns` は null。よって「sandbox で落ちる」分類は今回の結果には当たらない。
- TMPDIR の注: run の既定 TMPDIR は長く、unix socket の `SUN_LEN`（108 byte）を超えて 100 件以上落ちる（前段 runtime closeout の記録と同じ）。短い `/local/x/cl` で流した。

## 本番への移行（人が実施）

本番 daemon・DB・config はこの unit では変更していない。本番 config はこの実行環境に見つからず、以下は移行の手順である。人が実行する。

### 1. 事前準備

1. daemon を止める前に、現行 config（`~/.config/celeris` 配下）と DB を退避する。DB は daemon 停止後に `sqlite3 <DB> ".backup '<退避先>/celeris.db'"` で取る（WAL を含めて一貫した写しになる）。`<DB>` は現行 config の store の path。
2. 退避した config と DB の sha256 を控える。失敗時の戻しに使う。

### 2. config の書き換え（一回の更新で行う。旧形式との併用はしない）

| 旧（削除） | 新（置き換え先） |
|---|---|
| `[[providers]]` の `id`・`adapter`・`account_pool`・`accounts`・`max_runs`・`models`・`env_keys`・`credential_refs` | `[llm_sources.<id>]`（`kind`・`api_format`・`accounts`・`max_runs`・`models`・`credential_ref`） |
| `[[providers]]` の `adapter`・`command`・`args`・adapter `settings` | `[harnesses.<id>]`（`kind`・`api_formats`・`source_kinds`・`capabilities`・`command`・`args`） |
| provider の `tiers`・`tier_models` | 削除。model は `[[cos.route_candidates]]` か `[route_defaults.<harness>]` の `candidates` で具体的に指定 |
| `providers.d/` と `providers_include` | 削除。中身は上の `[llm_sources.*]` と `[harnesses.*]` へ移す |
| `[llm_proxy.models]` の tier alias（langmem・LDR・paperqa が使っていたもの） | 削除。各利用箇所に具体的な `source`・`model` を書く（例: `[[llm_routes]]` の `harness`・`source`・`model`・`via`・`from_api`・`to_api`） |
| CoS の provider 指定 | `[[cos.route_candidates]]` に `{ source, model }` を優先順で並べる（`RouteCandidateConfig` は `source` と `model` のみ） |

書き換えの例は `config/celeris.multi-account.example.toml`（`[llm_sources.*]`・`[harnesses.*]`・`[route_defaults.*]`）と `config/celeris.research.example.toml`（`[[llm_routes]]`）にある。

- 旧い `[[providers]]`・`providers.d`・`providers_include` が 1 つでも残ると、loader が読み込みエラーで拒否し、新しい書き方を案内する（`legacy_providers_rejected_with_new_config_guidance`）。黙って無視はしない。
- `account_limits` の旧 key（`adapter`）は、migration 0068 で `claude-code`→`claude_oauth`、`codex`→`codex_oauth`、それ以外はそのまま `source_id` に写る。写った `source_id` が新 config の `[llm_sources.<id>]` と一致するかを確認する。一致しない行は手で直すか、新 source の id に合わせる。

### 3. DB migration

- 新 daemon の起動時に migration が走る（`SCHEMA_VERSION` = 69）。0068 は transaction 内で `tasks` の harness 欄・`runs` の `selected_*` 欄・`account_limits` の `(source_id, account_id)` key を作る。0069 は `model_role_assignments`・`model_role_scopes` を削除する。
- stats・cooldown・account 上限は失わない。`account_limits` の行は 0068 で移る。credential ファイル・account login state・events は移動・削除しない。
- 起動時（`daemon/bootstrap.rs`）に `resolve_migrated_task_harnesses` が走り、`worker_hint.adapter` を登録済み harness と照合する。解決できない task は `harness` を null のままにし、`harness_migration_unresolved` を task ごとに 1 件追記する。

### 4. reload と確認

1. 新 config を置いて daemon を再起動する（reload）。起動ログに config エラーが無いことを確認する。旧 block が残っていれば起動時に読み込みエラーで止まる。
2. 確認項目:
   - `GET /api/v1/llm/sources`・`GET /api/v1/harnesses`（api-v2 の新 API）で、source と harness が config どおりに見える。
   - CoS が `claude_oauth` 源で起動でき、各用途（coding・軽い仕事・langmem・LDR・paperqa）の route が適格であることを、設定の `route_defaults` と `llm_routes` に照らして確かめる。
   - `harness_migration_unresolved` の event を探し（`GET /api/v1/events` などで type を絞る。経路は現行 API で確かめる）、対象 task の harness を人が決める。
   - `account_limits` の行数を DB で数え、移行前の行数と一致することを確かめる。
3. 失敗時の戻し: daemon を止め、退避した config を戻し、退避した DB を戻す。旧版 daemon（`celeris@<旧 sha>`）を再起動する。migration 0068 は DB を書き換えるので、DB を戻さずに旧 daemon を動かさない。

## 提案

- `test-parallel.sh` は TMPDIR の長さを先に検査し、長すぎたら警告するか短い base を使う。今回も run の既定 TMPDIR では 100 件以上が SUN_LEN で落ちた。
- 試験で作る unix socket の path は、TMPDIR に依らず短い dir（`/tmp` 直下の専用 dir など）を使う helper にする案を別 task で検討する。
- 本番 config の実物が手元に無いため、移行前に現行 config を取り出して上の表と突き合わせる作業は人が行う。

## ADR 修正

`agent-docs/adr/2026-10-11-llm-source-harness-selection.md` を人の redesign 決定に合わせて改訂した。

- `worker_hint.adapter` は task 選択経路から除去。既存 task は migration で `harness` 欄へ移し、解決不能なら null と `harness_migration_unresolved` event を記録する。
- `[llm_proxy.models]` の tier alias は移行期間なしで削除し、langmem・LDR・paperqa は具体的な source/model を設定する。
- 通常 task は `[route_defaults.<harness_id>]` の優先順 `candidates` を `default_route_policy` が選ぶ。明示候補が無い場合は fail closed。coding の Claude/Codex、軽量仕事の Qwen を例示した。
- CoS は `claude_code` harness に固定し、`[cos.route_candidates]` の候補を同じ admission で選ぶ。task 01M4E0WA5Z の代替 source fallback を保持する例を追加した。
- lane/Tier は source/model 対応に使わず、選択入力にするかは task 01M4M8D4AQ に委ねる。`RouteRequest.lane` は維持。
- 本番設定はこの実行環境に見つからなかった。model ID は repo の production host 用 config example と model catalog で確認した。

## 確認

以下の文書検査はすべて exit 0。

- `bash scripts/dev/check-doc-links.sh`
- `bash scripts/dev/check-adr-numbers.sh`（182 files）
- `bash scripts/dev/progress-index.sh --check`
- `sh "$CELERIS_WU_SCOPE_PATHS"`（変更は ADR と本進捗ファイルのみ）
- `git diff --cached --check`
