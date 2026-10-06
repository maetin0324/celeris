# CoS chat harness adapters: 結合と検証

---
tasks: [01M47VN94QK6KHXQDNPCA0V7KK]
status: completed
completed: 2026-10-06
---

## 実装

- `task-dispatch` の chat queue → launch → 実 `ClaudeCodeAdapter` / `CodexAdapter` / `AcpAdapter` → sink → SQLite の経路を偽 CLI・偽 ACP で試験した。各 harness で初回と 2 run 目の resume、text/tool/status の `chat_events` を確認した。
- Claude は image block、Codex は `--image`、ACP は交渉後の image block を確認した。ACP の画像能力が無い場合は `unsupported` と `image was not inspected` が prompt に残る。
- launch で Claude/Codex の実装済み画像能力を context に接続した。ACP の能力は実 agent の応答まで未確認のまま保つ。偽実行ファイルの書込みは別プロセスに任せ、ETXTBSY を避けた。
- 非 CoS conversation と通常 WU continuation の分岐は変更していない。

## 検証

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-dispatch cos_chat_harness_e2e_ --lib` | 3 passed、0 failed |
| `sh scripts/dev/check-doc-links.sh` | exit 0 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0 |
| `sh scripts/dev/progress-index.sh --check` | exit 0 |
| `CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh` | exit 0。nextest 4168 passed / 0 failed / 12 skipped、doc test 0 failed / 1 ignored。134 binaries、`summary_parsed=true`、352.5s + 7.9s。ログは run の `test-parallel.log` |
| `cargo clippy --workspace -- -D warnings` | exit 0（51.3s） |
| `cargo clippy -p task-dispatch --all-targets -- -D warnings` | exit 0。結合試験コードも lint 済み |
| `cargo fmt --all -- --check` | exit 0 |

## 未解決・提案

現時点で機能上の未解決事項は無い。実 CLI・実 ACP agent との互換性は、この run では偽プロトコルでの確認に限る。実環境で利用する CLI の版と ACP capability 応答は運用時に確認する。
