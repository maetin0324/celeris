---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58, 01M45RPPM85XTGYC17WCZQCER1]
---
# model routing（`[model_routing]`）への移行手順

[ADR 2026-10-04-multi-objective-model-routing](../../agent-docs/adr/2026-10-04-multi-objective-model-routing.md) §9 に従う。
この文書は Phase 1 の初版で、後の Phase が節を足す。対象は本番の `~/.config/celeris/config.toml`。
**実行者は人**。本番の設定・daemon・DB を自動では変えない。以下は人が実行する手順。

Phase 4（§9）で変わること:

- `mode = "shadow"` が **decision shadow**（追加呼出しなしで候補 policy の判断を audit に残す）を有効にする。
  **実行 shadow**（候補モデルで実際に生成する）は `[model_routing.shadow]` の opt-in-capped 設定で個別に有効化し、既定 off。
- 過去の routing event を読み取り専用で export し、`celerisctl routing evaluate` で再現できる比較 report（v1）を作る
  （§9.5）。
- DB に migration 0049（実行 shadow の日次上限の共有予約表）が入る（§9.4）。

Phase 1 で変わること:

- `[model_routing]` 節が増えた。中身は `mode`、`[[model_routing.models]]`、`[[model_routing.deployments]]`、
  `[model_routing.policies.<lane>]`（`<lane>` は `frontier`・`standard`・`cheap`）。
- 旧形（`[llm_proxy.models.*]`・`[llm_proxy.sources.*]`・`[[providers]]` の `tier_models`・`model`・`llm_source`）は
  今までどおり読む。起動時に catalog に正規化し、移行先を警告で示す。
- **実行経路は変わらない**。dispatcher と proxy の選択は `mode` によらず従来（legacy）の規則で行う。
  catalog は検証・表示・trace のために作る。旧形のままで移行しなくても動作は同じ。

## 1. 現行 config の控え

1. 稼働中の release を確かめる: `systemctl --user list-units 'celeris@*'`。
2. 設定の控えを権限を保って取る。日付を付け、後で rollback に使う。

   ```sh
   cp -a ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-model-routing-YYYYMMDD
   # providers.d を使っている場合
   cp -a ~/.config/celeris/providers.d ~/.config/celeris/providers.d.bak-model-routing-YYYYMMDD
   ```

3. 控えと稼働中 release の組（release 名・sha）を記録しておく。rollback は**旧 binary と旧 config を組で戻す**（§7）。

## 2. 旧キー → `[model_routing]` 対応表

旧形から導く値は `llm_proxy::legacy_catalog::normalize_legacy_config`（proxy 側）と
`Config::routing_catalog`（`crates/celeris/src/config/model_routing.rs`）が作る。
旧設定から分からない能力・品質・価格・revision は `unknown` か欠測のまま。URL・API key・accounts_dir は catalog に写さない。

| 旧キー | 導かれる値 | 新形での書き場所 |
|---|---|---|
| `[llm_proxy.models.claude]` の `<lane> = "<wire>"`（`claude_oauth` が有効なとき） | model `legacy:claude:<wire>`、deployment `legacy:claude-oauth:<Lane>`（source `claude-oauth`、`allowed_lanes = [<lane>]`、billing subscription） | `[[model_routing.models]]` / `[[model_routing.deployments]]` |
| `[llm_proxy.models.gpt]` の `<lane>`（`codex_oauth` が有効なとき） | model `legacy:gpt:<wire>`、deployment `legacy:codex-oauth:<Lane>` | 同上 |
| `[llm_proxy.models.qwen]` の `cheap` と有効な `[[llm_proxy.sources.openai_compatible]]`（`id = "<id>"`） | model `legacy:qwen:<wire>`、source ごとに deployment `legacy:openai-compatible:<id>:Cheap`（billing self_hosted、`allowed_lanes = ["cheap"]` 固定） | 同上。Qwen の `frontier`・`standard` は読まない |
| `[llm_proxy.sources.*]`（`claude_oauth`・`codex_oauth`・`openai_compatible`） | deployment の `source_ref`（`claude-oauth`・`codex-oauth`・`openai-compatible:<id>`） | source 自体は `[llm_proxy.sources.*]` に残す（移動しない） |
| `[[providers]]` の `tier_models.<lane>`（`unavailable_reason` が無いもの） | model `<model_id か name>`、deployment `provider:<provider id>/<lane>` | `[[model_routing.deployments]]` |
| `[[providers]]` の具体 `model`（`celeris/` を含まず、`llm_source` が `none`/`unknown` でない） | model `<model>`、deployment `provider:<provider id>`（lane は provider の tiers） | 同上 |
| `[[providers]]` の `llm_source` | deployment の `source_ref`（`claude_oauth`・`codex_oauth`・`openai_compatible:<id>` など） | `[[model_routing.deployments]]` の `source_ref` |
| `[llm_proxy] prefer_free` | 3 lane の policy の `prefer_free` | `[model_routing.policies.<lane>] prefer_free` |

