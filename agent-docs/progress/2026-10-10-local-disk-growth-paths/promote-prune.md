---
title: "backup-retention / promote-prune: promote 前・rollback 前 backup の保持"
tasks: [01M4HZCJF54DG0A7EHHE43YXCA]
status: done
updated: 2026-10-10
---

# backup-retention / promote-prune: promote 前・rollback 前 backup の保持

ADR `agent-docs/adr/2026-10-10-local-disk-growth-paths.md` D4 の promote 側を実装した（ADR 本文は record 葉が付記する）。

## 完了

- `scripts/selfdeploy/prune-backups.sh <backups_dir>`（新規、POSIX sh、100755）。
  - 対象は `<YYYYMMDD-HHMMSS>-pre-<12 桁 hex>.sqlite3`（既定 10 本、`CELERIS_PROMOTE_BACKUP_KEEP`）と
    `<YYYYMMDD-HHMMSS>-pre-rollback.sqlite3`（別枠、既定 3 本、`CELERIS_ROLLBACK_BACKUP_KEEP`）だけ。名前の辞書順（＝時刻順）で新しい方を残す。
  - keep は正の整数のみ（0・非数は拒否、何も消さない）。
  - 消す候補がある型ごとに、残す最新 1 個を `sqlite3 'file:<path>?mode=ro' 'PRAGMA integrity_check;'` で検査。
    どれか 1 つでも出力が `ok` でない・失敗なら、両型とも 1 本も消さずに stderr へ理由を出して exit 1。
  - 消す本体と対の `-wal`/`-shm`/`-journal` は一緒に消す。他の名前（`celeris-*.sqlite3`・json・log・`*-pre-celeris*`・単独 -wal/-shm 等）には触れない。
- `scripts/selfdeploy/promote.sh`: 成功末尾の `promoted $SHA12` の log の直後に `sh prune-backups.sh "$SD_BACKUPS"` を呼ぶ。出力は promote log へ。失敗は `warning:` を log に残すだけで昇格は失敗にしない。
- 試験 `scripts/selfdeploy/tests/promote_backup_retention.sh`（一時 dir と sqlite3 だけ）:
  `promote_backup_retention_default_keeps_10`、`_env_override`、`_rollback_separate_3`、`_other_files_untouched`、
  `_integrity_failure_deletes_nothing`（promote 型・rollback 型それぞれ最新を壊す）、`_promote_calls_prune`。

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| 保持試験 | `bash scripts/selfdeploy/tests/promote_backup_retention.sh` | `promote_backup_retention: all ok`（26 項目）、exit 0 |
| 試験が integrity 検査を見ていること | `integrity_ok` を常に成功にした改変で同試験 | integrity の 5 項目が FAIL（改変は戻した） |
| promote.sh 構文 | `bash -n scripts/selfdeploy/promote.sh` | exit 0 |
| 既存 promote 試験 | `sh …/promote_authorization_marker.sh`・`promote_live_abort.sh`・`promote_web_follows_release.sh`・`release_notes_promote.sh`、`bash …/promote_handoff_settled.sh` | 全て exit 0 |

## 未解決

- rollback.sh は `pre-rollback` backup を作るが prune は呼ばない（Objective どおり promote 末尾だけ）。rollback 後も次の promote で刈られる。
- 合計 byte 上限（D4 の 64 GiB）は定期側（periodic-retention 葉）の担当で、この script は本数だけを見る。
