---
tasks: [01M44H0SRV70E32AQ6C5N37MSK]
---
# 多目的モデルルーティング: 現行実装の棚卸し（2026-10-04）

対象はこの WorkUnit の基点 `c86128865adc`。ここでいう **provider** は `[[providers]]` の adapter 実行枠、**source** はモデルと資格情報の供給元である。ADR-0132 D1 がこの境界の正本であり、ADR-0053 D2 の古い「provider = LLM source」は履歴として読む。後続の設計は [ADR-0069](../../adr/0069-routing-four-layers.md)、[ADR-0132](../../adr/0132-provider-llm-source-split-and-cheap-qwen.md)、[ADR-0049](../../adr/0049-portable-providers-and-codex-usage.md)、[ADR-0124](../../adr/0124-atomic-direct-route.md)、[ADR-0061](../../adr/0061-coding-harness-routing-foundation.md)、[構成図](../../../docs/architecture-map.md) と整合させる。

## 1. 現在の判断点

| 順 | 判断と入力 | 実装位置 | 出力・境界 |
| --- | --- | --- | --- |
| 担当・harness | 組織 matching と `Task.genre`。coding の候補生成骨格 `RoutingSignals::from_task` / `StaticRoutingPolicy::decide` は存在するが、ADR-0061 D4 のとおりライブの作成経路には未配線 | `crates/task-core/src/routing.rs:57,99,120`、`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1158` | 担当は `Event::Assigned`、harness は `WorkerStarted.adapter`。モデル policy と同じ名前の `RoutingPolicy` trait をここに流用しない |
| task / WU の lane | `TaskFeatures::infer` の 9 軸、`TaskFeatureHints`、規則表、組織の `LaneCeiling`。人・system の明示 tier は policy を通さない。WU は task lane に上限を取る | `crates/task-core/src/model_policy.rs:248,417,570,602,634,763`、`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1114`、`crates/task-dispatch/src/dispatcher/work_units.rs:1885` | `LaneDecision`。`Tier` の wire 名は frontier/standard/cheap |
| retry lane | 過去 event から `attempt_history`、`EscalationPolicy::for_task/decide` を適用 | `crates/task-core/src/retry_policy.rs:109,138,157,269`、`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1134` | 失敗を同 lane で数え、条件を満たせば 1 段だけ上げる |
| adapter/provider/account | `StaticPolicy::select` は設定順、tier、adapter 指定、provider cooldown を見る。`select_provider_for` は concurrency と account pool を加え、pool の最高 score を非 pool fallback より優先する。継続 session は先に sticky 判定 | `crates/task-dispatch/src/policy.rs:22,148,223`、`crates/task-dispatch/src/dispatcher/provider_select.rs:124,203,344` | adapter と `[[providers]].id`、CLI 用 account。`llm_source` は参照属性であってこの provider 行そのものを source としない |
| 残量による実行 lane と CLI model | 選んだ account の新鮮な観測値を `measured_remaining` で読み、`select_tier` で lane を下げる。`TieredAdapter::model_for_tier` が `tier_models` を解決、無ければ旧 adapter/provider model | `crates/task-dispatch/src/dispatcher/dispatch_run.rs:433-505`、`crates/task-core/src/model_routing.rs:8,50,77` | `WorkerStarted` と `RoutingDecided`。写像が欠ける/使用不可なら unroutable。旧 provider の空写像は旧動作 |
| proxy の source/model/account | `parse_model` の `celeris/<tier>` 等を `ProxyState::attempts_for` が順位付き `(Attempt, upstream_model)` にする。`rank_relays` は到達性、`rank_across_pools` は account score、`qwen_tier_model` は cheap 制限 | `crates/llm-proxy/src/server.rs:289-399,822`、`crates/llm-proxy/src/selection.rs:49,101,156,174`、`crates/llm-proxy/src/naming.rs` | OpenAI 互換要求単位の最終 source/model/account。adapter が proxy を呼ぶ場合、dispatcher の provider 選択とは別の第 2 判断。`sources/{claude,codex,relay}.rs` は送信・変換を担当 |