`<Lane>` は deployment id の中だけ先頭大文字（`Frontier`・`Standard`・`Cheap`）。設定値の lane は小文字。

### 新形の書き方

旧形から導いた id と同じ id を書くと、その項目を上書きする（出自の警告が出る）。新しい id なら追加になる。

```toml
[model_routing]
mode = "legacy"            # legacy（既定）| shadow | enforce（§8。heuristic の明示 opt-in が要る）

[[model_routing.models]]
id = "legacy:qwen:qwen3.8-27b"   # 旧形由来の id を上書き
revision = "v1"
family = "qwen"

[[model_routing.deployments]]
id = "legacy:openai-compatible:qwen:Cheap"
source_ref = "openai-compatible:qwen"
model_profile_id = "legacy:qwen:qwen3.8-27b"
upstream_model = "qwen3.8-27b"
allowed_lanes = ["cheap"]
host = "local-gpu"

[model_routing.policies.cheap]
min_quality = 0.55
```

使えるキー（不明なキーは起動エラー）:

- `[[model_routing.models]]`: `id`（必須）、`revision`、`family`、`provenance`、`capabilities`、`context_limits`、`quality`、`pricing`。
- `[[model_routing.deployments]]`: `id`・`source_ref`・`model_profile_id`・`upstream_model`（必須）、`allowed_lanes`、`billing`、
  `host`、`region`、`trust_zone`、`external_network`、`retains_data`、`resource_group_id`、`concurrency_limit`、
  `rpm_limit`、`tpm_limit`、`price_override`、`adapter_constraints`。新規 deployment で `allowed_lanes` を省くと 3 lane 全部。
- `[model_routing.policies.<lane>]`: `version`、`objective`（`quality_first`・`balanced`・`resource_first`）、`min_quality`、
  `weights`（`quality`・`cost`・`latency`・`pressure`）、`normalization`（`cost_reference_usd`・`latency_reference_ms`）、
  `constraints`（`allowed_deployments`・`allowed_sources`・`external_network_allowed`・`data_retention_allowed`・
  `required_region`・`max_cost_usd`・`max_latency_ms`）、`fallback`、`escalation`、`local_preference`、`prefer_free`。

`source_ref` に書けるのは、設定にある source だけ: `claude-oauth`（`claude_oauth`）、`codex-oauth`（`codex_oauth`）、
`openai-compatible:<id>`（`openai_compatible:<id>`）、`provider:<provider id>`。

旧 provider ID・account dir・資格情報・実行経路は自動で改名・移動しない。旧キーは Phase 1 では消さなくてよい
（旧キーが選択の正本のまま）。

## 3. 起動時の警告とエラーの読み方

### 警告（起動は続く）

daemon は起動時（と reload 時の読み直し）に `model routing compatibility warning` を `key` 付きで 1 キー 1 回出す。
値はキー・旧出自・移行先だけで、URL や secret は出ない。

| 警告の `key` | 意味 | 対応 |
|---|---|---|
| `llm_proxy.models.<family>.<lane> -> model_routing.models/deployments (legacy)` | 旧 proxy 形から deployment を導いた | Phase 1 では対応不要（情報）。新形に移すかは任意 |
| `providers.<id>.tier_models.<lane> -> model_routing.deployments (legacy)` | provider の `tier_models` から deployment を導いた | 同上 |
| `providers.<id>.llm_source -> model_routing.deployments.source_ref (legacy)` | provider の `llm_source` を source として読んだ | 同上 |
| `model_routing.models.<id> overrides llm_proxy.models/providers.tier_models -> model_routing.models` | 新形が旧形由来の model を上書きした | 意図した上書きか確かめる |
| `model_routing.deployments.<id> overrides llm_proxy.models/providers.tier_models -> model_routing.deployments` | 新形が旧形由来の deployment を上書きした | 同上 |
| `model_routing.policies.<Lane> overrides llm_proxy.prefer_free -> model_routing.policies (legacy)` | lane の policy を新形で書いた | 同上 |

### エラー（起動・reload を拒否する）

