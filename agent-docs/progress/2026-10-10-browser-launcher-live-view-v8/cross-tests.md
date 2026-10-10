---
tasks: [01M4JJ5S8NJBG76NJ74B8B6TK0, 01M4JM350YVRFTS7WQS01J0CD3]
status: done
updated: 2026-10-10
completed: 2026-10-10
---
# Browser launcher Live View cross layer tests

## 試験一覧

`crates/task-worker/tests/browser_live_cross.rs` に次の 5 prefix を実装。

- `browser_launcher_v8_daemon_v7_launcher_`: v7 launcher で live を無効化し、credential login/action を継続
- `browser_launcher_v7_daemon_v8_launcher_`: 古い daemon が v8 launcher に frame 接続を要求しない
- `browser_live_frame_auth_section_owner_only`: auth section 中も owner slot は frame を受け取り、LiveEmitter の sink と tool result は frame を含まない
- `browser_launcher_live_view_rejects_input_`: frame 経由の入力要求を拒否し CDP 入力 command を送らない
- `browser_live_frame_no_persist_`: marker frame を slot で受け、LiveEmitter sink、tool result/artifact、launcher tempdir/artifacts の file scan で marker/frame bytes を検査

実 browser は使わない fake launcher × fake CDP cross test のため、userns opt-in 実 browser variant はこの file にありません。tracing subscriber の log capture もこの WorkUnit では追加していません。永続性 test は frame relay が記録物を生成しない対象（tempdir/artifacts）を走査しています。

## 実行結果

- `TMPDIR=/tmp cargo nextest run -p task-worker --test browser_live_cross`: 5 passed, 0 skipped
- `TMPDIR=/tmp cargo test -p task-worker --lib -- browser_launcher browser_live`: 60 passed, 0 failed（983 filtered）
- `cargo clippy -p task-worker --all-targets -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

本体修正なし。fake fixture の追加公開なし。未解決事項: tracing log capture と userns opt-in 実 browser variant は未実装。

## task-api 層（api-cross）

`crates/task-api/tests/browser_live_cross.rs` に次の試験を追加・実装。

- `browser_live_frame_no_persist_`: API stream で本人に frame を届け、DB/WAL/SHM・全表・tracing log capture・artifacts/tempdir の走査で marker と frame bytes が無いことを確認。
- `browser_live_frame_auth_section_owner_only`: credential auth section 中も本人だけが frame を受信し、他 owner/session/task/run と agent の永続 event 経路を拒否。
- `browser_live_view_existing_takeover_policy`: ADR-0080 D6 / ADR-0100 D2 の本人 grant と非 owner 拒否を固定。task-api の input/takeover endpoint は 404 で拒否され、CDP `Input.*` command が送られない。

## 横断 prefix の実行結果

以下は各コマンドを個別実行。すべて exit 0、各 1 passed / 0 failed。

- `TMPDIR=/tmp cargo test -p task-api --test browser_live_cross browser_live_frame_no_persist_`
- `TMPDIR=/tmp cargo test -p task-api --test browser_live_cross browser_live_frame_auth_section_owner_only`
- `TMPDIR=/tmp cargo test -p task-api --test browser_live_cross browser_live_view_existing_takeover_policy`
- `TMPDIR=/tmp cargo test -p task-worker --test browser_live_cross browser_launcher_v8_daemon_v7_launcher_`
- `TMPDIR=/tmp cargo test -p task-worker --test browser_live_cross browser_launcher_v7_daemon_v8_launcher_`
- `TMPDIR=/tmp cargo test -p task-worker --test browser_live_cross browser_launcher_live_view_rejects_input_`

## 全体検査

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0。nextest 5054 件中 5054 passed / 0 failed / 13 skipped、doc test は 3 passed / 0 failed。集約 `CELERIS_TEST_SUMMARY`: 5057 passed / 0 failed / 14 ignored。既知の sandbox 失敗なし。
- `cargo clippy --workspace -- -D warnings`: exit 0。

本 WorkUnit で本体修正なし。task-api に操作 route を追加せず、input/takeover の拒否を API 境界でも固定した。
