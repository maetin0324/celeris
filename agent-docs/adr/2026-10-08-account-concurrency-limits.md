---
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
---
# ADR 2026-10-08: 同時実行数をアカウントごとに一本化し、プロバイダの concurrency を deprecate する（暗黙アカウントを含む）

- 日付: 2026-10-08
- 状態: **Accepted**（設計。実装は同じ task の後続 WorkUnit `limits`・`implicit`・`api`・`web`・`gui`・`docs`・`verify` が行う）
- 人の依頼（2026-10-08 CoS chat thread `01M4D7BWDEC0WH5JCEZKC8VW56` seq 1）: 「celeris のプロバイダに関して、道具で同時使用可能個数をつけるのを撤廃し、
  アカウントごとに管理するようにしたいので今あるそれらの機能を全て deprecate し、プロバイダは LLM Source の管理に専念させたいです。
  また、アカウント画面から動的にそれぞれのアカウントの同時実行可能数を編集できるようにしたいです。」
- 人の決定 `nonpool-limit`: **(ii) implicit-account**（2026-10-08 の人のコメントで (i) から訂正。§(e)）
- 関連・上書き: [ADR-0012](0012-multi-account-and-worker-run.md)、[ADR-0017](0017-account-management-from-gui.md)、
  [ADR-0024](0024-claude-account-pool-and-budget-balancing.md)、[ADR-0089](0089-cos-runs-bypass-concurrency.md)、
  [ADR-0132](0132-provider-llm-source-split-and-cheap-qwen.md)（D1 の「`concurrency` は道具の欄として維持」と付記 L2(c)・L6 を**上書き**する）。
  上書きされる側の各 ADR の末尾に、この ADR を指す付記を足した（双方向）。
- 進捗: [agent-docs/progress/2026-10-08-account-concurrency-limits/adr.md](../progress/2026-10-08-account-concurrency-limits/adr.md)

## 文脈

同時実行数の上限が 2 か所にある。

1. **プロバイダ（道具の行）**: `[[providers]].concurrency`（`crates/celeris/src/config/providers.rs`、既定 1、0 は設定エラー）
   → `ProviderPolicy::concurrency_limit` → `task_dispatch::capacity::provider_full`。dispatcher（`dispatcher.rs`）・
   `provider_select.rs`（ADR-0132 付記 L2(c) のローカル優先を含む）・`routing_shadow.rs`・`cos_chat/launch.rs`・`dispatch_run.rs` が見る。
   API は `GET/POST/PATCH /api/v1/providers` の `concurrency`・`in_use`・`in_use_cos`、web/gui のプロバイダ画面に入力欄がある。
2. **アカウント**: `[accounts] max_runs_per_account`（全アカウント共通、既定 2）→ `capacity::account_run_limit`（CoS run は +1。ADR-0089 規則 2）。
   `GET /api/v1/accounts` は全体の `max_runs_per_account` と各 item の `in_use` を返すだけで、アカウントごとには変えられない。

人は道具ごとの枠をやめ、上限をアカウントに一本化し、アカウント画面から動的に変えたい。一方で本番には**アカウントを持たない行**がある
（`account_pool = false`。2026-10-08 の本番 config では `opencode-qwen`・`pi-qwen`・`ldr-qwen`・`paperqa-qwen`・`langmem-main`。
どれも `concurrency = 1` を明示している）。Qwen は GPU 1 枚なので、プロバイダの `concurrency` をただ消すと 1 本で止まらなくなる。
この扱いを人の決定 `nonpool-limit` に問い、人は **(ii) implicit-account**（非サブスクリプションの LLM source ごとに暗黙のアカウントを 1 件作り、
上限はそれに持たせ、他のアカウントと同じ画面・API で編集する。LLM source の config 側には上限を持たせない）を選んだ。

## 決定

### (a) プロバイダ側の同時実行数を deprecate する

