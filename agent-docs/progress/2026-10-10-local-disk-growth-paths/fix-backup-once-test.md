---
tasks: [01M4HZCJF54DG0A7EHHE43YXCA]
---
# backup_once 試験を 48 時間保持の規則に合わせる

## 原因

`periodic-retention` 統合後、`crates/celeris/src/db_maintenance/tests.rs` の
`backup_once_writes_a_restorable_copy_and_prune_keeps_only_the_newest` が落ちた
（tests.rs:212 `assert_eq!(remaining.len(), 2)`、left 3 / right 2）。

試験は `keep: 2` で 3 回 `backup_once` を呼び、最古の 1 本が刈られることを期待していた。
ADR 2026-10-10-local-disk-growth-paths の D5 では、直近 48 時間の定期 backup は `keep` を超えても保持される。
実時計で 1.1 秒おきに書いた 3 本は全て 48 時間以内なので、3 本とも残る。
製品の規則は正しく、試験の期待が旧規則のままだった。

## 修正

- 試験名を `backup_once_writes_a_restorable_copy_and_keeps_recent_generations` に変えた。
- 期待を「3 本とも残る」に変えた。`remaining` と書いた 3 path を sort して比較する。
- 最新の copy は従来どおり `integrity_check` をかけ、`SqliteStore::open` で task を読めることを確かめる。
- 1.1 秒の sleep は file 名（unix 秒）の衝突回避として残した。sleep は 2 回目以降の書き込みの前だけに移し、最後の 1 回の後の待ちをなくした。
- 古い世代の刈り込みは、注入時計の `backup_retention_` 試験（`backup_retention_daily_and_weekly_generations_are_kept` ほか）が既に固定している。この試験では再確認しない。

製品コード（`crates/celeris/src/db_maintenance.rs`）と ADR 本文は変更していない。

## 証拠

- 修正前: `cargo test -p celeris -- db_maintenance::`（`--lib` 相当）で `backup_once_writes_a_restorable_copy_and_prune_keeps_only_the_newest` が FAILED（13 passed / 1 failed、tests.rs:212、left 3 right 2）。
- 修正後: `cargo test -p celeris --lib -- db_maintenance::` → `test result: ok. 14 passed; 0 failed`（`backup_once_writes_a_restorable_copy_and_keeps_recent_generations` を含む）。
- `cargo test -p celeris --lib -- backup_retention_` → `test result: ok. 6 passed; 0 failed`。
- `cargo test -p celeris -- db_maintenance::` の終了コードは 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`）。

## 未解決事項

- 1.1 秒の sleep は残っている。file 名を unix 秒で作る限り、同一秒の 2 本目を避けるには待つ必要がある。
  時計を注入すれば消せるが、`backup_once` の公開 signature を変えることになるので、この unit では見送った。