| エラー文（`model_routing…` を含む） | 原因 |
|---|---|
| `model_routing.mode=enforce requires the Phase 2 heuristic opt-in` | `mode = "enforce"` に `[model_routing.estimator] kind = "heuristic"` が無い（§8） |
| `model_routing.estimator.kind=<kind>: only "heuristic" is supported` | heuristic 以外の estimator を書いた（mode を問わずエラー） |
| `model_routing.enforce_routes…` | `enforce_routes` が空か、`standalone`・`server` 以外を含む |
| `model_routing.retry.client must be 0` / `retry.total_attempts must be at least 1` / `retry: deadline_secs must be positive…` | `[model_routing.retry]` の値が不正 |
| `model_routing.observation_ttl_seconds: …` / `subscription_windows.<id>.reserve_value_usd …` / `resource_groups.<id>: …` | Phase 2 の state・cost の値が不正（非有限・負・0 の同時数・id の重複） |
| `model_routing.models: empty or duplicate id` / `model_routing.deployments: empty or duplicate id` | id が空か重複 |
| `model_routing.deployments.<id>: unknown model_profile_id` | 存在しない model を参照 |
| `model_routing.deployments.<id>: unknown source_ref` | 設定に無い source を参照 |
| `model_routing.deployments.<id>: legacy identity conflicts` | 旧形由来の同じ id と `source_ref`・`model_profile_id`・`upstream_model` が食い違う |
| `model_routing.deployments.<id>: allowed_lanes is empty` | lane が空 |
| `model_routing.deployments.<id>: Qwen is cheap only` | Qwen の deployment に cheap 以外の lane |
| `model_routing.models.<id>: quality index must be finite and in [0,1]` / `pricing must be finite and nonnegative` | 品質・価格の値が範囲外 |
| `model_routing.policies.<Lane>: weights must be finite, nonnegative and sum to one` など | policy の値が不正（`min_quality must be in [0,1]`、`normalization references must be finite and positive` も同類） |
| `model_routing.policies.<lane>: unknown deployment reference` / `unknown source reference` | `constraints` が catalog に無い deployment・source を指す |

## 4. 検証

この版に `[model_routing]` 専用の dry-run／validate コマンドは無い。次の 2 つで確かめる。

1. **設定の検証（daemon を起動しない）**: 新しい release の `celerisctl` で

   ```sh
   celerisctl config to-harnesses --config ~/.config/celeris/config.toml > /tmp/celeris-config-check.toml
   echo $?
   ```

   このコマンドは `Config::load` を通すので、§3 のエラーがあれば非 0 で終わり、エラー文を出す。exit 0 を確かめ、一時出力を消す。
   `celerisctl` は警告を表示しないので、**警告は daemon の起動ログで見る**（次項）。
2. **daemon 起動ログでの確認**: 再起動か reload の後に

   ```sh
   journalctl --user -u 'celeris@<release>.service' -n 200 --no-pager | grep -E 'model routing compatibility warning|model_routing'
   ```

   で §3 の警告が想定どおりで、エラーが無いことを確かめる。

反映: `[model_routing]` だけの変更は `POST /api/v1/reload`（検証と catalog 構築が成功したときだけ一括で差し替える。
失敗なら旧 catalog のまま。in-flight の読み手は旧 snapshot を使い切る）。`[llm_proxy.*]`（source・listen・models）を
変えた場合は従来どおり daemon の再起動で反映する（[provider と LLM source の移行手順](provider-llm-source-migration.md)）。

## 5. `mode = "legacy"` → `"shadow"`

1. §1 の控えを取ってから `[model_routing] mode = "shadow"` を書く。
2. §4 の検証をして reload する。
3. Phase 1 の `shadow` は**設定値として受け付けて catalog と policy の `mode` に載るだけ**で、dispatcher と proxy の選択は legacy と同じ。
   shadow の比較記録（候補の比較・上限付き shadow 実行）は Phase 4 で入る。Phase 1 で `shadow` にする利点は、
   後の Phase に向けて catalog の値を本番設定で検証しておけることに限られる。

`mode = "enforce"` は §8 の手順で有効にする（heuristic の opt-in が無ければ検証エラー）。

## 6. 指標・選択の確認

Phase 1 の選択は legacy なので、移行前後で次が変わらないことを確かめる。API は `Authorization: Bearer $TOKEN` を付ける
（token は安全に読み込み、ログに残さない。listen が違えば URL を替える）。

- `GET http://127.0.0.1:7710/api/v1/tasks/<task id>/routing`: 最近の task の `runs`（run ごとの lane・provider）が
  移行前と同じ傾向であること。元の event `routing_decided` の `record.resolution.selection.reason` は
  `local_preferred`・`local_full`・`local_down`・`pool`・`fallback`・`sticky` の従来の語彙のまま。
  dispatcher 経由の決定には optional な `record.optimizer`（候補順位の出自の trace）が付くことがある。無くても異常ではない。
