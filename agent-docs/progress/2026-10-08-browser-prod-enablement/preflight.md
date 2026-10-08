---
task: browser-prod-enablement
wu: preflight
status: done
completed: 2026-10-08
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

# preflight: 本番の browser 前提の点検（ADR 2026-10-08-browser-prod-enablement D5）

## 実装

- `celerisctl browser doctor [--json]` を追加。通常は daemon の `GET /api/v1/browser/readiness` を取得し、daemon の設定・PATH・release による判定を一項目一行で表示する。`OK/NG/WARN/SKIP`、NG なし=0、不足=1、API 不達=2。config と API/token は引数・env で差し替え可能。CLI は DB を開かない。
- celeris が `DoctorConfig` を API の読み取り専用 callback に結線。worker と同じ `ledger_status` と configured release/path で台帳・版・backend 能力を判定し、固定 DNS 問い合わせ、launcher Hello、credentiald control Ping、公開鍵の読み取り、DB site policy の validate/config 差、grant の credential_use と policy ID の実在を確認する。同期 probe は API の spawn_blocking で実行する。
- credentiald に秘密・vault・lease を変更しない Ping を追加。既存 control と同じ UID/PID/starttime admission を通る。launcher の Hello は session を作らず、protocol 版違いと本番での試験用 loopback 許可を拒否する。
- readiness は bearer 認証必須（token 未設定の loopback でも匿名の probe は拒否）。未結線なら 503。HTTP client に接続・読み書きの期限を設け、CLI は transport error や token の値を出さない。
- `docs/ops/browser-prod-enablement.md` に台帳再生成・release 昇格・不足項目の修正・manaba の初回確認を記載。既存 `browser-prod.md` の内容は保持して新手順へのリンクを追加。host の有効化は運用セッションが行い、利用者の操作は web の owner session・初回 credential form・都度承認に限定する。

## 検証

- `cargo test -p celeris --lib browser_doctor_`: 5 件合格。一時 TOML/DB/実行ファイルと loopback DNS・偽 Unix socket を使用。不足各項目の一行・修正案、正常構成、古い台帳、launcher の試験許可、broker 拒否、壊れた鍵、config/DB 差と未知の grant policy を検証。
- `cargo test -p celerisctl --test browser_doctor`: 4 件合格。一時 config と偽 HTTP API で各 NG の一行出力、JSON、終了値 0/1/2、引数/env の上書き、DB を作らないことを検証。
- `cargo test -p task-api --test browser_doctor`: 3 件合格。bearer 必須、tokenless loopback の拒否、未結線の 503、daemon callback を実行することを検証。
- `cargo test -p celeris-credentiald --test browser_doctor`: 1 件合格。一時 HOME/XDG の実 broker に Ping を送り、PID 許可済みは成功、未許可は permission_denied、秘密・lease を返さないことを検証。
- `cargo clippy --workspace -- -D warnings`: 合格（最終変更後にも実行）。
- `cargo fmt --all --check`、`git diff --check`、新規・更新 ops 文書の `check-doc-links.sh`: 合格。
- `bash scripts/dev/test-parallel.sh`: 最終コードで 4761 passed / 0 failed / 14 ignored、tmp_leftovers=0（nextest 127.8 秒、doctest 9.7 秒）。初回も 4760 passed / 0 failed で合格。途中追加の tokenless 拒否試験と HTTP client 修正を含めて再確認した。

差分範囲: `CELERIS_WU_BASE=4d6572f566da28e51a8d33cd28cf234c4034fd73` からの全 20 ファイルが celeris/celerisctl/credentiald/task-api、対象 ops 文書、この進捗内であることを確認。web・他の葉のファイルに差分なし。

本番 host の変更・サービス操作・実 manaba 認証・本番台帳生成は実行していない。web UI の編集・検証は並行の web 葉と全体 close-out の担当。この葉では web ファイルを変更していない。