## 2. frontier / standard / cheap の意味と既定写像

`Tier` は既に ADR-0069 で品質/予算の **lane** と定義されるが、実行側には固定の tier→モデル写像が残る。`ModelPolicy` の規則表は高い判断・不確実/検証困難・横断を frontier、機械的で検証可能かつ可逆な仕事を cheap、その他を standard にする（`model_policy.rs:570-629`）。`LaneCeiling` は許可集合と最大 lane を適用する。人が明示した tier はその上限を迂回する（同:634-679）。planner 等の system 固定や古い task は `worker_hint.tier` を使う。

| lane | proxy の Claude 既定 | proxy の GPT 既定 | proxy の Qwen 既定 | `celeris/<tier>` の候補 |
| --- | --- | --- | --- | --- |
| frontier | `claude-fable-5-1` | `gpt-6-astra` | なし | Claude/Codex |
| standard | `claude-opus-5-5` | `gpt-6-sol` | なし | Claude/Codex |
| cheap | `claude-sonnet-5` | `gpt-6-luna` | `qwen3.8-27b` | `prefer_free=true` なら到達可能な relay を先頭、その後 Claude/Codex |

出典は `crates/llm-proxy/src/config.rs:195-260`。`claude/<tier>`、`gpt/<tier>` は一方の OAuth pool、`qwen/cheap` は relay のみ。`<source>:<concrete-model>` は抽象 tier を経ず素通りする（`docs/guides/llm-source.md:14-19`）。Qwen の古い frontier/standard 設定は warning のうえ無視する（`config.rs:248-260`）。CLI adapter の `tier_models` はこれと別の写像である。

## 3. 残量、cooldown、in-use、retry、fallback

- 共通の account 評価 `task_dispatch::accounts::evaluate` (`crates/task-dispatch/src/accounts.rs:345`) は未ログイン→cooldown→同時実行上限→rejected→5時間/7日枠の 97% 枯渇を順に除外する。残りから `min(5h headroom, reset 時間補正付き 7d headroom) - 0.05 × in_use` を score にする。観測なしは `1 - 0.05 × in_use`。pool 内は score→in_use→id、proxy の pool 間は score→Claude 優先→in_use→id (`selection.rs:49-145`)。
- dispatcher の provider cooldown は `StaticPolicy::report` (`crates/task-dispatch/src/policy.rs:178`) が保持し、account cooldown は `record_account_failure` (`dispatcher/provider_select.rs:378`) が共有 `AccountBook` に書く。proxy も同じ帳簿に 401/429 の cooldown を書く (`llm-proxy/src/server.rs:232`)。ただし proxy の `max_concurrent_per_account` と CLI の `[accounts].max_runs_per_account` は別の in-use 枠 (`llm-proxy/src/config.rs:26-35`)。
- dispatcher は pool provider の最良 account を選び、使える pool が無いとき非 pool provider へ fallback (`provider_select.rs:124-195`)。sticky session は残量上位より先に試す (`provider_select.rs:203-239`)。選んだ account の**測定済み**残量が ≤3% なら保留、≤10% なら cheap、≤30% で frontier なら standard。それ以外と未知残量では元の lane (`model_routing.rs:75-101`)。quota 消費推定は別途 `QuotaEstimated` に記録し、選択自体を変えない (`dispatcher/quota_book.rs:104-163`)。
- proxy は cheap で `prefer_free` なら到達可能な relay を設定順に先頭に並べる。後続に Claude/Codex を account score 順に並べる (`server.rs:289-388`)。候補列は要求開始時に固定し、stream の最初の byte より前の失敗で次を試す。401/429 は account cooldown、stream 開始後の失敗は再送しない (`server.rs:822-974`)。候補ゼロの瞬間は短い待ちで再走査してから 503 (`server.rs:855-886`)。relay probe は `sources/relay.rs:21`、TTL は `config.rs:23-25`。
- task の retry/escalation は proxy の同一要求内 fallback と別物。`EscalationPolicy` は review/verification/low-quality の連続失敗を対象とし、供給側失敗や予算切れでは上げず、最大試行数と組織上限を守る (`retry_policy.rs:107-247`)。

