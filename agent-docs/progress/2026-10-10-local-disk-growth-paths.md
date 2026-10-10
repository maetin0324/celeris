---
title: "/local disk growth paths: ADR 作成"
tasks: [01M4HX54MZ1P6ZYZEB6ABHD6BV]
status: done
updated: 2026-10-10
---

# `/local` disk growth paths: ADR 作成

## 完了

- 2026-10-10 に `agent-docs/adr/2026-10-10-local-disk-growth-paths.md` を作成し、repo target、release-build lease、DB backup、dangling `.cargo-target` の原因と D1〜D5 を記録した。
- inv-targets / inv-release-db の findings を読み、記載されたコード位置と本番読み取り調査を根拠として統合した。`01M4D7RVKX` の残存理由は証拠不足のため断定せず、6時間猶予・木の checkout 追跡漏れ・cron apply 状態を候補として明記した。
- コード変更なし。`crates/` と `scripts/` に差分はない。

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| 調査 findings の存在 | `test -s /local/celeris/data/workspaces/01M4HX54MZ1P6ZYZEB6ABHD6BV/wu/inv-targets/artifacts/findings.md && test -s /local/celeris/data/workspaces/01M4HX54MZ1P6ZYZEB6ABHD6BV/wu/inv-release-db/artifacts/findings.md` | exit 0 |
| ADR・担当葉・進捗 | `test -s agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q worker-target-env agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q repo-target-gc agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q release-prune agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q backup-retention agent-docs/adr/2026-10-10-local-disk-growth-paths.md && test -s agent-docs/progress/2026-10-10-local-disk-growth-paths.md` | exit 0 |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| progress index | `sh scripts/dev/progress-index.sh --check` | `progress-index --check: ok`、exit 0 |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok (178 files)`、exit 0 |
| 製品コード差分なし | `git diff --quiet "$(git merge-base HEAD main)" -- crates/ scripts/` | exit 0（差分なし） |

## 未解決

- `01M4D7RVKX` の実際の sweep event / live DB 状態がなく、残存原因の個別確定はできない。
- D4 の提案既定値（promote 10本、rollback 3本、合計64 GiB）は実装時に設定値として導入する。実装前の本番 backup 削除はしない。
- 本番適用は人による手順が必要。lease prune による clean rebuild と backup retention の本番実行は未実施。

## 提案

後続の `worker-target-env`、`repo-target-gc`、`release-prune`、`backup-retention` を ADR の表に記した担当範囲と試験 prefix で実装する。適用前に dry-run と復元点の integrity check を確認し、release-build lease の再作成時間を運用枠に含める。
