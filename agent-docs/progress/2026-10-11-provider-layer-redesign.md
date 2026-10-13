---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
title: provider 層の再設計: 現状棚卸し
updated: 2026-10-12
status: done
completed: 2026-10-12
---
# provider 層の再設計: 現状棚卸し

## 棚卸し

確認対象は指定された config・core・dispatch・API・Web/旧GUI・文書と、追跡済みの config 例・ADR である。本番 config は読んでいない。本表は現状の実装を3層の意図に分類したもの。provider は source と harness を結ぶ関係に限定するのが目標であり、現状は provider 行に複数の責務が残る。

| 現在の欄・経路 | 現在の意味/実装 | 3層分類 | 問題・整理方向 |
|---|---|---|---|
| `config/providers.rs`: `adapter`, command/args, settings, env | 実行アダプタと固有設定 | harness | adapter 固有値は harness 定義へ。provider は harness 参照のみ |
| providers: `llm_source`, `model`, `tiers`, `tier_models` | source と model/実行可能 tier を同じ行に保持 | provider (source↔harness の対応・model mapping) | `llm_source` は source ID 参照、model 対応も関係の明示的な写像へ。source/adapter からの推定を廃止 |
| providers: `account_pool`, `concurrency` | account の選択/旧上限。`concurrency` は deprecated でも deserialize・警告・API 表示が残存 | account pool・上限は LLM source | pool を source の account collection に移し provider row から削除。deprecated concurrency も設定/API から削除 |
| `env_keys`, `credential_refs` と adapter env | credentials/env の伝搬 | LLM source（接続認証）または harness（プロセス環境） | source credentials と harness process env を別管理し provider に秘密を保持させない |
| `config/harness.rs` | harness 定義・実行契約 | harness | harness capability (tool/browser/credential 等) をここで宣言し、source 対応制約と区別 |
| `config/model_routing.rs`, `dispatch.rs` | tier と adapter/provider 候補選択、各種 routing 制約 | provider 関係 | 候補は明示された harness-source-model 関係から選ぶ。source 能力・account 状態は source 側を参照 |
| `config/cos.rs` と CoS session selection | CoS の実行経路・session の provider/source/harness照合 | provider関係 / harness | 現行照合 key は provider・harness・source・account・cwd・model。provider ID変更の影響を切り離す必要 |
| `config/model_catalog.rs` | catalog deployment に source と adapter constraint を併記し、provider row から seed を作る | source + harness + provider関係が混在 | catalog の供給元・対応 harness・選択関係を別オブジェクトとして表現 |
| `task-core/provider_source.rs` (`LlmSourceRef`, `SourceOrigin`, `Unknown`) | source参照とderived/unknown状態を型で表現 | LLM source参照 / 廃止対象 | SourceOrigin/derived と Unknown は明示的 source registry で不要にする |
| `AccountPoolSetting`; `task-core/store/account_limits.rs` | adapterごとの実account poolとsource単位implicit accountの上限永続化 | LLM source | 上限は既に account_limits に永続化。implicit account の鍵を source に統一し、provider行由来の暗黙生成をなくす |
| `task-dispatch/dispatcher/provider_select.rs`, `accounts` | provider候補とsource/accountの空き・利用量を組み合わせ選択 | provider関係 + source account | 関係候補の選択とsource account admissionを分ける |
| `task-api/handlers/providers.rs` GET/POST/PATCH | GETに`concurrency`, `in_use_cos`等。旧concurrencyも受理・警告。provider設定編集 | provider関係 / 廃止 | concurrency field削除。観測値はsource/account endpointへ移す。GET snapshot由来の値はtick遅延。`in_use_cos`もprovider行の恒久的責務にしない |
| `handlers/accounts.rs`, account limits | account CRUD、`max_runs`, `effective_max_runs`, implicit account | LLM source | account/上限をsource配下にする。source IDをaccountの安定した所有者に |
| `handlers/llm_sources.rs`, `model_assignments.rs` (handler実体は当該名の独立ファイルとして見つからず、routes/別moduleを要確認) | llm-proxy source と catalog assignment API | LLM source / provider mapping | handler配置・実際のroute登録を次段で確定。source設定とprovider mapping APIを分ける |
| `llm-proxy.sources.*`, `llm_proxy.models` | 接続source設定とsource/model/tier routing | LLM source | credentials/endpoint/quota/available modelsをsource層の正本に。`celeris` proxyはsource自体でなくproxy経路概念か、次段ADRで整理 |
| `web/routes/providers.tsx`, `accounts.tsx`, `models.tsx`; `gui/app/routes/providers.tsx`, `accounts.tsx` | provider編集、account管理、source表示/モデル割当 | provider / source / harness混在 | UIもsource/harness独立編集と対応関係編集へ分割。旧GUIはWebとの同等整理対象 |
| `docs/guides/providers.md`, `docs/ops/provider-llm-source-migration.md` | deprecated concurrency/implicit account運用や移行例を説明 | 廃止対象を含む文書 | 新3層の概念・本番手順へ更新が必要 |
| ADR-0053, ADR-0069, ADR-0132, `2026-10-08-account-concurrency-limits` | proxy/source・routing・provider/source分離・account concurrencyの決定 | 設計決定 | 旧決定のうちderived source/provider concurrency/implicit accountに依存する記述を更新または supersede |

### 本番8行の3層分解（repo資料からの推定）

