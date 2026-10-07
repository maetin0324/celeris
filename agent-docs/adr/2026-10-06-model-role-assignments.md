# catalog のモデルを legacy の役割（frontier / standard / cheap）へ画面で割り当て、その割り当てで routing を動かす

- 日付: 2026-10-06
- 状態: 実装中（phase 1）
- 関連: ADR 2026-10-06（opencode go と model catalog。D4/D5 と付記「tier / alias は routing には使わない」を本 ADR で改める）、ADR-0012（providers の model）、ADR-0017 M1（providers.d と reload）、ADR-0024（account pool）、ADR-0053（llm-proxy の `models.<source>`）、ADR-0069（routing 4 層）、ADR-0132（provider / llm_source 分離）、ADR 2026-10-04（多目的 model routing・shadow）

## 背景

人の要望（2026-10-07）: opencode go で扱えるモデルは幅が広い。画面で、catalog のモデルを legacy の役割（frontier / standard / cheap の lane。provider ごとの `tier_models` と `[llm_proxy.models.<source>]` に当たるもの）に手動で割り当てたい。配送後に人が本番で割り当て、その後 routing を shadow で動かす。

現状の制約:
- `model_catalog_overrides.tier` は人が書けるが routing に使われない（ADR 2026-10-06 付記）。routing に入るモデルは config の `[[providers]].model` / `tier_models` と `[llm_proxy.models.<source>]` だけ。
- 役割ごとのモデルは 3 か所（dispatcher の `TieredAdapter.models`、llm-proxy の `normalize_legacy_config`、`Config::routing_catalog()`）が config から別々に組み立てる。同じ config を見るので今は一致するが、DB の割り当てを足すなら 3 か所が同じ解決を見る必要がある。
- `POST /api/v1/providers` の `account_pool` は bool で、acp 行の `account_pool = "opencode-go"`（名前付き pool）を書けない。

## 決定

### D1. 割り当ては `source × 役割 → model_id` の表（SQLite）

- 表 `model_role_assignments(source TEXT NOT NULL, tier TEXT NOT NULL, model_id TEXT NOT NULL, note TEXT, updated_at INTEGER NOT NULL, updated_by TEXT NOT NULL, PRIMARY KEY(source, tier))`。migration `0053_model_role_assignments.sql`（main の次の空き番号。並行 branch の 0053 と交差したら後から main へ入る側が振り直す）。
- `source` は catalog と同じ名前（`claude-oauth` / `codex-oauth` / `opencode-go` / `openai-compatible:<id>`。`CatalogSource::is_valid` で検査）。`tier` は `frontier` / `standard` / `cheap`（`Tier` の文字列）。
- `model_id` は catalog の `model_id`（`<source>/` 接頭辞を付けない。opencode go は `opencode-go/<model>` を provider 行の `model` に書く慣習があるので、routing へ渡すときに **ACP 行だけ** `opencode-go/` を前置する。付記で突き合わせる）。catalog に行が無いモデルは割り当てられない（400 `model not in catalog`）。
- 書き込みは `ModelCatalogStore` の `model_role_assignment_set(source, tier, model_id, note, actor, now)` / `model_role_assignment_delete(source, tier, actor, now)`。どちらも同じトランザクションで `Event::ModelRoleAssignmentChanged { source, tier, model_id: Option<String>, previous: Option<String>, actor }` を catalog の疑似 task（`catalog_event_task_id()`、nil ULID）に追記する。`actor` は API の呼び手（`admin`。将来 token 名）。
- catalog の自動更新（`model_catalog_apply`）はこの表を触らない。消えたモデル（`available = 0`）や上書き `disabled` の割り当て行も残す（人が決めたことは機械が消さない）。
- `model_catalog_overrides.tier` は読まない（後方互換のため列は残す。画面からは外す）。

### D2. 実効の解決は 1 つの純粋関数、3 か所は同じ reader を見る

- `task_core::model_catalog::assignments`:
  - `RoleAssignment { source: CatalogSource, tier: Tier, model_id: String, note: Option<String>, updated_at: i64, updated_by: String }`（表の行）。
  - `AssignmentState = Assigned | Excluded { reason: &'static str }`（`catalog:unavailable` / `override:disabled`。順序は disabled を先に見る。ADR 2026-10-06 の `apply_model_catalog` と同じ語）。
  - `EffectiveAssignment { source, tier, model_id, state, note, updated_at, updated_by }`。
  - `AssignmentView { items: Vec<EffectiveAssignment> }` と `AssignmentView::build(assignments, entries, overrides)`（純粋。store の 3 表から組む）。`AssignmentView::get(source, tier) -> Option<&EffectiveAssignment>`。
  - `apply_to_bindings(bindings: &TierModels, source: &str, lanes: &[Tier], wire_prefix: Option<&str>, view) -> TierModels`: 割り当てがある lane は binding を置き換える（`Assigned` → `name = model_id, model_id = Some(prefix + model_id), unavailable_reason = None`、`Excluded` → `model_id = None, unavailable_reason = Some("assignment:<model_id> <reason>")`）。無い lane は従来の binding のまま（移行の互換）。bindings が空で割り当ても無ければ空のまま（legacy の挙動を変えない）。
  - trait `RoleAssignmentReader: Send + Sync { fn assignment_view(&self) -> Result<AssignmentView, String>; }`。`SqliteStore` が実装（毎回 3 表を読む。表は数十行で、解決は run の起動ごとなので cache は置かない）。試験用に `StaticAssignments(AssignmentView)` を task-core に置く。
