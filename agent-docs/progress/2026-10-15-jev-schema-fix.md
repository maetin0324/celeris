---
tasks: [01M50XES8WJQ3MRVPEVQQQZHSK]
status: done
completed: 2026-10-15
---
# Jev 実 API の schema 不一致の修正

設計: `agent-docs/adr/2026-10-15-jev-wire.md` 付記（A1〜A5）。

## 変更
- choice の criteria を object（ラベル → 説明）に、verifier の noul から criteria を除去
- 4xx の fallback 記録に status・error.code・error.message（`client_error:400 code=… message=…`）
- `classify_http_status(400)` → `RequestRejected`（再試行しない）
- fake server が公式 schema を検査し旧形を 400 で拒否

## 証拠
- `cargo fmt --all -- --check` / `bash scripts/dev/test-parallel.sh` / `cargo clippy --workspace -- -D warnings`（結果は task の完了報告に記載）

## 未解決事項
- 認証つき実呼び出しでの status は未確認（運用セッションで確認）。

## 本番への移行
- config の変更は無い。release の昇格後、classifier の記録が `jev_ok` になるか、`client_error:<status>` の詳細を見る（運用セッション）。
