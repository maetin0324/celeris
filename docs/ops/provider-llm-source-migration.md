---
tasks: [01M4R918GDSZBS2MC2XQABBJTM]
---
# 本番 provider 設定を source・harness 方式へ移行する

この手順は人が本番 host で実行する。Celeris の新 binary が `[[providers]]` と
`providers.d` を拒否するため、設定の変換と binary の切替を同じ停止時間内に行う。
この文書を作った run は本番 config・DB・credential・daemon を変更していない。

## 選択結果

`harness_selection` が task の role/genre/実行 class から harness を決め、各
`route_defaults.<harness>.candidates` が先頭から source/model を試す。候補は
compatibility、quota、cooldown、capacity を通った最初のものになる。

| 現在の用途 | 選択規則 | harness | 先頭候補 |
|---|---|---|---|
| coding worker | coding role の通常選択 | `claude_code` | `claude_oauth / claude-opus-5-5` |
| coding worker の予備 | Claude 候補が admission 不可のとき | `codex` | `codex_oauth / gpt-6.1-sol` |
| Pi coding | role `pi` または既存の Pi 指定 | `pi` | `opencode_go / opencode-go/grok-4.6` |
| 軽い仕事 | role `local-qwen` | `opencode` (ACP) | `qwen / qwen3.8-27b` |
| 文献調査 | literature harness | `paperqa` | `claude_oauth / claude-opus-5-5`、次に `codex_oauth / gpt-6.1-sol` |
| 深掘り Web 調査 | research harness | `local-deep-research` | `qwen / qwen3.8-27b`、次に Claude/Codex cheap |
| 知識整理 | LangMem の専用起動設定 | `langmem` | `qwen / qwen3.8-27b`、次に Claude/Codex cheap |
| CoS | `[cos].route_candidates` の順 | `claude_code` | `claude_oauth / claude-opus-5-5` |
| planner | execution class の順。満杯なら次へ | `claude_code`, `codex` | 上記 coding 候補の順 |
| reviewer | execution class の順 | `claude_code`, `codex` | 上記 coding 候補の順 |

`qwen` は本番 proxy の source ID として宣言し、tier alias を使わない。旧
`celeris/cheap` は利用可能なら Qwen を優先し、fallback に Claude/Codex cheap を使う。
旧 `celeris/standard` は Claude/Codex standard の account pool を使う。これを具体的な
source/model 候補として宣言する。CoS は本番の standard（Claude Opus）を先頭にし、既存 fallback の Codex
standard を後ろに保持する。

## 切替前の退避と確認

以下は本番 host で人が実行する。作業用コピーは同じ権限の private directory に置く。
設定と `providers.d` には secret reference や endpoint があるため、backup を共有・表示しない。

```sh
set -eu
cfg="$HOME/.config/celeris/config.toml"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
backup="$HOME/.config/celeris/config.toml.pre-source-migration.$stamp"
cp -a -- "$cfg" "$backup"
if [ -d "$HOME/.config/celeris/providers.d" ]; then
  cp -a -- "$HOME/.config/celeris/providers.d" "$HOME/.config/celeris/providers.d.pre-source-migration.$stamp"
fi
chmod 600 "$backup"
sha256sum "$cfg" "$backup"
```

`providers.d` の各行を下の source/harness 定義へ移し、include を外す。旧設定の secret
reference、account pool directory、proxy URL、PaperQA/LDR の settings path、LangMem の
secret reference は現行値を保つ。`~/.config/celeris/config.toml` の DB path、workspace、
API、cluster、browser、account 設定も変更しない。DB/events と credential directory は
移動・削除・再生成しない。

## 移行後の TOML

以下のブロックは移行に関係する設定の完成形である。既存の無関係な設定は同じファイルに
残す。コメントで示した endpoint/path/secret reference は backup から値を移し、秘密値を
この文書や shell history に書かない。Claude/Codex の account pool は既存の account 設定を
使う。

<!-- migration-runbook-toml:start -->
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

[harnesses.opencode]
kind = "acp"
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
candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }, { source = "claude_oauth", model = "claude-fable-5-1" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }]

