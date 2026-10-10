---
tasks: [01M4JJ5S8NJBG76NJ74B8B6TK0]
status: completed
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
