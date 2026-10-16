---
title: "Jev 実 API の schema 不一致の修正"
tasks: [01M50XES8WJQ3MRVPEVQQQZHSK]
status: done
updated: 2026-10-15
completed: 2026-10-15
updated: 2026-10-15
---
# Jev 実 API の schema 不一致の修正

設計: `agent-docs/adr/2026-10-15-jev-wire.md` 付記（A1〜A5）。

## 変更
- choice の criteria を object（ラベル → 説明）に、verifier の noul から criteria を除去
- 4xx の fallback 記録に status・error.code・error.message（`client_error:400 code=… message=…`）
- `classify_http_status(400)` → `RequestRejected`（再試行しない）
- fake server が公式 schema を検査し旧形を 400 で拒否

## 証拠
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0（nextest 5370 passed / 0 failed / 13 ignored、doctest exit 0）
- 追加試験: `jev_fake_server.rs` の `classifier_questions_match_official_schema_and_array_criteria_is_rejected`・`fake_server_rejects_array_criteria_with_400`・`client_error_keeps_status_code_and_message_and_separates_400_from_402`、`classify_stage.rs` の 400/402 reason 文字列、`jev_verifier.rs` の `noul_ping_has_no_string_criteria`、`credential_resume_tests.rs` の 400 → RequestRejected

## 未解決事項
- 認証つき実呼び出しでの status は未確認（運用セッションで確認）。

## 本番への移行
- config の変更は無い。release の昇格後、classifier の記録が `jev_ok` になるか、`client_error:<status>` の詳細を見る（運用セッション）。
