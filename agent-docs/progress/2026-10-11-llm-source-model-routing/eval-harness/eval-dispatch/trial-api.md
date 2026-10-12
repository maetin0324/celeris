# eval-dispatch: routing-eval trials API

- task: `01M4Q7C4X2QR3HYPTTQAG5GXQ9`
- Added `POST /api/v1/routing-eval/trials`, `GET /api/v1/routing-eval/trials`, and `POST /api/v1/routing-eval/trials/{trial_id}/cancel`.
- The API validates known source/model/harness IDs, repository presence, batch shape, and cancellation state. Response views omit the hidden check command and worker objective.
- Added API tests for idempotent submission, cancellation conflicts, and absence of `check_cmd` in GET output.
- Updated the API schema and generated web/GUI types. The GUI output is present; the local package-manager version prevented invoking its pinned generation command in this run.
- Strict catalog validation rejects unknown source/model/harness IDs even when a catalog is empty (fail closed).
- Validation: `cargo test -p task-api routing_eval -- --nocapture` (5 unit tests), `cargo test -p task-api --test routing_eval` (6 API tests), `UPDATE_SCHEMA=1 cargo test -p task-api schema::tests::committed_schema_matches_generated --lib`, and both pinned `gen:types` commands passed.
