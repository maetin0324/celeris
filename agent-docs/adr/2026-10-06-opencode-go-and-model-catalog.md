# opencode go を LLM source に加え、subscription と self-host の利用可能モデルを catalog で一括管理する

- 日付: 2026-10-06
- 状態: 実装済み（phase 1、人の確認待ち: 本番 config の追記は Fable が配送後に行う）
- 関連: ADR-0024（account pool）、ADR-0049（codex の利用枠確認）、ADR-0053（llm-proxy）、ADR-0069（routing 4 層）、ADR-0080（credential broker）、ADR-0131（cron job）、ADR-0132（provider / llm_source 分離）、ADR 2026-10-04（多目的 model routing）
- 一次情報の調査記録: run artifacts `opencode-go-research.md`（opencode の source `anomalyco/opencode@52a6c35825` を depth 1 で取得して確かめた）

## 背景

人の要望（2026-10-06）: opencode go の subscription を LLM source として使いたい。アカウント画面で opencode go の 5 時間・1 週間・1 か月の枠を見たい。opencode go は利用可能モデルが頻繁に入れ替わるので、claude・codex・self-host（openai_compatible）も含めて、利用可能モデルを自動で発見し一括で管理したい。

現状の制約:
- 利用枠の観測（`task-core::accounts::RateLimitObservation`、`task-core::quota::QuotaWindow`）は `five_hour`・`seven_day` の 2 窓に固定。account pool の adapter は `claude-code`・`codex` の 2 つ。
- モデルの集合は config の `providers.model` / `providers.tier_models` / `[model_routing].models` に手で書く前提。消えたモデルは人が気づいて書き換えるまで routing 候補に残る。

## 一次情報（opencode go）

1. **provider**: `opencode-go`。base URL `https://opencode.ai/zen/go/v1`、SDK は `@ai-sdk/openai-compatible`（models.dev の `api.json` の `opencode-go` 項目。`opencode models opencode-go --verbose` が同じ値を出す）。資格情報は `~/.local/share/opencode/auth.json` の `{"opencode-go": {"type": "api", "key": …}}`（`packages/opencode/src/cli/cmd/providers.ts:485` が書く）。要求は `Authorization: Bearer <key>`（`packages/console/app/src/routes/zen/go/v1/usage.ts:14`、`chat/completions.ts:9`）。
2. **利用量と上限**: `GET https://opencode.ai/zen/go/v1/usage`（Bearer 認証。`packages/console/app/src/routes/zen/go/v1/usage.ts`。公式文書・OpenAPI には載っていない undocumented な endpoint。認証なしで叩くと 401 `AuthError` が返ることを本番とは別の経路で確かめた）。200 の本文:
   ```json
   {"usage": {"rolling": {"status": "ok", "percent": 12, "resetsAt": "2026-10-06T12:00:00.000Z"},
              "weekly":  {"status": "ok", "percent": 40, "resetsAt": "…"},
              "monthly": {"status": "rate-limited", "percent": 100, "resetsAt": "…"}}}
   ```
   `rolling` が 5 時間窓、`weekly` が 1 週間、`monthly` が 1 か月。`status ∈ {ok, rate-limited}`、`percent` は 0..100 の整数。金額は返らない。401 は鍵の不備、403 `EntitlementError` は go の subscription が無い鍵。
   上限そのものは server 側の秘密（`ZEN_LIMITS`）で、文書（`packages/web/src/content/docs/go.mdx:145-152`）は「5 時間 = 月額上限の 20%、週 = 50%、月 = 100%」とだけ言う。
