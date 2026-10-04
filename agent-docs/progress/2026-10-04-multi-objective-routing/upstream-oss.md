---
tasks: [01M44H0SRV70E32AQ6C5N37MSK]
work_unit: upstream-oss
status: done
completed: 2026-10-04
---
# 多目的モデルルーティング: LLM router 6 実装の upstream 調査と再利用方式（2026-10-04）

この節の事実はすべて一次情報（GitHub API、各 repository の shallow clone のソース・LICENSE、Hugging Face の model API と LICENSE）から取った。取得日時は 2026-10-04 23:2x UTC。clone 先は worktree の外（`mktemp -d` の `/tmp/tmp.5T2Ay12UNr`）で、何も commit していない。論文は引いていない（実装のコメントとソースで手法の比較に足りたため）。現行の Celeris 側の対応は [inventory.md](inventory.md) を正本とする。

表の「90 日」は 2026-07-06T00:00Z 以降の数。commit 数は `commits?since=…&per_page=1` の Link ヘッダの last page で数えた。

## 1. 対象ごとの事実表

| 項目 | vLLM Semantic Router | RouteLLM | Arch-Router / plano（旧 archgw） | AutoMix | RouterBench | LiteLLM |
| --- | --- | --- | --- | --- | --- | --- |
| URL | https://github.com/vllm-project/semantic-router | https://github.com/lm-sys/RouteLLM | https://github.com/katanemo/plano（`katanemo/archgw` は改名でここへ転送）、model https://huggingface.co/katanemo/Arch-Router-1.5B | https://github.com/automix-llm/automix | https://github.com/withmartian/routerbench | https://github.com/BerriAI/litellm |
| 最新 release | `v0.4.0`（2026-09-27） | release・tag なし。pyproject 版 0.2.0 | `0.4.37`（2026-09-28）。model は HF sha `5b156890a91b`（lastModified 2026-04-02） | release・tag なし。setup.py 版 `automix-llm` 1.0.3 | release・tag なし | `v1.104.0`（2026-10-03。tag には `v1.105.0-rc.1` もある） |
| 調べた commit | `04be09cdfed2`（2026-10-04） | `0b64fdafe049`（2024-08-10） | `72002a62d90a`（2026-09-28） | `531af3ee3c4e`（2024-12-12） | `cc67d1008bd8`（2024-06-13） | `1d52985d0310`（2026-10-04） |
| SPDX | Apache-2.0（`LICENSE`、`bench/LICENSE`）。例外として `dashboard/wizmap/LICENSE` が MIT、`pkg/cache/testdata/paws-polarity/LICENSE` が PAWS データの notice | Apache-2.0 | code は Apache-2.0。**model weights は `license:other`**: KATANEMO COMMUNITY LICENSE（2026-04-02 版、Llama 3.2 Community License が下敷き、DigitalOcean, LLC）。base model は Qwen/Qwen2.5-1.5B-Instruct | Apache-2.0 | MIT（ただし copyright 行が "LangChain, Inc." のまま） | GitHub の判定は NOASSERTION。`enterprise/` は BerriAI Enterprise license（本番利用には subscription が要る）で、それ以外は MIT。`litellm/proxy/enterprise` は `enterprise/` への symlink |
| NOTICE・取り込む場合の notice | NOTICE ファイルは無い。Apache-2.0 §4 により、移植するなら LICENSE の写しと原典の明示、変更した旨の記載が要る | NOTICE は無い。Apache-2.0 の条件は左と同じ | code に NOTICE は無い。**weights を配布・同梱する場合**: 契約書の写し、"Built with DigitalOcean" の表示、Notice ファイルへの定型文、AUP の遵守が要る（HF の `LICENSE` §3.b） | NOTICE は無い。Apache-2.0 の条件は同じ | MIT の copyright と許諾文を残す | MIT 部分は copyright と許諾文を残す。`enterprise/` のコードは取り込み不可 |
| 言語と依存の大きさ | Go の本体（`src/semantic-router`。go.mod は直接 46、間接 77。Envoy go-control-plane、milvus、grpc、OTel）と Rust の FFI（`candle-binding` は candle 0.9.2-alpha・tokenizers・hf-hub、`onnx-binding` は ort 2.0.0-rc.10、`ml-binding` は linfa）。CLI は Python で依存 8。clone は 775 MB | Python。torch、transformers、datasets、openai、litellm、sklearn、pandas ほか（core 11。extras は serve・eval） | Rust（crate 5 個、`crates/Cargo.lock` は 469 package）。Envoy に proxy-wasm を載せ、`brightstaff` は独立の hyper server。CLI は Python（依存 11）。model は 1.5B の LLM | Python。numpy、pandas、scipy、tqdm（軽量）。self-verification 本体は notebook | Python。pin した巨大な freeze（torch 2.1.2、transformers 4.35、sentence-transformers、modal、pymongo、GCS、anthropic、replicate など） | Python。core 21、proxy の extras 34+19+10。`litellm/router.py` 1 本で 14,888 行。clone は 290 MB（blob は 2 MB 以下に絞った） |
| 更新頻度（90 日） | commit 978、release 1（v0.4.0） | 0、0（最終 2024-08） | commit 35、release 11 | 0、0（最終 2024-12） | 0、0（最終 2024-06） | commit 13,309、release 100 件以上（per_page 上限で頭打ち） |
| API の形 | Envoy External Processor（gRPC ExtProc）。単体の router service ではなく、Envoy の後ろの sidecar/gateway。設定は YAML（signals / projections / decisions / providers.models）。k8s CRD・operator・Helm がある | Python の `Controller` と、OpenAI 互換 server（FastAPI、Chat Completions のみ）。model 名に `router-<router>-<threshold>`（例 `router-mf-0.11593`）を書いて選ぶ | Envoy gateway（12000 番 = model listener、12001 番 = 内部の LLM endpoint）。routes は YAML の `routing_preferences`（`name`, `description`, `models`, `selection_policy.prefer: cheapest|fastest|none`）。router model は OpenAI 互換 chat で呼び、`{"route": [...]}` を受け取る | Python の `Automix` class（`train` / `infer` / `evaluate`）。入力は SLM と LLM の score と verifier の確信度を列に持つ DataFrame | script（`evaluate_routers.py`）と `AbstractRouter.batch_route_prompts`。dataset は pandas pickle | Python library の `Router` と、OpenAI 互換の proxy server。戦略は `async_get_available_deployments` を実装し、`CustomRoutingStrategyBase` で差し替えられる |
| 主要 file path | `src/semantic-router/pkg/classification/classifier_signal_*.go`, `…/classifier_projections.go`, `pkg/decision/engine.go`・`selection.go`, `pkg/selection/{selector,factory,candidate,static,elo,router_dc,automix,pomdp_solver,hybrid,latency_aware,multi_factor*,rl_driven,ml_adapter,offline_metrics}.go`, `pkg/extproc/shadow_dispatch.go`・`shadow_dispatch_call.go`, `pkg/shadowdataset/manifest.go`, `pkg/fallback/{orchestrator,circuit_breaker}.go`, `pkg/routerreplay/`, `config/config.yaml` | `routellm/routers/routers.py`（`Router.calculate_strong_win_rate`、`ROUTER_CLS`）、`routers/matrix_factorization/model.py`、`routers/similarity_weighted/utils.py`、`routers/causal_llm/`、`calibrate_threshold.py`、`openai_server.py`、`controller.py`、`evals/{evaluate,benchmarks}.py` | `crates/brightstaff/src/router/{orchestrator,orchestrator_model_v1,model_metrics}.rs`、`handlers/llm/model_selection.rs`、`config/plano_config_schema.yaml:652-680`、`config/envoy.template.yaml:541-549`、`demos/llm_routing/preference_based_routing/` | `automix/automix_methods.py`（`Threshold`, `DoubleThreshold`, `POMDPSimple`, `GreedyPOMDP`, `AutomixUnion`）、`automix/main.py`、`colabs/Step2_SelfVerify.ipynb` | `evaluation/AIQ.py`（`calculate_area_under_curve`, `get_non_descreasing_convex_hull_of`, `calc_AIQ`）、`evaluation/eval.py`、`routers/{abstract_router,knn_router,mlp_router,svm_router,run_cascading_router}.py` | `litellm/router.py`、`litellm/router_strategy/{simple_shuffle,least_busy,lowest_tpm_rpm_v2,lowest_latency,lowest_cost}.py`、`litellm/router_utils/{cooldown_handlers,cooldown_cache,get_retry_from_policy,fallback_event_handlers,health_state_cache}.py`、`litellm/types/router.py`（`RetryPolicy`、`AllowedFailsPolicy`）、`model_prices_and_context_window.json` |