- **config**: `[[providers]].concurrency` は引き続き**読める**（`deny_unknown_fields` でも構文エラーにしない）。型は `Option<usize>` に変え、
  「書かれていたか」を区別する（§(e) の引き継ぎに使う）。書かれていれば読込時に 1 行ごと `warn`
  （`provider <id>: concurrency is deprecated (ADR 2026-10-08 account-concurrency-limits); …`）を出す。文言は、プール行なら
  「無視する。アカウントごとの上限を使う」、非プール行なら「無視する。上限はその行の LLM source の暗黙アカウント `implicit/<llm source id>` が持つ（アカウント画面で編集する）」。
  **起動も `POST /reload` も止めない**。`concurrency = 0` も**読めて警告だけ**（設定エラーにしない。値を何にも使わないので 0 が不正値になる余地がない。`providers.d` の写しも同じ規則）。
- **dispatch**: どの経路も `concurrency` を見ない。`capacity::provider_full` は削除し、満杯の判定は §(c) のアカウントの上限だけにする。
  `ProviderPolicy::concurrency_limit` も dispatch の判定からは外す（`StaticPolicy` が値を保持するのは §(e) の既定値を作るため）。
- **API の入力**: `POST /providers`・`PATCH /providers/{id}` の本文の `concurrency` は**受け付けるが効かない deprecated 欄**。
  型の検査（非負整数。0 は従来どおり 400 `bad_request`）だけを行い、値は `providers.d` に**保存しない**（既存の `providers.d` に書かれた値は
  config と同じく読める）。受け取ったら `warn` を 1 行出す。応答の形は変えない。
- **API の応答**: `ProviderView` / `ProviderConfigView` / `GET /status` の `providers[]` の `concurrency` は**残す**（generated 型と旧 gui を壊さないため）。
  schema に `deprecated: true` を付け、値は「config に書かれた値、無ければ旧既定の 1」で、**意味は持たない**（何とも比べない）。削除は別の ADR で決める。
- **`in_use` / `in_use_cos` の意味**: `in_use` は「この行で走っている CoS 以外の run（プロバイダを使うレビュー run を含む）の数」、`in_use_cos` は
  「この行で走っている CoS run の数」。どちらも**観測値**で、上限と比べる値ではなくなる。ADR-0089 規則 3 の「`in_use` は CoS を含まない」は維持する。
- **web / 旧 gui**: プロバイダ画面から「同時実行数（concurrency）」の入力欄と `in_use / concurrency` の分数表示を外し、`in_use`（＋`in_use_cos`）を
  「実行中」として数だけ出す。旧 gui は入力欄を外す最小変更に留める（置き換え予定のため）。

### (b) プロバイダ行は LLM source の管理に専念する

- `[[providers]]` の行が持つ意味は、id・`adapter`（＋`command`/`args`/`settings`/`extensions`/`tools`/`env`/`env_from_secrets` の道具の起動情報）・
  `llm_source`・`kind`・`tiers`・`model`・`tier_models`・`account_pool`・`account_id` である。**同時実行数は持たない**。
- **ADR-0132 D1 の上書き**: D1 の「既存の `adapter`、`tiers`、`concurrency`、`command`、`args`、`settings` は道具の欄として維持する」から
  `concurrency` を外す（deprecated・dispatch では見ない）。他の欄は維持。
- **ADR-0132 付記 L2(c)・L6 の上書き**: ローカルの行（L1）の空きは、その行の LLM source の**暗黙アカウント**（§(e)）の空きで判定する。
  L6 の「ローカルの行の同時実行数は既存の `[[providers]].concurrency`（既定 1）で決める。新しい欄は足さない」は、
  「ローカルの行の同時実行数はその行の LLM source の暗黙アカウントの上限で決める（既定は `max_runs_per_account`。§(e)）。アカウント画面で変える」に置き換える。
  L3 の `local_full` の意味（前段で見送ったローカルの行が満杯）はそのまま。
