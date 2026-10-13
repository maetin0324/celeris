---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: provider 層の再設計 — docfix（移行 runbook と注釈から廃止済みの /providers・tier alias・旧設定互換の案内を消す）
completed: 2026-10-13
updated: 2026-10-13
---

# docfix: 移行 runbook と注釈の旧い案内を消す

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。codex reviewer の差し戻し（移行 runbook の後半に旧手順、`stats.rs`・試験の注釈に `GET /providers` 前提、進捗 file 末尾の空行）への対応。

## 直したこと

- `docs/ops/provider-llm-source-migration.md`: 「選択結果」の表の途中の空行（表が切れていた）を消した。tier alias の説明を「廃止され読み込みエラー。旧 alias の選び方は `route_defaults` の具体的な候補として宣言する」に直した。旧 337 行以降（`GET /api/v1/providers` での確認、`qwen/cheap`・`celeris/cheap`・`celeris/standard` の proxy 確認、`config to-harnesses` と providers API での旧 rollback、「旧設定は互換で読める」、`[[providers]].concurrency` を外す手順と `GET /providers` の観測値、`routing_decided` の `selection.reason`（今は記録されない）の表）を消し、次に置き換えた:
  - 前提: config とリリースは対で切り替え・対で戻す。`verify.sh` は本番 config path を読むので移行後 config を `CELERIS_CONFIG` で渡す。
  - 切替: 上限の控え（`GET /llm/accounts`）→ config 編集 → 新リリースの `celerisctl routing show`（CoS の `route_candidates` もここで確認）と `verify.sh` → config を置いて `promote.sh`（`live_ok` 偽で停止 → 起動）→ Qwen の暗黙アカウントに `PATCH /llm/sources/qwen/accounts/qwen {"max_runs":1}` → `GET /health`・`/llm/sources`・`/harnesses`・`/llm/accounts` → proxy に `qwen/qwen3.8-27b` を送り `x-celeris-source` を確認 → task を流して `routing_decided` の harness・source（`resolution.provider`）・model と `GET /tasks/{id}/routing` を照合。
  - rollback: config を旧版の backup に戻す → 前の版の `celerisctl config to-harnesses` で検査 → `rollback.sh` でリリースを前の版に戻す（schema が新しすぎる場合の `--restore-db` の判断を含む）→ 確認 → DB に残る上限の照合。
  - LangMem を proxy に向けるときの `model` は `qwen/qwen3.8-27b` の形（proxy は `<source_id>/<model_id>` だけを受ける）と注記。
