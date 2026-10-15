---
task: 01M50Y0B625RX15CXYY3S17YQ6
status: done
completed: 2026-10-15
---
# verify の件数照合が 1 node 500 件超の reports で落ちる問題

## 内容
- ADR 付記 `agent-docs/adr/2026-10-15-verify-counts-without-page-limit.md`（D1〜D4）: (a) を選択。
- `GET /api/v1/reports/count`・`GET /api/v1/messages/count` を追加（task-api）。
- `scripts/selfdeploy/verify.sh` の collect は一覧ではなく件数の口で数える。口が失敗したら errors に積み検査 2 を落とす。
  旧い node ごとの一覧走査・500 上限・capped 判定は削除。n-1-compat は `N1_COUNT_KEYS` で reports/messages を照合しない。

## 証拠
- `cargo test -p task-api --test reports`: 5 passed（`count_endpoints_count_every_row_beyond_the_page_limit`: reports 1723・1 node 650、messages 650）
- `bash scripts/selfdeploy/tests/verify_web_app_start.sh`: exit 0
- `cargo fmt --all -- --check`: exit 0
- test-parallel / clippy: 完了報告に記載

## 本番への移行
無し（config 欄の変更なし）。

## 未解決事項
- 本番 DB 写しでの verify 実行は運用セッションの昇格時に行う。

## 提案
無し。
