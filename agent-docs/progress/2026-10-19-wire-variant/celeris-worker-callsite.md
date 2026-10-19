---
wu: celeris-worker-callsite
status: done
completed: 2026-10-19
---
# celeris-worker-callsite
- crates/celeris の RunContext 使用は `RunContext::default()`（daemon/adapters.rs:477）のみで literal なし。コード変更不要。
- `cargo check -p celeris --all-targets` exit 0。`cargo check --workspace --all-targets --exclude celerisctl` エラーなし。
- 残る制約: celerisctl は ctl-worker-callsite の担当。