[route_defaults.codex]
candidates = [{ source = "codex_oauth", model = "gpt-6.1-sol" }, { source = "codex_oauth", model = "gpt-6-astra" }, { source = "codex_oauth", model = "gpt-6-luna" }]

[route_defaults.pi]
candidates = [{ source = "opencode_go", model = "opencode-go/grok-4.6" }, { source = "qwen", model = "qwen3.8-27b" }]

[route_defaults.opencode]
candidates = [{ source = "qwen", model = "qwen3.8-27b" }]

[route_defaults.local-deep-research]
candidates = [{ source = "qwen", model = "qwen3.8-27b" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "codex_oauth", model = "gpt-6-luna" }]

[route_defaults.paperqa]
candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }, { source = "codex_oauth", model = "gpt-6.1-sol" }]

[route_defaults.langmem]
candidates = [{ source = "qwen", model = "qwen3.8-27b" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "codex_oauth", model = "gpt-6-luna" }]

[harness_selection]
rules = [
  { role = "local-qwen", required_capabilities = [], harness = "opencode" },
  { role = "pi", required_capabilities = [], harness = "pi" },
  { genre = "coding", required_capabilities = [], requires_repo = true, harness = "claude_code" },
  { genre = "literature", required_capabilities = [], harness = "paperqa" },
  { genre = "web-research", required_capabilities = [], harness = "local-deep-research" },
]
execution_class_defaults = { planner = ["claude_code", "codex"], review = ["claude_code", "codex"] }

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
model = "qwen3.8-27b"
api_key_secret = "<現在のsecret reference>"

[adapters.local_deep_research]
settings = { "llm.model" = "qwen3.8-27b", "llm.openai_endpoint.url" = "<現在のQwen endpoint>/v1", "llm.openai_endpoint.api_key" = "<現在のsecret reference>" }

[adapters.paperqa]
settings = "<現在のPaperQA settings path>"
```
<!-- migration-runbook-toml:end -->

`[adapters.paperqa].settings` が指す PaperQA settings では `llm`、`summary_llm`、
`agent.agent_llm` を `qwen3.8-27b`、endpoint は現在の proxy URL、認証は既存 secret とする。
opencode ACP の既存 `OPENCODE_CONFIG` と `CELERIS_API_TOKEN` secret injection は保持し、
model を `qwen3.8-27b` にする。LangMem・LDR・PaperQA の本番 endpoint は backup の値を
転記する。`providers.d` の eight IDs は新形式の宣言を確認した後に退避名へ移し、旧 include
を外す。TOML の説明用 placeholder は実 config に残さない。

## 読み込み・切替・確認

1. daemon を停止し、設定全体と `providers.d` を編集する。新 binary は旧 provider config を
   読めないため、設定保存前に binary を差し替えない。
2. 新 binary と同じ release の `celerisctl` で、実際の起動引数にある config path を
   指定して `celerisctl routing show --config "$cfg"` を実行する。この command は config と
   routing registry を読み、DB を開かない。source/harness/candidate の表が出ることを確認する。
3. 新 binary に差し替え、daemon を起動する。GUI/API で harness route と source status を確認し、
   低リスクな coding task、Pi task、`local-qwen` task、CoS の順に実行して run audit の
   harness/source/model を上表と照合する。planner/reviewer は候補順を確認する。
4. 設定 parse error、source が unavailable、期待した model/harness と異なる場合は追加 task を
   投入せず rollback する。

## rollback

daemon を停止し、同じ backup の `config.toml` と `providers.d` を元の path へ `cp -a` で戻す。
旧 binary を元の release から戻して daemon を起動し、health と既存 run/event を確認する。
DB、events、credential、account directory は変更していないため復元対象にしない。新 binary
起動後に DB migration が行われた場合でも、DB を古い binary へ安易に戻してはならない。
release の DB migration rollback 手順に従い、DB の復元が必要なら daemon 停止中に取得した
整合 backup のみを使用する。旧 binary が schema を読めないと分かった時点で再起動を止め、
新 binary と移行後 config を戻して起動する。

<!-- target-side operational notes retained below -->

選んだ理由は `routing_decided` の `record.resolution.selection` に残る。

| `reason` | 意味 |
| --- | --- |
| `local_preferred` | ローカルの行を選んだ |
| `local_full` | ローカルの行が満杯で、順位付けに倒した |
| `local_down` | ローカルの行が不通または cooldown 中で、順位付けに倒した |
| `pool` | 合うローカルの行が無く（または cheap 以外で）、プールの行を選んだ |
| `fallback` | プールを持たない行に倒した（従来の fallback） |
| `sticky` | 継続セッションの行に留まった |

適用後の確認（人が実行する。読み取りだけ）:

```sh
# トンネルの health（選択が見るのと同じ先）
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18000/v1/models
# 直近の cheap の選択理由
sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3 \
  "select json_extract(json,'$.record.resolution.provider'), json_extract(json,'$.record.resolution.selection.reason'), count(*)
   from events where json_extract(json,'$.type')='routing_decided'
     and json_extract(json,'$.record.resolution.lane')='cheap'
     and json_extract(json,'$.record.resolution.selection.reason') is not null
   group by 1,2;"