- `GET http://127.0.0.1:7710/api/v1/llm/sources`: 各 source の状態。Qwen が cheap だけに対応していること。
- `GET http://127.0.0.1:7710/api/v1/llm/routing/catalog`（Phase 1 で追加する読取専用 API。版にあれば）:
  `mode` と、§2 の対応表どおりの models・deployments・policies が出ていること。secret は出ない。
- proxy に `model = "celeris/cheap"` を送ったときの応答 header `x-celeris-source` が、移行前と同じ source であること
  （Qwen が利用可能なら `openai-compatible:<id>`）。確認リクエストに機密データを入れない。

## 7. rollback

- **設定だけ戻す**: `[model_routing] mode = "legacy"` に戻して `POST /api/v1/reload`。次の run／request から従来処理。
  （Phase 1 では shadow でも選択は同じなので、これは表示上の戻しになる。）新形の節を消したいときは §1 の控えを戻して reload。
- **binary を戻す**: 旧 binary は `[model_routing]` を知らず、設定の不明キーを拒否して起動しない
  （`Config` は `deny_unknown_fields`）。旧 release に戻すときは**必ず §1 で控えた旧 config と組で戻す**。

  ```sh
  cp -a ~/.config/celeris/config.toml.bak-model-routing-YYYYMMDD ~/.config/celeris/config.toml
  ```

  の後、旧 release の `celerisctl config to-harnesses --config …` で exit 0 を確かめてから、
  人が旧 release の service（`celeris@<旧 release>.service`）に戻し、§4 の起動ログを確かめる。
- DB の構造 migration は Phase 1 に無い（event JSON への optional 欄の追加だけ）ので、DB を戻す手順は要らない。
  Phase 2 を含む release は migration 0048 を当てる（§8.5）。

## 8. Phase 2: `mode = "enforce"`（heuristic の opt-in）

Phase 2 の release で変わること:

- `[model_routing]` に state・cost・retry の設定が増えた。**どれも未設定なら unknown か既定値で、0 では埋めない**。
- `mode = "enforce"` は `[model_routing.estimator] kind = "heuristic"` を明示したときだけ受け付ける。既定は従来どおり legacy。
- enforce が選択を変えるのは **dispatcher（run の provider 選択）だけ**。source の状態（残量・cooldown・in-use）で候補を外し、
  同じ lane の別 source を選ぶ。無ければ run を始めずに待つ（defer）。**残量のために lane を下げない**（legacy の `select_tier` 降格は enforce では起きない）。
- proxy（`celeris/<lane>` の要求）の候補順は enforce でも legacy のまま。`enforce_routes` は検証されて保持されるだけで、この版の proxy は使わない。
- proxy の同一要求内 fallback は **mode によらず** 次の規則になる: 失敗の分類ごとの上限、総回数、deadline、stream は最初の byte の前だけ次へ倒す、
  5xx・network が続いた deployment は一時的に外す（breaker）。**401/429 以外の 4xx は次の候補へ倒さない**（従来は倒していた）。
- DB に migration 0048（`llm_proxy_requests` に NULL 可の相関欄と部分索引）が入る。この版では相関欄に値は書かれない。

### 8.1 有効化の前に

1. §1 の手順で config の控えを取る（`config.toml.bak-model-routing-YYYYMMDD`）。
2. 昇格前の DB backup があることを、既存の昇格手順どおり確かめる（0048 は列と索引の追加だけ）。

### 8.2 書き方

```toml
[model_routing]
mode = "enforce"
enforce_routes = ["standalone", "server"]   # 省略時も両方。他の値・空はエラー
observation_ttl_seconds = 300               # 省略時 300。これより古い残量の観測は「不明」（満タンにも枯渇にもしない）
context_safety_margin = 512                 # 任意。run の context 長に積む余裕 token 数

[model_routing.escalation]
quality_failures_per_lane = 2               # 任意。同じ lane の品質失敗がこの回数に達したら 1 段上げる
max_total_attempts = 4                     # 任意。ADR-0069 の既定上限。どちらも 0 は不可

[model_routing.estimator]
kind = "heuristic"                          # enforce の opt-in。heuristic 以外はエラー

# 任意。書かない窓の shadow price は「不明」のまま（0 にしない）
[model_routing.subscription_windows.five_hour]
reserve_value_usd = 2.0
[model_routing.subscription_windows.seven_day]
reserve_value_usd = 10.0

# 任意。self-host の同時数と費用係数（書かない成分は「不明」）
[[model_routing.resource_groups]]
id = "gpu0"
concurrency_limit = 4
usd_per_gpu_second = 0.0005
usd_per_wait_second = 0.0001

# 任意。proxy の fallback（省略時: 401/429/local 3、5xx/network 2、client 0、総 6、deadline 120 秒）
[model_routing.retry]
total_attempts = 6
deadline_secs = 120
client = 0                                  # 0 以外はエラー
```

