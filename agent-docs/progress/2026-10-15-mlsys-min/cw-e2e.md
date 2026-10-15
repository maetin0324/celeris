---
task: 01M4Z2B2HA2FEK1ZRWKS8MXSRR
unit: cw-e2e
status: done
completed: 2026-10-15
---
# cw-e2e: 待機→登録→検証→再開の結合試験（mock）

ADR 2026-10-15-mlsys-min-decision-assisted D4 の名前（`waiting_for_auth`・`secret_stored`・集約キー `credential:<secret_id>`）のまま。新しい名前なし。実装の欠陥は見つからず、本体コードの変更なし（試験のみ。`credential_gate.rs` 試験の helper を `pub(super)` にした）。

## 前提の補正
base（85a13e20）には cw-enter が入っていなかった（integrate-build-b が cw-resume だけ merge）。cw-enter ブランチを衝突なしで merge してから試験を書いた（`merge cw-enter into cw-e2e`）。integrate-build-c では二重取り込みにならない（祖先）。

## 試験
- `crates/task-dispatch/src/dispatcher/tests/credential_e2e.rs`（一時 DB・`Scripted` mock 検証・注入時計・外部 network/LLM なし）8 件:
  - 未登録→該当 unit だけ待機（兄弟・task Status・run は触れない）→受信箱 item 1 件（3 task 集約、用途・/accounts・自動で続行）→task が増えても item は 1 件のまま→保存→検証 1 回→3 unit が一度だけ再開→item 消える→何度 tick しても増えない。
  - 誤キー: 同じ item（id・created_at 不変、題名・文面が「更新してください」）、再検証なし、更新で再開。
  - 429（Transient）: 待機維持、item は「登録待ち」のまま、注入時計の backoff 後に再開。
  - daemon 再起動: 途中落ちの wait を補完、新 resumer が続け、再々起動でも二重再開なし。
  - event 取り逃し: ファイルだけで reconcile 再開、遅れて届く重複 event は無害。
  - 重複 event 5 回 + 3 resumer 並行: 再開ちょうど 1 回。
  - cancel 済み task: 再開しない（wait は cancelled）、同じ鍵を待つ生きた task は再開。
  - 値が出ない: 誤キー・権限不足・一時障害・有効の 4 値を順に登録し、wait 行・受信箱・event・work unit・report の Debug、および一時ディレクトリの SQLite ファイル（-wal 含む）のバイト列に値が無いことを検査。
- （cw-notify-e2e で 1 本の通し試験に置換済み）旧 `credential_wait_notify.rs` 2 件（偽 webhook = loopback TcpListener）: 2 task が同じ鍵を待つ→受信箱 item 1 件→escalation outbox（直経路。cw-direct で triage を経ない形に差し替え）→Discord へ 1 通だけ送信（/accounts リンク・「自動で続行」、値なし）。登録→検証→再開した後は未送信の通知が送る前に取り下げられる。

## 証拠
- `cargo test -p task-dispatch credential_e2e`: 8 pass。`cargo test -p celeris --test credential_wait_notify`: 2 pass。
- `bash scripts/dev/test-parallel.sh`: exit 0、5192 pass / 0 fail / 13 ignored。
- `cargo clippy --workspace -- -D warnings`: exit 0（`--tests` 付きも exit 0）。

## 未解決・提案
- （解消済み: cw-direct）通知は triage を経ず outbox 直経路。
- 「待機中 worker run なし」は gate 直呼びで確認（dispatch の tick 全体は通していない）。
- ログ（tracing）への値の不出現は構造上（値を読んで log に渡す経路が無い）で、tracing 出力の捕捉試験はない。DB・event・item・report・error 型・SQLite ファイルで確認。