### 対象ごとの要点

- **vLLM Semantic Router（最重要）**: 層の分離は依頼文の想定どおりにある。
  - 層の流れ: signal の抽出（`classifier_signal_dispatch.go`）→ projection（`partitions`、`weighted_sum` の score、`threshold_bands` の mapping。`config/config.yaml:755-800`）→ decision（AND/OR の規則木を `DecisionEngine.EvaluateDecisions` が評価し、priority・tier・confidence で `selectBestDecision`）→ candidate（decision の `modelRefs` と `candidateIterations`。capability・予算の適格判定は `pkg/extproc/selection_eligibility.go`・`selection_capabilities.go`）→ selection。
  - selection の interface（`pkg/selection/selector.go:308`）は `Select(ctx, *SelectionContext) (*SelectionResult, error)`、`Method()`、`UpdateFeedback(ctx, *Feedback)`、`Tier() AlgorithmTier`、`ExternalDependencies() []Dependency`。算法ごとに本番成熟度（Tier）と外部依存を自己申告させる形は、Celeris の pluggable estimator にそのまま参考になる。
  - 算法: static、elo、router_dc、automix（`automix.go` と `pomdp_solver.go`）、hybrid、latency_aware、multi_factor、rl_driven、gmtrouter、prompt、decision_model、session_aware、knn/kmeans/svm/mlp（`ml_adapter.go` から Rust の linfa）。
  - **shadow dispatch** は実際に要求を写して送る。decision 単位の plugin で、primary の応答を組み立てた後に標本化した要求を副 model へ POST する。lane ごとに `max_concurrency`・`max_queue_depth`・timeout・retries の上限があり、primary を待たせず変えもしない。結果は completed / failed / dropped と reason code で、`pkg/routerreplay` に既定では大きさ・tokens・出力の SHA-256 だけ（抜粋は任意で上限付き）を残す。`pkg/shadowdataset/` が primary と shadow の観測を版付きの比較 dataset にする。
