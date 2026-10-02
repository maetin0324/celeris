---
tasks: [01M3YF3NSR46FM314VHG14T3N6]
---
# provider と LLM source の分離・Qwen cheap 専用化の移行手順

対象は 2026-10-02 の本番 `~/.config/celeris/config.toml`。実行者は人。以下は適用例であり、
このタスクでは本番の設定・DB・daemon を変更しない。[ADR-0132](../adr/0132-provider-llm-source-split-and-cheap-qwen.md) D2・D7 に従う。

## 何を変えるか

`[[providers]]` は adapter / harness の実行枠で、LLM source の登録場所ではない。
`kind = "adapter"` と `llm_source` を明示する。source と tier の対応は `[llm_proxy.sources.*]` と
`[llm_proxy.models]` に置く。`celeris/<tier>` は proxy が実行時に source を選ぶ抽象モデルであり、
`llm_source = "celeris"` は Qwen 固定を意味しない。

| 2026-10-02 の provider ID | 移行後の ID | adapter とモデル |
|---|---|---|
| `opencode-qwen` | `opencode` | `acp`、cheap の `celeris-proxy/celeris/cheap` |
| `ldr-qwen` | `ldr` | `local-deep-research`、`celeris/cheap` |
| `paperqa-qwen` | `paperqa` | `paperqa`、`openai/celeris/standard` |
| `langmem-main` | `langmem-main` | `langmem`、`celeris/cheap` |
| `claude-pool` / `codex-pool` | 同じ | 各 CLI の OAuth source |

旧 ID は旧設定として読み込めるが、改名によって provider の stats・cooldown・
`providers.d/<id>.toml` の鍵が変わる。履歴や cooldown は新 ID に自動移管されない。
`providers_include` を使う場合は該当ファイル名とファイル内の `id` を対応させ、
同じ ID の行を本体と include の両方に残さない。

## 移行後の設定例

既存の DB、workspace、アカウントディレクトリ、API listen などは維持する。
以下は該当節の抜粋で、`<…>` は人が秘密ストアまたはファイルから設定する値を示す。
秘密の値を本書・ログ・コミットに書かない。

```toml
[llm_proxy]
enabled = true
prefer_free = true

[llm_proxy.sources.claude_oauth]
# accounts_dir は既存の [accounts] claude_dir を使う

[llm_proxy.sources.codex_oauth]
# accounts_dir は既存の [accounts] codex_dir を使う

[[llm_proxy.sources.openai_compatible]]
id = "qwen"
base_url = "http://127.0.0.1:18000/v1" # 既存の到達可能な中継先

[llm_proxy.models.qwen]
cheap = "qwen3.8-27b" # frontier / standard は書かない

[[providers]]
id = "claude-pool"
kind = "adapter"
adapter = "claude-code"
llm_source = "claude_oauth"
account_pool = true
tiers = ["frontier", "standard", "cheap"]
concurrency = 4
# tier_models 等の既存設定を保持

[[providers]]
id = "codex-pool"
kind = "adapter"
adapter = "codex"
llm_source = "codex_oauth"
account_pool = true
tiers = ["frontier", "standard", "cheap"]
concurrency = 4
# tier_models 等の既存設定を保持

[[providers]]
id = "opencode"
kind = "adapter"
adapter = "acp"
llm_source = "celeris"
tiers = ["cheap"]
concurrency = 1
model = "celeris-proxy/celeris/cheap"
env = { OPENCODE_CONFIG = "<proxy を使う opencode 設定 JSON の絶対パス>", OPENCODE_DISABLE_PROJECT_CONFIG = "1", LITELLM_MODEL = "celeris/cheap" }
env_from_secrets = { CELERIS_API_TOKEN = "<API token の secret id>" }

[[providers]]
id = "ldr"
kind = "adapter"
adapter = "local-deep-research"
llm_source = "celeris"
tiers = ["cheap"]
concurrency = 1
model = "celeris/cheap"

[[providers]]
id = "paperqa"
kind = "adapter"
adapter = "paperqa"
llm_source = "celeris"
tiers = ["standard"]
concurrency = 1
model = "openai/celeris/standard"

[[providers]]
id = "langmem-main"
kind = "adapter"
adapter = "langmem"
llm_source = "celeris"
tiers = ["cheap"]
concurrency = 1

[knowledge.langmem]
enabled = true
provider = "openai-compatible"
base_url = "http://127.0.0.1:18100/v1"
model = "celeris/cheap"
api_key_secret = "<API token の secret id>"
```