## 4. RoutingDecided / RoutingAudit と表示

`Event::RoutingDecided {run_id, record}` (`crates/task-core/src/model.rs:1235`) の `RoutingRecord` は org_node、harness、`LaneDecision`（proposed/final lane、tier source、rule_id、policy_version、features、reasons、clamp、hint、escalation、shadow）、`LaneResolution`（実行 lane、adapter、provider、account、model_id、reasoning_effort）、quota_reason、work_unit_id を持つ (`model_policy.rs:501-542`, `model_routing.rs:34-48`)。dispatcher は worker 起動時に記録する (`dispatch_run.rs:527-564`)。proxy 内の最終 source/model、候補ごとの除外理由・score はこの event に**入らない**。proxy の応答ヘッダ `x-celeris-source/account` と `llm_proxy_requests` の source/account/upstream_model/latency/usage は別記録 (`llm-proxy/src/server.rs:766,408-438`, `docs/guides/llm-source.md:138-144`)。

`task_core::routing_audit::routing_audit` (`crates/task-core/src/routing_audit.rs:20-169`) は `WorkerStarted`、`RoutingDecided`、`WorkerFinished`、`ReviewVerdict`/review 遷移を run_id で突き合わせ、担当、harness、adapter、provider、account、lane、model、effort、features、規則/版/理由、escalation、outcome、USD/tokens/wall_ms/retries、合否/失敗 criterion を返す。これは event の投影で、候補集合や source score の監査ではない。`task_ops::routing_audit::task_routing_audit` (`crates/task-ops/src/routing_audit.rs:9`) が store から読み、`GET /api/v1/tasks/{id}/routing` (`crates/task-api/src/routing.rs:17-45`) が task の `routing` 出自と `runs` を返す。run 無しは空配列、未知 task は 404。

GUI は `gui/app/routes/tasks.$id.tsx:118,248` から `TaskRoutingPanel.tsx:25-129` に渡し、最新 run の lane/model/provider/規則/理由・特徴・cost/review と escalation 履歴を表示する。web は `web/features/tasks/execution-panel.tsx:26-42,105-127` で同 API を読み、run の lane/model/org/rule を簡略表示する。source 状態は別 API `GET /api/v1/llm/sources` (`crates/task-api/src/llm_sources.rs:25-38`) にあり、`SourceView` の到達性、account 残量/cooldown、直近1時間の回数/tokens と現在の `celeris` tier 解決先 (`crates/llm-proxy/src/sources_view.rs:14-145`) を GUI の accounts 画面 (`gui/app/routes/accounts.tsx:541-689`) に表示する。web は providers 画面で adapter 実行枠と LLM source を分ける (`web/features/ops/providers-screen.tsx:211-275`)。

## 5. 現行設定の形

`LlmProxyConfig` (`crates/llm-proxy/src/config.rs:14-65`) は `[llm_proxy]` に `enabled?`, `listen`, `prefer_free=true`, `probe_cache_secs=60`, `max_concurrent_per_account=4`, `cooldown_fallback_secs=300` を持つ。`[llm_proxy.sources.claude_oauth]` と `[llm_proxy.sources.codex_oauth]` は任意で、accounts_dir（省略時は `[accounts]` の dir）、endpoint、enabled、Codex の sampling/effort 設定を持つ。`[[llm_proxy.sources.openai_compatible]]` は `id`, `base_url`, `api_key?`, `enabled` の複数行。`[llm_proxy.models.claude|gpt|qwen]` は各 `Tier → model ID` (`config.rs:89-260`)。`config/celeris.example.toml:189-223` と `docs/guides/llm-source.md:150-181` に例があるが、例のコメントには古いモデル ID も残るため、**既定値の正本は `config.rs`** とする。