- **RouteLLM**: interface は 1 個の関数 `calculate_strong_win_rate(prompt) -> float` と閾値で、2 択（strong/weak）に限る。
  - `mf` と `sw_ranking` は**要求ごとに OpenAI の embeddings API（`text-embedding-3-small`）を呼ぶ**（`matrix_factorization/model.py:88,113`、`similarity_weighted/utils.py` は import 時に `OpenAI()` を作る）。外部 API key と外部ネットワークが要る。
  - `bert` と `causal_llm`（Llama-3-8B）は手元で推論する。weights は HF の `routellm/*`。
  - 閾値は `calibrate_threshold.py` が strong の呼び出し率から決める。offline 評価（`evals/evaluate.py`）は「品質差の 20/50/80% に要る strong 呼び出し率」、AUC、APGR。
  - 2024-08 以降更新が無い。
- **Arch-Router / plano**: archgw は plano へ改名した。0.4.37 の routing は `OrchestratorService::determine_route`（`orchestrator.rs:320`）で、既定の router model は "Plano-Orchestrator"。Arch-Router-1.5B は `llm_routing_model` で指定する代替として docs と demo にだけ残る。
  - route は `name` と自然文の `description` だけで、domain/action の 2 軸の欄は無い。domain/action の区別は description の書き方に委ねている。
  - router model の prompt は `orchestrator_model_v1.rs:117`。8192 token・16 turn に切り詰める。route を決めた後は、その route の `models` を `model_metrics.rank_models` が `prefer: cheapest|fastest` で並べる。cost・latency のデータが無い model は最後に回して warning を出す（`model_metrics.rs:187-212`）。
  - retry は Envoy の route retry_policy に任せる（429/5xx、0.5〜5 s の backoff）。並べた残りの model は呼び出し側の fallback 候補として返すだけで、process 内の再試行ループは無い。
  - **weights の license は Apache ではない**（上表）。