- ADR-0012・ADR-0017・ADR-0024・ADR-0089 の `concurrency` に関する記述も、末尾の付記のとおりこの ADR が優先する。

### (c) アカウントごとの上限

- **欄名は `max_runs`**（1 アカウントで同時に走らせる非 CoS の run の上限）。アカウントごとの保存値で、無ければ既定に従う。
- **実効上限**（`effective_max_runs`）の決め方:
  1. そのアカウントに保存された `max_runs` があればそれ。
  2. 無ければ `[accounts] max_runs_per_account`（`[accounts]` が無い構成では組み込みの既定 2）。
  旧 `[[providers]].concurrency` は、暗黙アカウントでも実効上限の入力にしない（人の決定 `nonpool-limit` (ii)。LLM source の config 側に上限を持たせない）。
  どれを使ったかを `max_runs_source`（`"account"` / `"default"`）として返す。
- `[accounts] max_runs_per_account` は**全アカウントの既定値**として残す（意味が「全アカウントの共通上限」から「保存値の無いアカウントの既定」に変わる）。
  これの変更は従来どおり再起動が要る（ADR-0024 M7）。アカウントごとの `max_runs` は再起動も reload も要らない（§(d)）。
- **CoS +1（ADR-0089 規則 2）は実アカウントで維持する**: CoS run の上限は `effective_max_runs + 1`。`capacity::account_run_limit(effective, cos)` の形は
  変えず、第 1 引数を全体の既定からアカウントごとの実効上限に替える。**暗黙アカウントには +1 しない**（§(e)）。
  `[execution] max_cos_runs`（既定 2）と `max_concurrency`（非 CoS 全体）も維持する。最悪の同時数は
  「`max_concurrency` + `max_cos_runs`」、1 実アカウントあたり「`effective_max_runs` + 1」。
- **数え方**: 実アカウントの `in_use` は従来どおり `(adapter, id)` で走っている run の全数（CoS を含む。ADR-0089 規則 2「アカウントは消費する」）。
  非 CoS の run は「`in_use` のうち非 CoS の数 < `effective_max_runs`」で載る（ADR-0089 規則 3 と同じく CoS が葉の枠を食わない）。
  CoS run は「`in_use`（全数）< `effective_max_runs` + 1」で載る。
- **整合させる箇所**（後続 WorkUnit `limits`・`implicit`）: 次はどれも「満杯のプロバイダ」の判定を、プール行なら「選べるアカウントが残っていない」
  （`accounts::evaluate` の `at_capacity` を実効上限で判定）、非プール行なら「暗黙アカウントが満杯」に置き換える。
  - `dispatcher.rs`（`dispatch_one`・満杯集合・`account_usable`・`pick_account`・`sticky_provider`）
  - `dispatcher/provider_select.rs`（`select_provider_for` の候補落とし、ADR-0132 付記 L2(c) / L3 のローカルの空き）
  - `routing_shadow.rs`（影の routing の可否判定。本物と同じ関数を呼ぶ）
  - `dispatcher/cos_chat/launch.rs`（CoS chat run のプロバイダ・アカウント選択。+1 は実アカウントだけ）
  - `dispatch_run.rs`（`celerisctl worker run` を含む直接起動の経路）
  - `GET /providers` の `in_use` / `in_use_cos`（§(a) の観測値。数え方は不変）
  - `GET /accounts` の `in_use`・`excluded_reason = "at_capacity"`（実効上限で判定）
- `capacity.rs` の純粋関数として、実効上限の解決（`effective_account_limit(stored, default) -> (usize, Source)`）と
  暗黙アカウントの満杯判定を足し、`provider_full` を消す。

### (d) 保存先と API

#### 保存先: SQLite（daemon の DB）の新しい表