- `crates/task-api/src/stats.rs`: 冒頭の注釈を「LLM source 別の run 集計。キーは `WorkerStarted.provider`（source ID）。`GET /llm/accounts` の暗黙アカウントの stats に使う」に直した。field・関数の注釈も同じ語に。
- `crates/task-api/tests/daemon_providers_config.rs` → `daemon_clusters_config.rs` に改名し、冒頭の注釈から `GET /providers` を外した。中の試験（daemon の watch・clusters・config・schema）は今の API の試験なので残した。旧 `/providers` の試験は既に edges-fix で消えている。
- `docs/api/v1/api.md`: §5.8 を「LLM source 別の集計」に、§3.29 の参照語を合わせた。§3.36 の `used_by` が走査する設定を実装（`[adapters.*].env_from_secrets`・`[knowledge.langmem].api_key_secret`・`[llm_sources.<id>].credential_ref`）に合わせ、例の `scope` を実装の値 `source` にした。
- `crates/task-api/src/types.rs`（`SecretUse` の doc）: `scope` を `"adapter" | "source"` に。`UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` で `docs/api/v1/api-v1.schema.json` を再生成し、`web/api/generated/schema.json` に同じ内容を写した。
- `crates/celeris/src/daemon/adapters.rs`・`crates/celeris/src/daemon/tick_loop.rs`: 注釈の「adapter/provider」「proxy の `celeris/cheap`」を今の仕組みの語に。
- 同種の残り（grep で洗った）:
  - `docs/guides/llm-source.md`: §3 の `celeris/<tier>` による選択の説明を `<source_id>/<model_id>` の中継に、§5 の `[llm_proxy.models.claude]` 例を削除、§8 の `/v1/models` で `celeris/*` を見る・`gpt/cheap` を送る・「4 つのプロバイダ」を向ける手順を具体的な source/model に。
  - `docs/guides/knowledge.md`: LangMem の設定例から `[[providers]]`・`celeris/cheap`・壊れた anchor（`providers.md#同時実行数はアカウントで決める`）を消し、`[harnesses.langmem]`・`[route_defaults.langmem]` と `qwen/qwen3.8-27b` に。
  - `docs/ops/model-routing-migration.md`: `providers.d` の控え、`selection.reason` の旧語彙、Qwen の cheap 対応、`celeris/cheap`・`celeris/<lane>` の proxy 確認を今の形に。
  - `docs/ops/opencode-go-and-model-catalog.md`: runbook の消えた節への anchor を api.md §3.29a に。
  - `docs/ops/selfdeploy.md`・`scripts/selfdeploy/lib.sh`: 置き場の説明から `providers.d/` を外した。`scripts/selfdeploy/verify.sh`: 検査 3 の説明・dry run・記録文の `providers` を、実際に叩いている `llm/sources`・`harnesses` に。
  - 試験 fixture の要求 model `celeris/cheap`（`crates/celeris/src/config/tests.rs` 2 箇所、`crates/celeris/src/daemon/routing_shadow_tests.rs`、`crates/llm-proxy/src/{shadow_tests,selection_state_tests,log}.rs`）を `qwen3.8-27b` / `qwen/qwen3.8-27b` に。値そのものを判定しない試験だけ。
- `agent-docs/progress/2026-10-11-llm-source-harness-runtime/dispatch-switch.md`: 末尾の空行を消した。

意図的に残したもの: ADR の経緯説明、過去の進捗記録、`docs/frontend/UX_AUDIT.md` の「（削除済み）」付き記述、api.md の改訂履歴と §3.19 の削除記録、読み込みエラーを出す `config/mod.rs`・`llm-proxy/src/config.rs` の検出と文言、`[[providers]]` を拒否することを試す `config/tests.rs` 冒頭、旧 alias が `unknown_source` になることを試す `llm-proxy/tests/proxy_integration.rs`、selfdeploy の migrate 試験と release inventory の fixture。

## 証拠

- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`（exit 0）
- `sh scripts/dev/check-adr-numbers.sh` → `ok (198 files)`（exit 0）
- `sh scripts/dev/progress-index.sh --check` → `ok`（exit 0）
- `python3 -I scripts/dev/check-architecture-map.py` → `OK: 353 件のパスを確認した`（exit 0）
- `git diff --check` → 出力なし（exit 0）
- `CARGO_TARGET_DIR=/local/celeris/data/scratch/ap-docfix-target TMPDIR=/tmp cargo clippy --workspace -- -D warnings` → exit 0
- `CARGO_TARGET_DIR=… TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0、passed 4995・failed 0・ignored 14（nextest 167 binaries + doctest 10）

## 未解決事項

- runbook の手順は文書だけの変更で、本番では実行していない（本番 config・DB・service には触れていない）。

## 提案

- `crates/task-api/src/handlers/accounts.rs` の暗黙アカウントの `stats` は `ImplicitAccountLive.models`（model id）をキーに `StatsState::view` を引いているが、`StatsState` のキーは `WorkerStarted.provider`（source ID）なので、暗黙アカウントの `runs` などは常に 0 になっている疑いがある。`live.source` をキーにする修正と試験を別 WU で検討する（今回は注釈の修正に留めた）。
- `docs/ops/provider-llm-source-migration.md` の「移行後の TOML」の `[knowledge.langmem]` は `model = "qwen3.8-27b"` で、`base_url` が proxy なら `qwen/qwen3.8-27b` が要る。本文に注記したが、本番の接続先を人が確かめて TOML 例を決める。
