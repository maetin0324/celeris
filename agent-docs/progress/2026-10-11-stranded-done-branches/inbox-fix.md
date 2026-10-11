# inbox-fix

## Cause

Final review found two defects in the受信箱 `Unintegrated` item (`crates/task-ops/src/human_inbox.rs`):

- The item id was `unintegrated-{task_id}`, with no repository part. A task with two unintegrated repositories produced two items with the same id. The answer path (`.find(|x| x.id == id)` in `crates/task-api/src/inbox_notifications.rs`) always picked the first one, so an answer on the second item could go to the first repository's route.
- The `discard` and `hold` options had `needs_note = false`. The inbox answer API therefore did not require a note. The route itself rejects an empty reason with 400, so the failure surfaced one layer too late.

## Changes

- `human_inbox.rs`: the id is now `unintegrated-{task_id}-{repo_id}`. `repo_id` is used instead of the sanitised repo name because it is unique by construction, while sanitised names could collide. The route path still comes from `native`, which already carries the repository name.
- `human_inbox.rs`: `discard` and `hold` are `needs_note = true`. The existing check at `inbox_notifications.rs` (`needs_note` → 400) now rejects a missing or blank note before the route is called. The body sent to the route is still `{"reason": note}`.
- No other code depends on the id format. I searched `crates/`, `web/features`, `web/app` and the fixtures for `unintegrated-` and found no other use. `cos/operations.rs` does not reference it either.
- `crates/task-api/tests/integration_api.rs`: added `integration_api_inbox_unintegrated_items_are_per_repo_and_need_note`. It uses a store fixture with one Done task and two project repositories, each unintegrated. No git fixture is needed because `discard` and `hold` only write store rows.

## Checks

- `cargo test -p task-api --test integration_api` — passed (5 tests, including the new `integration_api_` test). The new test checks:
  - two items with different ids, one per repository;
  - `discard` and `hold` report `needs_note: true`;
  - `discard` without a note, with a blank note, and `hold` with an empty note each return 400, and both repositories stay `unintegrated`;
  - `discard` with the reason `重複 branch` on the second item returns 200. Only repo-b becomes `discarded` with that reason. repo-a stays `unintegrated`. `events_for` contains `IntegrationDiscarded` for repo-b and none for repo-a. Only repo-a's item remains in the inbox.
- `cargo test -p task-api schema` — passed (3 tests). No schema change was needed.
- `cargo test -p task-ops` — passed (522 unit tests, 5 integration tests).
- `cargo clippy --workspace -- -D warnings` — exit 0.
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` — exit 0. nextest: 5155 passed, 0 failed, 14 ignored. Doctests passed.
- A first `test-parallel` run with the default long `TMPDIR` had 108 failures. All were `path must be shorter than SUN_LEN` or fixture-readiness timeouts in browser and credentiald tests. None touched inbox or integration code. Setting `TMPDIR=/tmp` cleared them.
- Diff against the WU base (`b7eb86dd`): only `crates/task-api/tests/integration_api.rs` and `crates/task-ops/src/human_inbox.rs` changed.

## Open items

- None for this WU. The web `InboxKind` and generated types are unchanged, so the web typecheck was not rerun.
