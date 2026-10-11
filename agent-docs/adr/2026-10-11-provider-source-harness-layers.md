---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
---
# ADR 2026-10-11: provider を LLM source と harness の関係にする

- 日付: 2026-10-11
- 状態: 決定（実装待ち）
- 関連: [provider 層の再設計・棚卸し](../progress/2026-10-11-provider-layer-redesign.md)、[ADR-0132](0132-provider-llm-source-split-and-cheap-qwen.md)、[アカウント同時実行数](2026-10-08-account-concurrency-limits.md)、[破壊的変更を既定とする ADR](2026-10-11-breaking-changes-by-default.md)

## 背景

現行の `[[providers]]` は、LLM の供給元、実行アダプタ、モデル対応、account pool、同時実行数を一つの行に持つ。source を adapter や model 名から推定する経路があり、推定できない provider は `unknown` として残る。provider は実行手段そのものではなく、実在する LLM source と harness の対応および利用可能 model の宣言にする。

この決定は ADR-0132 と 2026-10-08 の account concurrency ADR のうち、derived source、旧 tiers/tier_models、provider concurrency、暗黙的な source/account key に依存する箇所を置き換える。過去の実装記録は履歴として残す。旧欄を読んで警告する互換分岐や切替 flag は設けず、削除した欄が設定に残れば読み込み時にエラーとする。これは「破壊的変更を既定とする」ADR に従う。

## 3 層の宣言

設定を `[[llm_sources]]`、`[[harnesses]]`、`[[providers]]` の3層に分ける。ID は各層で一意とし、参照は文字列 ID で明示する。LLM source は接続と account の正本、harness は adapter の実行契約、provider は source と harness の組み合わせおよび model 対応だけを表す。

### LLM source

`[[llm_sources]]` は `id`、`kind`、認証/account pool、同時実行数、quota、利用可能 model を持つ。`kind` は `claude_oauth`、`codex_oauth`、`opencode_go`、`openai_compatible:<id>`、`celeris` のいずれか。`celeris` は Celeris proxy を通る source であり、その上流選択は proxy の責務とする。認証情報の値は config に直接書かない。

```toml
[[llm_sources]]
id = "claude-main"
kind = "claude_oauth"
account_pool = "claude-code"
max_concurrency = 3
quota = "subscription"
models = ["claude-opus", "claude-sonnet"]

[[llm_sources]]
id = "qwen"
kind = "openai_compatible:qwen"
account_pool = "qwen-account"
max_concurrency = 1
quota = "local-gpu"
models = ["qwen3.8-27b"]
```

account pool の所属・実アカウント・暗黙 account の上限は source に結び付ける。quota は source の利用制約として記録し、harness/provider に重複させない。

### harness

`[[harnesses]]` は `id`、adapter 種別、話せる source 種類、能力、`command`、`args`、`settings` を持つ。能力は `tool`、`browser`、`credential` などを宣言する。source 種類の制約は接続可能性を示し、個別 source ID や利用可能 model を推定するものではない。

```toml
[[harnesses]]
id = "claude-code"
adapter = "claude-code"
source_kinds = ["claude_oauth"]
capabilities = ["tool", "browser", "credential"]
command = "claude"
args = ["--print"]

[harnesses.settings]
permission_mode = "acceptEdits"
```

### provider

`[[providers]]` は source と harness の明示参照、およびその組で使える model 対応だけを持つ。上限・認証・account pool は持たない。`llm_source` は必須の source ID 参照であり、adapter/model から導出しない。

```toml
[[providers]]
id = "claude-opus"
llm_source = "claude-main"
harness = "claude-code"
models = ["claude-opus"]

[[providers]]
id = "qwen-opencode"
llm_source = "qwen"
harness = "opencode"
models = ["qwen3.8-27b"]
```

provider の候補は、この明示された関係と model 対応から作る。`derived` source の状態は廃止する。参照のない provider を暗黙 source に束ねることも認めない。

## 削除する欄

以下は新しい3層へ移し、旧欄を受け入れる型・API field・画面を削除する。config に旧欄が残れば parse/validation error とし、新しい欄への移行を促す。読み込み互換、警告だけでの継続、切替 flag は置かない。

| 旧欄・旧概念 | 削除・移動先 |
|---|---|
| `[[providers]].concurrency` | 削除。上限は LLM source の account ごとの同時実行数へ移す |
| `[[providers]].kind`（`adapter` 判定） | 削除。実行 adapter 種別は `[[harnesses]].adapter` へ移す |
| provider の `derived` / `SourceOrigin::Derived` | 削除。全 provider が明示的な `llm_source` を参照する |
| `tiers` / `tier_models` | 旧 tier 欄を削除。provider の model 対応を `models` として明示し、tier routing は後続の routing 設計で定義する |
| `account_pool` on provider | provider 欄から削除し、LLM source/account の所有関係へ移す |
| provider API の `concurrency`、`in_use_cos` 等 provider 単位の上限・稼働数 field | provider API から削除。上限と利用状況は source/account の境界で扱う |
| provider に埋め込まれた command/args/settings | `[[harnesses]]` へ移す |

## unknown source の解消

`unknown` は有効な source 値や fallback ではなくす。起動時の config 検証で、各 provider の `llm_source` が `[[llm_sources]]` に登録された ID と一致すること、参照先の種類を harness が受け付けること、provider の model が source と harness の対応可能な範囲にあることを検査する。一つでも満たさなければ設定読み込みを拒否し、provider ID と不正な参照先を示す。

source が未登録、ID の誤記、互換性のない source kind のいずれも `unknown` に変換して実行を続けない。全ての provider は実在 source を参照し、source の追加・削除も同じ registry の検証を通す。

## API と画面

<!-- adr-surface WorkUnit: API・Web/旧GUI の決定を追記 -->

## routing の境界

<!-- adr-surface WorkUnit: routing・各 subsystem の境界を追記 -->

## 本番 config の移行

<!-- adr-surface WorkUnit: provider ID・stats/cooldown・DB/events/credential の保全方針を追記 -->
