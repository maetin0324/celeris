# routes-wire

## Cause

`task_ops::view::attach_integrations` had no production caller, so task summary and detail responses omitted the durable per-repository integration state and promotion view. The four human integration routes were also missing from the CoS mutation registry.

## Changes

- Passed `[selfdeploy] releases_dir` from the daemon into `ApiState` for task integration views.
- Task list and detail handlers now build an `IntegrationContext` from the task's project repository rows and attach integration and promotion data.
- Added an `integration_api_` HTTP test proving an unintegrated row appears in both detail and list responses.
- Classified the four integration routes as CoS `EXCLUDED` with `integration_human_decision` reasons in both the registry and ADR mutation tables. The route handlers require human admin credentials; merge performs Git filesystem effects, and the dispositions are explicit human decisions without transaction-aware CoS operation variants.
- Extended the deterministic API fixture with a local Git repository and release manifest. It verifies `unintegrated` on detail and list, then `integrated` plus `promoted` on detail.
- Updated the byte-for-byte task detail fixture to account for the API always attaching an empty integration list when no rows exist.

## Checks

- `cargo test -p task-api cos_ops_registry` — passed (2 tests, including ADR assignment/exclusion matching).
- `cargo test -p task-api --test integration_api integration_api_` — passed (4 tests).
- `cargo test -p task-api schema` — passed (3 schema tests, including committed schema match).
- `cargo test -p task-api --test list_and_detail detail_matches_task_ops_task_detail_byte_for_byte` — passed.
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` — passed (5,144 nextest tests; doctests passed; 13 nextest tests and 1 doctest ignored). Setting a short temporary root avoids unrelated Unix socket `SUN_LEN` failures from the run's long default path.