| 現行ID | LLM source層（推定） | harness層（推定） | provider関係・model mapping（推定） | 確度・未確定点 |
|---|---|---|---|---|
| `claude-pool` | `claude_oauth`; Claude accounts | `claude-code` | harness-source組、複数tierのClaude model mapping | 高。config例/ADR-0024, 0069。実account名・quotaは本番未参照 |
| `codex-pool` | `codex_oauth`; Codex accounts | `codex` | harness-source組、frontier/standard/cheap model mapping | 高。config例/ADR-0069。実account名は未確認 |
| `opencode-qwen` | `openai_compatible:qwen`（sourceはproxy経由になる可能性あり） | `acp` + opencode | cheap Qwen mapping | 高。ADR-0132/移行文書。現設定がdirectかproxyかは資料間で時点差あり |
| `pi-qwen` | `openai_compatible:qwen` | `pi` | Qwen model mapping | 中。account docsは同sourceで共有と記載、具体的本番modelは未確認 |
| `pi-opencode-go` | `opencode_go` subscription | `pi` (ACP-compatible route) | opencode-go model/tier mapping | 中。config tests/ADR-2026-10-06 が型を示す。本番行の明示設定未確認 |
| `ldr-qwen` | `celeris` proxy (upstream Qwenはproxy側) | `local-deep-research` | `celeris/cheap` | 高。移行手順に記載。proxy upstream runtime選択は別 |
| `paperqa-qwen` | **unknown (誤推定を避ける)**。文書はproxy利用を示すが、現configの明示sourceは本番非参照 | `paperqa` | PaperQA settings / `openai/celeris/standard` との記載あり | 中。repositoryの過去例・文書は食い違う。下記コードがsource判定不能になりうる根拠 |
| `langmem-main` | `celeris` proxy | `langmem` | `celeris/cheap` | 高。移行手順/設定例に記載 |

`paperqa-qwen` が `unknown` となるコード根拠: `crates/celeris/src/config/providers.rs`（削除済み） の `derive_llm_source` (302行付近) は `fake`, `claude-code`, `codex`, ACP/Pi opencode-go、Celeris model、特定ACP Qwen条件だけを認識し、それ以外は `LlmSourceRef::Unknown` を返す。`paperqa` adapter はこの分岐にない。明示 `llm_source` が無い行では `provider_llm_source` (177–185行付近) が derived値へフォールバックし、validation/warning (368–371行付近) が `unknown_llm_source` を出す。PaperQA設定ファイル内のmodel/endpointをこの判定関数がsourceとして解析しないため、行IDの `-qwen` suffixだけではsourceを決定できない。従って資料上のproxy利用から本番sourceを断定せずunknownとした。

## 旧欄・互換の具体的な残存

- `ProviderConfig.concurrency` と `ProviderSpec.concurrency` が残り、設定読込時はdeprecated warningを出す。`task-api/handlers/providers.rs` はGET応答にもfieldを返し、POST/PATCHで値を受理して警告する。`in_use_cos` はprovider snapshot上のCoS run数。
- `provider_llm_source` は明示sourceと `derive_llm_source` の両方を許し、`SourceOrigin::{Explicit,Derived}` を返す。`Unknown` は有効な型値で、sourceが決まらない行を表現する。
- provider選択とimplicit account keyが解決済みsourceに結び付いている。paperqa等のsource不明行は `unknown` に束ねられ、source別上限の意味を壊す。
- `docs/guides/providers.md` はdeprecated concurrencyを「読める・warn」、implicit accountをsource単位と説明。これは旧互換を明文化している。

## 調査範囲・作業状態

参照した実装: `crates/celeris/src/config/{providers,harness,model_routing,dispatch,cos,model_catalog}.rs`、`crates/task-core/src/provider_source.rs` と account limit/store、`crates/task-dispatch/src/dispatcher/provider_select.rs` と accounts、`crates/task-api/src/handlers/{providers,accounts}.rs`。UIは指定されたWeb/GUI routeを確認。資料は `docs/guides/providers.md`、`docs/ops/provider-llm-source-migration.md`、ADR-0053/0069/0132、2026-10-08 concurrency ADR、config例を参照。

このWorkUnitでは現状棚卸しのみを記録し、コード・API・画面・既存設計文書は変更していない。本番設定も読まず、変更していない。次工程のADRで責務境界と旧欄の削除方針を確定し、実装工程が設定・API・画面・移行文書を更新する。

## 設計の要約

[決定 ADR](../adr/2026-10-11-llm-source-harness-selection.md) は provider と固定の source/harness 組を廃止する。task 内容から harness を決定的に選び、その harness 制約下で source×model を別の routing policy が選ぶ。候補列挙・harness 適格性・quota/cooldown/同時実行 admission と policy の境界を定め、評価根拠がない間は source ID/model ID の辞書順を既定にする。CoS は本番で `claude_code` harness と `claude_oauth` source を使う。`llm-proxy` は source でなく上流 source/account の認証・容量を保持する変換器であり、sources/models の alias 写像は proxy 接続設定、canonical source/model は source registry に置く。provider API と web・旧 GUI 画面を削除し、source/account/harness API と画面を責務別にする。run に選択 harness/source/model と理由を migration で追加し、events は追記専用とする。8 行の本番用途は source と harness を独立させて移行し、credential・account login・既存 events は保持する。`paperqa-qwen` の実 source/model は本番設定を確認してから明示し、推測で埋めない。本番操作は人が行う手順として記録する。

## harness と source の対応

この表は実装コード・追跡済み config 例・試験・ADR/進捗記録を突き合わせたもの。プロトコルは adapter が直接使う API、または harness の CLI が接続する API 形式を示す。CLI の OAuth は API 互換性と別軸である。能力欄は adapter の固有機能と task context の条件付き機能を分けて読む。記載のない source/harness の組み合わせは互換性を推測せず「未実測」とした。試験が fake CLI/HTTP である場合はその事実も明記し、実アカウントへの成功実績とは扱わない。外部ネットワーク・LLM 呼び出しを伴う試験は行っていない。

