---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
---
# rm-legacy 旧 routing 参照地図

調査対象は現 HEAD の `crates/`。確認は `git grep` で行った。目的は旧 shadow/enforce・estimator・optimizer・tier→model 解決・`[model_routing]` の撤去を、旧イベントの読み取りと selector/RoutePolicy の実行経路を維持して完了できる順に配置すること。

## 消す

| file / 型・経路 | 呼び出し元・追随箇所 |
|---|---|
| `crates/celeris/src/daemon/routing_shadow.rs`, `routing_shadow_tests.rs` | `daemon/services.rs` の proxy shadow wiring、`daemon/admin.rs`・`bootstrap.rs` の `set_routing_shadow`、`daemon.rs` の module 宣言。event 書き込み sink もここ。 |
| `crates/celeris/src/daemon/routing_sidecar.rs` と sidecar descriptor/試験 | `daemon/services.rs`、bootstrap/run の model routing runtime 組み立て。`crates/celeris/tests/routing_estimator_sidecar_validate.rs` と task-core sidecar protocol fixtures も estimator 専用参照。 |
| `crates/task-dispatch/src/dispatcher/routing_shadow.rs`, `routing_enforce.rs`, `routing_members.rs` | `dispatcher.rs` の module、fields、public exports・setter/listener、`dispatch_run.rs` の enforce 呼び出しと `LaneResolution` 組み立て、daemon wiring。`dispatch-enforce-heuristic-v1` はこの dispatcher shadow 経路に属する。 |
| `crates/llm-proxy/src/shadow.rs`, `shadow_budget.rs`, `shadow_tests.rs`, `estimator_shadow.rs`, `estimator_sidecar.rs` | `lib.rs` の module、`selection.rs` の shadow/enforce lane 優先・sidecar 経路、`server.rs`、`selection_state.rs` と関連 tests。`llm-proxy/tests/estimator_sidecar.rs` も削除対象。 |
| `crates/task-ops/src/routing_audit/estimator.rs` とその型/集計 | `routing_audit.rs` の import・summary 呼出し、および旧 audit の estimator 部分。予約 audit API (`routing_audit.rs` → `routing_shadow_reservation_audit`) も store 予約の撤去に合わせて消す。 |
| `crates/task-core/src/model_router/estimator.rs`, `estimator/sidecar.rs`, `optimizer.rs`, `policy.rs`, `shadow.rs`, `shadow/tests.rs`, `tests.rs` | `model_router/mod.rs`、`model_policy.rs`、config・dispatch・proxy・task-api・task-ops の imports。estimator/optimizer/shadow の内部相互参照があるため、個別に先行削除せず最後の core 葉でまとめて外す。 |
| `crates/task-core/src/model_router/metrics.rs` および旧 router 専用 `cost.rs` / `trace.rs` の不要部分 | `model_router/mod.rs` と `routing_audit.rs` 等。共有される trace/feedback は残す欄を参照し、ファイルごと削除せず必要型を分離する。 |
| `crates/task-core/src/model_routing.rs` の `LaneResolution` と tier→provider/model 解決 API | producer: `task-dispatch/src/dispatcher/dispatch_run.rs`; consumers: `task-core/src/model_policy.rs`, `routing_audit.rs`, `task-ops/src/routing_audit*`, `routing_outcome/tests.rs`, `task-api/src/stats/execution_metrics_comparison_tests.rs`。`CodingDefaultResolution` 等 provider-era 専用欄も参照を確認して整理する。 |
| `crates/task-worker` の `with_tier_models` / `tier_models` binding | adapter trait と各 adapter/wrapper の定義および呼出し元。現行検索で `with_tier_models` の具体的 hit は無かったため、削除葉で再検索する。 |
| store の `crates/task-core/src/store/routing_shadow.rs`, `routing_shadow_tests.rs`、reservation API / 型 | `store/mod.rs`, `task-ops/src/routing_audit.rs`, `llm-proxy/src/shadow_budget.rs`。予約・予算 read/write API は全て廃止。 |
| `crates/task-core/migrations/0049_routing_shadow_budget.sql` が作った `routing_shadow_reservations` と `idx_routing_shadow_reservations_day` | migration drop で除去。events と `routing_shadow_recorded` の payload は変更・削除しない。drop migration の番号は rm-store 実装時に全 branch を走査して採番。 |
| `crates/celeris/src/config/model_routing.rs`、`config/mod.rs` の model_routing field/runtime・旧 catalog 統合、`config/tests.rs` の旧試験 | `model_catalog.rs` は `[model_catalog]` 発見/override と catalog を保つ。ただし旧 router `ModelProfile` を使った routing profile の adapter・実装、`model_routing` runtime 結合は撤去。`model_catalog` crate/API 自体を一括削除しない。`[model_routing]` 拒否は config lib に `model_routing_section_rejected*` 試験を置く。 |
| 旧設定・API・試験の影響箇所 | `docs/` config examples、`examples/`、`docs/api/v1/`・`docs/protocol/`、`web/api/generated/` と `web/api/realtime/` の旧 routing fields/type があれば更新。API/schema の生成物を更新し生成一致を確認。`crates/task-api/src/routing.rs`, `types.rs`, `query.rs` は旧 shadow projection を除き routing_decided/request/outcome/feature API を維持。 |

