# task-features store WorkUnit

tasks: [01M4NM332S384YYC4VC5DB7KCY]

Implemented migration `0069_route_records.sql` (schema version 69; versions 67 and 68 remain reserved for parallel migrations) creating the five ADR D7 append-only tables: `route_features`, `route_options`, `route_decisions`, `route_outcomes`, and `route_judgments`.

`SqliteStore` now has append APIs for all five record types. Feature and options inserts are content-addressed and idempotent; decision, outcome, and judgment inserts reject duplicates through primary keys. Outcome is unique per decision. Read APIs fetch features/options by hash, decision/outcome/judgment by id, judgments by feature/options hash pair, and decisions with joined features/options/outcome filtered by inclusive timestamp bounds and stage in `decision_id` order.

Verification:

- `cargo test -p task-core route_store_ -- --nocapture`: 3 passed. Covers pinned schema version, existence of all five tables, content insert idempotence/readback, duplicate decision/outcome refusal, stage filtering, and deterministic export order.
- `cargo test -p task-core store::client_open_tests -- --nocapture`: 6 passed.
- `git diff --check`: passed.

Retry repair: the prior run failed `cargo fmt --all -- --check` because `store/mod.rs` was not rustfmt-formatted. Ran `cargo fmt --all`; workspace formatting and clippy now pass. Clippy then identified `decision_row` below the test module; moved it above the tests and reran successfully.