| harness / 実装 | 話す形式・source 認証 | 受け付ける source（根拠） | model の渡し方 | 実装能力（条件付き機能を含む） | 実行実績 |
|---|---|---|---|---|---|
| claude-code (`task-worker/src/claude_code.rs`) | Claude CLI の stream-json worker protocol。CLI 自身が Anthropic API を使用し、認証は `claude_oauth` アカウント環境。OpenAI chat/responses 直結ではない | `claude_oauth`（account/provider 設定例）、または CLI env/config が向く先。`openai_compatible:qwen` や proxy は CLI の endpoint/auth 設定を別途与えた場合のみ候補で、組み合わせ実測なし | `--model`（adapter config または provider model）；env/settings は `[adapters.claude_code]` | tool 実行・repo 作業、skills 配送、画像入力、task context 付き browser/credential supervisor（許可された browser run に限る）。研究文献/Web 調査/記憶整理は専用機能なし | 実績あり: `claude_code` worker 試験・`claude_account` 試験は stub。OAuth 経路は ADR-0024 と account 運用記録。特定の実 source×model の現行接続成功は未実測 |
| codex (`task-worker/src/codex.rs`) | Codex CLI `exec --json` protocol; `codex_oauth` CLI login. OpenAI Responses API を CLI が扱うが adapter 自体は CLI protocol | `codex_oauth` のみ実績資料あり。OpenAI compatible endpoint を Codex CLI が受ける設定はこの repo の例/試験で確認できず未実測 | `--model`, `-c`/config, env (`[adapters.codex]`) | tool 実行・repo 作業・skills。task context を使う browser/credential 実行系あり（browser supervisor 経由）。専用文献/Web/記憶機能なし | 実績あり: Codex adapter/アカウント stub 試験・resume 試験、ADR-0008。本番 OAuth の実行可否は現環境では未実測 |
| opencode (ACP worker route; `task-worker/src/acp.rs`, config `adapters.acp`) | ACP JSON-RPC が worker 境界。OpenCode backend は OpenAI-compatible provider/API を想定可能。`opencode_go` は subscription/CLI 側認証 | `opencode_go` (ACP+opencode-go example)、`openai_compatible:qwen` (ACP-opencode example); celeris proxy endpoint を向ける構成も可能だが具体例の model/source に限る | ACP session config 内の model/provider、adapter settings/env (`OPENCODE_CONFIG_CONTENT` 等); provider 行が現状値を渡す | tool 実行・repo 作業・skills。browser/credential は一般 tool でなく browser supervisor/context がある task のみ。専用 literature/Web/記憶なし | 実績あり: ACP fake agent protocol tests と `config/celeris.acp-opencode.example.toml`, `config/celeris.opencode-go.example.toml`; opencode-go live account の外部呼出しは未実測 |
| pi (`task-worker/src/pi.rs`) | pi CLI の JSON/RPC harness protocol; pi model provider は Anthropic/OpenAI/OpenAI-compatible family を扱える構成。OAuth は pi/backend の CLI credentials に依存 | `openai_compatible:qwen` (pi-hashline config)、`opencode_go` は source/harness pair を示す既存記録あり。`claude_oauth`/`codex_oauth` の直結は資料に無く未実測。proxy は endpoint 設定を与える構成のみ | 起動引数と pi の model/provider config、環境変数。具体的な adapter/config 機構は `task-worker/src/pi.rs`、provider 側モデル指定 | tool 実行・repo 作業・skills。専用 browser/credential supervisor 接続、文献/Web/記憶機能はコード上の専用実装なし | 実績あり: adapter fake/process tests、`config/celeris.pi-hashline.example.toml` および model-family routing test。実 source×model の live 呼出しは未実測 |
| acp generic (`task-worker/src/acp.rs`) | Agent Client Protocol。backend が話す API は ACP process の実装次第（OpenAI chat/responses/Anthropic のいずれも ACP 自体からは断定不可） | `openai_compatible:qwen`, `opencode_go` の設定例。celeris proxy は backend endpoint/config で向ける場合に限る。OAuth source の一般受け入れ保証なし | ACP `session/new`/model config options と adapter env/settings | tool 実行・repo 作業・skills配送。browser/credential は共通 supervisor が context へ提供するとき。専用文献/Web/記憶なし | 実績あり: fake ACP server に対する protocol/session tests、上記 config examples。実 backend への接続は未実測 |
| local-deep-research / ldr (`task-worker/src/local_deep_research.rs` 等) | LDR Python harness。LLM API は OpenAI-compatible client、通常 celeris llm-proxy | celeris proxy 経由 (`celeris/cheap` / proxy URL) が research example に記載。`openai_compatible:qwen` を proxy source とする例。直接 `claude_oauth`/`codex_oauth`/`opencode_go` は未実測 | settings/config file（LDR 設定）、環境変数で proxy URL/key、設定で model | 一般 Web 調査・検索/複数ページ読解・出典付き報告。repo 必須でない。一般 tool/browser supervisor/credential、学術文献専用・記憶整理は別機能 | 実績あり: fake/stub harness tests と `config/celeris.web-research.example.toml`、LDR run 運用記録。外部検索・LLM を使う実接続はこの棚卸しでは未実測 |
| paperqa (`task-worker/src/paperqa.rs`) | PaperQA2 Python harness、OpenAI-compatible LLM client | celeris proxy 経由の記述は research config/doc にある。provider row の derived source は unknown になり得る（上の棚卸し）。直接 OAuth CLI source は未実測 | PaperQA settings file (`pqa -s`)、env/API base、model は settings に記述 | 学術文献検索・取得・PDF 根拠抽出・引用。repo 必須ではない。一般 Web 調査/browser supervisor/credential/記憶整理は専用能力に含まれない | 実績あり: PaperQA wrapper/config/stub 試験、`config/celeris.research.example.toml` と literature progress records。実際の検索/LLM 呼出しは未実測 |
| langmem (`task-worker/src/langmem.rs`) | Python LangMem harness が OpenAI-compatible API を利用 | celeris proxy 経由。`[knowledge.langmem]` の provider/base_url/model/api_key_secret が接続元。direct CLI OAuth は未実測 | `[knowledge.langmem]` の model/base_url/secret、adapter python command/env | 記憶・知識候補の抽出/整理（タスク単位）。repo は入力に応じるが必須でない。tool/browser/credential/文献/Web検索は専用機能なし | 実績あり: langmem fake/stub tests、config例と ADR-0047。実 LLM/proxy backend の実呼出しは未実測 |
| aider (`task-worker/src/aider.rs`) | Aider CLI。通常 OpenAI-compatible chat completion endpoint。 | config に設定した API endpoint/source。`openai_compatible:qwen` や celeris proxy は env/config を渡せば候補だが source pairing の repo 実績なし。OAuth CLI source は未実測 | CLI 引数 (`--model` 等)・環境変数・aider config | tool/patch による repo 編集が主用途、repo 必須。browser/credential/文献/Web調査/記憶整理なし | 実績あり: `config/celeris.aider.example.toml` と adapter stub/process tests。実 LLM source 接続は未実測 |
| browser specialist / browser系 (`task-worker/src/browser_specialist.rs`, `browser.rs`, `browser_supervisor.rs`) | 独立した LLM API harness ではなく、選択済み worker harness の browser tool/control/runtime 層 | 上流 source は親 worker harness が決定（browser 自身は source を受け付けない） | model は親 worker adapter に渡す。browser config/launch policy は別設定 | browser 操作、承認・credential injection gate、live event/ledger。研究検索専用ではない。repo 必須でない browser task もある | 実績あり: fake CDP/launcher・browser policy・credential tests と browser ADR 群。実サイト操作の接続実績は個別記録参照。browser と各 source の組合せを独立に検証した試験はなし |

