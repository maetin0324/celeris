# acp-fixture

tasks: [01M4SRWPFY0E50F6QF0GQV877X]

- Failure cause: `acp::tests::routing_context_ref_propagates_and_rejects_spoofing` failed at its `default-config` block (tests.rs:233). That block cleared `env`, so no proxy `baseURL` was visible and `configure_acp` (worker-route change f1fccabc) correctly attached no header. The test fixture was wrong, not the transport.
- Fixed the fixture: the `default-config` run now puts `baseURL` (`http://127.0.0.1:18100/v1`) in `OPENCODE_CONFIG_CONTENT`. The spoofing and ref-propagation assertions are unchanged.
- Added a negative case in the same test: a proxy provider without `baseURL` gets no `x-celeris-routing-context` header and records `context_transport: unsupported`, even when a daemon ref is present.
- Replaced the tier alias in the two OpenCode routing fixtures (`celeris-proxy/celeris/cheap`) with `celeris-proxy/qwen/qwen3.8-27b`. Those tests check headers only, so the model name does not change their meaning.
- Reviewed the sibling fixtures named in the objective. `the_task_tool_override_composes_with_the_routing_context_overlay` already has a `baseURL` in its inline config, so it needed no change. `aider::routing_context_ref_propagates_to_aider_proxy_settings` already sets `OPENAI_API_BASE`, and the aider transport ignores the model name. Its `openai/celeris/cheap` model string is left for the `worker-fixtures` unit, which owns `celeris/<tier>` in task-worker tests.
- Verification:
  - `cargo test -p task-worker --lib routing_context` → 6 passed, 0 failed (includes `routing_context_ref_propagates_and_rejects_spoofing`).
  - `cargo test -p task-worker` → exit 0; lib 1071 passed, 0 failed, 4 ignored; all other test binaries 0 failed.
  - `cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0.
  - `cargo fmt -p task-worker -- --check` → exit 0 (rustfmt touched only the edited lines).
- Crate-wide tier check: `grep -rnE 'celeris/(frontier|standard|cheap)' crates/task-worker` still listed the `worker-fixtures` files on this branch, because that unit's commit (5c239786) was not in this branch's base. Cherry-picked 5c239786 onto this branch (clean, no conflicts). Now the grep prints nothing and the check exits 0. The later integration merge sees the same content.
- Verification (this run, after the cherry-pick):
  - `! grep -rnE 'celeris/(frontier|standard|cheap)' crates/task-worker` → exit 0.
  - `cargo test -p task-worker --lib routing_context` → 6 passed, 0 failed.
  - `cargo test -p task-worker` → exit 0; lib 1071 passed, 0 failed, 4 ignored; other binaries 0 failed.
  - `cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0.
  - `cargo fmt -p task-worker -- --check` → exit 0.
