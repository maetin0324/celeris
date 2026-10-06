# CoS chat run 添付 stage

`attach` WorkUnit は `ChatAttachmentStore` の検証付き読出しを使い、message の添付を thread 専用 workspace に複写する。すべての添付元を検証してから stage を開始するため、欠落・ハッシュ不一致では stage 先を作らない。

- 作業場所は `<data_dir>/cos/threads/<thread_id>/workspace`。thread id は英数字だけを受け入れ、経路の symlink と `..` を拒否する。
- 添付は `workspace/attachments/<id>/<安全化した原名>` に置く。元名は manifest に保持し、経路には安全化した leaf だけを使う。stage 後は file `0400`、添付個別 dir `0500`。再配送時は既存 file の SHA-256 とサイズを再照合する。
- manifest は `id/name/media_type/size_bytes/sha256/path/delivery`。JPEG、PNG、WebP、GIF は `image`、その他は `file`。
- `cos_chat_run_attach_` の試験で一致、欠落、不一致、symlink、thread 不一致、原名の安全化、delivery の区別を確認する。

この leaf では dispatcher の起動経路への接続は行わない。後続の統合工程が `stage_message_attachments` と `workspace_dir` を呼ぶ。

## 再試行（attempt 2, 2026-10-06）
- 前回の check 不合格: clippy の `cloned_ref_to_slice_refs`（tests.rs）と範囲外の Cargo.toml/Cargo.lock（sha2 依存の追加）。
- 直し: sha2 の直接依存をやめ、既存依存 task-worker の `artifact::sha256_file` で stage 後のファイルを照合（`verify_staged`）。試験は `std::slice::from_ref`。
- 証拠: `cargo fmt --all -- --check && cargo clippy -p task-dispatch --all-targets -- -D warnings` exit 0、`cargo test -p task-dispatch --lib cos_chat_run_attach_` 5 passed、範囲 check の出力なし。