```

トンネルが落ちているあいだは `local_down` でプールに倒れ続ける。pegasus の接続（TOTP）を GUI のクラスタ画面で戻すと、
次の probe（最長 60 秒後）からローカルに戻る。

## 人が実行する適用と確認

1. 停止前に `config.toml`、`providers.d/`、PaperQA と opencode の JSON の控えを権限を保って取る。
   例: `cp -a ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-provider-split-20261002`。
   `providers.d` を使うなら `cp -a ~/.config/celeris/providers.d ~/.config/celeris/providers.d.bak-provider-split-20261002`。
   バックアップには秘密が含まれうるので共有しない。既存の同名バックアップがある場合は別の名前を選び、
以下の戻し方でもその名前を使う。上限の控えの取得が失敗したら先へ進まない。
2. `config.toml` と各道具の JSON を上の形へ編集する。旧 `models.qwen` の `frontier`・`standard` を削除する。
   `providers.d` の ID とファイル名を確認し、旧 ID を使う自動化があれば同時に更新する。
3. 稼働中リリースの `celerisctl config to-harnesses --config ~/.config/celeris/config.toml`
   を `$TMPDIR` 配下の一時ファイルへ出力して実行する（`/tmp` にはコピーしない）。このコマンドの `Config::load` は TOML を読み、参照・provider・proxy の設定を検証する。
   exit 0 と、警告に意図しない `-qwen` ID や不明な `llm_source` が無いことを確認し、確認後に一時ファイルを削除する。
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

## プロバイダの concurrency を外し、同時実行数をアカウントで管理する

実行者は人。このタスクでは本番の設定・DB・daemon・systemd を変更しない。
設計判断は [ADR 2026-10-08 account-concurrency-limits](../../agent-docs/adr/2026-10-08-account-concurrency-limits.md)、
利用者向けの説明は [providers ガイド](../guides/providers.md#同時実行数はアカウントで決める)、API は
[api.md](../api/v1/api.md) の §3.29・§3.29a にある。

### 何が変わるか

| 項目 | 変更前 | 変更後（このリリースから） |
| --- | --- | --- |
| `[[providers]].concurrency` | 行の同時実行数の上限 | deprecated。読めるが dispatch では使わない。読み込むたびに行ごとに警告が出る |
| プール行（`account_pool = true`） | 行の `concurrency` と各アカウントの `max_runs_per_account` の両方で止まる | 各アカウントの実効上限（`effective_max_runs`）だけで止まる |
| 非プール行（本番の `opencode-qwen`・`pi-qwen`・`ldr-qwen`・`paperqa-qwen`・`langmem-main`） | 行の `concurrency` | その行の LLM source の**暗黙アカウント** `implicit/<llm source id>` の実効上限（同じ source を使う行は 1 件を共有する）。保存値が無ければ `max_runs_per_account`（既定 2）。旧 `concurrency` は使わない |
| `[accounts] max_runs_per_account` | 全アカウント共通の上限 | 保存値の無いアカウントの既定値（変更には再起動が要る。従来どおり） |
| アカウントごとの上限 | 無い | アカウント画面か `PATCH /api/v1/llm/sources/{source_id}/accounts/{account_id}` で即時に設定（DB に保存） |

**リリースを入れ替えた時点で、非プール行の上限は旧 `concurrency` から既定 2 に変わる**（旧 `concurrency` は上限に効かない）。
Qwen 行（GPU 1 枚）を 1 本に保つには、新リリースの daemon が動いた直後に手順 3 で `max_runs = 1` を保存する。
PATCH は新リリースにしか無いので、保存までの数分は Qwen 行が 2 本まで走りうる。これは人が移行の時刻に合わせて確かめる。
プール行は行の上限が無くなるので、同時数が「選べるアカウントの実効上限の和」まで増えうる
（例: アカウント 3 件・既定 2 なら最大 6 本。旧 `claude-pool` の `concurrency = 4` では 4 本だった）。
全体の上限 `[execution] max_concurrency` と `max_cos_runs` は変わらない。

注意: 手順 4 で config から `concurrency` を消しても、上限は変わらない（旧値は既に効いていない）。Qwen 行の上限は手順 3 の保存値で決まる。
**手順 4 の前に、暗黙アカウントの上限（Qwen は 1）を保存しておく**（手順 3）。消し忘れても害は無いが、警告ログを消すために行う。

### 前提

- 新しいリリース（`account_limits` の migration と `PATCH /api/v1/llm/sources/{source_id}/accounts/{account_id}` を含むもの）が稼働していること。
  リリースの入れ替えは [selfdeploy.md](selfdeploy.md) に従う。
- 以下のコマンドは Bash で実行し、パイプ途中の失敗も検出する。API token をシェル変数 `TOKEN` に安全に読み込んで実行する（値を画面・ログに残さない）。
  API の listen が `127.0.0.1:7710` と違えば URL を替える。

```sh
set -o pipefail
API=http://127.0.0.1:7710/api/v1
```

### 人が実行する手順

#### 1. 控えを取る

```sh
cp -a ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-account-limits-20261008
# providers.d を使っていれば
cp -a ~/.config/celeris/providers.d ~/.config/celeris/providers.d.bak-account-limits-20261008
# 変更前の保存値・実効上限だけを控える（既存の控えを上書きしない）
(umask 077; set -C; curl -fsS -H "Authorization: Bearer $TOKEN" "$API/llm/accounts" \
  | jq -e '{max_runs_per_account, items: [.items[] | {source_id, account_id, max_runs, effective_max_runs, max_runs_source}]}' \
  > ~/.config/celeris/account-limits-before-20261008.json)