値は例。reserve_value・単価は運用者が決める（根拠の無い値を入れるより、書かずに「不明」とする方がよい）。

### 8.3 反映と確認

1. §4 の 1（`celerisctl config to-harnesses --config …` が exit 0）で検証する。§3 の Phase 2 のエラーが出たら直す。
2. `[model_routing]` の mode・state・cost・context safety margin・escalation の変更は `POST /api/v1/reload` で dispatcher に原子的に入る。不正なら旧設定のまま。
   **`[model_routing.retry]` は proxy の起動時に読むので、変えたら daemon の再起動が要る**。
3. 確認（API は Bearer 付き。token はログに残さない）:
   - `GET /api/v1/tasks/<task id>/routing`: enforce 後に始まった run の `optimizer.mode` が `enforce`。候補ごとの
     `excluded_reasons`（`quota_exhausted`・`cooldown`・`rate_limit`・`context_transport_unsupported` など）と、最終の
     `source_id`・`model`・`account_id` が出る。lane が要求どおり（下がっていない）こと。
   - GUI の task 詳細「ルーティング」、web の task 実行パネルで同じ内容が見えること。未知は「不明」と出て、$0 に見えないこと。
   - 残量の少ない lane で run が始まらないときは defer。event は残らないので daemon ログで理由を見る:

     ```sh
     journalctl --user -u 'celeris@<release>.service' -n 500 --no-pager | grep -iE 'enforce|defer'
     ```

   - cheap の Qwen 優先が変わっていないこと: §6 の `celeris/cheap` の `x-celeris-source` 確認を繰り返す。
4. この版の制限（異常ではない）: `GET /api/v1/llm/sources` の `deployments` は空、routing の `requests` は空・`audit_incomplete` は false。
   proxy の状態と要求単位の決定はまだ API に流れていない。

### 8.4 戻し方

- **enforce だけ戻す**: `mode = "legacy"`（または `"shadow"`）に書き換えて `POST /api/v1/reload`。次の dispatch から従来の選択
  （`select_tier` の降格を含む）に戻る。defer されていた task は次の tick で従来どおり選ばれる。
  `[model_routing.estimator]`・state・cost の節は legacy でも害は無いので残してよい。
- **retry を戻す**: `[model_routing.retry]` を消して daemon を再起動すると既定値に戻る。client の 4xx を次の候補へ倒す旧動作には
  設定では戻せない（旧 release に戻す）。
- **binary を戻す**: §7 と同じく、旧 config の控えと組で旧 release に戻す。旧 binary は Phase 2 の設定キーを知らず起動しない。

### 8.5 DB

- migration 0048 は列と索引の追加だけで、既存の行・event は書き換えない。Phase 2 の release を一度起動すると DB の版は 48 になる。
- 旧 release に戻すときの DB の扱いは既存の昇格・戻しの手順（昇格前 backup）に従う。この文書では DB を手で変えない。

## 9. Phase 4: shadow（既定 off）と過去ログの offline replay

ADR §7.1・§7.2・§10 Phase 4 に対応する運用手順。Phase 4 の release で変わること:

- `mode = "shadow"` で **decision shadow** が有効になる。primary の選択は legacy のままで、primary の決定の直後に
  **候補 policy**（dispatcher では enforce kernel `dispatch-enforce-heuristic-v1`、proxy では既定
  `llm-proxy:prefer-self-hosted/v1`）の判断を同じ候補・source 状態から計算し、event `routing_shadow_recorded`
  （kind `decision`・status `completed`）に 1 件残す。**上流への追加の HTTP 呼び出し・副作用は起きない**。
  `mode` が `legacy`・`enforce` のときは計算も記録もしない。
- **実行 shadow**（primary とは別の上流で要求のコピーを 1 回だけ生成する）は `[model_routing.shadow]`
  で opt-in し、かつ `execute = true` のときだけ走る。既定は off（queue も予約 store も作らず、送信 0）。
  実行は非同期（primary は shadow の完了を待たない）、tool call は実行せず、出力は SHA-256 と tokens だけを残す。
  日次の上限（request / tokens / effective USD）は共有 DB（migration 0049）の予約で、再起動・daemon handoff・
  2 instance でも合計で越えない。
- `GET /api/v1/tasks/<task id>/routing` の各 run に optional の `routing_shadow` 配列が出る
  （kind・status・reason・候補 model/source・primary との差・tokens・予約の消費）。primary の outcome・attempts・
  review とは別欄で、過去の run は欄が無い。GUI・web の task 画面にも primary と別欄で表示される。

### 9.1 decision shadow の有効化（`mode = "shadow"`）

