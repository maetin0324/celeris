---
tasks: [01M4HZCJF54DG0A7EHHE43YXCA]
---
# DB periodic backup retention

Implemented periodic backup retention in `crates/celeris/src/db_maintenance.rs`.

- Retains the configured newest generations, every backup from the preceding 48 hours, UTC daily generations, and ISO weekly generations.
- Counts periodic and recognized promote/rollback SQLite backups toward the configured byte limit. It prunes old periodic backups before promote backups and protects each type's newest backup plus the three newest rollback backups.
- Runs a read-only `PRAGMA integrity_check` on the newest recognized backup before deletion; errors or non-`ok` results skip all pruning and emit a warning.
- Added `[db] backup_daily_keep` (7), `backup_weekly_keep` (4), and `backup_max_total_bytes` (64 GiB), with config tests and example comments.

Checks: six `backup_retention_` tests and the DB config default/explicit-value test passed. Workspace clippy is being run for this unit.
