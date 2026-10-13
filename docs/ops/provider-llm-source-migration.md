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
| coding worker | genre `coding` → coding execution class | `claude_code`, `codex` | `claude_oauth / claude-opus-5-5` を先頭、admission 不可なら Codex `gpt-6.1-sol` |
| Pi coding | role `pi` または既存の Pi 指定 | `pi` | `opencode_go / opencode-go/grok-4.6` |
| 軽い仕事 | role `local-qwen` | `opencode` (ACP) | `qwen / qwen3.8-27b` |
| 文献調査 | literature harness | `paperqa` | `claude_oauth / claude-opus-5-5`、次に `codex_oauth / gpt-6.1-sol` |
| 深掘り Web 調査 | research harness | `local-deep-research` | `qwen / qwen3.8-27b`、次に Claude/Codex cheap |
| 知識整理 | LangMem の専用起動設定 | `langmem` | `qwen / qwen3.8-27b`、次に Claude/Codex cheap |
| CoS | `[cos].route_candidates` の順 | `claude_code` | `claude_oauth / claude-opus-5-5` |
| planner | execution class の順。満杯なら次へ | `claude_code`, `codex` | 上記 coding 候補の順 |
| reviewer | execution class の順 | `claude_code`, `codex` | 上記 coding 候補の順 |

`qwen` は本番 proxy の source ID として宣言する。tier alias（`[llm_proxy.models]`）は廃止され、
新しい binary では読み込みエラーになる（[ADR 2026-10-11 llm-source-harness-selection](../../agent-docs/adr/2026-10-11-llm-source-harness-selection.md)）。
旧 alias が暗黙に行っていた選び方（cheap は利用可能なら Qwen、次に Claude/Codex の cheap な model。
standard は Claude/Codex の standard な model）は、上表のとおり `route_defaults` の具体的な source/model
候補として宣言する。Pi は OpenCode Go を先頭にし、Qwen を予備にする。CoS は Claude（Opus）を先頭にし、
Codex を fallback として後ろに置く。

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
<!-- migration-runbook-toml:end -->

`[adapters.paperqa].settings` が指す PaperQA settings では `llm`、`summary_llm`、
`agent.agent_llm` を `qwen3.8-27b`、endpoint は現在の proxy URL、認証は既存 secret とする。
opencode ACP の既存 `OPENCODE_CONFIG` と `CELERIS_API_TOKEN` secret injection は保持し、
model を `qwen3.8-27b` にする。LangMem・LDR・PaperQA の本番 endpoint は backup の値を
転記する。`providers.d` の eight IDs は新形式の宣言を確認した後に退避名へ移し、旧 include
を外す。TOML の説明用 placeholder は実 config に残さない。

`[knowledge.langmem].base_url` を Celeris の llm-proxy（`[llm_proxy]` の listen）に向ける場合、
proxy は具体的な `<source_id>/<model_id>` だけを受けるため、`model` は `qwen/qwen3.8-27b` の形で書く。
Qwen の endpoint を直接指す場合は `qwen3.8-27b` のままでよい（上の TOML 例は proxy を指すので `qwen/qwen3.8-27b`）。

## 前提

- 新しいリリース（`[[providers]]`・`providers.d`・`providers_include`・`[llm_proxy.models]` を読み込み
  エラーにするもの）を `scripts/selfdeploy/release.sh` で作ってあり、まだ昇格していないこと。
  リリースの作成・検証・昇格・戻し方は [selfdeploy.md](selfdeploy.md) に従う。`verify.sh` は本番の
  config path（`$CELERIS_CONFIG` か `~/.config/celeris/config.toml`）をそのまま新リリースに読ませるので、
  旧 config のままでは読み込みエラーで落ちる。検証には移行後 config を `CELERIS_CONFIG` で渡す。
- 新 binary は旧 config（`[[providers]]` など）を拒否し、旧 binary は新 config（`[llm_sources]`・
  `[harnesses.<id>]`・`[route_defaults]` など）を読めない。**config とリリースは常に対で切り替え、対で戻す。**
- 以下のコマンドは Bash で実行する。API token はシェル変数 `TOKEN` に安全に読み込み、値を画面・ログに
  残さない。API の listen が `127.0.0.1:7710`、proxy が `127.0.0.1:18100` と違えば URL を替える。

```sh
set -o pipefail
API=http://127.0.0.1:7710/api/v1
PROXY=http://127.0.0.1:18100
```

## 読み込み・切替・確認

