---
tasks: [01M4HRQBN8XHKQRD1DAXHXBSEX]
wu: post-login-flake
status: done
completed: 2026-10-10
---
# post-login 試験の download 待ちを出来事待ちにする（integrate-close の偽の失敗）

## 経緯
integrate-close の全体試験で `browser::post_login_tests::daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages` が
「other-origin download cancelled」で落ちた（5015 passed / 1 failed）。crates/ に差分の無い段で、integrate-impl では合格。
旧 `wait_download(from, state)` は guid を見ず、`from` 以降の任意の download の `state` を 10 秒だけ待っていた。
2 本目（他 origin、fixture は header 後 2 秒で body）の `downloadWillBegin` が負荷で 10 秒内に届かないと落ちる。

## 変更（試験 helper のみ。本番 code は不変）
- `crates/task-worker/src/browser_post_login_tests.rs`
  - `Agent::wait_event`: `seen` の `from` 以降を走査し、無ければ pump して出来事を待つ。保険の上限は `DOWNLOAD_EVENT_TIMEOUT` = 60 秒（ADR-0125）。
    controller が観測を止めた（D2-6 の breach: 取消が間に合わず完了）ら待たずに終わる。
  - `wait_download_begin(from, known)`: `Browser/Page.downloadWillBegin`（または `known` に無い guid の progress）から guid を返す。
  - `wait_download(from, guid, state)`: その guid の progress が `state` になるまで待つ。
  - `tail(n)`: 失敗 message 用の末尾。
  - 試験側: 1 本目は begin の guid の `completed` を待ってから次の `from` を取る。2 本目は begin の guid（1 本目を除く）の `canceled` を待つ。
    失敗 message に `observation_stopped()` を出し、取消が間に合わず完了した（本物の欠陥・競合）場合と区別できるようにした。
- `crates/task-worker/src/browser_launcher_run_tests.rs`（`launcher_credential_post_login_pair_login_then_reads_only_the_lms`）: 同じ helper で同じ形に直した。

取消の出来事: `Browser.downloadProgress` は browser-level（sessionId なし）なので `take_agent_events_for` で agent に渡る。試験内に他の消費者は無い。

## 証拠
- `cargo clippy -p task-worker --tests -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `TMPDIR=/tmp/plf.XXXX cargo nextest run -p task-worker --lib -E 'test(daemon_post_login_pair_login_reads_lms) | test(launcher_credential_post_login_pair_login_then_reads_only_the_lms)'`
  → 2 passed（8.2 秒）。run の長い TMPDIR では launcher 試験が SUN_LEN で bind に失敗する（既知、変更と無関係）。
- 範囲: `git diff --name-only $CELERIS_WU_BASE` は crates/task-worker/src/ の 2 file と本進捗のみ。

## 未解決
- 負荷下の失敗が「2 本目の開始が遅いだけ」か「取消が間に合わず完了 → 観測停止」かは、旧 log からは区別できない。後者なら新 helper は
  `observation stopped: true` で即失敗する（試験は緩めていない）。再発時はその表示で判定する。