- `account_limits(adapter TEXT NOT NULL, account_id TEXT NOT NULL, max_runs INTEGER NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY (adapter, account_id))`
  を `crates/task-core/migrations/` の次の番号の migration で作る。`adapter` は実アカウントの `AccountAdapter::as_str()` か `"implicit"`（§(e)）。
  `browser_site_policies`（migration 0060）と同じく、event から作り直す派生表ではなく正本の設定表として扱う（replay の対象外）。秘密は持たない。
  保存値が無い＝「既定に従う」。
- **AccountBook（`<root>/.celeris-usage.json`）にしない理由**:
  1. 暗黙アカウントには根ディレクトリが無く、`[accounts]` が無い構成もある。AccountBook はアダプタの根ごとの file なので置き場が無い。
  2. AccountBook は観測値の帳簿（「タスクの真実ではない」）で、壊れていれば `warn` して空から始める。人が決めた上限が黙って既定に戻るのは困る。
     SQLite は書き込みが原子的で、`backups` の対象でもある。
  3. 帳簿の file はアカウントの認証ディレクトリと同じ根の下にある。上限は秘密と無関係なので、秘密の置き場から離す
     （DB は秘密を持たない。`[secrets]` や `<root>/<id>/` の認証 file とは別 file）。
  4. 書き込みは daemon（celeris）だけが行い、管理要求の経路（`AdminRequest`）で dispatcher の値と同時に更新できる。
- **反映**: PATCH は celeris 側の `AdminRequest::AccountLimitSet` で処理する。DB に書いてから dispatcher のメモリ上の表を更新するので、**次の tick から効く**。
  起動時と `POST /reload` のたびに表を読み直す（reload で `[[providers]]` の行が変わっても保存値は残る）。
- **使用中でも変更できる**。下げても走っている run は止めない（新しい run が `in_use` が下がるまで待つだけ）。上げれば次の tick で追加の run が載る。
- **削除**:
  - 実アカウント: `DELETE /accounts/{id}`（ADR-0024 M6）が `.removed/` へ移すときに、同じ処理の中で保存値も消す。`POST /accounts` で作るときも、
    同じ `(adapter, id)` の古い保存値が残っていれば消す（作り直しで古い上限を引き継がない。M6 と同じ理由）。
  - 暗黙アカウント: 単位が LLM source なので、**その source を使う非プール行が 1 本も無くなったとき**に保存値を消す。
    `DELETE /providers/{id}`・config の手編集で行を消す・行をプールに変える（`account_pool = false` → on）・行の `llm_source` を変える、
    のどれも reload / 再起動の読み直し時点で同じ判定に通る（「対応する source が config から消えた」暗黙アカウントの保存値を消し、`info` を 1 行出す）。
    同じ source を使う他の行が残っていれば保存値は**残る**（消えない）。
  - 行の id を変えても source が同じなら同じ暗黙アカウント（保存値は引き継ぐ）。逆に source を変えれば別の暗黙アカウントになり、
    古い source の保存値は上に書いたとおり消える。

#### API: `PATCH /api/v1/accounts/{adapter}/{id}` → 200 `AccountView`

- 要求本文 `{"max_runs": 3}` で設定、`{"max_runs": null}` で保存値を消して既定（§(c) の 2・3）に戻す。`max_runs` の欄が無い本文・未知の欄は 400 `bad_request`。
- **値の範囲は 1..=32**。0・負数・33 以上・整数でない値は 400 `bad_request`（`message` に範囲を書く）。32 は `[execution] max_concurrency` の
  実用域を十分に超える上限で、誤入力（3 → 300 等）を弾くためのもの。1 アカウントの run を止めたいときは cooldown・ログアウト・削除を使う（0 は意味を持たせない）。
- `{adapter}` は `claude-code` / `codex` / `opencode-go` / `implicit`。それ以外は 400 `bad_request`。`{id}` は既存のアカウント id 規則
  （1〜64 文字の ASCII 英数字・`-`・`_`）に合わなければ 400 `bad_request`。**暗黙アカウントの id は LLM source** なので、
  `openai_compatible:<source id>` の `:` だけは追加で許す（他は同じ規則。path にも DB の鍵にもそのまま使える）。