#### 未実測の組を確かめる手順

未実測とはコード上の設定経路が成立しそうでも、追跡済み試験・ADR・進捗・KBにその source×harness の接続成功記録が見つからないことを指す。以下はネットワーク・LLM呼出しをしない確認手順で、実接続の可否を証明するものではない。

1. `rg` で adapter の spawn 引数・env・設定ファイル生成箇所を確認し、モデル値と endpoint がどこから注入されるかを記録する。
2. `config/celeris.*.example.toml` の当該 adapter/source 例、`crates/task-worker/src/<adapter>` の mock process/HTTP 試験、`crates/task-dispatch` の adapter eligibility tests を確認する。明示 fixture が無ければ「未実測」のままにする。
3. OAuth CLI の組は、既存 `claude_account`/`codex_account` の stub login/auth tests と provider source resolve tests を追い、credential が意図せず別CLIへ流れないことをローカル fixture で確認する。実資格情報や login は使わない。
4. OpenAI-compatible/proxy の組は、ローカル fake HTTP server を使った request-shape test で path (`/v1/chat/completions` 等)・model・認証 header の変換を確認し、外部 URL を設定しない。proxy source mapping は `crates/llm-proxy/src/config.rs`, `sources_view.rs`, `selection.rs` の unit tests を確認する。
5. 新しい組を「実績あり」に変えるには、外部呼出しを伴わない契約試験を追加するか、別途認可された実環境検証の記録（source/model、CLI/API version、日時、成功応答）を ADR/進捗に残す。ここではどちらも実施しない。

### task から harness を決める材料（現状）

| 材料 | 現在の消費場所・コード | 現在 adapter が決まる作用 | 注意 |
|---|---|---|---|
| `genre` / `[[genres]]` | `config/celeris.example.toml`; `config/harness.rs` (`HarnessConfig`, `GenreConfig`); `task-core/src/model.rs` `harness_adapter` | genre が harness/role を引き、`[[roles]].adapter` または harness adapter を得る。`[[genres]].capabilities` は契約/説明で adapter router の強制条件ではない | genre を capability detector として自動解析する実装は確認できない |
| `requirements.browser` | task model/browser requirements、`task-dispatch/src/dispatcher/routing_context.rs`、browser policy/supervisor | browser context/policy/approval を構成し、worker adapter の browser 対応経路に渡す。存在だけで専用 harness を選ぶ selector ではない | browser action authorization は capability選択とは別の fail-closed gate |
| credential | task credential policy IDs と browser requirements; `task-core` browser policy、`task-worker/browser_credential.rs` | grant・承認・one-time consume を適用。credential の有無から別 LLM harness を選ばない | browser credential injection は browser supervisor の限定機能 |
| skills | org/task skill mounts と worker context (`task-worker/src/skills.rs`, `preamble.rs`) | adapter ごとに所定形式で配送（Claude `.claude/skills`, ACP/Codex 等）。skills 内容で adapter を選ぶ selector はない | assignee は skills/harness 組織運用で別途決定（ADR-0069） |
| repo の有無 | task repos/workspace、dispatcher workspace preparation; `task-core/src/execution_gate.rs` の実行分類 | coding harness の既定や execution routing の特徴量に使われる場合があるが、専用 research adapter を repo の有無だけで一般選択するものではない | aider 等 repo 必須という harness 制約は現行 registry で機械的に enforced されるとは限らない |
| `worker_hint.adapter` | `task-core/src/model.rs`、`task-dispatch/src/dispatcher/dispatch_run.rs`, `provider_select.rs` | explicit hint が候補 adapter を制約/優先。delegate 生成 (`task-core/src/delegate.rs`) は task/role/parent hint を継ぐ | これは既存の手動 override で、source/model routing と結合している |
| WU harness 欄 | execution plan / work unit の `genre` と worker hint (`task-dispatch/src/dispatcher/worker_task.rs`, `dispatch_run.rs`) | WU の genre/adapter hint を child task に反映し、最終 adapter は同じ dispatcher provider row selector が解決 | harness 欄が独立した登録済み harness IDか、adapter直指定かは経路ごとの schema を確認 |
| org profile の「受けられるハーネス」 | org profile/agent selection、ADR-0069 の担当選択規則 | 能力を持つ担当者・agent の eligibility/assignee 決定に使う。dispatcher の LLM adapter/source を直接決めるものではない | assignee/harness は異なる選択問題 |
| `adapter_policy = model_family` | `config/harness.rs`; `task-dispatch/src/dispatcher/coding_default.rs`, `dispatch_run.rs` | coding task の明示 adapter がない時、候補 provider の model family から claude-code/pi 等の既定を deterministic に決める | harness 選択が model/source に逆依存しており、目的の分離と逆方向の結合が残る |