- **AutoMix**: library 部分は閾値と POMDP の meta-verifier だけで、軽い（numpy・scipy）。few-shot の self-verification（「AI の答えは Correct か Incorrect か」を entailment として問う）は notebook にしかない。2024-12 以降更新が無い。vLLM SR は同じ手法を Go で再実装している（`pkg/selection/automix.go` の冒頭コメントが arXiv:2310.12963 を参照）。
- **RouterBench**: 指標は AIQ（非減少の cost-quality 凸包の下の面積）と oracle router。baseline は KNN・MLP・SVM・cascading。dataset は HF の `withmartian/routerbench` の pickle で、code は自動では取らない。依存が古く巨大で、2024-06 以降更新が無い。
- **LiteLLM**: router は 1 ファイル 14,888 行で、Redis 同期も前提にしている。
  - retry: `num_retries`、`RetryPolicy`（例外の種類ごとの回数）、`_time_to_sleep_before_retry`。
  - cooldown: `allowed_fails` は既定 3、`cooldown_time` は既定 5 s（`litellm/constants.py:93,95`）。`AllowedFailsPolicy` で例外の種類ごとに許容回数を変えられる。
  - fallbacks: 通常、context window 超過、content policy 違反の 3 種がある。
  - 戦略: shuffle、least-busy、tpm/rpm、latency、cost のほか、`auto_router`・`complexity_router`・`quality_router` も同居する。
  - `model_prices_and_context_window.json`（3 MB、4,473 key）は価格・context 長・45 種の `supports_*` 能力 flag を持つ事実上の共通台帳。schema は `model_prices_and_context_window.schema.json`。

## 2. コンポーネント別比較表

(a) = library または sidecar として再利用、(b) = 小さなアルゴリズムやデータ構造を Rust へ移植、(c) = 設計だけ参考にして自前実装。「追従コスト」は採った方式で upstream の変化を追う手間のこと。