```

バックアップには秘密が含まれうるので共有しない。既存の同名バックアップがある場合は別の名前を選び、
以下の戻し方でもその名前を使う。上限の控えの取得が失敗したら先へ進まない。

#### 2. 今の concurrency と実効上限を確かめる（読み取りだけ）

```sh
# config と providers.d に残っている concurrency
grep -n '^[[:space:]]*concurrency[[:space:]]*=' ~/.config/celeris/config.toml ~/.config/celeris/providers.d/*.toml 2>/dev/null
# 各アカウント（暗黙アカウントを含む）の実効上限と、その出どころ
curl -fsS -H "Authorization: Bearer $TOKEN" "$API/llm/accounts" \
  | jq -r '.max_runs_per_account as $d | "default=\($d)",
           (.items[] | [.adapter, .id, (.provider // "-"), (.max_runs|tostring), .effective_max_runs, .max_runs_source, .in_use] | @tsv)'
```

期待:

- 非プール行の LLM source ごとに `implicit	<llm source id>	<llm source id>	null	2	default` の行がある（既定 2。`[accounts]` が無ければ 2）。
  `providers` の欄にその source を使う行の id が出る（本番の Qwen 行は `opencode-qwen`・`pi-qwen` 等が同じ `openai_compatible:qwen` の 1 件にまとまる）。
  本番の Qwen 行は、新リリースに入れ替えた直後なら `effective_max_runs = 2`・`max_runs_source = default` になる
  （config の `concurrency = 1` は効かない）。手順 3 で保存すると `1`・`account` に変わる。
  既に `max_runs` が保存されている行は `max_runs_source = account` になり、保存値が優先される。
  確認のために保存値を解除せず、控えと現在の `effective_max_runs` を照合する。
- 保存値の無い実アカウント（`claude-code`・`codex`・`opencode-go`）は `max_runs_source = default`、`effective_max_runs` が `max_runs_per_account`。
- daemon のログに、`concurrency` を書いた行の数だけ警告がある（手順 6 で消えたことを確かめる比較元）:

```sh
journalctl --user -u 'celeris@<現在のリリース>.service' --since today --no-pager | grep 'concurrency is deprecated'
```

#### 3. 必要ならアカウント画面で上限を保存する

GUI の「アカウント」画面で各アカウント（「暗黙アカウント」の印が付いた行を含む）の上限を編集して保存する。API で行う場合:

```sh
# 暗黙アカウント: GPU 1 枚の Qwen は 1 本に保つ（config から concurrency を消しても 1 のまま）。
# id は行の id ではなく LLM source。同じ source を使う行（opencode-qwen・pi-qwen・ldr-qwen・paperqa-qwen）は
# この 1 件でまとめて止まるので、PATCH は source ごとに 1 回でよい。
curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"max_runs": 1}' "$API/llm/sources/openai_compatible:qwen/accounts/openai_compatible:qwen" | jq '{account_id, providers, max_runs, effective_max_runs, max_runs_source}'
# langmem-main のように source が celeris の行（同じ source を使う他の非プール行と 1 件を共有する）
curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"max_runs": 1}' "$API/llm/sources/celeris/accounts/celeris" | jq '{account_id, providers, max_runs, effective_max_runs, max_runs_source}'
# 実アカウント: プールの同時数を旧 concurrency に近づけたいときだけ（例: opencode-go のアカウントを 1 本に）
curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"max_runs": 1}' "$API/llm/sources/opencode_go/accounts/<アカウント id>" | jq '{account_id, max_runs, effective_max_runs, max_runs_source}'
```

- 自分の構成の source id は `GET /llm/accounts` の暗黙アカウントの `account_id` と `providers` で確認する（行の id では引けない）。
- 応答の `max_runs_source` が `account` になっていればよい。値は 1〜32。反映は次の tick からで、再起動も reload も要らない。
- 旧 `concurrency` と同じ上限を残したい非プール行も、その source の暗黙アカウントに `max_runs` を保存する。
  保存しない source は手順 4 の後で `max_runs_per_account`（本番は 2）に変わる。旧値と異なる場合は、その変更を意図していることを確認する。
- プール行の目安: 旧 `claude-pool` の `concurrency = 4` を保ちたいなら、Claude アカウントの `effective_max_runs` の和を 4 にする。
  下げても走っている run は止まらない（`in_use` が下がるまで新しい run が待つ）。

#### 4. config と providers.d から concurrency を外す

`~/.config/celeris/config.toml` の各 `[[providers]]` と `~/.config/celeris/providers.d/*.toml` から `concurrency = …` の行を消す。
他の欄は触らない。`[accounts] max_runs_per_account` は残す（既定値として使う）。

検査（稼働中リリースの `celerisctl`。`Config::load` で読み、exit 0 なら読める。変換結果は保存せず破棄する）:

```sh
check_file="${TMPDIR:?}/celeris-harnesses-check.toml"
celerisctl config to-harnesses --config ~/.config/celeris/config.toml > "$check_file"
printf 'exit=%s\n' "$?"
rm -f "$check_file"
grep -n '^[[:space:]]*concurrency[[:space:]]*=' ~/.config/celeris/config.toml ~/.config/celeris/providers.d/*.toml 2>/dev/null; echo "grep exit=$? (1 なら残り無し)"
```

検査が exit 0 になるまで反映しない。`providers_include` が別の場所を指していれば、
そのディレクトリも控え・編集・検査の対象にする。

`[[clusters]].concurrency`（クラスタのジョブ枠、ADR-0018）は別物なので消さない。上の grep に出たら、それが `[[clusters]]` の中かを確かめる。

#### 5. 反映する

`[[providers]]` と `providers.d` の変更だけなので `POST /api/v1/reload` で足りる:

```sh
curl -sS -X POST -H "Authorization: Bearer $TOKEN" -w '\nHTTP %{http_code}\n' "$API/reload"
```

HTTP 200 と `{"reloaded": true}` が返ればよい。400 なら本文のエラーを読み、config を直してから再度実行する
（同時に `[accounts]` を変えていると reload は 400 で拒否される。その場合は再起動する）。
reload で反映できない事情があれば、人が対象の user service を確かめて
`systemctl --user restart 'celeris@<現在のリリース>.service'` を実行する。保存した `max_runs` は DB にあるので、どちらでも残る。

#### 6. 確かめる（読み取りだけ）

```sh
# 実効上限: 暗黙アカウントは保存値（account）か既定（default）。旧 concurrency は使わない
curl -fsS -H "Authorization: Bearer $TOKEN" "$API/llm/accounts" \
  | jq -r '.items[] | [.adapter, .id, (.max_runs|tostring), .effective_max_runs, .max_runs_source, .in_use, (.excluded_reason // "-")] | @tsv'
# 警告ログが消えたこと（reload / 再起動の時刻より後に 1 行も出ない）
journalctl --user -u 'celeris@<現在のリリース>.service' --since '<手順 5 の時刻>' --no-pager | grep -c 'concurrency is deprecated'
# プロバイダの行の観測値（in_use / in_use_cos。上限とは比べない）
curl -fsS -H "Authorization: Bearer $TOKEN" "$API/providers" | jq -r '.items[] | [.id, .in_use, .in_use_cos] | @tsv'
```

期待:

- Qwen の暗黙アカウント（`implicit/openai_compatible:qwen`。`providers` に `opencode-qwen`・`pi-qwen` 等が出る）が `effective_max_runs = 1`（`max_runs_source = account`）。
- 保存値の無いアカウントは `max_runs_source = default`、`effective_max_runs` が `max_runs_per_account`。
- `grep -c` が `0`（一致が無いため grep の exit は 1）。journalctl 自体が成功していることも確かめる。
- 数分後、`in_use` が各アカウントの `effective_max_runs` を超えていない（CoS run を含めると実アカウントは +1 まで）。
  満杯のアカウントは `excluded_reason = at_capacity` になる。

### 戻し方

- **上限の値だけを戻す**: 手順 1 の `account-limits-before-20261008.json` の `max_runs` を確認し、
  この移行で変更したアカウントだけ、アカウント画面か PATCH で元の値を保存する。
  元が `null` だった場合は「既定に戻す」か、API で `{"max_runs": null}` を送る。

  ```sh
  curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -d '{"max_runs": null}' "$API/llm/sources/openai_compatible:qwen/accounts/openai_compatible:qwen" | jq '{account_id, providers, max_runs, effective_max_runs, max_runs_source}'
  ```

  即時に効く。解除後は GET の実効上限を再確認する。暗黙アカウントは保存値が無ければ `default`（`max_runs_per_account`）に戻る。旧 `concurrency` は上限に効かない。
- **config を戻す**: 手順 1 の控えを元の場所に戻し（`cp -a ~/.config/celeris/config.toml.bak-account-limits-20261008 ~/.config/celeris/config.toml`、
  `providers.d` は `cp -a ~/.config/celeris/providers.d.bak-account-limits-20261008/. ~/.config/celeris/providers.d/` で中身を戻す）、手順 4 の検査と手順 5 の reload を行う。警告ログが再び出るが、保存値の無い暗黙アカウントは旧 `concurrency` を**引き継がない**（`max_runs_per_account` になる）。
  config を外す前の挙動に戻す場合は、先に config を復元・reload してから保存値を控えの値へ戻す
  （先に保存値を解除すると、一時的に既定の上限へ変わるため）。
  プール行の `concurrency` は戻しても効かない（このリリースの仕様）。プールの同時数を絞るには、手順 3 でアカウントの `max_runs` を下げる。
- **リリースごと戻す**: [selfdeploy.md](selfdeploy.md) の手順に従う。DB の `account_limits` 表は残るが、旧リリースはこれを読まない。
  旧リリースでは config の `concurrency` が再び行の上限になるので、手順 4 で消していたら手順 1 の控えを先に戻しておく。