**file 単位の分類:** u-rm-legacy.md の全対象 file は上表の削除欄に記載。`Tier`/`lane` 一般、`task-api/src/llm_sources.rs` の source state、`model_catalog` の model 発見は削除対象ではない。

## 残す

- `task_core::llm_route::RoutePolicy` / route selection、`route_defaults`（harness × lane）、`task_core/src/route_select/`。実行時の source/model 選択点は dispatcher RoutePolicy に一本化する。`llm-proxy` は選択済み route の接続・認証・API変換を担い、独自の lane 優先順を持たない。
- `Tier` を方針ラベルとして保持。tier→model/provider binding と `with_tier_models` は削除する。
- `model_router::feedback` の `FeatureStage`, `RoutingFeaturesRecord`, `RoutingRequestRecord`, `RequestSourceAttempt`, `RoutingOutcome`、reward/pending outcome 投影は selector の評価・`routing_outcome` に使うため残す。`task-core/src/routing_audit.rs`, `task-ops/src/routing_outcome.rs`, `task-api/src/query.rs` が利用。
- `RoutingContext` と `context_registry` は dispatch で特徴を記録し proxy に opaque ref を渡す現用経路がある（dispatcher/routing_context.rs、daemon/run.rs・services.rs、llm-proxy/routing_context.rs・server.rs、task-worker protocol）。`task-api llm_sources` の `SourceState` / `Reachability` / opportunity cost view も GET source API・UI が使う。削除候補に含めず独立 module に維持する。
- `model_router::profiles` の `ModelProfile`, `DeploymentProfile`, `SourceState`, `Reachability`, `Billing`, `Capabilities`, `ContextLimits`, `Support`, `QuotaWindow` は llm-proxy `catalog.rs`/`selection_state.rs` と `task-api/src/llm_sources.rs` が使う。config model_catalog の残存統合が要るものは selector/evidence または catalog-owned 型へ分離し、残存 consumer を同じ呼び出し元先行段階で移す。
- `model_router::trace` の `RoutingTraceV1`, `CandidateTrace`, `ExcludedReason` は llm-proxy selection/state が request trace に使い、task-ops audit が表示する。selector-wire の `routing_decided` と `routing_request_decided` の trace へ必要な wire 型を移し、proxy consumer を更新した後で core 旧 module を削る。
- `task-api` の llm_sources、`routing_outcome_recorded`、guardrail（daemon/routing_guardrail.rs と tests）は引き続き利用。guardrail は `RoutingOutcome` を読む。feedback/cost/trace のうち使われる型は上記の移管対象。
- `task-core/src/model.rs` の `Event::RoutingShadowRecorded` と `task-api` query event kind / 旧 event read path は維持。新規書き込みだけを無くす。
- `[model_catalog]`（カタログ取得設定）と `task-core::model_catalog` は旧 `[model_routing]` とは別機能なので維持する。

## 履歴の型

`crates/task-core/src/routing_shadow_history.rs` を読み取り専用の serde/wire module とし、`routing_shadow_recorded` legacy JSON を deserialize できる payload を移す。最低限の wire 型: `SHADOW_RECORD_VERSION`, `ShadowRecord`（全 payload 欄: shadow_id, primary_decision_id, kind, status, reason, detail, policy_version, run_id, request_id, candidate_model/source, input/output_tokens, output_sha256, cash/effective_usd, latency_ms, reservation_id）, `ShadowKind`, `ShadowStatus`, `ShadowReason`。表示で旧 `detail` 内 JSON を解釈する `DecisionShadowComparison` と estimator summary の互換読取が必要か実例 fixture を見て判断する。書込み policy/allowlist/caps/admission、hash生成、reservation、sink は移さない。`task-ops` audit projection の表示型（`RoutingShadowAudit` の event-derived fields）を history type から作り、audit API を読み取り専用で維持する。

