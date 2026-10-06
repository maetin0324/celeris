# CoS chat 添付保存層

- WorkUnit: `attach-store`
- ADR: `agent-docs/adr/2026-10-05-cos-chat-home.md` D4
- `ChatAttachmentStore` は daemon の data dir 配下 `chat/attachments/<id>/blob` に保存する。原名は DB の表示情報だけにし、ULID のみを path に使う。staging と blob は同一 filesystem 内で atomic rename し、directory 0700・file 0600 にする。
- 読み込みは固定長バッファで byte 数を数え、上限超過時に停止する。SHA-256 は受信時と worker への読み渡し時に照合し、MIME は先頭 magic から検出する。
- SQLite の即時 transaction で既存 ready blob と有効 upload 予約の合計を検査する。完了・abort・lease 切れで予約を解放する。同一 `client_upload_id` の同名・同内容再送は同じ ID、異なる内容は衝突とする。同一 hash の別 upload は別 ID にする。
- `message/task/knowledge_inbox` の参照、参照ありの削除拒否、実行中 run の message 参照解放拒否、未送信 24 時間・最後の参照解放後 30 日の GC を実装した。crash で残った staging/blob も GC で回収する。
- 一時 SQLite/dir の `chat_attach_*` 試験 12 件は、容量境界、同時予約、冪等、hash・権限、symlink と `..`、参照と注入時計による GC を確認した。
- 検証: `cargo test -p task-core chat_attach_` 7/7、`cargo clippy --workspace -- -D warnings` 合格、`cargo fmt --all -- --check` 合格。`CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh` は 3,953 passed・13 ignored（doc-test を含む）・0 failed。全体検査後に足した 2 件の添付試験は個別で確認した。

## 再実行（attempt 2, 2026-10-05）

- 前回の check 不合格: `chat_attach_` の試験が 7 件（≥10 が条件）、`cargo clippy -p task-core --all-targets` が `cloned_ref_to_slice_refs` で失敗。
- 直したこと: 大きな試験 2 件を観点ごとに分け（hash・権限、ファイル上限ちょうど/超過、magic 不明、改ざん検出、冪等、メッセージ件数・別スレッド、参照と注入時計の GC）、`std::slice::from_ref` に置き換えた。
- 証拠: `cargo test -p task-core chat_attach_ -- --list` 12 件（check exit 0）、`cargo test -p task-core` 738 passed・0 failed、`cargo clippy -p task-core --all-targets -- -D warnings` exit 0、`cargo clippy --workspace -- -D warnings` exit 0。
