# config の動的変更 — 起動時にしか決められない欄以外を、再起動なしに Web UI から変える

- 日付: 2026-10-22
- 状態: 採用
- 決定者: 人（2026-10-22: 「config の変更にいちいち daemon の再起動が要るものが多すぎる。本当に起動時にしか設定できないもの以外は、Web UI から動的に変更できるようにする」）
- 関連: ADR 2026-10-11（破壊的変更を是とする）、ADR 2026-10-09-cos-operations-all-mutations（人だけの操作の除外）、
  ADR 2026-10-14-local-machine-settings（store の上書きが勝つ `max_concurrency`）、ADR 2026-10-08 account-concurrency-limits

## 背景

`[execution] max_replans` は起動時にしか読まれず、変更に再起動が要った（2026-10-21）。`POST /api/v1/reload` が反映する欄は
`reload_providers` の手書きの列挙で決まっており、欄ごとに「効く・効かない・エラーにする」が散らばっていた
（`[accounts]` と `[cos.attachments]` だけ個別に 400、他の起動時の欄は黙って無視）。dispatcher の `DispatchConfig` は起動時に
`Config::dispatch_config()` で 1 回作って持ち回し、reload は役割・分野・委譲だけを差し替えていた。

## 決定

### D1: 欄の分類は 1 つの表（`crates/celeris/src/config/fields.rs` の `RULES`）

欄の前置き（`.` 区切り）の最長一致で `class`（`dynamic` / `startup` / `seed`）と反映の単位 `apply_unit` を引く。
表に無い欄は `startup`（再起動が要ると言い過ぎる方へ倒す。動的だと言って効かないよりよい）。Web UI も同じ表を API で受け取る。

反映の単位: `next_tick`（次の tick の判断から）/ `next_dispatch`（次に起動する run・次にその判断をするところから。走行中の run は
起動時の写しを持つので変わらない）/ `restart` / `elsewhere`（真実は DB。別の画面・API）。

| 欄（config.toml） | 分類 | 反映の単位 | 理由 |
| --- | --- | --- | --- |
| `max_concurrency` | 動的 | next_tick | tick ごとに実効値を解く（store の上書き＝ローカル設定画面が勝つ） |
| `error_cooldown_secs` `lease_grace_secs` `retry_backoff_*` `max_requeues` | 動的 | next_tick | dispatcher の設定を差し替える |
| `idle_timeout_secs` `kill_grace_secs` `review_timeout_secs` | 動的 | next_dispatch | run の起動時に読む |
| `plan` `review` `dispatch` | 動的 | next_tick | 次の判断から |
| `reviewer` | 動的 | next_dispatch | 次に起動する review run から |
| `execution`（`max_replans`・`max_repairs*`・`gate`・`continuation*`・`parallel`・`max_parallel_work_units`・`max_cos_runs`・`planner`・`tree` ほか全部） | 動的 | next_dispatch | dispatcher は判断のたびに共有の `DispatchConfig.execution` を読む。差し替えれば次にその判断をするところから効く |
| `sessions.rollover_tokens` `sessions.continuation_resume` | 動的 | next_dispatch | 次の継続 run から |
| `routing`（`mode`） | 動的 | next_dispatch | 次の routing の判断から |
| `routing_eval.reserved_runs` | 動的 | next_tick | 枠の計算に毎 tick 使う |
| `jev` | 動的 | next_tick | tick ループが設定の変化を見て分類段を作り直す |
| `llm_sources` `llm_routes` `route_defaults` `route_selection` `harness_selection`（`execution_class_defaults` を含む）`harness_declarations` `harnesses` `adapters` | 動的 | next_dispatch | route の登録簿・adapter 一式を作り直して差し替える |
| `roles` `genres` `delegation` `conversation` `cos`（下の 2 つを除く） | 動的 | next_dispatch | 次に起動する run・委譲・対話 run から |
| `reports` `notify` `maintenance` | 動的 | next_tick | tick ループが毎回読む／掃除・ディスク監視を組み直す |
| `selfdeploy.delivery_default_departments` | 動的 | next_tick | 次の配送の判断から |
| `cos.attachments` `cos.stream_retention_days` | 起動時のみ | restart | upload の middleware と背景の GC が起動時に値を持つ |
| `db` | 起動時のみ | restart | DB を開くのは起動時の 1 回（path・busy_timeout・checkpoint） |
| `workspace_root` | 起動時のみ | restart | 足回りの root は起動時に解く（実行中の上書きは ADR 2026-10-14 の store 経路） |
| `tick_ms` | 起動時のみ | restart | tick ループの周期 |
| `api` | 起動時のみ | restart | listen address・token・socket は起動時に開く |
| `clusters` | 起動時のみ | restart | ssh master・forward の表を起動時に組む |
| `accounts` | 起動時のみ | restart | AccountBook の保存先・root が固定される。個々の account の `max_runs` は `PATCH /accounts/…` で DB が正（次の tick から。再起動不要） |
| `secrets` `memory` `handoff` `selfdeploy`（上の 1 欄を除く）`delivery` | 起動時のみ | restart | 置き場・引き継ぎ・releases の手順は起動時に決まる |
| `workspace` `scratch` `github` `containers` `storage` | 起動時のみ | restart | 作業場所・scratch の seed/reflink・コンテナ runtime などの基盤を起動時に作る（userns / sandbox の基盤を含む） |
| `cron` `knowledge` `docs_maintenance` `model_catalog` | 起動時のみ | restart | executor 表・知識の根・langmem の接続を起動時に組む |
| `llm_proxy` `mcp` | 起動時のみ | restart | listener を起動時に開く |
| `browser` | 起動時のみ | restart | runtime・egress・launcher の基盤（userns / sandbox）は起動時に決まる |
| `routing_eval`（`reserved_runs` 以外） `sessions`（上の 2 欄以外） | 起動時のみ | restart | 基盤を起動時に作る |
| `org` `org_include` | 種 | elsewhere | 空の DB の種。組織の真実は DB（`POST/PATCH /api/v1/org`） |