- **dispatcher**（`Dispatcher::set_role_assignment_reader(Arc<dyn RoleAssignmentReader>)`。daemon が起動時に store を渡す）: provider 選択（`legacy_provider_profiles` の model_id・`select_provider*` の候補）と run 起動（`dispatch_run` / `review_spawn` の `model_for_tier`）の両方で、`ProviderLive.llm_source` から catalog の source 名を求め `apply_to_bindings` した **実効 bindings** を使う。`WorkerAdapter::with_tier_models(&self, TierModels) -> Option<Arc<dyn WorkerAdapter>>`（既定 `None`、`TieredAdapter` は `models` を差し替えた複製を返す）で run 用の adapter を包む。`Excluded` の lane はその provider を候補から外し（他 provider へ落ちる）、理由を `provider_selection` の除外理由 `assignment_excluded` に残す。候補が尽きれば従来どおり Unroutable。reader が無い（試験・celerisctl）なら従来どおり config。
- **llm-proxy**（`ProxyState` に `Option<Arc<dyn RoleAssignmentReader>>`。daemon が同じ store を渡す）: `normalize_legacy_config_with(config, view)` が `models.claude / gpt / qwen` の lane を割り当てで置き換え（`claude-oauth` → claude、`codex-oauth` → gpt、`openai-compatible:<id>` → その relay の cheap）、`Excluded` の lane の deployment は作らず `warnings` に書く。既存の `normalize_legacy_config(config)` は空 view の薄い包み。`ModelRequest::Tiered` の解決は毎要求で reader を読む。
- **routing catalog**（`celeris::model_discovery` の `routing_catalog_with_store` 相当の経路と `GET /api/v1/llm/routing/catalog`）: `apply_role_assignments(&mut RoutingCatalog, view)` が `legacy:<source>:<Tier>` と `provider:<id>/<lane>` の deployment の `upstream_model` / `model_profile_id` を割り当てへ置き換え（profile が無ければ `unknown_model(wire, "model_role_assignments")` を足す）、`Excluded` は deployment を外して `warnings` に `model_role_assignments: deployment <id> excluded (<reason>)`。`apply_model_catalog` の後に適用する。
- 3 か所が同じ `(source, tier) → model_id` を返すことを試験で固定する（`crates/celeris/tests/` の結合試験: 同じ store に割り当てを置き、dispatcher の `legacy_provider_profiles`・llm-proxy の `normalize_legacy_config_with`・routing catalog の deployment を突き合わせる）。
- dispatcher・store・llm-proxy に LLM 呼び出しは入れない。config の再読み込みは不要（DB を書けば次の解決から効く）。

### D3. opencode go は画面の割り当てだけで使える

- `POST /api/v1/providers` / `PATCH` の `account_pool` を `AccountPoolSetting`（`true` / `false` / pool 名の文字列）にする（JSON で bool も文字列も受ける。既存の bool client は壊れない）。`check_account_pool_adapter` は acp 行の `"opencode-go"` を `[accounts].opencode_dir` があるときだけ許す。
- 画面の「opencode go を使う」は `llm_source = opencode_go` の provider 行が無いとき、`{id: "opencode-go", adapter: "acp", llm_source: "opencode_go", account_pool: "opencode-go", tiers: [frontier, standard, cheap], concurrency: 1}` を `POST /api/v1/providers` で作る（providers.d に書いて reload。ADR-0017 M1）。`tier_models` は書かない。lane のモデルは割り当て（D2）で決まり、割り当てが無い lane は `model` も `tier_models` も無いので `resolve` が「未設定」を返してその lane の候補にならない。
- 枠切れの fallback は既存（ADR 2026-10-06 D3）: pool の全 account が `*Exhausted` なら provider_select から外れ、同じ lane の次の provider へ落ちる。本 ADR では割り当て済み lane の opencode-go provider が候補に入ること、Exhausted で次の provider へ落ちることを dispatcher 試験で固定する。

