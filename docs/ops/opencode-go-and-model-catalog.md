---
tasks: [01M49KDZXXBKP8K2CZDXWTCJ5S]
---
# opencode go の導入とモデル catalog の有効化手順

対象は本番 host の `~/.config/celeris/config.toml`。実行者は人。設計は
[ADR 2026-10-06](../../agent-docs/adr/2026-10-06-opencode-go-and-model-catalog.md)。設定の全体例は
`config/celeris.opencode-go.example.toml`。以下は人が host で実行する手順で、agent は実行しない。

## 1. account pool の dir を作る

opencode go の資格情報は opencode の data dir の `opencode/auth.json`（`{"opencode-go": {"type": "api", "key": …}}`）。
この `opencode/auth.json` が account dir の目印で、worker には `XDG_DATA_HOME=<account dir>` が渡る。

```sh
mkdir -p ~/celeris/opencode-accounts/main
ln -s ~/.local/share/opencode ~/celeris/opencode-accounts/main/opencode
```

- symlink にすると、opencode の db・snapshot も `~/.local/share/opencode` に書かれる（`XDG_DATA_HOME` が account dir になるため、
  run 中の opencode はここを data dir として使う）。人の対話用 opencode と状態を共有する。
- 共有したくなければ symlink にせず、`opencode/` を実 dir にして `auth.json` だけを複製する（`mkdir -p …/main/opencode && cp
  ~/.local/share/opencode/auth.json …/main/opencode/`）。この場合、鍵を更新したら複製も更新する。
- 鍵はログ・events・API に出ない。

## 2. config の追記

```toml
[accounts]
opencode_dir = "~/celeris/opencode-accounts"
# opencode_go_usage_url = "https://opencode.ai/zen/go/v1/usage"   # 既定。変えるのは試験だけ

[[providers]]
id = "opencode-go"
kind = "adapter"
adapter = "acp"
llm_source = "opencode_go"
account_pool = "opencode-go"      # acp 行は pool の adapter 名を文字列で書く（true / "claude-code" / "codex" も可）
tiers = ["standard"]
concurrency = 1
model = "opencode-go/<model>"     # `celerisctl models list` で見える model_id
env = { OPENCODE_DISABLE_PROJECT_CONFIG = "1" }   # OPENCODE_CONFIG は opencode 側の設定を固定したいときだけ追加

[model_catalog]
# refresh_interval_seconds = 3600
# opencode_go_models_url = "https://opencode.ai/zen/go/v1/models"
# opencode_cli = "opencode"
```

- `[accounts] opencode_dir` に既定は無い。書かなければ opencode go の pool は使われない。
- `[model_catalog]` は省略してよい（括弧内が既定）。`refresh_interval_seconds = 0` で自動発見を止める（手動は使える）。
- 発見で見えたモデルは routing に自動では入らない。使いたいモデルは provider 行の `model` / `tier_models` に書く。

## 3. 配送後の確認

1. `celerisctl models discover` を実行する。source ごとに ok / error が出る。
2. `celerisctl models list` で source（`claude-oauth` / `codex-oauth` / `opencode-go` / `openai-compatible:<id>`）ごとのモデルを見る。
3. `GET /api/v1/accounts` に adapter `opencode-go` の account があり、`five_hour` / `seven_day` / `one_month` の 3 窓が出る。
   idle の logged-in account は 300 秒おきに確認される。すぐ見たいときは `POST /api/v1/accounts/opencode-go/main/check`。
4. web の `/accounts` に 3 本目のバー（1 か月）、`/models` に source ごとの表と発見ボタン・上書き編集があることを見る。

## 3a. 画面から役割へモデルを割り当てる（config を編集しない）

ADR 2026-10-06 model-role-assignments。`[[providers]]` の `model` / `tier_models` を書かなくても、web の `/models` 画面で
source ごとに「frontier / standard / cheap にはこのモデル」を決められる。

1. `[accounts] opencode_dir` を設定し account を作っておく（§1・§2）。これだけは config が要る。
2. `/models` で opencode-go の source を開き、`発見` でモデル一覧を取る（`celerisctl models discover` でも同じ）。
3. 役割（frontier / standard / cheap）ごとにモデルを選んで保存する。保存前の影響表示（preview）で、その枠を使う provider 行と
   llm-proxy の lane が何から何に変わるかを確かめる。決めた値は DB に残り、次の解決から効く（daemon の再起動も reload も要らない）。
4. `opencode go を使う` を押すと、`llm_source = opencode_go` の provider 行が無いときだけ
   `{id: "opencode-go", adapter: "acp", llm_source: "opencode_go", account_pool: "opencode-go", tiers: [frontier, standard, cheap]}` を
   `POST /api/v1/providers` で作る（providers.d に書いて reload）。割り当てが無い役割はその provider の候補にならない。
   `account_pool` は `true` / `false` に加えて pool 名の文字列（`"opencode-go"`）を API でも設定できる。名前付き pool は
   `[accounts]` に対応する dir（opencode なら `opencode_dir`）があるときだけ受け付ける（無ければ 422 `invalid_provider`）。

API: `GET /api/v1/llm/models/assignments`（割り当てと全枠の実効）、`PUT`/`DELETE …/assignments/{source}/{tier}`、
`POST …/assignments/preview`。割り当ての変更は event `model_role_assignment_changed` に残る。
`celerisctl models assign <source> <tier> <model_id>` / `unassign <source> <tier>` も同じ API を呼ぶ。

### Excluded（除外）とは

割り当て行は残るが、いまは routing に使えない状態。理由は 2 つ。`override:disabled` はそのモデルに `disabled` の上書きがある
（先に見る）、`catalog:unavailable` は発見で見えなくなった（`available = false`）。Excluded の枠は黙って別のモデルに差し替えず、
その provider / lane を「未設定」として候補から外す。モデルが戻る（`available = true`）か上書きを外せば、割り当てのまま自動で
Assigned に戻る。直したくなければ割り当てを外す（config の値に戻る）か別のモデルに替える。

## 4. 枠切れの挙動と『不明』

- usage の `status = "rate-limited"` の窓は利用率 100% として扱い、pool の全 account が枠切れなら provider 選択から外れて、
  同じ tier の次の provider へ落ちる。
- 取得できない窓は『不明』（API では `null`）。0 とは見なさず、除外理由にもしない。通信失敗・timeout・解析不能の確認は観測を更新しない
  （前回値を消さない）。`auth.json` が無い・鍵が読めない・401/403 は『ログインが要る』扱い。
- 採点は観測できた窓の最小の残りを使う（claude / codex は従来どおり 5 時間と 7 日の両方が要る）。

## 5. 既知の制限

- run 中の 429（`GoUsageLimitError`）の窓の特定と `resets_at` の記録は未実装。いまは本文の "usage limit" で Exhausted になるだけ
  （`crates/task-worker/src/provider.rs` の TODO）。次の定期確認で usage の値が入る。
- 発見は claude の OAuth token を更新しない。期限切れなら claude の発見は失敗し、catalog は変わらない（消えたと誤認しない）。
- 発見 hook は daemon 起動時の config の写しを使う。`[model_catalog]` や providers を変えたら daemon の再起動が要る。
- catalog の新モデルは routing に自動で入らない。override の `tier` か config で決める。
