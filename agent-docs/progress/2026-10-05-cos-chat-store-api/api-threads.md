---
tasks: [01M46XAR6KD97KMWBX2D6WW6TJ]
unit: api-threads
status: done
completed: 2026-10-05
---
# CoS chat の thread/message/run REST API

`crates/task-api/src/chat/mod.rs` に ADR D2 の thread 作成・一覧・検索・詳細・更新、message 一覧・投稿・取消、run 詳細・stop、queue 再開を配線した。ハンドラは `SqliteStore` の chat 操作を `spawn_blocking` で呼び、LLM 呼び出しや run 起動はしない。書込み時には chat event の live 購読を起こす `Notify` を呼ぶ。作成の再送は 200、stop の終端再送は 200、それ以外の各状態は store の `ChatError` から ApiProblem に写す。

`ChatState` は添付の data dir（未設定は `None`）・実 DB path・`ChatAttachmentLimits`・live 通知・CoS の enabled 判定をまとめて `ApiState` に置いた。`with_chat_attachments` は後続 wire 葉、`with_cos_enabled` は後続 cos-run 葉が使える。現時点の enabled は true。CoS 無効なら新規 message 送信だけ 503 にする判定は `post_message` の一箇所。既存の認証は管理 bearer だけなので chat の閲覧・書込みとも `require_admin` を使う。thread 冪等 scope は現行の単一管理者に対応する `admin` 固定値。

後続の兄弟葉に向けて `chat/stream.rs` と `chat/attachments.rs` は空 Router の stub とし、`chat/mod.rs` が両方を merge する。両葉は自分の route file だけを編集できる。`schema.rs` と `docs/api/v1` は後段の schema 葉が更新する。

## 確認

- `cargo check -p task-api`: exit 0。
- `cargo test -p task-api --test chat_api`: 5 passed。仮 run は store で claim し、実 worker を起動していない。待機や CPU 負荷はない。
- `cargo clippy -p task-api --all-targets -- -D warnings`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh`: 3987 passed、0 failed、13 ignored（doc test 込み）、exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 申し送り

- cos-run: `[cos]` の disabled 設定を `ApiState::with_cos_enabled(false)` に渡す。既存の CoS run の drain と Console 互換 facade は本葉では扱わない。
- api-sse: `state.chat.events` を live 更新通知に使う。store の event 行を購読前後に cursor で再読し、通知取りこぼしを補う。
- api-attach: `state.chat.attachment_data_dir`、`attachment_db_path` と `attachment_limits` を使う。wire 葉が data dir を渡すまでは未設定。