現状の大枠は「task hint / genre の adapter 条件を作る → `dispatcher/provider_select.rs` が provider row を source/account の利用可能性と合わせて選ぶ → row の adapter を実行する」。つまり task による adapter 条件と source/model 選択は独立した最終 selector ではなく、provider 行で結合されている。dispatcher は選択のための LLM 呼び出しを行わず deterministic に候補を絞る。

### source・model を決める現行経路

| 経路 | 現行の決定内容・コード位置 |
|---|---|
| `provider_select` | `crates/task-dispatch/src/dispatcher/provider_select.rs`: task tier/adapter hint、provider 優先順、source/account availability・quota/concurrency を評価し provider row を決定。source は provider の明示値または `config/providers.rs` の derive/unknown |
| legacy tier | `crates/task-core/src/model_routing.rs`, `config/providers.rs`, `dispatch_run.rs`: `frontier`/`standard`/`cheap` を run lane/tier として扱い、provider の `tiers`/`tier_models` と adapter model を決める。明示 model と tier mapping が候補を制約 |
| lane | `task-core::Tier` を dispatch/profile/reviewer/WU policy で決定。`crates/celeris/src/config/dispatch.rs` と `task-dispatch/src/dispatcher/dispatch_run.rs`; provider row 選択より前に実行 lane が決まる |
| `tier_models` | `ProviderConfig` の tier→model binding。`cos.rs` の provider resolve も tier binding/effective model を見る。harness と同じ provider 行内の model 決定 |
| `model_assignments` / catalog | model catalog assignment API・`task-api/src/routing_catalog.rs` と `crates/celeris/src/config/model_routing.rs` の `assignments`。routing/catalog の source/model 対応を保持するが、Phase 1 catalog は legacy execution selector を置換しない（`model_routing.rs` 冒頭コメント） |
| routing enforce/legacy | `crates/llm-proxy/src/selection.rs`: legacy lane は eligible deployment order/filter と `legacy_rank` を使う。state/estimator 経路は明示設定時に source state・制約・cost・予約を使う。`crates/task-dispatch/src/dispatcher/routing_enforce.rs` 側の enforce は config で許された routing mode に限定 |
| shadow | `config/model_routing.rs`, `task-dispatch` routing shadow, `llm-proxy/src/shadow.rs`: 本選択を置換せず、候補/決定を shadow log へ記録。decision shadow と実行 shadow は設定で区別 |
| CoS | `config/cos.rs` の `[cos]` provider/harness/tier/source/model/account の整合検査と `resolve_cos_provider`; `task-dispatch/src/dispatcher/cos_chat` の session/account/provider selection。worker routing とは独立設定だが現状 provider ID と harness を照合して結合 |
| llm-proxy source/model 写像 | `crates/llm-proxy/src/config.rs`: `sources.claude_oauth`, `codex_oauth`, `opencode_go`, `openai_compatible[]` と `models.{claude,gpt,qwen}[tier]`。`selection.rs` は `celeris/<tier>`, `claude/<tier>`, `gpt/<tier>`, `qwen/cheap`, `<source>:<model>` を解決し、`sources_view.rs` が source status/models を公開。proxy は上流 API に応じ request/response を翻訳する |
| account/admission | `task-dispatch/src/accounts` と `task-core` account limits: account/source 別 availability・max_runs・quota/cooldown を provider row selectorへ返す。候補選択と resource admission が一部同じ provider 選択処理に重なる |

`[llm_proxy.models]` の tier は proxy の既定 model alias で、provider `tier_models` および task lane と同一概念ではない。`celeris/<tier>` は proxy 内で source selection を引き起こすため、harness からの source choice を proxy に委譲する経路でもある。現状には tier・model assignment・proxy tier alias・provider model mapping の複数の入口が併存する。

- 2026-10-11 harness-heading（01M4M9S863 / adr-select）: 新 ADR の「## harness」節に小見出し「### 宣言の形（## harness）」を 1 行足した。受け入れ check の見出し grep（`^#.*## harness`）が当たるようにするための修正で、本文は変えていない。


## 本番への移行