| # | コンポーネント | (a) 再利用 | (b) 移植 | (c) 設計参考 | **推奨** | 出典（URL / file path） | license | 追従コスト |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | ModelProfile・能力 metadata | LiteLLM の `model_prices_and_context_window.json` を data として取り込む。MIT。ただし 3 MB・4,473 key で日々変わり、subscription（Claude/Codex の OAuth）や self-host Qwen の行は持たない | 欄名と型だけ写す（`input_cost_per_token`, `output_cost_per_token`, `max_input_tokens`, `max_output_tokens`, `supports_*`） | SR の `providers.models[].pricing{prompt_per_1m, cached_input_per_1m, completion_per_1m}` と `reliability`、plano の cost/latency 欠落時の扱いを参考にする | **(c)**。欄名は LiteLLM に合わせ、将来 (a) の取り込み（任意の import 命令）ができる形にする。台帳の正本は Celeris の設定と DB | https://github.com/BerriAI/litellm `model_prices_and_context_window.json`・`.schema.json`、https://github.com/vllm-project/semantic-router `config/config.yaml:14-60` | 欄名の参照だけなので notice は不要。data を同梱するなら MIT の notice が要る | 低。import を作るなら schema の変化を版ごとに確かめる |
| 2 | source/deployment の状態（SourceState / DeploymentProfile） | LiteLLM の `health_state_cache`・`CooldownCache`。Python で Redis 前提のため Rust の daemon に入らない | — | SR の `reliability{lb_policy, retry_count, consecutive_5xx, base_ejection_time, health_check_*}`、LiteLLM の deployment 単位の健康状態と pre-call check（`router_utils/pre_call_checks/`） | **(c)**。既存の `AccountBook`、relay probe、`SourceView` を `SourceState` へ合成する（inventory §6） | SR `config/config.yaml`（reliability）、LiteLLM `litellm/router_utils/health_state_cache.py`・`pre_call_checks/` | 参照だけ | 低 |
| 3 | routing policy | SR の decision engine。Go で Envoy ExtProc に組み込まれていて単体では使えない | AND/OR の規則木と priority/tier の選び方（`pkg/decision/engine.go`, `selection.go`）。数百行の Go | SR の層分離（signal → projection → decision → candidate → selection）、plano の `selection_policy.prefer` | **(c)**。frontier/standard/cheap を policy/SLO 名（quality-first / balanced / resource-first）に読み替え、層の境界は SR に倣う。規則木は既存の `ModelPolicy` の規則表で足りるので移植しない | SR `pkg/decision/engine.go`, `pkg/selection/selector.go`、plano `config/plano_config_schema.yaml:652-680` | 参照だけ | 低 |
| 4 | signal・context の抽出 | SR の classifier 群（BERT・ModernBERT を candle/ONNX で）、または Arch-Router-1.5B を sidecar で。どちらも prompt 本文を分類する。GPU か CPU 推論と model weights が要り、Arch-Router は community license | — | SR の signal の種類（domain・complexity・context・conversation など）、plano の route（自然文の description） | **(c)**。Celeris の文脈は prompt ではなく task metadata（`TaskFeatures`、受け入れ条件、review・検査の履歴）なので、`RoutingContext` を自前で組む。prompt の分類器は Phase 5 の sidecar の対象としてだけ残す | SR `pkg/classification/classifier_signal_*.go`、plano `crates/brightstaff/src/router/orchestrator_model_v1.rs:117`、HF `katanemo/Arch-Router-1.5B` LICENSE | 参照だけ。Arch-Router の weights を使うなら Katanemo Community License の表示義務がある | 低（(a) を選べば中〜高: model と license の改版を追う） |
| 5 | quality estimator | RouteLLM を sidecar（OpenAI 互換 server か、`calculate_strong_win_rate` を包む小さな HTTP）で使う。人の決定 adapter-plus-routellm はこれを shadow でだけ試す | RouteLLM の `sw_ranking`（類似度で重みを付けた Elo）の数式 | SR の `Selector.Tier()` と `ExternalDependencies()` による自己申告 | **既定は (c)**（表と規則に基づく自前の決定的 estimator）。**Phase 5 だけ (a)**: 汎用 sidecar adapter で RouteLLM を shadow で試す。`mf`・`sw_ranking` は外部の OpenAI embeddings を呼ぶので、既定の候補は `bert`（手元推論）にする。試験には外部ネットワークを入れない（偽 sidecar を使う） | RouteLLM `routellm/routers/routers.py`, `openai_server.py`, `routers/matrix_factorization/model.py:88,113` | Apache-2.0。別 process の sidecar で同梱しなければ notice は不要。weights（HF `routellm/*`）の license は未確認（§5） | (a) は中。upstream は 2024-08 から止まっているので commit `0b64fdafe049` に pin する。依存（torch・transformers・litellm）の更新は Celeris 側で持つ |
| 6 | 多目的 selection | SR の selector 群。Go と FFI なので不可 | SR `hybrid.go`（Elo・RouterDC・AutoMix と cost の重み付き合成、477 行）、`latency_aware.go`、`multi_factor*.go` の重み付き和。LiteLLM `lowest_cost.py`（304 行） | plano の `rank_models`（cheapest / fastest、データ欠落は最後）、SR の `SelectionResult` が理由を持つ形 | **(c)**。重み付き和と制約による除外は数十行の自明な式で、移植しても得が少ない。監査に必要な「落ちた候補と理由・各 score」は SR の `SelectionResult` の形を参考に自前で定義する。Elo の更新式などを将来入れるときだけ (b) にして出典を残す | SR `pkg/selection/{hybrid,latency_aware,multi_factor,elo}.go`、plano `crates/brightstaff/src/router/model_metrics.rs:187-212`、LiteLLM `litellm/router_strategy/lowest_cost.py` | 参照だけ。(b) にするなら Apache-2.0 の notice が要る（§5 の未決事項） | 低 |
| 7 | cooldown・retry・fallback | LiteLLM の Router（Python）を前に置く。依頼の方針（llm-proxy 全体は置換しない）に反する | SR `pkg/fallback/circuit_breaker.go`（154 行。closed / open / half-open と `half_open_probes`） | LiteLLM の `AllowedFailsPolicy`（例外の種類ごとの許容回数）、context window 超過・content policy の専用 fallback、`RetryPolicy`。plano の Envoy retry（429/5xx、backoff） | **(c)**。既存の proxy の fallback（stream の最初の byte までは次へ、401/429 は account の cooldown）を保ち、足すのは 2 つ: 例外の種類ごとの許容回数と、half-open の試し打ち。どちらも教科書どおりの状態機械なので自前で書く | LiteLLM `litellm/router_utils/cooldown_handlers.py`, `litellm/types/router.py:118,814`、SR `pkg/fallback/circuit_breaker.go`, `orchestrator.go` | 参照だけ | 低 |
| 8 | cascade・escalation | AutoMix の library（`Automix` class）。入力は要求単位の verifier の確信度で、Celeris の軌跡（review・検査の失敗）とは入力が合わない | AutoMix の `Threshold`・`DoubleThreshold`（数十行）、SR の `pomdp_solver.go`（348 行） | AutoMix の考え方（安い model → 自己検証 → 確信度が低ければ上げる）と IBC（Incremental Benefit per Cost） | **(c)**。Celeris の escalation は task の軌跡（受け入れ条件・review・tests・retry の失敗の蓄積）に沿い、既存の `EscalationPolicy` を拡張する。要求単位の self-verification と POMDP は今は入れない。IBC は Phase 4 の評価指標の候補にする | AutoMix `automix/automix_methods.py`, `automix/main.py`、SR `pkg/selection/automix.go:33-49`, `pomdp_solver.go` | 参照だけ | 低 |
| 9 | shadow dispatch | SR の shadow plugin。Envoy ExtProc の中でしか動かない | — | SR の設計を参考にする: decision 単位の opt-in、標本化、`max_concurrency`・`max_queue_depth`・timeout、primary を待たせない・変えない、completed / failed / dropped と reason code、出力は既定でハッシュと tokens だけ、版付きの比較 dataset（`shadowdataset`） | **(c)**。人の決定（opt-in-capped、既定 off）にちょうど合う。Phase 4 は (1) 決定だけを記録する shadow（追加の LLM 呼び出しなし）を既定にし、(2) 実際に送る shadow は上限付きの opt-in にする | SR `pkg/extproc/shadow_dispatch.go`, `shadow_dispatch_call.go`, `pkg/config/plugin_config.go:275`, `pkg/shadowdataset/manifest.go`, `config/fragments/plugin/shadow-dispatch/sampled.yaml` | 参照だけ | 低 |
| 10 | offline 評価・指標 | RouterBench・RouteLLM の eval script を Python で回す。依存が巨大で古く、dataset は一般 QA で Celeris の coding や review の結果とは領域が違う | 指標の式を移植する: RouteLLM の APGR と「品質 x% に要る strong 呼び出し率」（`evals/evaluate.py:77-121`）、RouterBench の非減少凸包と AIQ（`evaluation/AIQ.py`、数十行）、AutoMix の IBC | SR の `routerreplay` と offline dataset・metrics（`pkg/selection/offline_dataset.go`, `offline_metrics.go`） | **(b)**。指標の式だけを Rust へ移植し、Celeris の過去 log（`RoutingDecided`、`WorkerFinished`、review の結論、cost・latency・retries）に当てる。dataset は取り込まない | RouteLLM `routellm/evals/evaluate.py`、RouterBench `evaluation/AIQ.py`、AutoMix の IBC、SR `pkg/selection/offline_metrics.go`, `pkg/routerreplay/` | Apache-2.0（RouteLLM）と MIT（RouterBench）。式を逐語で移すなら関数の頭に出典と license を書き、third-party notice に載せる。式を論文から独立に書くなら不要 | 低。upstream は止まっていて式は変わらない |