### D4. API

- `GET /api/v1/llm/models/assignments` → `{ items: [EffectiveAssignmentView], effective: [RoleSlotView] }`。
  - `EffectiveAssignmentView { source, tier, model_id, state: "assigned" | "excluded", excluded_reason: string|null, note, updated_at (RFC3339), updated_by }`。
  - `RoleSlotView { source, tier, model_id: string|null, origin: "assignment" | "config" | null, excluded_reason: string|null, providers: [provider id], proxy: bool, available: bool|null, last_seen: RFC3339|null }`: source × 3 役割の全枠。`origin = config` は割り当てが無く config（`tier_models` / `model` / `llm_proxy.models`）から来た値。`available` / `last_seen` は catalog の行から（行が無ければ null）。source の集合は catalog の source ∪ provider 行の llm_source ∪ llm-proxy の有効 source。
- `PUT /api/v1/llm/models/assignments/{source}/{tier}` 本文 `{ model_id, note? }` → 200 `{ item: EffectiveAssignmentView, impact: ImpactView }`。catalog に無い model_id は 400、`source` / `tier` 不正は 400。
- `DELETE /api/v1/llm/models/assignments/{source}/{tier}` → 204（無ければ 404）。
- `POST /api/v1/llm/models/assignments/preview` 本文 `{ source, tier, model_id: string|null }` → 200 `{ impact: ImpactView }`（書かない）。`ImpactView { changes: [{ kind: "provider" | "proxy", id, tier, before: string|null, after: string|null, excluded_reason: string|null }] }`: 変更で `(source, tier)` を使う provider 行と llm-proxy の lane がどう変わるか。
- `GET /api/v1/llm/models` の項目に `assigned_tiers: [tier]` を足す（この model が割り当たっている役割。表の一覧で見せる）。
- `model_id` に `/` が入る場合は `%2F`（既存の override と同じ）。

### D5. web の `/models` 画面

- 画面の先頭に source ごとの「役割の割り当て」カード: 3 役割の行（役割・今のモデル・由来 badge『割り当て』/『config』・availability・最終確認・枠の状態）。枠の状態は `GET /api/v1/accounts` の pool（`claude-oauth` → claude-code、`codex-oauth` → codex、`opencode-go` → opencode-go）の最小残量を使い、取れない source は『不明』。
- 行の「変更」で catalog のモデル（その source、`available` かつ `disabled` でないもの）から選ぶ。確定前に `preview` の影響（provider / proxy ごとの before → after）を同じ dialog に出し、人が確認して `PUT`。『解除』は `DELETE`（確認あり）。
- `llm_source = opencode_go` の provider が無ければ opencode-go のカードに「opencode go を使う（provider を追加）」ボタン（D3）。
- 既存の表はそのまま。override 編集から `tier` を外す（routing に使わない。D1）。変更は `web/features/ops/models-screen*.tsx`・`web/api/` の client・e2e fixture（`/models`）に留め、shell / nav / screens.ts には触れない（CoS chat の web task と衝突させない）。

### D6. 試験

- task-core: `AssignmentView::build`（available=0 / disabled → Excluded）、`apply_to_bindings`（置き換え・互換・prefix）、store の set / delete / event / `model_catalog_apply` が表を消さないこと。
- dispatcher: 割り当て > config、Excluded lane の provider 除外と次 provider への fallback、opencode-go 行の候補入りと pool Exhausted の fallback、reader なしは従来どおり。
- llm-proxy: `normalize_legacy_config_with` の置き換えと Excluded。
- celeris: 3 か所一致の結合試験、`apply_role_assignments`。
- task-api: 4 endpoint と `assigned_tiers`、providers の `account_pool` 文字列。
- web: component test（割り当て・解除・preview・opencode go ボタン）、e2e の `/models`（fixture）。
- 外部ネットワークに出ない。CPU を焼く負荷をかけない。本番の config / daemon / DB に触れない。

## 却下した案

- `model_catalog_overrides.tier` を routing に使う: model ごとの tier は「その model をどの役割に出すか」で、「役割に何を出すか」（1 役割 1 model）の問いに合わない。複数 model が同じ tier を名乗ると選べない。役割側を主キーにする。
- 割り当てを `providers.d/<id>.toml` の `tier_models` へ書き戻して reload: provider 行ごとに書くと source（claude-oauth など）で共有される llm-proxy の lane に届かない。DB に 1 つ持ち、3 か所が同じ reader を見る方が真実が 1 つになる。
- dispatcher が割り当てを tick ごとに cache: 表は小さく、解決は run 起動ごと。cache の失効管理より毎回読む方が単純で「DB の変更で次の解決から効く」を満たす。

## 付記: 実装突き合わせ

（phase 1 の実装後に書く）
