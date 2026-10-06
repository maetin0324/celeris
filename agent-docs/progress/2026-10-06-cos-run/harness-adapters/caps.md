# harness-adapters / caps: CoS 能力表と画像 delivery

---
tasks: [01M47VN94QK6KHXQDNPCA0V7KK]
status: done
completed: 2026-10-06
---

## 変更

- `task-worker::cos_chat` に 3 harness の継続方式と道具・画像能力表、画像 delivery の純関数、能力不足の reason を追加。
- `CosChatContext.harness_capabilities` は実行時に確認済みの能力だけを保持。未指定は未確認。prompt の添付行に `actual=native|path+tool|unsupported|path` を明記。
- dispatcher は未確認を `None` として渡す。adapter ごとの画像入力、tool 許可、session 継続は次の WorkUnit が配線する。

## 確認

- `cargo test -p task-worker cos_chat_harness_caps_ --lib`: 4 passed（能力表と native / path+tool / unsupported）。
- `cargo test -p task-worker cos_chat_run_proto_ --lib`: 6 passed（非 CoS の prompt と request JSON の不変を含む）。
- `cargo test -p task-worker --lib the_cos_conversation_run`: 3 passed（既存 Claude/Codex/ACP の CoS 対話制限）。
- `UPDATE_SCHEMA=1 cargo test -p task-worker committed_schema_matches_generated --lib`: 1 passed。
- `cargo check -p task-dispatch --all-targets`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`、文書リンク・ADR 番号検査: exit 0。