## 3. 推奨のまとめ

- **巨大な外部 router は fork も埋め込みもしない**。
  - vLLM SR は Envoy ExtProc と Go/Rust FFI と BERT 系 weights の束。LiteLLM は 1.5 万行の Python router に enterprise の区分がある。plano は Envoy と WASM の gateway。どれも llm-proxy（Rust、in-process）の「第 2 判断」の位置には入らない。
  - 一方で SR の層分離・`Selector` interface（Tier と外部依存の自己申告）・shadow dispatch の上限設計は、Celeris の routing kernel の設計にほぼそのまま使える。
- 方式の内訳:
  - **(c) が 8 件**: ModelProfile、SourceState、policy、context、selection、cooldown/fallback、escalation、shadow。
  - **(b) は 1 件**: offline 評価の指標の式（APGR・凸包 AIQ・IBC）。
  - **(a) は 1 件**: Phase 5 の quality estimator の sidecar（RouteLLM、shadow のみ）。
  - 将来の任意: LiteLLM の価格台帳の import（(a) の data 取り込み）。
- 前提を変える事実が 3 つある:
  1. Arch-Router-1.5B の weights は Apache ではなく Katanemo Community License（表示義務あり）。plano 本体はすでに既定 model を Plano-Orchestrator に替えている。
  2. RouteLLM の `mf`・`sw_ranking` は要求ごとに OpenAI の embeddings API を呼ぶ。外部 API key も外部ネットワークも持たない構成では `bert` が現実的な候補になる。
  3. RouteLLM・AutoMix・RouterBench は 2024 年から更新が止まっている。追従コストは小さいが、保守もされない。pin して使う。