3. **枠切れの実行時信号**: 推論要求への 429 `{"type":"error","error":{"type":"GoUsageLimitError","message":"5-hour|Weekly|Monthly usage limit reached. …"},"metadata":{"limitName":"5 hour"|"weekly"|"monthly"}}` と `retry-after` 秒（`packages/console/app/src/routes/zen/util/handler.ts:505-531`）。`x-ratelimit-*` ヘッダは無い。
4. **モデルの列挙**: `opencode models opencode-go` は models.dev の cache（`~/.cache/opencode/models.json`、5 分 TTL）を読み、`provider/model` を 1 行 1 件で出す（`packages/opencode/src/cli/cmd/models.ts`。`--json` は無い）。gateway 自身は `GET https://opencode.ai/zen/go/v1/models`（認証なし）で OpenAI 形式 `{"object":"list","data":[{"id":…}]}` を返し、CLI より多い（別名を含む）。
5. **claude**: `GET https://api.anthropic.com/v1/models` を llm-proxy の `sources/claude.rs` と同じ OAuth bearer と `anthropic-beta` ヘッダで呼ぶ（Anthropic の公開 API。OpenAI 形式の `data[].id`）。**codex**: `codex app-server` の JSON-RPC `model/list`（`codex app-server generate-json-schema` の `ModelListParams` / `ModelListResponse { data: Model[], nextCursor }`。ADR-0049 の `account/rateLimits/read` と同じ経路）。**self-host**: `GET <base_url>/v1/models`（OpenAI 互換）。

## 決定

### D1. 利用枠の窓に `one_month` を足す

- `QuotaWindow` に `OneMonth`（文字列 `one_month`）、`RateLimitObservation` に `one_month: Option<RateWindow>` を足す。API の `AccountUsageView` と event `quota_estimated` も同じ欄を持つ。`five_hour` / `seven_day` / `one_month` はどれも `Option`。**取得できない窓は `null`（画面では『不明』）で、0 にしない**。
- account pool の採点（`task-dispatch::accounts::evaluate`）は観測できた窓だけの最小値を使う。`ExcludedReason` に `OneMonthExhausted` を足す。model router の `SourceState.quota_windows` には `window_id = "one_month"`（`window_duration_s` = 30 日）で載せる。claude・codex に将来 1 か月窓が来たら同じ欄に入る。

### D2. opencode go は account pool の第 3 の adapter

- `AccountAdapter::OpencodeGo`（文字列 `opencode-go`）。account dir の目印は `opencode/auth.json`（opencode の data dir の形そのまま）。worker には `XDG_DATA_HOME=<account dir>` を渡す（opencode は `xdg-basedir` で data dir を解決する）。`accounts.opencode_dir` を config に足す。本番では `<opencode_dir>/main/opencode` を `~/.local/share/opencode` への symlink か複製にする（docs/ops に手順）。
- **利用枠の確認**は推論を使わず `GET /zen/go/v1/usage` を `Authorization: Bearer` で呼ぶ決定的な HTTP。`rolling → five_hour`、`weekly → seven_day`、`monthly → one_month`、`percent/100 → utilization`、`resetsAt → resets_at`。鍵は account dir の `auth.json` から読んで要求にだけ使い、ログ・events・API に値を書かない（`Debug` は redacted）。401/403 は `not_logged_in` 相当、ネットワーク不可は『不明』（観測を更新しない）。確認は codex と同じく `UsageChecks::poll`（idle の logged-in account を 300 秒おき）と手動（`POST /api/v1/accounts/{adapter}/{id}/check`）で行う。base URL は `accounts.opencode_go_usage_url` で差し替え可能にし、試験は偽 HTTP server に向ける。
- 枠切れ: 観測の `status == "rate-limited"` の窓は `utilization = 1.0` とみなす。run の 429 `GoUsageLimitError` は `limitName` で窓を決めて `utilization = 1.0`・`resets_at = now + retry-after` として記録し、cooldown に入れる（codex の rate_limit と同じ扱い）。

### D3. LLM source `opencode_go`

