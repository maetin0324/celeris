---
task: 01M4Z298AH2603BFE5DHKNDKQ9
unit: cw-direct
status: done
completed: 2026-10-15
---
# cw-direct: credential 待ち通知を CoS triage から外し outbox 直経路へ

ADR: `agent-docs/adr/2026-10-15-credential-wait-skip-triage.md`。

## 変更
- `task-core::chat::triage`: `cos_triage_direct_escalate`（escalated の item + escalation outbox を 1 transaction・冪等）。
- `task-dispatch::cos_chat::triage`: `credential_wait` を triage 入力（reconcile・events）から除き、直送する。
- `celeris::notify::triage::render`: packet の `headline` で見出しを上書き可。
- 試験: `credential_e2e` に tick 全体の試験を追加し、旧い triage 代用（`offer_to_triage`）を削除。`credential_wait_notify.rs` を直経路に差し替え。
- 旧記述: credential-wait.md / cw-e2e.md / cw-enter.md の「triage を 1 回経由する」を解消済みに更新。

## 証拠
- `cargo test -p task-dispatch credential_`: 28 passed（tick 全体の試験を含む）。
- `cargo test -p celeris --test credential_wait_notify`: 2 passed。
- `bash scripts/dev/test-parallel.sh`: exit 0（5193 passed, 0 failed, 13 ignored）。`cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決事項
- tracing ログの値不出現のログ捕捉試験は cw-logcap の担当。