`[[providers]]` は `id`, `kind="adapter"`（旧行は省略可）, `adapter`, `tiers`, `concurrency`, `model`, `tier_models`, `llm_source`, `account_pool`, `account_id`, env/command 等 (`crates/celeris/src/config/providers.rs:14-60`)。`tier_models` は `Tier → ModelBinding {name, model_id?, unavailable_reason?, reasoning_effort?}` (`task-core/src/model_routing.rs:8-22`) で Claude Code/Codex のみに許す (`config/providers.rs:358-361`)。`llm_source` は `celeris`/`claude_oauth`/`codex_oauth`/`openai_compatible:<id>`/`none` の参照で、旧行の推定と警告、明示時の整合検証がある (`config/providers.rs:142-161,202-264,302-350`)。Qwen 直結 ACP は cheap のみ、proxy の `celeris/<tier>` を使う ACP は全 tier を受けられる (`config/providers.rs:229-243`、ADR-0132 D3/D5)。`[accounts]` の CLI 並列上限と source の OAuth ディレクトリも互換設定である。

## 6. 新要素と既存抽象の対応・衝突

| 新要素 | 足す場所 / 新設の要否 | 既存実装との重複・衝突 |
| --- | --- | --- |
| `ModelProfile` | **新設**のモデル固有情報。`ModelBinding`/`ModelsConfig` は旧 tier 写像を読み込む互換 adapter とし、model ID、対応能力、品質/価格等を profile に正規化する | `ModelBinding` は CLI provider の 1 lane の束縛、`ModelsConfig` は proxy source ごとの束縛。どちらもモデル自体の共通台帳ではない。双方を独立の正本として増やすと ID/effort がずれる |
| `SourceState` / `DeploymentProfile` | **新設**の source/deployment 静的属性と動的状態。`SourcesConfig`/`LlmSourceRef`、`AccountBook` の観測・cooldown、`SourceView`/relay probe、provider capacity を入力として合成 | CLI provider の concurrency と proxy source の account in-use は別カウンタ。self-host の到達性だけで負荷/待ち行列/電力は測れない。`QuotaMethod::Free` (`task-core/src/quota.rs:42-58`) を self-host の資源費ゼロと誤読しない |
| モデル選択の `RoutingPolicy` | **新設**の純粋な候補生成/制約/score/選択境界を proxy selection と dispatcher の間で定義し、旧 tier は policy/SLO 名として維持 | `task_core::routing::RoutingPolicy` (`routing.rs:99`) はハーネス候補用、`ProviderPolicy` (`task-dispatch/src/policy.rs:62`) は adapter 実行枠用、`ModelPolicy` (`model_policy.rs:600`) は TaskFeatures→lane 用。同名を拡張して source 決定まで混ぜない。`selection.rs` と `provider_select.rs` の account 評価は同じ `accounts::evaluate` を再利用する |
| `RoutingContext` | **新設**の実行時スナップショット。`TaskFeatures`/`TaskRouting`、WU view、`attempt_history`、受け入れ条件/レビュー/検査履歴を組み立てる | `TaskFeatures` は 9 軸の静的推論、`RoutingSignals` は未配線の harness 用。生の Task を proxy へ運ぶと ADR-0069 の層分離に反する。必要な値と出自だけを渡す |
| `QualityEstimator` | **新設 trait** と決定的な既定実装。モデル×context の品質代理値を返し、後で外部推定器を shadow → opt-in で接続 | `ShadowClassifier` (`model_policy.rs:486-498`) は lane 推定の予約で、候補モデルの品質推定ではない。`MetricsAwareRoutingPolicy` (`routing.rs:182-210`) も harness の成功率デモで未配線。直接再利用しない |
| `Policy Optimizer` | **新設の offline 評価/提案側**。`RoutingAudit`、`QuotaEstimated`、proxy request log、review 結果から比較し、版付き policy artifact を作る境界 | `routing_audit` は決定済み run の投影で、棄却候補・反実仮想・proxy 内の最終 source を持たない。オンラインの選択者に昇格させず、shadow 比較と人の採用を経る |

