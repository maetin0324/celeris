---
title: "測定環境 launcher: 単一入口 script measure-launcher.sh と無 LLM e2e（launcher）"
tasks: [01M56HCNS9SYAS7TAEA7Z3E4CZ]
unit: launcher
status: done
updated: 2026-10-18
---
# launcher

完了日: 2026-10-18。

## 実装
- `scripts/campaign/measure-launcher.sh`（POSIX sh）。`<prepare|start|preflight|watch|grade|export|stop> --env <file>`。
  env ファイルは `KEY=VALUE` の sh 形式（相対 path は env の dir 基準）。`--api-url` は daemon config の `[api] listen`、token は `[api] token_file`
  （環境変数 `CELERIS_MEASURE_TOKEN` に入れ CLI には名前だけ渡す。値は argv・ログ・出力に出ない）から導き、`--measure-config` 等も同じ env から生成する。
  - `prepare`: 必須入力（manifest・normal config・canaries・template・資産 dir）の欠落を全件拒否、`validate`、測定専用 token を 0600 で作成、daemon 実行ファイルの sha256 を固定。
  - `start`: 固定 hash と一致する実行ファイルを `--config <daemon_config>` だけで起動（`launcher.pid` / `launcher.meta` に pid・starttime）、health 待ち、`env-check`。生きた自 daemon があれば再起動せず接続。
  - `preflight`: `celeris-campaign preflight`（env-check を含む）。
  - `watch`: preflight → `env-lock`（初回作成・再開は照合）→ `watch`。どれか不一致なら外部実行 0 件で非 0。`EXECUTOR`（既定 celeris。無課金 e2e は mock）。
  - `grade`: `grade-out` + `check`。`export`: `export`（`RECORDS` があれば final accounting）。
  - `stop`: pidfile の pid 1 つだけ。starttime と cmdline の `--config` が一致しなければ何も送らず非 0（pid 再利用）。SIGTERM → 上限で SIGKILL。`systemctl`・`pkill` 不使用。
- 試験 `crates/campaign/tests/launcher.rs`（5 件）: 実 script・実 `celeris-campaign`・実 task-api router（実 HTTP、案件/アカウントは正式 API/正式 pool）。
  stub は A の `claude`・`bwrap`・測定 daemon 実行ファイル（常駐するだけ）のみ。
  - 一致: prepare → start → preflight → watch → grade → export、再開 watch は env-lock 不変・台帳不変、固定記録とログに token 無し、同一 path の A 差替えは再開で拒否。
  - 拒否（A の claude が `--version` 以外で呼ばれない・task を作る POST 0・台帳不変を確認）: 別 daemon、起動後の config 書換え、版不一致、必須入力欠落（資産・env 欄）、daemon 実行ファイル差替え。
  - stop: 本番に見立てた別 process（別 pidfile）に signal が届かない、pidfile が別 process を指しても拒否。

## 証拠
- `cargo test -p celeris-campaign --test launcher`: 5 passed。
- `cargo fmt --all -- --check`: exit 0。`cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決事項・提案
- 無 LLM e2e の watch は `EXECUTOR=mock`（B/C を実 dispatcher が動かす経路は `crates/celeris/tests/campaign_lock_e2e.rs` が担当）。launcher 経由の celeris executor は実 daemon でのみ確認でき、未確認（承認待ち）。
- 実 `celeris` binary を起動する e2e は worker sandbox 外が前提のため未実施（daemon は stub。HTTP は実 router）。
- ADR の `--env` 一本化・旧 CLI 欄削除、D2-5/6（/proc と port 所有）は未実装のまま（launcher は旧 CLI 欄を生成して使う）。