2026-10-12 に人が本番 `~/.config/celeris/config.toml` を読み取り、8 provider 行と proxy model alias を確認した。旧行は `claude-pool`（claude-code; frontier/standard/cheap）、`codex-pool`（codex; frontier/standard/cheap）、`opencode-qwen`（ACP; `qwen-local/qwen3.8-27b`）、`pi-qwen`（Pi; `qwen-local/qwen3.8-27b`）、`pi-opencode-go`（Pi; `opencode-go/grok-4.6`）、`ldr-qwen`（LDR; `celeris/cheap`）、`paperqa-qwen`（PaperQA; proxy `standard`）、`langmem-main`（LangMem; proxy cheap/standard）である。`providers.d` include は設定されていたが、該当する追加 TOML は存在しなかった。credential や endpoint の値は文書に転記しない。

| 選択規則 | harness | 先頭 source/model | fallback / 用途 |
|---|---|---|---|
| genre `coding` | coding execution class: `claude_code` → `codex` | Claude `claude_oauth / claude-opus-5-5` | admission 不可なら Codex `codex_oauth / gpt-6.1-sol` を試す |
| Pi role の固定 harness | `pi` | `opencode_go / opencode-go/grok-4.6` | Qwen `qwen / qwen3.8-27b` は後順位 |
| `local-qwen` role | `opencode` (ACP) | `qwen / qwen3.8-27b` | 軽い仕事を明示 role で選ぶ |
| literature genre | `paperqa` | `claude_oauth / claude-opus-5-5` | 旧 `celeris/standard` の候補; Codex `gpt-6.1-sol` が次 |
| research genre | `local-deep-research` | `qwen / qwen3.8-27b` | 旧 `celeris/cheap`; 次に Claude/Codex cheap |
| LangMem 専用起動 | `langmem` | `qwen / qwen3.8-27b` | 旧 `celeris/cheap`; 次に Claude/Codex cheap |
| CoS | `claude_code` | `claude_oauth / claude-opus-5-5` | `codex_oauth / gpt-6-astra` を fallback |
| planner / reviewer execution class | `claude_code`, `codex` | Claude Opus、次に Codex 6.1 | admission 不可なら次候補、全候補不可なら defer |

本番に反映する設定全文（既存の無関係な項目は保持し、placeholder は現在値に置換する）は次のとおり。秘密値は含めず、backup から endpoint/secret reference/path を転記する。

