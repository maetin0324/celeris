---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# sweep-core

- `task-ops::stranded::sweep` accepts daemon-resolved task/repository branch records and an injected timestamp, checks `git merge-base --is-ancestor`, records unresolved reasons, and skips held/discarded states. The store emits events only for state transitions.
- Added `AttentionItem::Unintegrated`, surfaced it in the human inbox with merge/discard/hold action links, and suppresses it when an open integration request already exists.
- Added an inbox regression test proving one persisted unintegrated task state produces exactly one `unintegrated` attention item.
- Removed `Skip::Silent` from delivery. Existing same-head delivery is now a durable `DeliverySkipped` reason.
- `cargo test -p task-ops stranded_ --lib` — passed (2 tests): a temporary git fixture detects an unintegrated task, repeats without a duplicate event, then recognizes the fast-forwarded head as integrated; inbox state yields exactly one item.
- `sh "$CELERIS_WU_SCOPE_PATHS"`, `git diff --check`, and the `Skip::Silent` absence check — passed.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed.
- Remaining integration: `sweep-daemon` must resolve DB worktree/repository records and schedule this function. API/CLI operation routes and task filtering for root/repo-changing candidates are later units.
