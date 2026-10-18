---
status: done
completed: 2026-10-18
---
# trial-state WU
- ADR agent-docs/adr/2026-10-18-a-trial-scoped-state.md、Sandbox を auth_dir + state_root へ置換（旧 config_dir 削除）、main.rs 更新。
- 証拠: `cargo test -p celeris-campaign` 全 pass、`cargo clippy -p celeris-campaign --all-targets -- -D warnings` / `cargo fmt --all -- --check` exit 0。
- 実 LLM 呼び出しなし。preflight 側の再開検査は preflight-split WU。
- 提案: 認証は ro bind のため sandbox 内の token 更新は戻らない。長時間試行で問題が出たら別途検討。