```toml
[llm_proxy]
enabled = true

[llm_proxy.sources.claude_oauth]
# accounts_dir は既存の [accounts] claude_dir を使う

[llm_proxy.sources.codex_oauth]
# accounts_dir は既存の [accounts] codex_dir を使う

[[llm_proxy.sources.openai_compatible]]
id = "qwen"
base_url = "<現在のQwen endpoint>/v1"

[llm_sources.claude_oauth]
kind = "claude_oauth"
api_format = "anthropic"
models = ["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5"]

[llm_sources.codex_oauth]
kind = "codex_oauth"
api_format = "openai_compatible"
models = ["gpt-6-astra", "gpt-6.1-sol", "gpt-6-luna"]

[llm_sources.qwen]
kind = "openai_compatible"
api_format = "openai_compatible"
endpoint = "<現在のQwen endpoint>/v1"
models = ["qwen3.8-27b"]

[llm_sources.opencode_go]
kind = "opencode_go"
api_format = "openai_compatible"
models = ["opencode-go/grok-4.6"]

[harnesses.claude_code]
kind = "claude_code"
api_formats = ["anthropic"]
source_kinds = ["claude_oauth", "codex_oauth"]
capabilities = ["tools", "repo", "skills"]

[harnesses.codex]
kind = "codex"
api_formats = ["openai_compatible"]
source_kinds = ["codex_oauth"]
capabilities = ["tools", "repo", "skills"]

[harnesses.pi]
kind = "pi"
api_formats = ["openai_compatible"]
source_kinds = ["opencode_go", "openai_compatible"]
capabilities = ["tools", "repo", "skills"]
# Pi-* provider 宣言から、稼働中 backup の Hashline extension path を転記する。
extensions = ["<backup の Hashline extension path>"]
# Hashline の実際の read/edit tool 名を backup と extension README で照合して転記する。
tools = ["<Hashline read tool>", "<Hashline edit tool>", "bash", "grep", "find", "ls"]
# env は旧 Pi provider の値を転記し、秘密値は env_from_secrets に secret id を指定する。
# env = { OPENAI_BASE_URL = "<backup の値>" }
# env_from_secrets = { CELERIS_PI_API_KEY = "<secret id>" }

[harnesses.opencode]
kind = "acp"
model_namespace = "qwen-local"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible"]
capabilities = ["tools", "repo", "skills"]

[harnesses.local-deep-research]
kind = "local-deep-research"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible", "claude_oauth", "codex_oauth"]
capabilities = ["web", "sources", "citations"]

[harnesses.paperqa]
kind = "paperqa"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible", "claude_oauth", "codex_oauth"]
capabilities = ["literature", "citations"]

[harnesses.langmem]
kind = "langmem"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible", "claude_oauth", "codex_oauth"]
capabilities = ["memory"]

[[llm_routes]]
harness = "local-deep-research"
source = "qwen"
model = "qwen3.8-27b"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"

[[llm_routes]]
harness = "paperqa"
source = "qwen"
model = "qwen3.8-27b"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"

[[llm_routes]]
harness = "langmem"
source = "qwen"
model = "qwen3.8-27b"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"

[[llm_routes]]
harness = "local-deep-research"
source = "claude_oauth"
model = "claude-sonnet-5-5"
via = "llm_proxy"
from_api = "anthropic"
to_api = "openai_compatible"

[[llm_routes]]
harness = "local-deep-research"
source = "codex_oauth"
model = "gpt-6-luna"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"

[[llm_routes]]
harness = "langmem"
source = "claude_oauth"
model = "claude-sonnet-5-5"
via = "llm_proxy"
from_api = "anthropic"
to_api = "openai_compatible"

[[llm_routes]]
harness = "langmem"
source = "codex_oauth"
model = "gpt-6-luna"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"

[[llm_routes]]
harness = "paperqa"
source = "claude_oauth"
model = "claude-opus-5-5"
via = "llm_proxy"
from_api = "anthropic"
to_api = "openai_compatible"

[[llm_routes]]
harness = "paperqa"
source = "codex_oauth"
model = "gpt-6.1-sol"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"

[[llm_routes]]
harness = "claude_code"
source = "codex_oauth"
model = "gpt-6-astra"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "anthropic"

[route_defaults.claude_code]
candidates = [{ source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "claude_oauth", model = "claude-opus-5-5" }, { source = "claude_oauth", model = "claude-fable-5-1" }]

[route_defaults.claude_code.classes.coding]
candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }]

[route_defaults.codex]
candidates = [{ source = "codex_oauth", model = "gpt-6.1-sol" }, { source = "codex_oauth", model = "gpt-6-astra" }, { source = "codex_oauth", model = "gpt-6-luna" }]

[route_defaults.pi]
candidates = [{ source = "opencode_go", model = "opencode-go/grok-4.6" }, { source = "qwen", model = "qwen3.8-27b" }]

[route_defaults.opencode]
candidates = [{ source = "qwen", model = "qwen3.8-27b" }]

[route_defaults.local-deep-research]
candidates = [{ source = "qwen", model = "qwen3.8-27b" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "codex_oauth", model = "gpt-6-luna" }]

[route_defaults.paperqa]
candidates = [{ source = "qwen", model = "qwen3.8-27b" }, { source = "claude_oauth", model = "claude-opus-5-5" }, { source = "codex_oauth", model = "gpt-6.1-sol" }]

[route_defaults.langmem]
candidates = [{ source = "qwen", model = "qwen3.8-27b" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "codex_oauth", model = "gpt-6-luna" }]

[harness_selection]
rules = [
  { role = "local-qwen", required_capabilities = [], harness = "opencode" },
  { role = "pi", required_capabilities = [], harness = "pi" },
  { genre = "literature", required_capabilities = [], harness = "paperqa" },
  { genre = "web-research", required_capabilities = [], harness = "local-deep-research" },
]
execution_class_defaults = { coding = ["claude_code", "codex"], planner = ["claude_code", "codex"], review = ["claude_code", "codex"] }

[execution.planner]
# adapter は削除する。budget/permission 等の既存値は保持する。
max_turns = 70
max_wall_secs = 900

[cos]
enabled = true
harness = "claude-code"
# tier / llm_source は削除し、具体的な候補順を route_candidates に置く。
route_candidates = [
  { source = "claude_oauth", model = "claude-opus-5-5" },
  { source = "codex_oauth", model = "gpt-6-astra" },
]
max_turns = 70
max_wall_secs = 900

[knowledge.langmem]
enabled = true
provider = "openai-compatible"
base_url = "<現在のLangMem proxy URL>"
model = "qwen/qwen3.8-27b"
api_key_secret = "<現在のsecret reference>"

[adapters.local_deep_research]
settings = { "llm.model" = "qwen3.8-27b", "llm.openai_endpoint.url" = "<現在のQwen endpoint>/v1", "llm.openai_endpoint.api_key" = "<現在のsecret reference>" }

[adapters.paperqa]
settings = "<現在のPaperQA settings path>"
```

本番では作業前に config と `providers.d` を権限付きで backup し、daemon 停止中に8行を source/harness/route 設定へ移して include と旧行を除く。DB path、events、credential files、account directories は保持する。新 binary の config validation が成功してから同一 release の binary を切替・起動し、API の source status と run audit の harness/source/model を coding、Pi、local-qwen、CoS の順に確認する。失敗時は daemon を停止し config/include と旧 binary を backup/release から戻す。DB migration 後は DB を旧版へ安易に戻さず、release の migration rollback 規則を適用する。詳細な人向けコマンドと確認事項は [移行手順](../../docs/ops/provider-llm-source-migration.md) にある。

## 検証（closeout-v2、2026-10-12）

検証は作業ツリー（親 `d47bc3b4`、本 commit の変更を含む）で実行した。同じ変更を含む commit が作業ブランチ `celeris/01M4RARW8GVJ7TA3XDDGP15FW3` に入る。

| 検査 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | exit 0。base の `crates/celeris/src/config/tests.rs` が未整形だったため `cargo fmt --all` で整形（この 1 file のみ） |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0。`error` 行 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest 4902 件 passed・0 failed・13 skipped（doc-test 成功）。`CELERIS_TEST_SUMMARY`: passed 4905 / failed 0 / ignored 14、`nextest_exit` 0、`doctest_exit` 0、`tmp_leftovers` 0 |
| sandbox 既知失敗（browser・launcher・credentiald・CDP 系） | 今回の実行では 1 件も失敗しなかった。名前の列挙対象は無し（`userns` は null） |
| `sh scripts/dev/check-doc-links.sh` | exit 0 |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0（182 files） |
| `sh scripts/dev/progress-index.sh --check` | exit 0 |

