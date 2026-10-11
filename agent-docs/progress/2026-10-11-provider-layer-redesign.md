---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
title: provider 層の再設計: 現状棚卸し
updated: 2026-10-11
status: running
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

`paperqa-qwen` が `unknown` となるコード根拠: [providers.rs](../../crates/celeris/src/config/providers.rs) の `derive_llm_source` (302行付近) は `fake`, `claude-code`, `codex`, ACP/Pi opencode-go、Celeris model、特定ACP Qwen条件だけを認識し、それ以外は `LlmSourceRef::Unknown` を返す。`paperqa` adapter はこの分岐にない。明示 `llm_source` が無い行では `provider_llm_source` (177–185行付近) が derived値へフォールバックし、validation/warning (368–371行付近) が `unknown_llm_source` を出す。PaperQA設定ファイル内のmodel/endpointをこの判定関数がsourceとして解析しないため、行IDの `-qwen` suffixだけではsourceを決定できない。従って資料上のproxy利用から本番sourceを断定せずunknownとした。

## 旧欄・互換の具体的な残存

- `ProviderConfig.concurrency` と `ProviderSpec.concurrency` が残り、設定読込時はdeprecated warningを出す。`task-api/handlers/providers.rs` はGET応答にもfieldを返し、POST/PATCHで値を受理して警告する。`in_use_cos` はprovider snapshot上のCoS run数。
- `provider_llm_source` は明示sourceと `derive_llm_source` の両方を許し、`SourceOrigin::{Explicit,Derived}` を返す。`Unknown` は有効な型値で、sourceが決まらない行を表現する。
- provider選択とimplicit account keyが解決済みsourceに結び付いている。paperqa等のsource不明行は `unknown` に束ねられ、source別上限の意味を壊す。
- `docs/guides/providers.md` はdeprecated concurrencyを「読める・warn」、implicit accountをsource単位と説明。これは旧互換を明文化している。

## 調査範囲・作業状態

参照した実装: `crates/celeris/src/config/{providers,harness,model_routing,dispatch,cos,model_catalog}.rs`、`crates/task-core/src/provider_source.rs` と account limit/store、`crates/task-dispatch/src/dispatcher/provider_select.rs` と accounts、`crates/task-api/src/handlers/{providers,accounts}.rs`。UIは指定されたWeb/GUI routeを確認。資料は `docs/guides/providers.md`、`docs/ops/provider-llm-source-migration.md`、ADR-0053/0069/0132、2026-10-08 concurrency ADR、config例を参照。

このWorkUnitでは現状棚卸しのみを記録し、コード・API・画面・既存設計文書は変更していない。本番設定も読まず、変更していない。次工程のADRで責務境界と旧欄の削除方針を確定し、実装工程が設定・API・画面・移行文書を更新する。
