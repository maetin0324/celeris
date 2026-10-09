# browser credential 台帳判定の証拠完全性

---
tasks: [01M4F73EKB2FFGAKRY2041RZ49]
---

## 実装

- `task_core::browser_backend` に ADR D1 の P4-A isolation 19 件と egress negative 21 件を登録した。
- `required_evidence` が P4-A の全試験を返し、既存 `case_passed` が要求名の全件 `passed` と同一 case 内の失敗・未実施なしを確認する。したがって case 名だけの旧 ledger は credential / identity restore を認定しない。
- task-worker の台帳試験で、全証拠が通った場合だけ `credential_backends` に `claude-code` が入り、欠落・failed・not_run の場合は公開 backend を維持しつつ credential を空にし、`task_code(true)` が `ledger_lacks_credential` になることを固定した。
- celerisctl JSON 試験でも、suite 名だけの ledger は `backends` を維持し `credential_backends` を空で返す。
- task-worker、task-api、celeris doctor の既存 credential fixture に P4-A の証拠を追加した。

## 検証

- `cargo test -p task-worker browser_ledger_credential_ --lib` — 成功（1 passed）。
- `cargo test -p celerisctl --test browser_ledger_release browser_ledger_release_` — 成功（8 passed）。
- `cargo test -p task-core browser_backend::tests:: --lib` — 成功（8 passed）。
- `cargo clippy --workspace -- -D warnings` — 成功。
- `cargo fmt --all`、`git diff --check` — 成功。
- `git diff --name-only "$CELERIS_WU_BASE"` は上記の台帳判定・関連 credential fixture 6 ファイルのみ。