`task-ops` に実際の旧形式 JSON fixture を置き `legacy_shadow_history*` 試験で deserialize と audit 応答を検証する。`routing_shadow_recorded` event enum/JSON shape と payload 名を変えず、旧 fixture の optional/missing field もテストする。

## migration

`0049_routing_shadow_budget.sql` が作成する `routing_shadow_reservations` と `idx_routing_shadow_reservations_day` は新 migration で `DROP TABLE`（index は table drop に伴い消える。明示 drop するなら順序を守る）。既存 0049 は書き換えない。`events` / `routing_shadow_recorded` は drop・rewrite しない。

採番は rm-store 葉が `git branch -a` の全 head にある `crates/task-core/migrations/` を調査して空き番号を決める。同時に `SCHEMA_VERSION`、migration registration、`crates/celerisctl` の no-migrate fixture/期待版、migration tests の固定値を追随する。既存 schema version 固定試験を含め crate test で確認する。

## RoutePolicy 境界

先行 ADR `2026-10-11-llm-source-harness-selection.md` は dispatcher が harness 決定後に route candidate 順で source/model を決め、proxy は指定 source への接続・認証・quota・形式変換を担い、lane/Tier から model の優先度を作らないと定める。後続 selector の `route_defaults.<harness>.lanes.<lane>` は明示候補の設定であり、この境界を保つ。

したがって `llm-proxy/src/selection*.rs` に残る旧 lane 優先規則・shadow/enforce 選択を除去し、proxy は dispatcher の選択 route を実行する形にする限り衝突しない。lane の意味や candidate order を proxy 側に複製する変更は先行 ADR と衝突する。survey では衝突は認められないため plan_issue は不要。

## 葉ごとの割り当て

依存順は consumer を先、task-core/config を後とする。各段完了時点で参照元が消えているため build 可能になる順序。

| 葉 | 消す file / 呼び出し元の処理 | 次に進める条件 |
|---|---|---|
| `hist` | 旧 `ShadowRecord` の読み取り型を `routing_shadow_history.rs` に分離。task-ops routing audit の read path を新 history 型へ付替え、fixture/`legacy_shadow_history*` を追加。ここでは新規 shadow producer をまだ残し、wire compatibility を先に成立させる。 | task-core と task-ops が build/test 可能。 |
| `rm-daemon` | daemon `routing_shadow*`・`routing_sidecar*` module/tests、services/admin/bootstrap/run wiring と config runtime injection を外す。event sink/write path を消す。 | daemon crate が build 可能。 |
| `rm-dispatch` | dispatcher `routing_shadow`, `routing_enforce`, `routing_members`・fields/listener/setters/exports、dispatch_run の enforce/LaneResolution producer を削る。task-ops/model_policy consumers の型参照を selector trace/outcome 側へ移す。 | task-dispatch とその依存 consumer が build 可能。 |
| `rm-proxy` | proxy shadow/estimator/sidecar files, tests, modules と selection lane priority を削る。selection は explicit selected route を実行する。proxy routing trace/context/request recording を残す。 | llm-proxy の build/test が可能、dispatcher policy 以外に source/model selection を持たない。 |
| `rm-store` | task-ops reservation audit API と store routing_shadow reservation methods/tests/API を削除、drop migration 追加。events は残す。 | migration registration・schema version・no_migrate/migration tests 追随済み。 |
| `rm-config` | celeris config model_routing module/runtime/validation/catalog profile glue と旧 fixtures/config examples を整理。[model_routing] を route_defaults を案内する error にし `model_routing_section_rejected*` を追加。`[model_catalog]` は維持。 | celeris config build/tests が可能。 |
| `rm-router` | 最後に task-core `model_router` の estimator/optimizer/policy/shadow・不要旧 modules と `model_routing::LaneResolution`/tier binding を削除。shared context/feedback/profile/trace wire 型は consumer 移設済みの範囲だけ除去し、必要なものは新 module へ残す。`with_tier_models` を全 tree 再検索。 | workspace build、schema/docs/generation と旧参照再検索を行える。 |

各葉の許可範囲には呼び出し元の追随ファイルも含める。順序は `hist → rm-daemon → rm-dispatch → rm-proxy → rm-store → rm-config → rm-router`。store/config は旧 router 最終削除より前に終え、最終葉後に workspace 全体が build する構成とする。
