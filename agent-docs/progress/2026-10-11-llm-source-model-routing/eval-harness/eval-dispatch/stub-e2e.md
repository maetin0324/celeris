---
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
---
# stub-e2e

Added `crates/celeris/tests/routing_eval_run.rs`, which starts a temporary Celeris daemon and submits two trials through the routing-eval HTTP API. The stub harness records its prompt and returns a fixed successful result; acceptance commands remain reserved for dispatcher scoring. The test checks terminal trial state, distinct worktrees and sessions, recorded harness version, eval decisions, outcomes, run rows, and no overlap with `reserved_runs = 1`.

The fixture uses `source_id = "none"`, matching the source that this fake provider resolves to, and `harness_id = "fake"`, matching the adapter used for dispatch. The fake adapter now accepts the fixed model as a no-op because stub execution does not call a model provider; the selected model remains recorded by the evaluation route. A PATH symlink exposes the stub under the adapter ID so the daemon's harness version probe records `eval-stub 1.2.3`.

Verification: `TMPDIR=/tmp/eds cargo test -p celeris --test routing_eval_run` passed. `cargo fmt` initially touched unrelated files owned by other WorkUnits; those changes were restored, and the final scope contains only this test, this progress note, and the fake adapter's model-binding support.