1. §1 の手順で config の控えを取る（`config.toml.bak-model-routing-YYYYMMDD`）。
2. `[model_routing]` の `mode = "shadow"` を書く。
3. §4 の 1（`celerisctl config to-harnesses --config …` が exit 0）で検証し、`POST /api/v1/reload` で反映する。
   dispatcher（worker run の経路）は reload 直後から decision shadow の記録を始める。
    **llm-proxy 側の decision shadow は proxy の起動時に配線されるので、この release への再起動が効くまで出ない**
    （reload のみの場合、daemon ログに `model_routing.mode = shadow was enabled by reload; the llm-proxy
    decision shadow starts after a restart (dispatcher shadow is active)` の warning が出る）。
4. 確認（API は Bearer 付き。token はログに残さない）:
   - §6 の確認で primary の選択が移行前と変わらないこと（decision shadow は記録だけ。選択は legacy）。
   - `GET /api/v1/tasks/<task id>/routing` の `runs[].routing_shadow` に kind `decision`・status `completed` の
     1 件が出る（新しい run から）。`candidate_model` / `candidate_source` が primary と同じか別か、差の欄が
     付いていること。

### 9.2 実行 shadow の opt-in と上限（既定 off）

`[model_routing.shadow]` を書く。`execute = false`（既定）では queue も予約 store も作らず、送信 0。

**実行 shadow を有効にするには下の全項目が要る**（`candidate_policy` と 4 次元の allowlist、`sample_rate`、
日次の上限 3 種、`max_concurrency`・`max_queue_depth`・`timeout_ms`）。どれか 1 つでも欠けると `Config::load`
（起動・reload 検証）がエラーで、旧設定のままになる。

```toml
[model_routing.shadow]
execute = true                    # 実行 shadow の opt-in。既定 false
candidate_policy = "candidate-v1" # 候補 policy の識別（記録と report に載る）
sample_rate = 0.1                 # [0,1]。decision_id の安定 hash で標本化（率を上げても既存の標本は外れない）

[model_routing.shadow.allowlist]  # 4 次元とも空でないこと。各次元は完全一致か "*"（何でもよい）
task_kinds = ["*"]                #   空の次元は「対象なし」で、"*" とは別
roles = ["*"]
lanes = ["cheap"]
sources = ["openai-compatible:qwen"]   # 呼ぶ側（候補の）source id。"*" 可

# 日次上限（UTC 日界で計数）。開始前に output 上限込みの最悪消費を原子的に予約する
daily_max_requests = 10
daily_max_tokens = 10000
daily_max_effective_usd = 1.5

# queue と実行
max_concurrency = 2
max_queue_depth = 4
timeout_ms = 3000
```

allowlist の値の読み方:

- `lanes` は `frontier` / `standard` / `cheap`。
- `sources` は **呼ぶ側の source id**。`claude-oauth`・`codex-oauth`・`openai-compatible:<id>`（`<id>` は
  `[llm_proxy.sources.openai_compatible]` の `id`）。
- 実行 shadow は **openai-compatible の source だけ**で生成できる。allowlist の `sources` に
  `claude-oauth` / `codex-oauth` を書くと対象判定は通るが、実行は `failed/unsupported_source` で終わる
  （`mode = "shadow"` の proxy 経路では decision shadow の記録は出る）。

実行の性質:

- 上流への送信は要求の**コピー 1 回**（`stream = false`）。tool call の出力は解釈も実行もしない。
  repo / DB / 外部サービスへの副作用は起きない。
- primary が先に resource group の枠を取り、shadow は残り枠だけ使う。primary が予約待ちになると未開始の
  shadow は `dropped/primary_pressure`。primary と同じ非分離の resource group（group 不明なら同じ source）は
  `dropped/resource_group_shared`（primary の予約待ちを生まないための対象外判定）。
- 出力は `output_sha256` と tokens だけ。本文の保持はしない（§9.5 の export も routing の監査欄の allowlist だけ写す）。
- 最悪費用は catalog の deployment（`source_ref`・`upstream_model` 一致）の単価から見る。**単価が揃わない
  （費用が分からない）候補は実行対象にしない**（`dropped/unknown_cost`）。
- 日次上限（request / tokens / effective USD）のいずれかを予約段階で越えると `dropped/cap_exceeded`。
  確定は completed で実測、failed で実測と予約の大きい方、timeout で予約額。確定されない予約
  （process 停止）は reserved のまま、その日の上限に数え続ける。

`mode` と `execute` は独立の設定: `mode = "shadow"` は decision shadow の記録を、`execute = true` は実行
shadow の queue を制御する。`mode = "legacy"` のまま実行 shadow だけ有効にするのも可能（その場合 decision
shadow の記録は出ない）。