1. 「切替前の退避と確認」の backup を取る。加えて、各アカウントの上限の保存値を控える
   （DB にあり config の巻き戻しでは戻らないため。既存の控えを上書きしない）:

   ```sh
   (umask 077; set -C; curl -fsS -H "Authorization: Bearer $TOKEN" "$API/llm/accounts" \
     | jq -e '{max_runs_per_account, items: [.items[] | {source_id, account_id, max_runs, effective_max_runs, max_runs_source}]}' \
     > ~/.config/celeris/account-limits-before-source-migration.json)
   ```

2. 作業用コピーの config を「移行後の TOML」の形に編集する。`[[providers]]`・`providers.d`・
   `providers_include`・`[llm_proxy.models]`・`[execution.planner] adapter`・`[cos]` の `tier` /
   `llm_source` を消す。`providers.d` は新形式の宣言を確認した後に退避名へ移す。
3. 新リリースの `celerisctl` で作業用コピーを検査する。config だけを読み、DB を開かない:

   ```sh
   new_ctl=~/.local/celeris/releases/<新リリース>/bin/celerisctl
   "$new_ctl" routing show --config "<作業用コピーの config.toml>"
   ```

   exit 0 で、source・harness・route candidate と `[cos] route_candidates` の表が「選択結果」の表と
   一致することを確かめる（CoS は `claude_oauth / claude-opus-5-5`、次に `codex_oauth / gpt-6-astra`。
   CoS の候補は API に出ないので、ここで確かめる）。
   `legacy provider configuration is no longer supported` などの読み込みエラーが出たら、その欄を
   直してからやり直す。続けて `CELERIS_CONFIG="<作業用コピーの config.toml>" scripts/selfdeploy/verify.sh <新リリース>`
   で新リリースを検証する。旧 binary は新 config を読めないので N-1 検査は落ち、`live_ok` は偽になる。
4. 作業用コピーを本番の config path に置き、すぐに `scripts/selfdeploy/promote.sh <新リリース>` で昇格する
   （`live_ok` が偽なので [selfdeploy.md](selfdeploy.md) §4a の停止 → 起動になる）。置いてから昇格するまで
   `POST /api/v1/reload` を送らない（旧 binary は新 config を読めない）。起動後に
   `systemctl --user status 'celeris@<新リリース>.service'` と
   `journalctl --user -u 'celeris@<新リリース>.service' -n 100 --no-pager` で設定エラーが無いことを確認する。
5. GPU 1 枚の Qwen を 1 本に保つため、Qwen の暗黙アカウントに上限を保存する
   （[ADR 2026-10-08 account-concurrency-limits](../../agent-docs/adr/2026-10-08-account-concurrency-limits.md)。
   API は [api.md](../api/v1/api.md) §3.29a）。保存値は DB にあり、再起動・reload の後も残る:

   ```sh
   curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"max_runs": 1}' "$API/llm/sources/qwen/accounts/qwen" \
     | jq '{source_id, account_id, max_runs, effective_max_runs, max_runs_source}'
   ```

   応答の `max_runs_source` が `account` ならよい。暗黙アカウントはスナップショットから作るので、
   起動直後の最初の tick 前は 404 になりうる。そのときは数秒おいて再実行する。
6. 読み取りで確かめる:

   ```sh
   # health
   curl -fsS "$API/health" | jq .
   # LLM source ごとの到達性・残量・cooldown（qwen・claude_oauth・codex_oauth・opencode_go が並ぶ）
   curl -fsS -H "Authorization: Bearer $TOKEN" "$API/llm/sources" | jq .
   # harness ごとの選択規則と route candidates（「選択結果」の表と同じ順か）
   curl -fsS -H "Authorization: Bearer $TOKEN" "$API/harnesses" \
     | jq -r '.[] | [.id, (.route_defaults | map("\(.source_id)/\(.model_id)") | join(" > "))] | @tsv'
   # アカウントと暗黙アカウントの上限（qwen が effective_max_runs = 1・max_runs_source = account）
   curl -fsS -H "Authorization: Bearer $TOKEN" "$API/llm/accounts" \
     | jq -r '.items[] | [.source_id, .account_id, .implicit, .effective_max_runs, .max_runs_source, .in_use, (.excluded_reason // "-")] | @tsv'
   ```

   期待: `pi` の候補が `opencode_go/opencode-go/grok-4.6 > qwen/qwen3.8-27b`、`claude_code` の先頭が
   `claude_oauth/claude-opus-5-5`、`local-deep-research` と `langmem` の先頭が `qwen/qwen3.8-27b`。