secret の値そのもの（欄名に `secret` `token` `password` `api_key` `credential` `private_key` を含む欄）は、動的な欄の中でも
この画面で変えない（`PATCH` は 422）。一覧・履歴では `<redacted>` に伏せる。credential は人が入れる既存の経路（`/secrets`）のまま。

### D2: 反映は 1 つの経路（`apply_config`）

`POST /reload`（CoS `daemon.reload`）と `PATCH /config/fields` は同じ `apply_config(dispatcher, config, new_config)` を通る。
失敗しうる組み立て（route 登録簿・adapter 一式・credential gate・CoS launch）を先に全部済ませ、稼働中の状態を書き換えるのは
そのあとだけ。dispatcher の動的な欄は `Dispatcher::apply_dispatch_config(Config::dispatch_config())` 1 本で入る
（起動時にしか決められない `clusters`・`accounts`・`workspace_roots`・`containers`・`knowledge`・scratch・build cache は触らない）。
旧い `reload_config`（役割・分野・委譲だけ）・`set_delivery_policy`・`[accounts]`/`[cos.attachments]` だけの個別 400 を消した。
起動時の欄をファイルだけ書き換えて再起動していない場合は、reload は黙って無視する代わりに設定画面が「再起動待ち」
（`pending_restart`、`value` ≠ `running`）と出す。

### D3: 真実の置き場は config.toml 1 つ（DB に持たない）

変更は検証してから config.toml に書き戻す（`toml_edit`。コメント・並び・他の欄の書式は保つ。一時 file → rename、権限は引き継ぐ）。
DB に持つ案は、人が手で書く config.toml と DB の二重の置き場になり、どちらが勝つかの規則が要るので採らない
（`max_concurrency` の store 上書きだけは ADR 2026-10-14 の既存の経路で、この表では「`elsewhere` ではなく動的」と数え、store が勝つ）。
手順: ① 分類・secret を検査（1 欄でも不可なら全体を 422）→ ② 本文を編集し `Config::load_text`（起動時と同じ検証）→
③ 書き戻し → ④ `apply_config`。④ が落ちたら本文を元に戻す（何も変わらない）。ファイルを人が手で直していた分も
同時に反映される（`PATCH` は「読み直し＋この変更」）。

### D4: 変更の履歴は `config_events`（migration 0091、追記専用）

`ts` / `actor`（`admin`）/ `key` / `class` / `apply_unit` / `before` / `after`。UPDATE・DELETE を持たない。値の変わらない欄は記録しない。
`GET /config/events` が新しい順に最大 200 件返し、Web UI の設定画面に出る。

### D5: API と権限

`GET /config/fields`（欄・分類・稼働中の値・分類表）、`PATCH /config/fields`（`{changes:[{key, value}]}`、`null` は欄を消す）、
`GET /config/events`。不正値・未知の欄・型違いは 422 `config_invalid`、起動時のみ・種・secret の欄は 422 `config_restart_required`
（detail に再起動が要る旨）。`PATCH` は人だけ（`human_only`。CoS は `daemon.reload` だけを持つ）。

### D6: Web UI

設定画面（`/settings`）。動的な欄は編集して保存、起動時のみの欄は値と「再起動が要る」旨、secret は値を出さない。変更履歴を同じ画面に出す。

## 本番への移行

config.toml の書き換えは不要（欄は増減していない）。DB は migration 0091 が `config_events` を足すだけ（release の handoff で適用）。
