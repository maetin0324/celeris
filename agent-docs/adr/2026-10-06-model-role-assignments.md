# catalog のモデルを legacy の役割（frontier / standard / cheap）へ画面で割り当て、その割り当てで routing を動かす

- 日付: 2026-10-06
- 状態: 実装済み（phase 1。本番での割り当てと shadow 開始は配送後に人が行う）
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

## 付記: 実装突き合わせ（2026-10-07）

phase 1（commit dae4c31a・86d8b8cc と続く修正）の実装で、本文から変えた点・決めた点。

- **D2 の適用順は『割り当て → catalog』**: `refresh_routing_catalog` / `apply_catalog_to_snapshot`（`crates/celeris/src/model_discovery.rs`）は `apply_role_assignments` を先に、`apply_model_catalog` を後に呼ぶ。本文の「`apply_model_catalog` の後に適用」だと、config のモデルが catalog で消えて落とされた lane を、別の利用可能モデルへの割り当てで生き返らせられなかった。Excluded の割り当ては `apply_role_assignments` が外すので、後段の catalog 適用は重ねて落とさず、warning も 1 回。
- **provider lane seeds**: `apply_role_assignments(catalog, view, seeds: &[ProviderLaneSeed])`。`Config::provider_lane_seeds()`（`config/model_routing.rs`）が provider 行ごとに `{provider_id, source, adapter, lanes, config_order}` を渡し、`model` も `tier_models` も無い行（opencode go の acp 行）の割り当て済み lane に `provider:<id>/<lane>` の deployment（upstream は `wire_prefix_for` + model_id）を作る。`provider:<id>` の複数 lane deployment は割り当てのある lane だけ `provider:<id>/<lane>` に分け、残りは元の deployment に残す。
- **model なし行は acp 行だけが『割り当てだけで routing』**（`Dispatcher::effective_tier_models`、`provider_select.rs`）: reader があり、`adapter == "acp"`、`model` なし、`tier_models` 空の行は全 lane を `assignment:none` から始めて割り当てで上書きする（割り当てが無い lane は候補にならない）。claude-code / codex の行は model が無くても CLI の既定モデルで走れるので対象外（最初は全 adapter に掛けて e2e の account pool 試験 3 本が unroutable で落ちた）。reader が無い（試験・celerisctl）なら従来どおり。
- **除外の記録**: `ProviderCandidateOutcome` に variant は足さず、`outcome = Unsupported`・`detail = "assignment_excluded: assignment:<model> <reason>"` で残す（schema と生成型を増やさない）。全 provider が除外なら従来の「合う provider が無い」と同じく unroutable の warn を出して Ready のまま、次 tick で再評価。sticky session が除外 provider を指していれば飛ばす。
- **effective bindings の適用範囲**: `legacy_provider_profiles`・`select_provider_excluding`・`legacy_optimizer_trace`・`dispatch_run`（run の adapter を `with_tier_models` で包む。実効 map が行の `tier_models` と同じなら包まない）・`review_spawn`（reviewer run と `WorkerStarted` の model）。公開 `Dispatcher::effective_lane_model(provider_id, tier)` は run が実際に受け取る model を返す（結合試験で使う）。`ProviderLive.llm_source` が無い行（snapshot publisher 無し・source 不明）には何も掛けない。quota による lane の引き下げより前に包むので、引き下げ先が Excluded なら既存の `model_for_tier` の失敗経路（Unroutable）になる。
- **llm-proxy**: `normalize_legacy_config_with(config, view)`、`LegacyCatalog.warnings`。有効な source の lane は `[llm_proxy.models]` に値が無くても割り当てがあれば deployment を作る。`openai-compatible:<id>` は cheap lane だけ。wire は接頭辞なしの `model_id`。warning は要求ごとに debug で出す。`ProxyState::with_role_assignments` に daemon が dispatcher と同じ reader（`Arc<SqliteStore>`）を渡す（`daemon/services.rs`、`daemon/bootstrap.rs`）。
- **task-core の型**: `AssignmentState` / `EffectiveAssignment` / `AssignmentView` は Serialize + JsonSchema だけ（Deserialize なし）。`apply_to_bindings` は割り当てが 1 つも当たらなければ bindings をそのまま返し、当たったときだけ、束縛も割り当ても無い lane を `assignment:none` にする。Assigned の束縛は元の `reasoning_effort` を保ち、Excluded は消す。`tier_str` / `tier_from_str` を公開。delete は行が無ければ event を書かない。
- **API**（`crates/task-api/src/model_assignments.rs`）: 形は D4 のとおり。400 の problem code は `model_not_in_catalog`（preview の未知 model も 400）、404 は `model_assignment_not_found`。`RoleSlotView` の config 由来の値は provider 一覧（`ProviderLive.tier_models[lane]` → `model`）から取り、proxy lane は routing catalog の `legacy:*` deployment から取る。割り当て中の proxy lane は config 値が判別できないので解除 preview の `after` は `null`（gui-api.md §3.127.4）。actor は定数 `admin`。providers の `account_pool` は `AccountPoolSetting`（bool / pool 名）。acp 行の `"opencode-go"` は API が見る accounts roots に OpencodeGo があるときだけ受け、無ければ 422 `invalid_provider`。PATCH の `account_pool: true` は名前付き pool の行ならその名前を保つ。
- **CLI**: `celerisctl models assign <source> <tier> <model_id> [--note]` / `unassign <source> <tier>`。
- **web**: client は `web/api/model-assignments.ts`（D4 の型は生成型 `web/api/generated/types.ts` と並存。path は gateway が `/api/v1` へ写す `/api/llm/models/assignments…`）。override editor から `tier` を外し、表には `assigned_tiers` の badge。枠の表示は accounts API の各窓の `1 - utilization` の最小値を同じ adapter の全 account で取る。provider 追加後は providers 画面と同じく `POST /api/reload` を呼ぶ。
- **未実装・残り**: run 中の 429 の窓別記録（ADR 2026-10-06 D2 の TODO のまま）。本番での割り当てと shadow 開始は配送後に人が行う（`docs/ops/opencode-go-and-model-catalog.md`）。
