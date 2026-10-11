# cli: celerisctl integration

## 変更

- `crates/celerisctl/src/commands/integration.rs` を追加。`celerisctl integration` の 5 subcommand:
  - `list [--state <s>] [--task <id>] [--json]` → `GET /api/v1/integrations[?state=]`、`--task` は `GET /api/v1/tasks/{id}/integrations`（state は client 側で絞る）。`--state` は clap の value_parser で `unintegrated|integrated|discarded|held` 以外を弾く。
  - `merge <task> <repo>` → `POST …/integrations/{repo}/merge`（body なし）
  - `discard <task> <repo> --reason <r>` / `hold …` → `POST …/{discard|hold}` body `{"reason": r}`。`--reason` は必須（clap）。
  - `reopen <task> <repo>` → `POST …/reopen`
- 認可: daemon の route は人の admin token のみを受ける（CoS run credential は server 側で拒否）。CLI は cron・models と同じく config の `[api]` から `read_token()` を読んで送る。
- `main.rs`: `Command::Integration { command, config }` を追加し、`cron`/`models` と同じく DB を開く前に処理する。CoS run 下では `cos_mapped` が登録済み operation を持たないので、mutation は従来どおり「audited CoS operation mapping が無い」で拒否される。`list` は CoS の read-only 許可に**入れていない**（run から admin token を使う経路を増やさないため）。
- `commands/mod.rs`: `pub mod integration;`
- 試験: `crates/celerisctl/src/commands/integration_tests.rs`（`integration_api_` 接頭辞、6 件）。引数解析・request の path/body・`--reason` 必須・`--state` の不正値・state filter と表の整形を確かめ、ローカル listener への HTTP 往復（method/path/bearer/body）も確かめる。外部ネットワークは使わない。

## 判断

- HTTP 呼び出しは `cron::request` / `cron::ApiConfig`（`pub(crate)`）を再利用した。ただしそのエラー文は「cron API returned HTTP …」のままなので、integration の失敗もそう表示される。共通化（daemon_api モジュールへの移動）は cron/models の変更になるので今回はしない。提案として後で。
- 出力: 既定は表（`task_id repo_id state branch head_sha reason` を tab 区切り 1 行）、`list --json` は `{"items": […]}`。action 系は daemon の応答 JSON をそのまま pretty で出す（cron と同じ）。repo 名は応答に無いので repo_id で出す。

## 検証

- `cargo test -p celerisctl integration_api_` → 6 passed, 0 failed（`commands::integration::tests::integration_api_*`）
- `cargo test -p celerisctl`（TMPDIR=/tmp で実行）→ 133 + 他の test binary すべて passed, 0 failed。
  - 注: TMPDIR を既定（run の tmp、約 100 文字）のまま実行すると、既存の `commands::browser::tests::approve_talks_one_line_to_a_private_socket` が `bind: path must be shorter than SUN_LEN` で落ちる。この試験は本変更と無関係（環境の path 長）。
- `cargo clippy -p celerisctl --all-targets -- -D warnings` → exit 0（警告 0）
- `cargo fmt -p celerisctl` 適用済み（他 crate への差分なし）

## 未対応・未解決

- docs の CLI 説明: `docs/ops/cron-jobs.md` のような celerisctl の利用ページは integration 用に無い。API の表は routes 葉で `docs/api/v1` に入っている。CLI の利用文書は今回書いていない（人が要るなら次の葉）。
- 実 daemon への結合試験（取り残し→一覧→route 操作→events）は e2e 葉の担当。この葉は HTTP の形だけをローカル listener で確かめた。
- 提案: `cron::request` を `commands/daemon_api.rs` に移し、エラー文を呼び出し元に依らない形にする（cron/models/integration で共有）。