最初の test-parallel は exit 100（celerisctl `commands::scratch::tests` の 4 件が失敗）。原因は試験の config に廃止した `[scratch.l2]`・`[scratch.cache_server]` を書いていたこと。この節を読み込みエラーにしたため、試験 fixture から外して直し、上の表は直した後の実行。

## 旧い仕組みの残りの洗い出し（closeout-v2）

grep で洗った結果と、この run で消したもの。

消した（この run）:
- `[scratch.sccache]`・`[scratch.l2]`・`[scratch.cache_server]`: 「警告して無視する」経路（`ScratchConfig::deprecated_sections` と `Config::load` の warn）を消し、フィールドも消した。`deny_unknown_fields` により読み込みエラーになる。試験 `removed_scratch_cache_sections_are_load_errors` で固定した。
- `scripts/selfdeploy/verify.sh` の probe 対象 `/api/v1/providers`（既に route が無く 404 になる）を `/api/v1/llm/sources`・`/api/v1/harnesses` に置き換えた。
- stale な説明: `crates/task-api/src/admin.rs`（providers.d の説明）、`crates/task-api/src/cos/operations.rs`、`crates/task-api/tests/common/mod.rs`（providers.d 書き込み先）の doc comment を現状に合わせた。
- ADR 状態: `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` を「実装済み」にした。

既に消えていたこと（確認のみ）: `[[providers]]` を読む経路（読み込みエラーの案内だけ残る。`config/mod.rs` の `providers` / `providers.d` 検出は意図的な案内）、`/api/v1/providers` route、`provider_select`、`SourceOrigin`、`RoutingMode::Legacy`、`legacy_catalog`、`default_planner_adapter`、`llm_proxy.models` の tier alias（読み込みエラー）、`worker_hint.adapter`（migration 0068 の移行 SQL の中だけ）。

残っていて、この run では消さなかったもの（提案節へ）。

## 未解決事項

- `scratch.sccache`・cache server の欄が `task-ops/src/daemon.rs` の doc comment と `web/api/generated/schema.json`・`gui/app/celeris/types.ts` の説明文に残る（説明が廃止した設定を指す）。直すには schema 再生成（UPDATE_SCHEMA=1 と gui の gen:types）が要る。
- 名前の残り（挙動には影響なし）: `crates/celeris/src/daemon/admin.rs` の `reload_providers`、`crates/task-core/src/model_routing.rs` の `ProviderSelection` / `ProviderCandidate`（API 欄名 `provider` は crates 側に残す方針。InFlight.provider の中身は source id）。
- 本番 config の移行: 本番の `~/.config/celeris` の 8 行の移行は人が `docs/ops/provider-llm-source-migration.md` の手順で行う（この run では本番を触っていない）。

## 提案

1. 済み（closeout-fix5）: 非 cargo fixture の現行 config 形への移行。`scripts/test_portable_providers.py` の `[[providers]]` 試験 fixture も書き換えた。`scripts/portable-providers.py` は `[[harnesses]] adapter` を書く現行の欄を扱うので残した（close5/scripts.md）。
2. 済み（closeout-fix5）: 現行文書の更新は `knowledge.md`・`selfdeploy.md`・`opencode-go` §3a の範囲。`gui-api.md`・`gui/docs` は tree に無い。API 文書の削除済み endpoint 節（§3.19 など）は残っているが、「削除」と明記してある。
3. 済み（closeout-fix5）: `scripts/selfdeploy/migrate-to-celeris.sh` は tree に無い（撤去済み）。
4. `scratch.sccache`・cache server の説明文と schema を schema 再生成と一緒に直す（UPDATE_SCHEMA=1）。
5. `reload_providers`・`ProviderSelection` / `ProviderCandidate` は内部名の残り。API 欄名（provider は source ID）と区別したうえで整理する。
6. 本番移行の後で、旧 `providers.d` と `[[providers]]` を含む backup を人が確認してから削除する。

## 検証（closeout-v3、2026-10-12）

統合後 HEAD `36c2a8e3e209b8ac3d647053c9c7e3b7fffbf55e` のソース状態で実行。文書の小修正はコードを変更しない。

| 検査 | 結果 |
|---|---|
| `TMPDIR=/tmp/x bash scripts/dev/test-parallel.sh` | exit 0。nextest 4902 passed / 0 failed / 13 skipped、doc-test 3 passed / 0 failed / 1 ignored、`CELERIS_TEST_SUMMARY` passed 4905 / failed 0 / ignored 14。`tmp_leftovers=1` は script が後片付けし、`userns=null`。 |
| sandbox 既知失敗（browser・launcher・credentiald・CDP 系） | 今回は失敗 0 件。対象名の列挙なし。 |
| `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` | exit 0。 |
| doc links / ADR 番号 / progress index | すべて exit 0（ADR 182 files）。 |

test-parallel の当初指定候補 `/local/x` は read-only、`/var/tmp/x` は作成不可、`/dev/shm/x` は作成後に見えなくなり利用不可だったため、許可された短い一時 path `/tmp/x` を使った。

旧い仕組みの残りを再走査し、README の `[[providers]]` 指示、LLM source ガイドの proxy tier alias 運用説明、opencode-go の旧 provider POST 手順、architecture map の `provider_select.rs` 参照はこの closeout で現行記述へ直した。旧 ADR/API 仕様・GUI 設計履歴・移行 script/fixture は上の提案に列挙し、実行コードの利用経路とは分けた。実装上は `ProviderConfig`・`provider_select`・`/api/v1/providers` route・`worker_hint.adapter` 読取・`default_planner_adapter`・proxy tier alias の選択経路は残っていない。移行エラー案内、廃止を示す説明、migration SQL、旧 fixture と履歴文書中の文字列は意図的な残存。