- arch-adr への引き継ぎ: `QualityEstimator` の trait に、SR の `ExternalDependencies()` と `Tier()` に当たる自己申告（成熟度、外部依存、ネットワークの要否）を持たせる。estimator は最終決定権を持たず、score と理由だけを返す。

## 4. 取得コマンドの記録

すべて 2026-10-04 に、この run の中で実行した。`gh` は既存の認証を使った。

```sh
T=$(mktemp -d)   # /tmp/tmp.5T2Ay12UNr（worktree の外）
for r in vllm-project/semantic-router lm-sys/RouteLLM katanemo/archgw automix-llm/automix withmartian/routerbench BerriAI/litellm; do
  gh api repos/$r --jq '{full_name,html_url,default_branch,language,license:.license.spdx_id,pushed_at,stargazers_count,archived,size}'
  gh api repos/$r/releases/latest --jq '{tag_name,published_at,name}'      # RouteLLM・automix・routerbench は 404（release なし）
  gh api "repos/$r/commits?per_page=1" --jq '.[0]|{sha:.sha[0:12],date:.commit.committer.date}'
done
# 90 日の commit・release と最新 tag（archgw は katanemo/plano へ転送されるので plano で引いた）
S=2026-07-06T00:00:00Z
gh api -i "repos/$r/commits?since=$S&per_page=1"   # Link ヘッダの rel="last" の page 番号を数として読む
gh api "repos/$r/releases?per_page=100" --jq "[.[]|select(.published_at>\"$S\")]|length"
gh api "repos/$r/tags?per_page=3" --jq '[.[].name]|join(",")'
gh api repos/katanemo/archgw --jq .html_url        # → https://github.com/katanemo/plano
# shallow clone（worktree の外）
cd $T; for r in vllm-project/semantic-router lm-sys/RouteLLM katanemo/plano automix-llm/automix withmartian/routerbench; do git clone --depth 1 https://github.com/$r.git; done
git clone --depth 1 --filter=blob:limit=2m https://github.com/BerriAI/litellm.git
# Hugging Face の model card の metadata と LICENSE
curl -s https://huggingface.co/api/models/katanemo/Arch-Router-1.5B    # sha 5b156890…、license other、base Qwen2.5-1.5B-Instruct
curl -sL https://huggingface.co/katanemo/Arch-Router-1.5B/resolve/main/LICENSE
# 以降は clone の中で、ソースと LICENSE を読み取りだけで確かめた（ls, sed -n, grep -n, wc -l, find -name LICENSE*）
```

