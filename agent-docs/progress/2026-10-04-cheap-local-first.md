---
title: cheap lane のローカル優先（Qwen を先に選ぶ）
tasks: [01M44G5KKF8VJ0ARJH8J843T7D]
status: done
updated: 2026-10-04
---

# cheap lane のローカル優先（Qwen を先に選ぶ）

完了日: 2026-10-04。設計は [ADR-0132 付記（2026-10-04、cheap lane のローカル優先）](../adr/0132-provider-llm-source-split-and-cheap-qwen.md) L1〜L8。

## 何を直したか

本番の runs（2026-10-02 以降）では cheap の routing 422 件のうち Qwen（`acp` / `qwen-local/qwen3.8-27b`）は 2 件だけだった。
`select_provider_for` がアカウントプールの行を score で選び、プールを持たない行（`opencode-qwen`）を `best_pool.or(fallback)` の fallback に回していたため。

- cheap lane の worker run は、順位付けの前にローカルの行（L1）を設定順に見て、空いていて health が落ちていなければ選ぶ。
- ローカルが満杯（`[[providers]].concurrency`、既定 1）・不通（`GET <base_url>/models`、3 秒、60 秒キャッシュ）・cooldown 中・adapter 指定に合わないときだけ従来の順位付けに倒す。見送ったローカルの行は fallback からも外す。
- standard / frontier・reviewer run・CoS の対話 run・継続セッションは変えていない。
- `routing_decided` の `record.resolution.selection` に `reason`（`local_preferred` / `local_full` / `local_down` / `pool` / `fallback` / `sticky`）と見た候補を残す。
- `[execution] cheap_local_first`（既定 `true`）で切れる。

## 変更した場所

| 層 | 場所 |
| --- | --- |
| 記録の型 | `crates/task-core/src/model_routing.rs`（`ProviderSelection` ほか、`LaneResolution.selection`） |
| 選択 | `crates/task-dispatch/src/dispatcher/provider_select.rs`（前段と候補の記録）、`policy.rs`（`offers` / `adapter_of`）、`dispatcher.rs`（`set_local_providers` / `set_local_provider_probe` / probe キャッシュ）、`dispatch_run.rs`（`prefer_local = true` と記録） |
| 設定 | `crates/celeris/src/config/providers.rs`（`local_cheap_providers`）、`config/execution.rs`（`cheap_local_first`）、`daemon/bootstrap.rs`・`daemon/admin.rs`（起動と reload の配線） |
| 試験 | `crates/task-dispatch/src/dispatcher/tests/cheap_local_first.rs`（6 件）、`crates/celeris/src/config/tests.rs` の `cheap_local_first_*`（4 件）、`policy/tests.rs`、`model_routing.rs` の serde 試験 |
| 生成物 | `docs/api/v1/{api-v1,event}.schema.json`、`gui/app/celeris/types.ts`、`web/api/generated/{schema.json,types.ts}` |
| 文書・設定例 | `docs/ops/provider-llm-source-migration.md`（節「cheap lane のローカル優先」）、`docs/architecture-map.md`、`config/celeris.example.toml`、`config/celeris.acp-opencode.example.toml` |

## 証拠コマンドと結果（2026-10-04）

| コマンド | 結果 |
| --- | --- |
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest Summary `3920 tests run: 3920 passed (1 slow), 12 skipped`、`nextest_exit 0`・`doctest_exit 0`（`CELERIS_TEST_SUMMARY` の `passed: 0` は既知の集計パーサーの数え漏れ。件数は Summary の原文） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo test -p task-dispatch --lib cheap_local_first` | exit 0、6 passed |
| `cargo test -p celeris --lib config::` | exit 0、113 passed（`cheap_local_first_*` 4 件を含む） |
| `cargo test -p task-core --lib model_routing` | exit 0、4 passed |
| `UPDATE_SCHEMA=1 cargo test -p task-core --lib` / `-p task-api --lib` / `-p task-worker --lib`、`pnpm gen:types`（gui）、`node scripts/gen-types.mjs`（web） | exit 0。生成物 5 ファイルに `local_preferred` が入った |
| `pnpm typecheck`（gui、`react-router typegen && tsc -b`）/ `pnpm typecheck`（web、`tsc -b`） | exit 0 / exit 0 |
| `bash scripts/dev/check-adr-numbers.sh` / `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` / `bash scripts/dev/check-doc-links.sh` / `python3 scripts/dev/check-architecture-map.py` | すべて exit 0（`ok (136 files)`・`ok`・`ok`・`232 件のパスを確認`） |

試験が確かめること:

- `cheap_local_first_picks_local_when_free`: cheap でローカルが空いていればローカル（`local_preferred`）。
- `cheap_local_first_falls_to_pool_when_local_full`: 満杯ならプール（`local_full`）。probe は呼ばない。
- `cheap_local_first_falls_to_pool_when_local_down`: 不通ならプール（`local_down`）。60 秒以内は再 probe しない、61 秒後は再 probe する。プールも使えないときに不通のローカルを fallback で選ばない。
- `cheap_local_first_standard_lane_is_unchanged`: standard は従来どおりプール。reviewer の経路（`select_provider`）は cheap でも従来どおりで probe しない。
- `cheap_local_first_unsupported_adapter_goes_to_pool`: adapter 指定に合わなければプール（`pool`、候補は `unsupported`）。
- `cheap_local_first_routing_decided_records_reason`: tick を通して `routing_decided` に `selection.reason` が残る（`local_preferred` と `local_down`）。

試験は偽の probe を差し込み、外部ネットワークにも実ポートにも出ない。dispatcher・store に LLM 呼び出しは入れていない（足した I/O は `GET /models` の probe だけ）。

## 未解決事項

- 本番への反映は未実施。この版の昇格と daemon の再起動は人が行う。本番 config は変更不要（`opencode-qwen` は `tiers = ["cheap"]`・`concurrency = 1` で、導出される `llm_source` は `openai_compatible:qwen`、probe 先は `[[llm_proxy.sources.openai_compatible]] id = "qwen"` の `http://127.0.0.1:18000/v1`）。確認手順は `docs/ops/provider-llm-source-migration.md` の節「cheap lane のローカル優先」。
- probe は tick の中で同期に走る（ADR-0052 D1 の知識 probe と同じ）。トンネルが応答しないまま接続だけ受ける状態では、60 秒に 1 回、最長 3 秒 tick が止まる。
- GUI / web の画面には `selection` をまだ出していない（生成型だけ更新）。

## 提案

- reviewer run も cheap ではローカルを先に選ぶかは人の判断にしたい。今回は外した（Qwen が書いたものを Qwen が合格にする組を既定にしない。GPU 1 枚の枠を review が塞がない）。必要なら `[reviewer]` 側の切り替えを足す。
- `GET /providers` と providers 画面に、ローカルの行の health（直近の probe 結果）と `selection.reason` の内訳を出すと、Qwen が使われているかを DB を引かずに見られる。
- probe を tick の外（`tunnel_prober` と同じ専用スレッド）へ移すと、応答しないトンネルで tick が止まらない。
