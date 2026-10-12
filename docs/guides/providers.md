---
tasks: [01M4R918GDSZBS2MC2XQABBJTM]
---
# LLM source と harness の設定

Celeris は source・model の候補と、task を実行する harness を別々に設定します。旧
`[[providers]]` / `providers.d` は受け付けません。移行手順は
[本番 provider 設定の移行](../ops/provider-llm-source-migration.md)を参照してください。

## LLM source と model

`[llm_sources.<id>]` は接続先、API 形式、利用可能な model と account pool を宣言します。
`[route_defaults.<harness>].candidates` は source/model の候補を優先順で並べます。
候補は harness の API 形式と source kind の互換性、quota、cooldown、capacity を満たす
ものから選ばれます。tier 名や model family から候補を推測することはありません。

## Harness 選択

`[harnesses.<id>]` は実行方式と能力を宣言します。`[harness_selection].rules` は role、
genre、repo の有無、execution class などから harness を決めます。planner と reviewer
などは `execution_class_defaults` の候補順を使います。候補がすべて admission 不可なら、
coding task は coding execution class の順に harness を admission し、Claude Code が使えない場合は Codex を試します。planner/reviewer も各 class の候補順を使い、全候補が使えない場合は defer します。

```toml
[llm_sources.claude_oauth]
kind = "claude_oauth"
api_format = "anthropic"
models = ["claude-opus-5-5"]

[harnesses.claude_code]
kind = "claude_code"
api_formats = ["anthropic"]
source_kinds = ["claude_oauth"]
capabilities = ["tools", "repo", "skills"]

[route_defaults.claude_code]
candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }]

[harness_selection]
execution_class_defaults = { coding = ["claude_code", "codex"], planner = ["claude_code", "codex"], review = ["claude_code", "codex"] }
```

本番の coding 候補は Claude Code / `claude-opus-5-5` を先にし、Codex /
`gpt-6.1-sol` を次にします。Pi は `opencode_go` の `opencode-go/grok-4.6` を先にし、
Qwen (`qwen3.8-27b`) を後順位にします。軽い仕事は `local-qwen` role から ACP/OpenCode
harness を選び、Qwen source を使います。CoS は `[cos].route_candidates` に
`claude_oauth / claude-opus-5-5`、続けて Codex standard を指定します。

## 特化 harness

LDR、PaperQA、LangMem は tier alias（`celeris/cheap`、`celeris/standard`）を設定に残さず、
source と具体的な model 候補を宣言します。cheap は利用可能なら Qwen `qwen3.8-27b` を
先にし、Claude/Codex cheap を続けます。standard は Claude Opus を先にし Codex 6.1 を
続けます。LDR/PaperQA は source/model route を持ち、LangMem は
`[knowledge.langmem]` の endpoint/model/secret reference と LangMem harness を設定します。
秘密の値は config へ直書きせず、credential store の reference を使ってください。

## 本番移行

新 binary は旧 `[[providers]]` と `providers.d` を読み込みエラーにします。停止前 backup、
新形 TOML、読み込み確認、切替確認、rollback は
[運用移行手順](../ops/provider-llm-source-migration.md)に従ってください。
移行時も DB/events、credential、account directory を削除・作り直さないでください。
