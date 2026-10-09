---
tasks: [01M3YF3NSR46FM314VHG14T3N6, 01M4D7RVKXY9A967ZXDADWVHQJ]
---
# provider と LLM source の分離・Qwen cheap 専用化の移行手順

対象は 2026-10-02 の本番 `~/.config/celeris/config.toml`。実行者は人。以下は適用例であり、
このタスクでは本番の設定・DB・daemon を変更しない。[ADR-0132](../../agent-docs/adr/0132-provider-llm-source-split-and-cheap-qwen.md) D2・D7 に従う。

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
`[[providers]].concurrency` は deprecated なので書かない。同時実行数はアカウント（アカウントを持たない行はその行の暗黙アカウント）で決める。
既に書いてある行から外す手順は [下の節](#プロバイダの-concurrency-を外し同時実行数をアカウントで管理する)。
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
# tier_models 等の既存設定を保持

[[providers]]
id = "codex-pool"
kind = "adapter"
adapter = "codex"
llm_source = "codex_oauth"
account_pool = true
tiers = ["frontier", "standard", "cheap"]
# tier_models 等の既存設定を保持

[[providers]]
id = "opencode"
kind = "adapter"
adapter = "acp"
llm_source = "celeris"
tiers = ["cheap"]
model = "celeris-proxy/celeris/cheap"
env = { OPENCODE_CONFIG = "<proxy を使う opencode 設定 JSON の絶対パス>", OPENCODE_DISABLE_PROJECT_CONFIG = "1", LITELLM_MODEL = "celeris/cheap" }
env_from_secrets = { CELERIS_API_TOKEN = "<API token の secret id>" }

[[providers]]
id = "ldr"
kind = "adapter"
adapter = "local-deep-research"
llm_source = "celeris"
tiers = ["cheap"]
model = "celeris/cheap"

[[providers]]
id = "paperqa"
kind = "adapter"
adapter = "paperqa"
llm_source = "celeris"
tiers = ["standard"]
model = "openai/celeris/standard"

[[providers]]
id = "langmem-main"
kind = "adapter"
adapter = "langmem"
llm_source = "celeris"
tiers = ["cheap"]

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
`OPENCODE_CONFIG` が無い Qwen 直指定や `llm_source = "openai_compatible:qwen"` の ACP 行も cheap に限定される。管理 API で `tiers` を省略すると `[cheap]` が保存され、非 cheap の明示指定は拒否される。
frontier / standard でも opencode を使う場合は、Qwen 固定の `OPENCODE_CONFIG` と別の行を作り、
proxy の `celeris/frontier` / `celeris/standard` を使う。

## cheap lane のローカル優先（ADR-0132 付記 2026-10-04）

cheap lane の worker run は、アカウントプール（`claude-pool`・`codex-pool`）の順位付けより先にローカルの行を選ぶ。

| 項目 | 内容 |
| --- | --- |
| ローカルの行 | `account_pool` が無く、`tiers` に cheap を含み、実効 `llm_source` が `openai_compatible:<id>`（本番の `opencode-qwen`）か、`celeris` で proxy に有効な `openai_compatible` source と `[llm_proxy.models.qwen].cheap` がある行。`paperqa`・`local-deep-research`・`langmem` の行は含めない |
| 同時実行数 | その行の暗黙アカウント `implicit/<行の id>` の実効上限（ADR 2026-10-08 account-concurrency-limits）。保存値が無ければ旧 `[[providers]].concurrency` を引き継ぐ。GPU 1 枚の Qwen は 1 のままにする（アカウント画面で設定。[下の節](#プロバイダの-concurrency-を外し同時実行数をアカウントで管理する)） |
| health | `GET <source の base_url>/models`（本番は `http://127.0.0.1:18000/v1/models`）。時間切れ 3 秒、結果は 60 秒キャッシュ。LLM は呼ばない |
| プールに倒す条件 | ローカルの行が満杯・不通・cooldown 中・その仕事の adapter 指定に合わない |
| 変わらないもの | standard / frontier、reviewer run、CoS の対話 run、継続セッション |
| 切り替え | `[execution] cheap_local_first = false`（既定 `true`）で従来の選び方に戻る。`POST /api/v1/reload` で反映される |

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
[gui-api.md](../api/v1/gui-api.md) の §3.29・§3.29a にある。

### 何が変わるか

| 項目 | 変更前 | 変更後（このリリースから） |
| --- | --- | --- |
| `[[providers]].concurrency` | 行の同時実行数の上限 | deprecated。読めるが dispatch では使わない。読み込むたびに行ごとに警告が出る |
| プール行（`account_pool = true`） | 行の `concurrency` と各アカウントの `max_runs_per_account` の両方で止まる | 各アカウントの実効上限（`effective_max_runs`）だけで止まる |
| 非プール行（本番の `opencode-qwen`・`pi-qwen`・`ldr-qwen`・`paperqa-qwen`・`langmem-main`） | 行の `concurrency` | 行の**暗黙アカウント** `implicit/<行の id>` の実効上限。保存値が無ければ `max_runs_per_account`（既定 2）。旧 `concurrency` は使わない |
| `[accounts] max_runs_per_account` | 全アカウント共通の上限 | 保存値の無いアカウントの既定値（変更には再起動が要る。従来どおり） |
| アカウントごとの上限 | 無い | アカウント画面か `PATCH /api/v1/accounts/{adapter}/{id}` で即時に設定（DB に保存） |

**リリースを入れ替えた時点で、非プール行の上限は旧 `concurrency` から既定 2 に変わる**（旧 `concurrency` は上限に効かない）。
Qwen 行（GPU 1 枚）を 1 本に保つには、新リリースの daemon が動いた直後に手順 3 で `max_runs = 1` を保存する。
PATCH は新リリースにしか無いので、保存までの数分は Qwen 行が 2 本まで走りうる。これは人が移行の時刻に合わせて確かめる。
プール行は行の上限が無くなるので、同時数が「選べるアカウントの実効上限の和」まで増えうる
（例: アカウント 3 件・既定 2 なら最大 6 本。旧 `claude-pool` の `concurrency = 4` では 4 本だった）。
全体の上限 `[execution] max_concurrency` と `max_cos_runs` は変わらない。

注意: 手順 4 で config から `concurrency` を消しても、上限は変わらない（旧値は既に効いていない）。Qwen 行の上限は手順 3 の保存値で決まる。
**手順 4 の前に、暗黙アカウントの上限（Qwen は 1）を保存しておく**（手順 3）。消し忘れても害は無いが、警告ログを消すために行う。

### 前提

- 新しいリリース（`account_limits` の migration と `PATCH /api/v1/accounts/{adapter}/{id}` を含むもの）が稼働していること。
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
(umask 077; set -C; curl -fsS -H "Authorization: Bearer $TOKEN" "$API/accounts" \
  | jq -e '{max_runs_per_account, items: [.items[] | {adapter, id, max_runs, effective_max_runs, max_runs_source}]}' \
  > ~/.config/celeris/account-limits-before-20261008.json)
```

バックアップには秘密が含まれうるので共有しない。既存の同名バックアップがある場合は別の名前を選び、
以下の戻し方でもその名前を使う。上限の控えの取得が失敗したら先へ進まない。

#### 2. 今の concurrency と実効上限を確かめる（読み取りだけ）

```sh
# config と providers.d に残っている concurrency
grep -n '^[[:space:]]*concurrency[[:space:]]*=' ~/.config/celeris/config.toml ~/.config/celeris/providers.d/*.toml 2>/dev/null
# 各アカウント（暗黙アカウントを含む）の実効上限と、その出どころ
curl -fsS -H "Authorization: Bearer $TOKEN" "$API/accounts" \
  | jq -r '.max_runs_per_account as $d | "default=\($d)",
           (.items[] | [.adapter, .id, (.provider // "-"), (.max_runs|tostring), .effective_max_runs, .max_runs_source, .in_use] | @tsv)'
```

期待:

- 非プール行ごとに `implicit	<行の id>	<行の id>	null	2	default` の行がある（既定 2。`[accounts]` が無ければ 2）。
  本番の Qwen 行（`opencode-qwen`・`pi-qwen`）は、新リリースに入れ替えた直後なら `effective_max_runs = 2`・`max_runs_source = default` になる
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
# 暗黙アカウント: GPU 1 枚の Qwen 行は 1 本に保つ（config から concurrency を消しても 1 のまま）
curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"max_runs": 1}' "$API/accounts/implicit/opencode-qwen" | jq '{id, max_runs, effective_max_runs, max_runs_source}'
curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"max_runs": 1}' "$API/accounts/implicit/pi-qwen" | jq '{id, max_runs, effective_max_runs, max_runs_source}'
# 実アカウント: プールの同時数を旧 concurrency に近づけたいときだけ（例: opencode-go のアカウントを 1 本に）
curl -fsS -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"max_runs": 1}' "$API/accounts/opencode-go/<アカウント id>" | jq '{id, max_runs, effective_max_runs, max_runs_source}'
```

- 応答の `max_runs_source` が `account` になっていればよい。値は 1〜32。反映は次の tick からで、再起動も reload も要らない。
- 旧 `concurrency` と同じ上限を残したい非プール行（`ldr-qwen`・`paperqa-qwen`・`langmem-main` 等）も、同じく `max_runs` を保存する。
  保存しない行は手順 4 の後で `max_runs_per_account`（本番は 2）に変わる。旧値と異なる場合は、その変更を意図していることを確認する。
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
curl -fsS -H "Authorization: Bearer $TOKEN" "$API/accounts" \
  | jq -r '.items[] | [.adapter, .id, (.max_runs|tostring), .effective_max_runs, .max_runs_source, .in_use, (.excluded_reason // "-")] | @tsv'
# 警告ログが消えたこと（reload / 再起動の時刻より後に 1 行も出ない）
journalctl --user -u 'celeris@<現在のリリース>.service' --since '<手順 5 の時刻>' --no-pager | grep -c 'concurrency is deprecated'
# プロバイダの行の観測値（in_use / in_use_cos。上限とは比べない）
curl -fsS -H "Authorization: Bearer $TOKEN" "$API/providers" | jq -r '.items[] | [.id, .in_use, .in_use_cos] | @tsv'
```

期待:

- Qwen の暗黙アカウントが `effective_max_runs = 1`（`max_runs_source = account`）。
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
    -d '{"max_runs": null}' "$API/accounts/implicit/opencode-qwen" | jq '{id, max_runs, effective_max_runs, max_runs_source}'
  ```

  即時に効く。解除後は GET の実効上限を再確認する。暗黙アカウントは保存値が無ければ `default`（`max_runs_per_account`）に戻る。旧 `concurrency` は上限に効かない。
- **config を戻す**: 手順 1 の控えを元の場所に戻し（`cp -a ~/.config/celeris/config.toml.bak-account-limits-20261008 ~/.config/celeris/config.toml`、
  `providers.d` は `cp -a ~/.config/celeris/providers.d.bak-account-limits-20261008/. ~/.config/celeris/providers.d/` で中身を戻す）、手順 4 の検査と手順 5 の reload を行う。警告ログが再び出て、保存値の無い暗黙アカウントは旧 `concurrency` を引き継ぐ。
  config を外す前の挙動に戻す場合は、先に config を復元・reload してから保存値を控えの値へ戻す
  （先に保存値を解除すると、一時的に既定の上限へ変わるため）。
  プール行の `concurrency` は戻しても効かない（このリリースの仕様）。プールの同時数を絞るには、手順 3 でアカウントの `max_runs` を下げる。
- **リリースごと戻す**: [selfdeploy.md](selfdeploy.md) の手順に従う。DB の `account_limits` 表は残るが、旧リリースはこれを読まない。
  旧リリースでは config の `concurrency` が再び行の上限になるので、手順 4 で消していたら手順 1 の控えを先に戻しておく。