後続実装で特に守る境界は、(a) adapter/harness と source を別設定にする ADR-0132、(b) `task-core` の決定的な policy と dispatcher/proxy の I/O 分離、(c) `celeris/<tier>` 外部インターフェース、(d) account の共有帳簿と別々の in-use 枠である。ADR-0124 の `TaskRouting.route` / `Event::ExecutionRouted` は planner を通すかどうかの判定であり、モデル候補の選択とは別である。進行中の cheap lane Qwen dispatcher 修正（task `01M44G5KKF`）はこの基点には含めず、後続の実装前にその結果を取り込んで再確認する。

## 7. 壊してはならない互換試験（現在の test 名）

| 目的 | test 名と所在 |
| --- | --- |
| proxy の source 順・Qwen cheap・fallback | `cheap_only_celeris_cheap_prefers_the_reachable_relay_when_prefer_free`, `cheap_only_celeris_cheap_falls_back_when_relay_connection_is_refused`, `cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen`, `cheap_only_relay_send_failure_falls_back_to_claude_cheap`, `cheap_only_unreachable_qwen_falls_back_to_gpt_cheap`, `cheap_only_models_lists_only_the_configured_sources` (`crates/llm-proxy/tests/proxy_integration.rs`) |
| OAuth pool、cooldown、再走査 | `claude_429_falls_back_to_the_next_account_and_records_a_cooldown`, `sources_view_reports_cooldown_and_hourly_counts`, `no_source_available_rescans_after_a_short_wait_and_then_succeeds`, `no_source_available_gives_up_with_retry_after_when_nothing_ever_recovers` (同上); `rank_across_pools_returns_a_full_fallback_order`, `select_across_pools_picks_the_higher_scoring_pool`, `select_from_pool_skips_cooldown_and_not_logged_in`, `rank_relays_keeps_config_order_among_reachable_sources` (`crates/llm-proxy/src/selection_tests.rs`) |
| 設定互換 | `cheap_only_default_and_legacy_qwen_config` (`crates/llm-proxy/src/config/tests.rs`); `provider_kind_legacy_production_inference_warnings_and_cheap_tier`, `provider_kind_explicit_source_rejects_adapter_model_and_missing_reference`, `provider_kind_qwen_direct_acp_source_reference_is_cheap_only`, `provider_kind_qwen_direct_acp_proxy_model_keeps_all_tiers` (`crates/celeris/src/config/tests.rs`) |
| lane・quota・escalation | `rule_table_maps_features_to_lanes`, `org_ceiling_clamps_the_lane_and_records_why`, `hints_override_axes_and_explicit_tiers_skip_the_policy`, `work_unit_lane_is_capped_by_the_task_lane` (`crates/task-core/src/model_policy/tests.rs`); `tier_resolution_never_substitutes_a_missing_or_disabled_model`, `difficulty_and_observed_quota_control_selection` (`crates/task-core/src/model_routing.rs`); `escalates_one_step_after_repeated_review_failures_and_is_bounded`, `never_escalates_above_the_ceiling_or_on_supply_side_failures` (`crates/task-core/src/retry_policy.rs`) |
| dispatcher と audit/API | `portable_work_uses_headroom_across_pools_then_falls_back`, `select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one`, `routing_applies_quota_tier_and_explicit_account_to_the_executed_model`, `lane_policy_decides_the_tier_and_records_the_routing_decision`, `repeated_review_failures_escalate_the_retry_lane_one_step` (`crates/task-dispatch/src/dispatcher/tests/routing_and_quota.rs`); `joins_routing_usage_wall_retries_and_review_per_run` (`crates/task-core/src/routing_audit.rs`); `routing_returns_per_run_audit_and_dropped_assignee`, `routing_is_empty_without_runs_and_404_for_unknown_tasks` (`crates/task-api/tests/routing.rs`) |

これは基点の試験名の固定リストであり、この WorkUnit では実行せずコードも変更しない。後続 Phase はここに候補ごとの除外理由、最終 source/model/account、shadow 比較を加える際、上記の既存期待値を保つ。