有効化の手順:

1. §1 の控えを取る。DB backup（昇格手順）があることを確かめる（§9.4）。
2. 上の節を書く。
3. §4 の 1 で検証（exit 0）し、**daemon を再起動する**。reload で `[model_routing.shadow]` の上限を変えたり
   off にしたりするのは効くが、**起動時に off だった proxy の実行 queue は reload では作られない**
   （daemon ログに `model_routing.shadow.execute was enabled by reload; the llm-proxy execution shadow
   starts after a restart` の warning が出る）。
4. 確認:
   - `GET /api/v1/tasks/<task id>/routing` の `routing_shadow` に kind `execution` の記録が出る（allowlist・
     標本の当たりから）。status `completed`（`input_tokens` / `output_tokens` / `reservation` 付き）か、
     落ちたものは status `dropped` / `failed` と `reason`。
   - 上限の消費（UTC 日）:

     ```sh
     sqlite3 -readonly <db> \
       "SELECT day, COUNT(*), SUM(charged_tokens), SUM(charged_micro_usd) FROM routing_shadow_reservations GROUP BY day ORDER BY day DESC LIMIT 7;"
     ```

     （`<db>` は §9.5 の DB path。本番 DB への書き込みはしない。）

停止して既定 off に戻す（§9.3）:

- `execute = false`（`[model_routing.shadow]` の節を消すのと同じ）にし、`POST /api/v1/reload` する。
  以後の submit は `dropped/off`、queue で待っている未開始分も `dropped/off`（detail `reconfigured_off`）。
  実行中の分は開始時の設定で完走し、その消費は確定する。
- `mode` を `legacy` に戻す。§1 の控えを戻して reload すると Phase 1 の状態へ戻る。

### 9.3 戻し方

- **shadow だけ戻す**: `mode = "legacy"`（`[model_routing.shadow]` がある場合は `execute = false` も）にし
  `POST /api/v1/reload`。次の dispatch / request から従来の選択（§8.4 と同じ）。decision / 実行 shadow の
  記録が止まる（過去に取った event と report は残る）。
- **binary を戻す**: §7 と同じく、旧 config の控えと組で旧 release に戻す。旧 binary は
  `[model_routing.shadow]` を知らず起動しない（`deny_unknown_fields`）。DB は §9.4 の扱い。

### 9.4 migration 0049（additive）と DB

- Phase 4 の release を一度起動すると migration 0049 が当たり、`routing_shadow_reservations`
  （実行 shadow の日次上限の共有予約。**新しい表と索引の追加だけ**で、既存の表・行・event は書き換えない）が
  作られ、DB の版は 49 になる。本文・prompt・credential を持つ列はない。
- **戻すときは設定を戻すだけでよい（§9.3）**。表を残したまま旧 release に戻しても、旧 binary はこの表を
  読まない・書かないので選択は従来どおり。§7・§8.5 の binary rollback の手順（旧 config と組）に従う。
- 昇格（新しい release へ）前に DB backup（昇格手順）を取っておく。DB を手で変えない。

### 9.5 offline replay: `routing export` / `routing evaluate`

過去の routing event（`routing_decided` / `routing_shadow_recorded` / outcome / `llm_proxy_requests` の
相関欄）を読み取り専用で抽出し、JSONL + manifest（schema version 1）にした dataset から、
`legacy` / `heuristic` / `shadow_recorded` の 3 政策の判断を再現して report v1（JSON、任意で Markdown）を書く。
同じ dataset（と seed）からは同じ report が byte 単位で再生成される。

DB path の確認: 認証付き `GET /api/v1/config` の `db`、または `[db].path`（本番は
`/local/celeris/data/db/celeris.sqlite3` が多い）。

```sh
# export: daemon 稼働中（WAL あり）でも読み取り専用で開き、DB / -wal / -shm / -journal の中身も mtime も変えない
celerisctl --db /local/celeris/data/db/celeris.sqlite3 \
  routing export \
  --out /tmp/routing-dataset-YYYYMMDD \
  --since 2026-09-01T00:00:00Z --until 2026-10-01T00:00:00Z \
  --seed 7 \
  --policy-hash <policy の識別> --catalog-hash <catalog の識別> --estimator-hash <estimator の識別>

# evaluate: DB を開かない。同じ dataset 2 回で report.json / report.md は byte 一致
celerisctl routing evaluate \
  --dataset /tmp/routing-dataset-YYYYMMDD \
  --out /tmp/routing-report.json \
  --markdown /tmp/routing-report.md
```

