# モデル供給・アカウント・ハーネス

ハーネスは「何をどの契約で実行するか」、プロバイダは「どの実行アダプタとモデルを使うか」、
アカウントは「どの認証・利用枠を使うか」です。CoS は特定の会社のモデルに依存しません。

## 汎用の仕事を自動選択にする

対話・計画・実装・分析・執筆の `[[harnesses]]` では `adapter` を省略します。
必要な `tier`、指示、成果物、予算はそのまま残します。

```toml
[[harnesses]]
id = "conversation"
conversation = true
tier = "standard"

[[providers]]
id = "claude-pool"
adapter = "claude-code"
tiers = ["standard", "frontier", "cheap"]
account_pool = true

[[providers]]
id = "codex-pool"
adapter = "codex"
tiers = ["standard", "frontier", "cheap"]
account_pool = true

[accounts]
claude_dir = "~/.local/celeris/claude-accounts"
codex_dir = "~/.local/celeris/codex-accounts"
max_runs_per_account = 2
```

これは既存の設定へ組み込む抜粋です。プロバイダの行には同時実行数を書きません（[同時実行数はアカウントで決める](#同時実行数はアカウントで決める)）。
CoS の指示文や他のハーネスを消す必要はありません。
各 CLI の認証は GUI の「アカウント」で行います。Codex だけを使う構成でも、Claude の設定は不要です。
モデル名はプロバイダ側で指定します。別サービスのモデル名を共有する必要はありません。

選択順は、要求する tier と adapter の契約、アカウントの空き（実効上限）、cooldown を確認した上で:

1. 使用可能なプールのアカウントを、Claude / Codex をまたいで残量スコア順に選ぶ。同点はプロバイダの設定順。
2. プールが使えなければ、非プールの供給元（例: ACP 経由のローカルモデル）を設定順に選ぶ。
3. 候補がすべて一時的に使用不能なら待機する。要求 tier を勝手に下げない。

`adapter` を明示すれば固定です。PaperQA、Local Deep Research、LangMem は専用の契約なので、
その adapter を指定した仕事だけに使います。対話や通常の計画が誤って調査・知識整理に流れることはありません。
スキルの一致とプロバイダの tier は別で、ローカルモデルの tier は実際に任せられる能力に合わせて登録してください。

## 同時実行数はアカウントで決める

設計判断: [ADR 2026-10-08 account-concurrency-limits](../../agent-docs/adr/2026-10-08-account-concurrency-limits.md)。

- **`[[providers]].concurrency` は deprecated です。** 書いてあっても読み込めますが、プロバイダの上限として dispatch では使いません。
  非プール行では移行用に暗黙アカウントの既定値として参照します（下の実効上限の順序）。
  読み込むたびに（起動と `POST /api/v1/reload`）、行ごとに次の警告が 1 行出ます。
  `provider <id>: concurrency is deprecated (ADR 2026-10-08 account-concurrency-limits); …`。
  `concurrency = 0` は従来どおり設定エラーです。管理 API（`POST /providers`・`PATCH /providers/{id}`）の本文に
  `concurrency` を入れても型だけを検査し、`providers.d/` には保存しません。応答の `concurrency` は互換のために残していますが、意味は持ちません。
- **上限はアカウントごとの `max_runs`** です。GUI の「アカウント」画面で各アカウントの上限を編集できます。
  API では `PATCH /api/v1/accounts/{adapter}/{id}` に `{"max_runs": 3}` を送ります。`{"max_runs": null}` を送ると保存値を消して既定に戻ります。
  値は 1〜32 です。変更は再起動も reload も要らず、次の tick から効きます。下げても、走っている run は止めません。
  保存先は daemon の DB（`account_limits` 表）で、再起動と `POST /reload` を跨いで残ります。
- **実効上限**は、次の順に決まります。`GET /api/v1/accounts` の各 item の `effective_max_runs` と、どれを使ったかを示す `max_runs_source` で確認できます。
  1. そのアカウントに保存した `max_runs`（`"account"`）
  2. 暗黙アカウントだけ: 行に書かれた旧 `concurrency`（`"legacy_concurrency"`）
  3. `[accounts] max_runs_per_account`（`"default"`。`[accounts]` が無い構成では 2）
- `[accounts] max_runs_per_account` は、保存値の無いアカウントの**既定値**です。これを変えたときは、従来どおり再起動が必要です。
- **暗黙アカウント**: アカウントを持たない行（`account_pool` が無い、または false。ローカルの Qwen、PaperQA、LDR、LangMem など）には、
  行ごとに暗黙のアカウントが 1 件あります（`adapter = "implicit"`、id は行の id）。アカウント画面に「暗黙アカウント」の印が付き、他のアカウントと同じ上限の欄で出ます。
  API のパスは `/api/v1/accounts/implicit/<provider id>` です。ログイン・確認・削除はありません。
  行を消す、またはプールに変えると、次の reload か再起動でその暗黙アカウントの保存値も消えます。
- **CoS の対話 run**（ADR-0089、Phase R6-5）は、全体の `max_concurrency` に数えません。葉の run で枠が埋まっていても起動します。
  CoS の run もアカウントを消費します。実アカウントでは、走っている run が最も少ないアカウントに `effective_max_runs + 1` 本目まで載ります。
  暗黙アカウントでは +1 しません（CoS を含む全数で比べます。GPU 1 枚の Qwen に 2 本目を載せないため）。
  CoS の同時数の絶対上限は `[execution] max_cos_runs` です（既定 2、0..=8。`0` で例外を無効化）。
- `GET /providers` の `in_use`（CoS 以外）と `in_use_cos`（CoS）は、その行で走っている run の**観測値**です。上限と比べる値ではありません。
- プール行の実質的な同時数は、「そのプールで選べるアカウントの実効上限の和」と `[execution] max_concurrency` で決まります。
  本番の config から `concurrency` を外す手順は [provider-llm-source-migration.md](../ops/provider-llm-source-migration.md#プロバイダの-concurrency-を外し同時実行数をアカウントで管理する) にあります。

```toml
[execution]
max_cos_runs = 2
```

## coding worker の選択

`claude-code`、`codex`、`acp`、`aider` は provider の `adapter` で登録する。
モデル階層と adapter の選択は現在の組織 profile と provider 設定に従う。
特定の adapter が必要な仕事は task の `worker_hint.adapter` で指定できる。
`aider` の設定例は `config/celeris.aider.example.toml` にある。

## Codex の残量

`codex app-server` の `account/read` と `account/rateLimits/read` から取得します。
推論もタスク作成も行いません。active デーモンはログイン済み・未使用の Codex アカウントを 5 分ごとに確認し、
GUI の「確認」からも即時に取得できます。ログイン中のアカウントは自動確認しません。

短期・長期の利用率、残り割合、リセット時刻、観測時刻を GUI に表示します。
取得できない枠は不明とし、リセット時刻を推定しません。API キーなど ChatGPT 以外の認証では、
ChatGPT 利用枠の取得対象外であることを「最後の確認」に表示します。
通信失敗時は前回の観測を保持するので、観測時刻と確認結果も見てください。

## 既存設定の移行

旧設定には `conversation` などに `adapter = "claude-code"` が残っている場合があります。
更新した Celeris を用意した上で、次のツールで確認します。

```sh
python3 scripts/portable-providers.py ~/.config/celeris/config.toml
python3 scripts/portable-providers.py ~/.config/celeris/config.toml --apply
```

対象は既知の汎用ハーネス（旧 `roles` も含む）の adapter 固定だけです。
専用ハーネス、プロバイダ、モデル、認証設定は保持します。適用前に権限 0600 のバックアップを作り、
設定の内容や秘密は標準出力へ出しません。特定の汎用ハーネスを固定して使う場合は、その adapter を残してください。

ハーネス変更にはデーモンの再起動が必要です。通常のリリース手順は [selfdeploy.md](../ops/selfdeploy.md) を参照してください。
既存タスクに保存された明示的な adapter 指定は変更しません。移行後に Console から送った依頼から自動選択になります。

設計判断: [ADR-0049](../../agent-docs/adr/0049-portable-providers-and-codex-usage.md)。

## Codex の worktree と成果物

Codex worker は `workspace-write` を既定とし、dispatcher が選んだ成果物ディレクトリを
`--add-dir` で追加する。リポジトリの worktree と結果ファイルの保存先が異なっても書き込める。

Git の管理領域への書き込みやブラウザ起動など、sandbox 外の操作が必要な自律タスクでは、
対応する Codex CLI で自動承認レビューを明示的に設定できる（本環境では 0.154.0 で実機確認）。

```toml
[adapters.codex]
extra_args = ["--approve-for-me"]
```

これは sandbox の無効化ではなく、個々の権限要求を Codex のレビュアーが審査する方式。
拒否された操作は実行できない。管理設定による制限も維持される。
[OpenAI の sandbox と承認の説明](https://learn.chatgpt.com/docs/sandboxing) を参照。