7. proxy を経由する harness（LDR・PaperQA・LangMem）のために、proxy が具体的な source/model を
   中継することを確かめる。確認リクエストには機密データを入れず、token と応答本文をログへ残さない:

   ```sh
   curl -sS -o /dev/null -D - -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"model":"qwen/qwen3.8-27b","messages":[{"role":"user","content":"hi"}]}' \
     "$PROXY/v1/chat/completions" | grep -i '^\(HTTP\|x-celeris-source\)'
   ```

   200 と `x-celeris-source: openai-compatible:qwen` を期待する。Claude を試すときは model を
   `claude_oauth/claude-sonnet-5-5`（`x-celeris-source: claude-oauth`）にする。
8. 低リスクな coding task、Pi task、`local-qwen` task、CoS の順に 1 件ずつ実行し、run ごとの
   harness・source・model を「選択結果」の表と照合する。`routing_decided` の event に残る
   （DB path は config の `db`。読み取り専用で開く）:

   ```sh
   sqlite3 -readonly "<本番 DB path>" \
     "select ts, json_extract(json,'$.record.harness'), json_extract(json,'$.record.resolution.adapter'),
             json_extract(json,'$.record.resolution.provider'), json_extract(json,'$.record.resolution.model_id')
        from events where json_extract(json,'$.type')='routing_decided'
        order by id desc limit 20;"
   ```

   `resolution.provider` は選ばれた source ID、`resolution.model_id` はその model。task 1 件ずつなら
   `GET /api/v1/tasks/<task id>/routing` の `runs` でも同じ内容を見られる。Pi task は
   OpenCode Go が使えるあいだ `opencode_go`、使えないときだけ `qwen` になる。planner/reviewer は
   execution class の候補順（`claude_code` → `codex`）で選ばれていることを確認する。
9. 設定の読み込みエラー、source が unavailable のまま戻らない、期待した harness・source・model と
   異なる、のいずれかなら追加の task を投入せずに次節で戻す。

## rollback

release を前の版に戻し、config を旧版の backup に戻す。新 binary は旧 config を拒否し、旧 binary は
新 config を読めないので、片方だけを戻すとどちらの binary も起動時に config を拒否する。必ず対で戻す。

1. config を旧版の backup に戻す（「切替前の退避と確認」で取ったもの。秘密を含むので表示しない）。
   移行後 config は調査用に別名で残す:

   ```sh
   cfg="$HOME/.config/celeris/config.toml"
   cp -a -- "$cfg" "$cfg.post-source-migration.$(date -u +%Y%m%dT%H%M%SZ)"
   cp -a -- "$HOME/.config/celeris/config.toml.pre-source-migration.<stamp>" "$cfg"
   # providers.d を使っていた場合（退避名へ移したものを元の名前に戻す）
   cp -a -- "$HOME/.config/celeris/providers.d.pre-source-migration.<stamp>" "$HOME/.config/celeris/providers.d"
   ```

2. 前の版の `celerisctl` で戻した config を検査する（config だけを読み、DB を開かない。出力は捨てる）:

   ```sh
   old_ctl=~/.local/celeris/previous/bin/celerisctl
   "$old_ctl" config to-harnesses --config "$cfg" > "${TMPDIR:?}/celeris-rollback-check.toml"; echo "exit=$?"
   rm -f "${TMPDIR:?}/celeris-rollback-check.toml"
   ```

   exit 0 になるまで次へ進まない。
3. リリースを前の版に戻す（`scripts/selfdeploy/rollback.sh`。[selfdeploy.md](selfdeploy.md) §5）。
   新リリースの DB migration で `schema_version` が上がり旧 binary が起動できない場合、`rollback.sh` は
   拒否する。そのときは `--restore-db` で昇格前の DB backup へ戻すか（昇格後の仕事は失われる）、
   新リリースと移行後 config の対に戻して原因を直すかを人が決める。DB・events・credential・account
   directory はこの移行で変更しないので、それ以外の復元対象にしない。
4. `systemctl --user status`・`journalctl` で設定エラーが無いこと、`GET /api/v1/health` と既存の
   run・event を確認する。
5. 切替の手順 5 で保存した上限は DB の `account_limits` に残る（rollback では消えない）。旧リリースの
   アカウント画面で各アカウントの上限を切替の手順 1 の控えと照合し、違っていれば控えの値に戻す。
