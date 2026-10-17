---
status: done
completed: 2026-10-17
---
# WU api: evidence_mode を通常 task-api に通す

- POST /tasks の routing_override に evidence_mode(none/pinned/latest) を受け、model の検証で矛盾指定は 4xx。
- GET /tasks/{id} の TaskDetail.routing_override に実効値を出す。retry は API 層で override を複製先へ写す。
- schema（docs/api/v1・web）と生成型は再生成済み。
- 証拠: `cargo test -p task-api --test project_plan` 6 passed（evidence_mode_round_trips_through_post_get_child_and_retry）、
  `bash scripts/dev/test-parallel.sh` 5621 passed / 0 failed、clippy・fmt exit 0。
