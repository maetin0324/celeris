# Dispatch feature snapshot record

tasks: [01M4NM332S384YYC4VC5DB7KCY]

At worker routing time, dispatch extracts the task or active WorkUnit snapshot through `features_for_subject`, stores it through `insert_route_features` (content-addressed by the feature hash), then adds `{schema, template, hash}` to `RoutingRecord.features`. Extraction/store failures emit a warning and leave the optional reference absent while dispatch continues. The brief is never copied into the event. Older routing events continue to deserialize with `features` absent.

The `route_dispatch_records_feature_reference_and_snapshot` test verifies `celeris.task-features/1`, `task-brief/1`, and equality between the event hash and the stored `route_features` row.

Schema generation completed for `docs/api/v1/api-v1.schema.json` and `docs/api/v1/event.schema.json` using their existing `UPDATE_SCHEMA=1` tests. GUI `gen:types` could not run: the available Corepack pnpm shim invokes v12.6.0 while the project pins 11.27.0; invoking cached 11.27.0 directly then failed opening its configured package-store SQLite database. `docs/protocol` has no routing event schema; the event contract is `docs/api/v1/event.schema.json`.
