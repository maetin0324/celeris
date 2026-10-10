---
title: "backup-retention / adr-note: ADR に backup 保持の実装付記"
tasks: [01M4HZCJF54DG0A7EHHE43YXCA]
status: done
updated: 2026-10-10
---

# backup-retention / adr-note: ADR に backup 保持の実装付記

ADR `agent-docs/adr/2026-10-10-local-disk-growth-paths.md` の D4 節に「実装付記（backup-retention）」を足した。コードは変えていない。

## 完了

- 付記に書いた名前: `scripts/selfdeploy/prune-backups.sh`、`CELERIS_PROMOTE_BACKUP_KEEP` / `CELERIS_ROLLBACK_BACKUP_KEEP`、試験 `scripts/selfdeploy/tests/promote_backup_retention.sh` の 6 関数、`crates/celeris/src/db_maintenance.rs` の `plan_backup_retention` / `prune_backups` / `prune_backups_with_check` / `integrity_check`、`[db]` の `backup_daily_keep` / `backup_weekly_keep` / `backup_max_total_bytes`、試験接頭辞 `backup_retention_`（6 件）。
- 名前は実ファイルで確認した（grep で定義・試験名を照合）。
- 付記に「合計 byte 上限は定期側だけが見る」「rollback.sh は prune を呼ばない」を事実として書いた。
- 親の進捗ファイルに、本葉の両記録と既存 fix 葉への link を追加した。

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| リンク検査 | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| 進捗索引 | `sh scripts/dev/progress-index.sh --check` | `progress-index --check: ok`、exit 0 |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok (178 files)`、exit 0 |

## 未解決

- promote 側の合計 byte 上限は未実装（ADR D4 の 64 GiB は定期側の prune だけが適用する）。付記に差分として明記した。
- ADR の状態は「実装待ち」のまま。最終化は統合葉（integrate-record）で行う。