- 存在しないアカウント（実: 根の下にディレクトリが無い。暗黙: その source を使う非プール行が無い）は 404 `account_not_found`。
  実アダプタの根が設定されていない構成は 409 `accounts_unavailable`（既存の管理系と同じ）。暗黙アカウントは `[accounts]` が無くても編集できる。
- 管理系（`token_file` 未設定でも 401。ADR-0017 M3）。`standby` / `draining` の間は 503 `standby`。
- 応答は更新後の `AccountView`（§下の GET と同じ形）。
- **路の衝突**: 既存の `/api/v1/accounts/{id}/check`・`/{id}/login` と同じ深さになる。axum（matchit 0.8）の制約で最初の段の引数名は既存と同じ `{id}` にし
  （意味は adapter）、静的な `check` / `login` が優先される。id が `check` / `login` のアカウントも編集できるよう、その 2 つの路にも PATCH を登録して
  同じ処理へ渡す（試験で固定する）。

#### API: `GET /api/v1/accounts` の追加

- 各 item に `max_runs`（保存値。無ければ `null`）、`effective_max_runs`、`max_runs_source`、`implicit`（bool）、`providers`（暗黙アカウントだけ。
  その LLM source を使う非プール行の id の一覧。設定順）を足す。`excluded_reason = "at_capacity"` は実効上限で判定する。
- 全体の `max_runs_per_account` は残す（既定値）。`[accounts]` が無い構成では従来 0 を返していたが、暗黙アカウントの既定に使う値（組み込みの 2）を返す。
  `items` は暗黙アカウントを含むので、`[accounts]` が無くても空とは限らない。
- `docs/api/v1/gui-api.md`（§3.29〜3.35 と providers の節）・JSON schema・web の生成型を同じ変更で更新する（後続 `api`）。

### (e) アカウントを持たない行: 暗黙アカウント（人の決定 `nonpool-limit` = (ii) implicit-account、確定）

- **作る対象**: `account_pool` が off（`account_pool = false` / 省略）の `[[providers]]` 行（`providers.d` を含む）が使う
  **解決済み LLM source ごとに 1 件**（`Config::provider_llm_source` が返す値。`llm_source` を書かない行は `derive_llm_source` の派生 source）。
  ADR-0132 の検証で `account_id` はプール行にしか書けないので、非プール行はちょうど「実アカウントを持たない行」である。
  adapter（acp・pi・paperqa・langmem・local-deep-research・fake）に依らず一律に作る（例外を作ると上限の無い行が生まれるため）。
  **同じ source を使う行は 1 件を共有する**: 本番の `opencode-qwen` と `pi-qwen`（どちらも `openai_compatible:qwen`）は同じ暗黙アカウントで、
  上限も in_use も 1 件分（GPU 1 枚を 2 行で取り合わない）。LLM source の config 側には上限の欄を持たない。
- **識別子**: `adapter = "implicit"`、`id = <llm source id>`。API の路は `/api/v1/accounts/implicit/<llm source id>`。
  - source の id は `LlmSourceRef` の文字列表現（`celeris` / `claude_oauth` / `codex_oauth` / `opencode_go` /
    `openai_compatible:<source id>` / `none` / `unknown`。`LlmSourceRef::to_key()`）で、config と API と DB で同じ綴りを使う。
  - アカウント id の規則（1〜64 文字の ASCII 英数字・`-`・`_`）に `openai_compatible:<id>` の `:` だけを足して path と DB の鍵に使える（§(d)）。
  - 実アカウントは `(AccountAdapter, id)` で識別され、`AccountAdapter` の文字列は `claude-code` / `codex` / `opencode-go`。`implicit` は
    **予約語**とし、`AccountAdapter` に同名を足さない。よって実アカウントとは決して衝突しない。
  - 鍵は行の id ではないので、**行の id を変えても source が同じなら同じ暗黙アカウント**（保存値は引き継ぐ）。
    行の `llm_source`（または派生 source を変える adapter・model）を変えると別の暗黙アカウントに移る（古い側の保存値は §(d) の prune で消える）。