調べた HEAD: semantic-router `04be09cdfed2`、RouteLLM `0b64fdafe049`、plano `72002a62d90a`、automix `531af3ee3c4e`、routerbench `cc67d1008bd8`、litellm `1d52985d0310`。

## 5. 未確認事項と未決事項

- **RouteLLM の HF weights と RouterBench の HF dataset の license は「宣言なし」**。`curl -s https://huggingface.co/api/{models/routellm/bert_gpt4_augmented,models/routellm/mf_gpt4_augmented,models/routellm/causal_llm_gpt4_augmented,datasets/withmartian/routerbench}` で取った（sha はそれぞれ 86237e3df400 / 5eb3dc745cbe / 43d13d6be832 / 784021482c3f）。どれも `cardData.license` が空で、`license:` tag も無い。
  - `causal_llm` は Meta-Llama-3-8B の派生なので、Llama 3 の license を引き継ぐと考えるのが自然。ただしこれは推定で、確認していない。
  - したがって weights を Celeris に同梱・再配布しない。Phase 5 の sidecar は人が手元で HF から取る形にとどめ、本番切り替えの前に人に license の判断を上げる。
- **LiteLLM の 90 日 release 数**: API の 1 page（100 件）で頭打ちになったため「100 件以上」とした。正確な数は判断に効かないので数えていない。
- **`vscode-extension/LICENSE` と `terraform/provider/LICENSE`（LiteLLM）**: 読んでいない。router の取り込みには関係しない。
- **Celeris に LICENSE ファイルと Cargo の `license` 欄が無い**: (b) で Apache-2.0 や MIT のコードを逐語で移すなら、third-party notice を置く場所（例: repo 根の `THIRD_PARTY_NOTICES.md`）を決める必要がある。今回の推奨では逐語の移植は指標の式だけで、論文の式から独立に書けば notice は要らない。どちらにするかは arch-adr か p4 で決める（提案: 独立に書き、出典は doc comment に URL で残す）。
- 論文は引いていない。手法の比較には、実装のコメント（SR の `automix.go` が arXiv:2310.12963 を参照）とソースで足りた。

## 提案

- arch-adr: §2 の 10 行をそのまま ADR の「再利用方式」の節に引き、`QualityEstimator` に外部依存とネットワーク要否の自己申告を入れる。
- p4-shadow-eval: SR の shadow の上限（`max_concurrency`、`max_queue_depth`、timeout、verdict と reason code、出力はハッシュのみ）を受け入れ条件に写す。
- p5-estimator: RouteLLM sidecar の既定 router は `bert`（外部 API 不要）にする。`mf` は OpenAI の key が要る旨を明記する。
