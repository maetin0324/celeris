---
title: "export-cli: celerisctl campaign export"
tasks: [01M4Z716SMR90GX0WDEHA9B02Z]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# export-cli（unit）

`celerisctl campaign export <campaign-id> --out <dir> [--manifest-hash H]` を足した（`crates/celerisctl/src/commands/campaign.rs`）。`SqliteStore::decision_log_export_campaign`（読み出しのみ）の結果を `records.jsonl` と `summary.json` に書く。DB は書かない。

- 秘密: 出力前に全 string 欄を走査し、既知接頭辞（sk- / ghp_ 等）の長い token、`api_key=…` 型、`Bearer …` を `[redacted]` にする（`fallback` 理由などの自由文対策）。
- 決定的: 実行日・乱数を使わない。同じ DB から 2 回出すと byte 一致（試験で確認）。
- campaign 未実装の間は、行の `campaign_id` に記録された値（root task id 等）をそのまま campaign id に使う。

## 証拠

- `cargo test -p celerisctl --test campaign_export`: 1 passed（行数、再処理・別 campaign で二重計上なし、欠測 null・実支出を定価で代用しない、2 回の出力が byte 一致、秘密が出力に無い）。
- `bash scripts/dev/test-parallel.sh`: 5168 passed, 0 failed。
- `cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決事項

- `export_cursor` は None のまま（入力行に seq が無いため）。
- 行の `campaign_id` が None の run は export 対象外。root task id を campaign id として使うには、writer（log-wire）が `campaign_id` に root id を入れる必要がある（次 unit / log-verify で確認）。

## 提案

- campaign 実装時に `campaign_id` の書き手を一本化する。