- **一覧での見せ方**: `GET /accounts` の `items[]` で実アカウントの後ろ、source を使い始めた行の設定順に並べる。`implicit: true`・
  `id: "<llm source id>"`・`providers: [その source を使う非プール行の id…]`・`dir: null`・`logged_in: true`（ログインの概念が無い）・
  `usage: null`・`score: null`・`login_pending: false`。`stats` は `WorkerStarted.provider` が `providers` のどれかの run を集計する（行をまたいで合計）。
  web のアカウント画面は「暗黙（LLM source <id>）」の印と行の一覧を付けた別の節に出し、上限の編集 UI は実アカウントと同じ部品を使う。
  ログイン・確認・削除のボタンは出さない。
- **削除・作成**: 暗黙アカウントは `POST /accounts`・`DELETE /accounts/{id}`・ログイン・確認の対象外（`?adapter=implicit` は 400 `bad_request`）。
  その source を使う行の追加とともに現れ、**source を使う行が全て無くなったとき**消える（保存値の削除は §(d)）。
- **既定上限**: §(c) の順。保存値が無ければ `max_runs_per_account`（既定 2）。旧 `concurrency` は引き継がない。本番の Qwen 行
  （`opencode-qwen`・`pi-qwen`）は `concurrency = 1` を明示しているが、**この変更の後は既定の 2 本まで載る**（2 行合わせて 2 本）。
  1 本のままにするには、移行前にアカウント画面（または `PATCH /api/v1/accounts/implicit/openai_compatible:qwen`）で
  その source の暗黙アカウントの `max_runs = 1` を保存しておく。手順は docs/ops/provider-llm-source-migration.md に書く。
- **CoS +1 は無し**: 暗黙アカウントの上限は CoS run を含めた全数で比べる（`in_use + in_use_cos < effective_max_runs`）。
  これは ADR-0089 規則 2 の「プールでないプロバイダには例外を作らない（CoS も含めた合計で比べる）」をそのまま暗黙アカウントへ移したもので、
  GPU 1 枚の Qwen に CoS が 2 本目を載せない。CoS の run も review run も、source が同じなら同じ 1 件に数える。
- **数え方**: 暗黙アカウントの `in_use` は**その source を使う全ての非プール行**で走っている全 run（非 CoS・CoS・プロバイダを使うレビュー run）。
  旧 `provider_full` の非プールの式（`in_use + in_use_cos >= limit`）と同じ数え方を行のぶんだけ足し合わせたもので、上限の値は保存値か既定になる。
- **派生 source を共有する行**: `llm_source` を書かない行は派生 source で束ねる。派生が `none`（fake・browser-specialist）や `unknown` になる行は
  その 1 文字列ごとに 1 件を共有するので、別々の枠が要る行は `llm_source` を明示する（本番の行はどれも明示済みか `openai_compatible:qwen` 等に解決される）。
- **prune**: 起動時（`build_dispatcher`）と `POST /reload` のたびに、今の config の非プール行が使う source の集合に無い `implicit/*` の保存値を消す
  （`info` を 1 行）。判定は「行が消えたか」ではなく「**source が config から消えたか**」なので、行の追加・削除・pool 化・`llm_source` の変更の
  どれもこの 1 つの規則で足りる。source の表は prune より前にディスパッチャへ渡す（`set_provider_llm_sources`）。

### (f) 別勘定で今回は触らないもの

- **llm-proxy の `max_concurrent_per_account`**（`crates/llm-proxy/src/config.rs`、`reservation.rs`・`selection.rs`）: proxy が上流の資格情報アカウント
  1 つあたりに同時に流す **HTTP 要求**の数。この ADR の `max_runs` は dispatcher が 1 アカウントに同時に載せる **run** の数で、単位も勘定する場所も違う。
  `celeris/<tier>` を使う run 1 本は proxy に複数の要求を出しうるし、claude-code / codex の CLI は proxy を通らずに直接上流へ行く。
  どちらも他方を読まない。同じ subscription に両方が効く構成はありうるが、合算は今回しない。