PaperQA の設定 JSON は [`config/paperqa.proxy.example.json`](../../config/paperqa.proxy.example.json) を基に
`llm`・`summary_llm`・`agent.agent_llm` を `openai/celeris/standard`、`api_base` を proxy、
`api_key` を API token にする。`[adapters.paperqa].settings` にはコピー先の `.json` を除いたパスを置く。
LDR は `[adapters.local_deep_research.settings]` の `llm.model = "celeris/cheap"`、
`llm.openai_endpoint.url` を proxy、`api_key` を同じ token にする。
opencode の JSON は [`config/opencode.openai-compat.example.json`](../../config/opencode.openai-compat.example.json) を基に
`celeris-proxy/celeris/cheap` を選び、`CELERIS_API_TOKEN` を秘密ストアから渡す。
現行の設定検査は opencode の `celeris-proxy/` 接頭辞を見ないため、行の
`LITELLM_MODEL = "celeris/cheap"` は `llm_source = "celeris"` の検査にも使う。
Qwen 直指定の旧 opencode 経路を一時的に残すなら、その行の `tiers` は `cheap` だけにする。
frontier / standard でも opencode を使う場合は、Qwen 固定の `OPENCODE_CONFIG` と別の行を作り、
proxy の `celeris/frontier` / `celeris/standard` を使う。

## 人が実行する適用と確認

1. 停止前に `config.toml`、`providers.d/`、PaperQA と opencode の JSON の控えを権限を保って取る。
   例: `cp -a ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-provider-split-20261002`。
   `providers.d` を使うなら `cp -a ~/.config/celeris/providers.d ~/.config/celeris/providers.d.bak-provider-split-20261002`。
   バックアップには秘密が含まれうるので共有しない。
2. `config.toml` と各道具の JSON を上の形へ編集する。旧 `models.qwen` の `frontier`・`standard` を削除する。
   `providers.d` の ID とファイル名を確認し、旧 ID を使う自動化があれば同時に更新する。
3. 稼働中リリースの `celerisctl config to-harnesses --config ~/.config/celeris/config.toml > /tmp/celeris-harnesses-check.toml`
   を実行する。このコマンドの `Config::load` は TOML を読み、参照・provider・proxy の設定を検証する。
   exit 0 と、警告に意図しない `-qwen` ID や不明な `llm_source` が無いことを確認する。
   一時出力も確認後に削除する。
4. 人が対象の user service を確認して `systemctl --user restart 'celeris@<現在のリリース>.service'`
   を実行する。`POST /api/v1/reload` は provider ファイルを読み直せても、proxy の source や
   listen の変更まで反映する手段としては使わない。再起動後は `systemctl --user status` と
   `journalctl --user -u 'celeris@<現在のリリース>.service' -n 100 --no-pager` で設定エラーが無いことを確認する。
5. API token をシェル変数 `TOKEN` に安全に読み込み、`GET http://127.0.0.1:7710/api/v1/providers` で
   新 ID、`kind = adapter`、`llm_source.source` と `origin = explicit` を確認する。
   `GET http://127.0.0.1:7710/api/v1/llm/sources` で Qwen の cheap 対応と他 source の状態を確認する。
   いずれも `Authorization: Bearer $TOKEN` を付ける。実際の API listen が違えば URL を替える。
6. `GET http://127.0.0.1:18100/v1/models` に同じ Bearer を付け、`qwen/cheap` があり
   `qwen/frontier`・`qwen/standard` が無いことを確認する。
   `POST /v1/chat/completions` に `model = "celeris/cheap"` を送り、応答の
   `x-celeris-source` を見る。Qwen が利用可能なら `openai-compatible:qwen`、
   到達不能なら `claude-oauth` または `codex-oauth` を期待する。
   `celeris/standard` では `x-celeris-source` が Qwen にならないことも確認する。
   確認リクエストには機密データを入れない。token と応答本文をログへ残さない。

## 戻し方

検証・起動・応答のいずれかが失敗したら、人が控えた `config.toml`、`providers.d/`、
PaperQA と opencode の JSON を元の場所に戻し、同じ `celerisctl config to-harnesses --config …` で検査し、
同じ user service を再起動する。旧設定の ID と `kind` / `llm_source` の省略は互換で読める。
ただし ADR-0132 D3 により旧 `models.qwen.frontier` / `standard` は読み込まれても無視され、
Qwen を非 cheap tier に戻すことはない。戻した後も providers API、`/v1/models`、
`x-celeris-source` を再確認する。
