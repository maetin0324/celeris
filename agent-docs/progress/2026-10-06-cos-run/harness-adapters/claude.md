# harness-adapters / claude: CoS chat の Claude Code 写像

---
tasks: [01M47VN94QK6KHXQDNPCA0V7KK]
status: done
completed: 2026-10-06
---

## 実装

- `RunContext.cos_chat` がある run だけ Secretary の `--allowedTools` 制限を外す。非 CoS の対話には従来の read tool 6 件を残す。
- session の初回 `--session-id <UUID>`、後続 `--resume <UUID>`、拒否検出は既存経路を共用する。
- 確認済み能力が native image input を示す画像は `--input-format stream-json` の user message に base64 image block と media type を入れる。path+tool と unsupported は共通層の prompt の理由を残し、画像 block を送らない。native のファイルが欠ける、サイズが違う、型が不正な場合は起動前に失敗する。
- CoS の stream-json thinking は公開 `summary` 欄だけを進捗に出す。text/tool_use/tool_result は既存の構造化進捗に写す。

## 確認

- `cargo test -p task-worker --lib claude_code::tests::`: 106 passed。`cos_chat_harness_claude_` 7 件で session 初回・resume・拒否、道具制限、native/path/unsupported 画像、イベント写像を確認。
- 同じ 106 件に非 CoS の Secretary allowlist と通常 WU continuation の既存試験を含む。
- `cargo clippy --workspace -- -D warnings`: exit 0。