- `LlmSourceRef::OpencodeGo`（文字列 `opencode_go`）。ACP（opencode）adapter の provider 行で `llm_source = "opencode_go"`、`model = "opencode-go/<model>"`、`account_pool = "opencode-go"` と書く。worker は account pool から選んだ account dir を `XDG_DATA_HOME` で渡すだけで、opencode が自分の auth.json で gateway に認証する（llm-proxy は経由しない）。
- routing: `routing_catalog()` は `opencode_go` の provider 行の `model` / `tier_models` から `ModelProfile`（`Billing::Subscription`、source `opencode-go`）を作る。deployment と tier の割り当ては従来どおり config（`tiers` / `tier_models`）で決める。fallback は `tier_models[tier].fallback`（既存の ModelBinding の仕組み）と provider の順序に従う。
- quota 切れの扱い: pool の全 account が `*Exhausted` なら provider_select から外れ、同じ tier の次の provider（claude-pool・codex・self-host）へ落ちる。ある窓の『不明』は除外理由にしない。

### D4. モデル catalog（SQLite）と発見

- 表 `model_catalog(source TEXT, model_id TEXT, display_name TEXT, first_seen INTEGER, last_seen INTEGER, available INTEGER, capabilities TEXT /*json*/, PRIMARY KEY(source, model_id))` と `model_catalog_overrides(source, model_id, disabled INTEGER, tier TEXT NULL, alias TEXT NULL, note TEXT NULL, updated_at, PRIMARY KEY(source, model_id))`。migration は `0052_model_catalog.sql`。**上書きは別表で、自動更新は触らない**。
- `source` は `claude-oauth`・`codex-oauth`・`opencode-go`・`openai-compatible:<id>`（llm-proxy / routing の source 名と同じ）。
- 発見は `task-core::model_catalog::discover` の純粋な解析（文字列 → `Vec<DiscoveredModel>`）と、`celeris::model_discovery` の取得（コマンド実行・HTTP）に分ける。取得:
  - `opencode-go`: `GET <go base>/models`（認証不要）を一次にし、失敗時は `opencode models opencode-go` の stdout（`opencode-go/` 接頭辞を外す）。
  - `claude-oauth`: `GET /v1/models`（account の OAuth token。pool の logged-in account を 1 つ使う）。
  - `codex-oauth`: `codex app-server` の `model/list`（`includeHidden: false`、cursor を辿る）。
  - `openai-compatible:<id>`: `GET <base_url>/v1/models`。
  - 取得に失敗した source は catalog を変えない（消えたと誤認しない）。
- 結果の反映: 新規は `first_seen = last_seen = now, available = 1`、既存は `last_seen = now, available = 1`、今回見えなかった既存は `available = 0`（行は残す）。変化（追加・消失・復活）は event `model_catalog_changed {source, added[], removed[], restored[]}` に追記する。LLM は呼ばない（dispatcher・store に LLM を入れない）。
- 周期: daemon の tick loop に throttle 付きの job（既定 `model_catalog.refresh_interval_seconds = 3600`、`0` で無効）。手動は `POST /api/v1/llm/models/discover`（任意の `source` 指定）と `celerisctl models discover`。cron の task は作らない（LLM の run を起こす理由が無い）。
- routing への反映: `routing_catalog()` は config 由来の `ModelProfile` に対し、catalog で `available = 0` か override で `disabled = 1` のモデルを `unavailable_reason` 付きで候補から外す。catalog は config を**足さない**（新モデルを自動で routing に入れない。人が override の `tier` か config で決める）。

### D5. 一括管理の画面と API

- `GET /api/v1/llm/models` → `{items: [{source, model_id, display_name, available, first_seen, last_seen, capabilities, override: {disabled, tier, alias, note} | null, routing: {deployments: [...], tiers: [...]}}], last_discovery: {source: {at, ok, error}}}`。
- `PUT /api/v1/llm/models/{source}/{model_id}/override`（本文 `{disabled, tier, alias, note}`）と `DELETE` 同 path。`POST /api/v1/llm/models/discover` は 202 で結果の要約を返す。
- web: `/models` 画面（『モデル』）。source ごとの表（モデル・状態・最終確認・tier/deployment・上書き）、発見ボタン、上書きの編集。nav への追加は providers 画面からの導線 1 つと screens.ts の台帳登録に留める（CoS chat の web task と衝突させない）。

### D6. 試験