- **model routing の `ResourceGroupEntry.concurrency_limit`**（GPU 資源グループの圧力計算。`task_core::model_router::cost`）: 変えない。
- **`[execution] max_concurrency` / `max_cos_runs`**: 全体の上限として維持（§(c)）。
- **`[[clusters]].concurrency`**（ADR-0018。クラスタのジョブ枠）: 変えない。
- **`[accounts] max_runs_per_account`**: 削除しない。既定値として残す（§(c)）。

## 帰結

- プール行（本番 `claude-pool` の 4、`codex-pool` の 2、`pi-opencode-go` の 1）の行全体の上限は無くなる。行の同時数は
  「そのプールの選べるアカウントの実効上限の和」と `max_concurrency` で決まる。今と同じ並びを保ちたい場合は、移行手順（docs/ops）に従い
  アカウント画面で各アカウントの `max_runs` を合わせる（例: opencode-go のアカウントが 1 件なら `max_runs = 1`）。
- 非プール行（本番の Qwen 行）は LLM source 単位の暗黙アカウントの既定が 2 になる。移行前に `max_runs = 1` を保存しないと、
  同じ source を使う行は合わせて 2 本まで走る（移行手順を参照）。同じ source を束ねるので、行を足しても枠は増えない。
- `max_runs_per_account` は再起動、アカウントごとの `max_runs` は即時（次の tick）。
- 旧 config・旧 `providers.d`・旧 API クライアントは壊れない（読めて、警告が出て、効かない）。

## 検証条件（後続 WorkUnit が試験で示す）

- 旧 config の `[[providers]].concurrency` が読めて警告が出ること、プール行では dispatch に使われないこと（`concurrency = 1` のプール行で
  `max_runs_per_account = 2` のアカウント 2 件に 4 本載る等）。
- 実アカウントの上限で非 CoS が止まり、CoS は +1 まで載ること。暗黙アカウントは旧 `concurrency = 1` を無視し、保存値 1 なら 1 本で止まり、
  CoS も +1 されないこと。保存値が無ければ既定 2。
- 暗黙アカウントが **LLM source 単位**であること: 同じ source を使う 2 行が 1 つの上限と in_use を共有し、保存値 1 で 2 本目が止まる
  （`task-dispatch` の `implicit_account_is_per_llm_source`）。`PATCH /accounts/implicit/<llm source id>` が `POST /reload` を跨いで残ること
  （`celeris` の `implicit_source_account_limit_survives_reload`）。
- `PATCH /api/v1/accounts/{adapter}/{id}` の設定・`null` での解除・400・404・409・`check` / `login` という id の編集、変更が次の tick から効くこと、
  daemon の再起動と `POST /reload` を跨いで残ること、アカウント・行の削除で消えること。
- `GET /accounts` に `effective_max_runs`・`max_runs_source`・暗黙アカウントが出ること。gui-api.md・schema・生成型の更新。
- web のアカウント画面で上限を編集でき、プロバイダ画面から入力欄が消えていること（画面試験）。
- `cargo clippy --workspace -- -D warnings`。本番 config・systemd・daemon・本番 DB は変更しない（本番の移行は人が docs/ops の手順で行う）。

## 付記（2026-10-11、provider/source/harness の三層化）

[ADR 2026-10-11 provider source harness layers](2026-10-11-provider-source-harness-layers.md) が、provider concurrency の廃止に加え、source/account の所有関係、provider API field、implicit account の source key に関する本 ADR の該当決定を置き換える。旧 provider 欄の警告付き受け入れは行わず、移行後は設定読み込みエラーとする。アカウント上限の意味と既存 DB/event/credential の保全は新 ADR の移行方針に従う。