- `--db`（全体の flag）は `routing export` だけが使う。**明示した `--db` だけ**読み取り専用で開く
  （`CELERIS_DB` / `CELERIS_RUN_DB` / `./celeris.sqlite3` の既定は解決せず、無い DB は作らない）。
  store を通さない（migration はしない）。daemon 稼働中の DB を直接読むのはこの経路の読み取り専用の
  ためであるが、**運用では昇格・定期 backup の写しを `--db` に与えることを推奨する**
  （live DB の `file:…?mode=ro&immutable=1` は WAL が無いときだけ使われる。WAL ありは通常の
  read-only open）。
- `--policy-hash` / `--catalog-hash` / `--estimator-hash` は export が設定を読まないため呼び手が渡す
  （省略時は `unspecified`）。manifest の再現性識別に使われる。
- `--baseline <file>`（evaluate）は RouterBench 型の baseline（下）を `external_benchmark_baseline` に
  **別欄**で載せる。Celeris の観測と混ぜない。

dataset（`dataset.jsonl` / `manifest.json`、`schema = "celeris.routing.dataset.v1"`）:

- manifest: `policy_hash` / `catalog_hash` / `estimator_hash`、期間（`from_utc` / `until_utc`）、抽出条件、
  masking（`allowlisted audit metadata only; no prompt/response/credential/event JSON`）、split の規則、
  `seed`、`rows`、`missing_rate`（`features` / `outcome` / `cash` / `api_latency` / `task_wall`）。
- 行（1 行 = routing の決定 1 件と相観測）: `task_id` / `split` / `run_id` / `decision_id`、
  `primary_model` / `primary_source`、`candidates`（model / source / `eligible` / `score` / 推定 cost /
  `excluded_reasons`）、`task_kind` / `role`、`acceptance_passed` / `review_passed` /
  `failed_criterion_ids`、`cash_usd` / `effective_usd`、`api_latencies_ms`（相関の取れた
  `llm_proxy_requests` のみ）/ `task_wall_ms`、`retries` / `escalated` / `quota_exhausted`、
  `shadows`（kind / status / reason / 候補 model・source / cost / latency）。
- split は **task 単位**（SHA-256(seed big-endian ‖ task_id) の先頭 8 byte を mod 10: 0-6 train / 7
  calibration / 8-9 test）。同一 task の retry と review は同じ split に属し、別 split へ漏らさない。
  行の `split` が seed と不一致だと evaluate は拒否する。

report（`report.json`、`schema = "celeris.routing.report.v1"`）:

- `policies`: key は `legacy`（実際の primary）/ `heuristic`（記録された候補のうち eligible で score 最大の
  再評価）/ `shadow_recorded`（kind `decision`・status `completed` の記録）。各 `PolicyReportV1`:
  `decisions` / `observed_outcomes`、`acceptance_success_rate` / `failed_criteria`、
  `cash_usd_mean` / `effective_usd_mean`（実測）と `estimated_cash_usd_mean` /
  `estimated_effective_usd_mean`（候補の推定を別欄）、`api_latency_p50_ms` / `api_latency_p95_ms`
  と `task_wall_p50_ms` / `task_wall_p95_ms`（別欄で混ぜない）、`retry_rate` / `escalation_rate` /
  `quota_exhaustion_rate`、`constraint_violations`（= 0 が gate）、`source_ratio`、
  `unknown_rate` / `timeout_rate` / `drop_rate`、`counterfactual_coverage`、`unknown_reason`。
- **未選択モデルの品質を捏造しない**: 候補 policy が選んだ model が実際の primary と異なる行は
  task の合否が観測されていないため unknown として数える（`counterfactual_coverage` は観測済み outcome の
  割合で、低い場合は反実仮想の範囲が狭いことを report に残す）。
- `paired_metrics`（`APGR` / `AIQ` / `IBC`）: 同一 task で weak / strong / routed の観測済み合否と cost が
  揃った **paired outcome が無いと undefined 理由を返す**。CLI（v1）には paired 入力を渡す口は無い
  （`task_ops::routing_replay::evaluate_with_paired` が口。必要なら `--paired <file>` 相当を足す）。
- `--policy <legacy|heuristic|shadow>` を複数回付けると report の `policies` を絞る
  （`shadow` → key `shadow_recorded`）。

RouterBench 型の外部 baseline（`--baseline <file>` の JSON）:

```json
{ "name": "routerbench-<dataset>-<pin>", "models": { "claude": { "quality": 0.83, "cost_usd": 0.0031 }, "gpt": { "quality": 0.80, "cost_usd": 0.0025 } } }
```

- `name`（識別）と `models`（model 名 → `quality` / `cost_usd`、未観測は省略可）。
  `external_benchmark_baseline` に**別欄**で載るだけ。Celeris の観測指標や `policies` とは混ぜない。
  版 pin・出所（commit / 日付）を `name` か report の隣で管理する。