- 解析は fixture（偽 CLI の stdout・偽 HTTP の JSON）で決定的に。消失・復活・上書き保持は store の試験。usage の 3 窓と『不明』は task-worker の偽 HTTP server と web の component test。外部ネットワークには出ない。実機（本番 host の opencode go・claude・codex）での確認は Fable が配送後に 1 回行う。

## 却下した案

- models.dev の `api.json` を catalog の一次にする: 公開 catalog で「その subscription で使えるか」を表さない。opencode go は gateway の `/models`、claude・codex は各 API の方が subscription の実態に近い。
- catalog の新モデルを自動で tier に入れる: routing の品質評価が無いモデルが勝手に候補になる。人の override で入れる。
- cron の task として発見を回す: task は LLM run を起こす枠組み。発見は決定的な処理なので tick loop の throttle job にする。

## 付記: 実装突き合わせ（2026-10-06）

phase 1（commit 0bbdafa6）の実装で、本文から変えた点・決めた点。

- **`account_pool` は bool ではなく `AccountPoolSetting`**（`task_core::AccountPoolSetting`、`crates/celeris/src/config/providers.rs`）。`true` / `false` / adapter 名の文字列を受け、acp 行は `account_pool = "opencode-go"` で名前付き pool を指す。行の adapter と名前付き pool が食い違う（claude-code 行で `"codex"` など）と `Config::validate` が拒否する。`llm_source` を省略した acp 行は、pool が opencode-go か model が `opencode-go/` 始まりなら `opencode_go` と推定する。
- **`measured_remaining`**（`crates/task-dispatch/src/accounts.rs`）: `one_month` を持つ観測は、観測できた窓（5 時間・週・月）だけの最大利用率から残りを出す。持たない観測（claude / codex）は従来どおり 5 時間と 7 日の両方が揃うことを要求する。
- **確認結果の写像**（`crates/task-worker/src/opencode_account.rs` の `AccountCheckResult`）: `auth.json` なし・鍵が読めない・401/403 は `AuthFailed`、429 は `Throttled`、通信失敗・timeout・解析不能は `SpawnFailed`（観測は更新しない）。
- **`ModelCatalogChanged` は nil ULID の疑似 task に追記**（`task_core::model_catalog` の疑似 task id、`store/model_catalog.rs`）。events は task ごとの列で、catalog はシステム全体の事柄で task に属さないため。
- **routing への反映は `unavailable_reason` ではなく deployment の除去**: `apply_model_catalog`（`crates/celeris/src/config/model_catalog.rs`）が `available = 0` か `disabled` のモデルを指す deployment を候補から外し、理由（`catalog:unavailable` / `override:disabled`）を `warnings` に書く。上書きの `disabled` は catalog に行が無いモデルにも効く。`tier` / `alias` は routing には使わない。
- **self-host の取得先は `<base_url>/models`**（`crates/celeris/src/model_discovery.rs`）。ADR 本文の `<base_url>/v1/models` と違い、config の `base_url` が `…/v1` を含む前提（qwen は `http://127.0.0.1:18000/v1`）。claude は `<base_url>/v1/models?limit=1000`、token は account dir の `.credentials.json` から読む（更新はしない）。
- **空の発見結果は失敗扱い**（`non_empty`）: 全モデルが「消えた」にならないよう、catalog を変えない。
- **`one_month` を event に載せるのは OpencodeGo の pool の run だけ**（`quota_estimated`、`crates/task-dispatch/src/dispatcher/quota_book.rs` の `quota_windows_for`）。claude / codex の event は従来の 2 窓のまま。
- **run 中の 429 の窓の特定は未実装**（D2 の `GoUsageLimitError` / `limitName` / `retry-after`）。`crates/task-worker/src/provider.rs` に TODO を残した。いまは "usage limit" で Exhausted になり、次の定期確認で窓の値が入る。
- **sticky session は acp の pool に適用しない**: opencode go の run は毎回 pool から account を選ぶ。
- 発見 hook（`POST /api/v1/llm/models/discover`）は daemon 起動時の config の写しを使うので、`[model_catalog]` の変更は再起動後に効く。
