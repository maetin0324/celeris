---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
---
# hist: legacy routing history types

Moved the serialized event payload types into `task_core::routing_shadow_history` and updated the event and task-ops audit readers to use that module. Kept the existing producer module compiling through type re-exports. Added a redacted legacy event fixture with omitted optional fields and verified deserialization plus its estimator audit projection.

Check: `cargo test -p task-ops --lib legacy_shadow_history_fixture_deserializes_and_appears_in_audit` passed.
