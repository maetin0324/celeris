---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: e2e API cooldowns use source and account IDs
updated: 2026-10-11
---
# e2e API cooldowns use source and account IDs

`/daemon` の cooldown view を `source`・`account_id` に切り替え、暗黙 account は source ID を account ID として公開するよう変更した。API schema と gui/web 型を再生成した。API scenario は fake source の暗黙 account を照合する。

## Checks

- `UPDATE_SCHEMA=1 cargo test -p task-api schema`: unit schema checks passed; integration schema endpoint initially observed stale embedded schema during regeneration.
- `node web/scripts/gen-types.mjs`: passed.
- `bash scripts/dev/test-parallel.sh -E 'test(daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded)'`: passed (1 focused test; doctests passed).
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test -p task-core -p task-api -p task-worker schema_matches`: passed (`committed_schema_matches_generated`).
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh -E 'package(e2e) or package(task-api)'`: passed (704 passed, 3 ignored; doctests passed). The initial run's unrelated task-api browser failures were caused by the long inherited temp path; shortening `TMPDIR` resolved them. The provider admin view fixture now expects `cooldown: null` because the legacy provider API no longer exposes source/account cooldowns.

## Notes

旧 provider config API の表示用 cooldown は新しい source/account view と対応しないため、この transitional handler は `None` を返す。`api-remove` が provider API を削除する。
