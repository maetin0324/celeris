---
title: "/local disk growth paths: ADR 作成"
tasks: [01M4HX54MZ1P6ZYZEB6ABHD6BV, 01M4J36GBBT2BDJD24ZGMHJF68]
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

## 実装の記録（backup-retention）

- promote 側: [promote-prune.md](2026-10-10-local-disk-growth-paths/promote-prune.md)
- 定期側: [periodic-retention.md](2026-10-10-local-disk-growth-paths/periodic-retention.md)
- 試験の既存 `backup_once` の 48 時間規則への合わせ: [fix-backup-once-test.md](2026-10-10-local-disk-growth-paths/fix-backup-once-test.md)
- ADR の実装付記（D4 節）: [ADR](../adr/2026-10-10-local-disk-growth-paths.md)

## close-out（統合後 HEAD の全体検査、2026-10-10）

ADR 2026-10-10-local-disk-growth-paths の 3 経路（repo 直下 target、release-build の古い test binary、DB backup の無制限保持）と dangling `.cargo-target` を塞ぐ実装が統合された。検査は HEAD `622096b4`（ops-runbook 込み）で行い、全て exit 0。製品コードは本 close-out で変えていない。

### 葉の成果

| 葉 | 成果 | 記録 |
|---|---|---|
| worker-target-env（D1） | 作業ツリーの run・check に scratch の `CARGO_TARGET_DIR` を渡す | [記録](2026-10-10-local-disk-growth-paths/worker-target-env.md) |
| repo-target-gc（D2） | 終端 task の repo 直下 target を猶予後に候補化・削除（running・生存子孫は保護） | [記録](2026-10-10-local-disk-growth-paths/repo-target-gc.md) |
| release-prune（D3） | release-build の前回 test binary を刈り、上限超過時は target を作り直す | [記録](2026-10-10-local-disk-growth-paths/release-prune.md) |
| ledger-target | browser-ledger の target 解決を lease 経由に統一、dangling link を除去 | [記録](2026-10-10-local-disk-growth-paths/ledger-target.md) |
| backup-retention（D4・D5） | promote/rollback 前 backup の刈り込み、定期 backup の保持規則（48 時間・日次・週次・総量） | [promote](2026-10-10-local-disk-growth-paths/promote-prune.md)、[定期](2026-10-10-local-disk-growth-paths/periodic-retention.md)、[試験修正](2026-10-10-local-disk-growth-paths/fix-backup-once-test.md)、[ADR 付記](2026-10-10-local-disk-growth-paths/adr-note.md) |
| ops-runbook | 人が本番 host で行う手順と確認方法 | [docs/ops/local-disk-growth.md](../../docs/ops/local-disk-growth.md) |
| verify-record（本 WU） | ADR 状態を「実装済み」へ、本進捗を close | この文書、[ADR](../adr/2026-10-10-local-disk-growth-paths.md) |

### 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。nextest passed 5041 / failed 0 / ignored 14、doctest exit 0。nextest 293 秒。ログ: `artifacts/test-parallel.log` |
| 試験の一時領域 | 同上の末尾 | `tests left 3 entries in TMPDIR (removed on exit)` の警告のみ（exit に影響なし）。daemon e2e の時間切れは無し（disk_watch の確認は不要。実行時の `/local` は 68% 使用） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`）。ログ: `artifacts/clippy.log` |
| fmt | `cargo fmt --all -- --check` | exit 0。ログ: `artifacts/fmt.log` |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | exit 0、`check-doc-links: ok` |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | exit 0、`check-adr-numbers: ok (178 files)` |
| progress 索引 | `sh scripts/dev/progress-index.sh --check` | exit 0、`progress-index --check: ok` |
| ADR 状態 | `grep -n '状態' agent-docs/adr/2026-10-10-local-disk-growth-paths.md` | `実装済み（2026-10-10）` |

sandbox 既知失敗（browser・launcher・credentiald・CDP 系）は今回 0 件で、分類の対象は無かった。

### 未解決事項

- 本番未適用。配送後の daemon 差し替え、既存 repo 直下 target（例: `01M4F3V643`）の削除、release-build の初回刈り込み、既存 backup の刈り込み、dangling `.cargo-target` の修理は人が [docs/ops/local-disk-growth.md](../../docs/ops/local-disk-growth.md) の手順で行う。本進捗は本番での効果を確認していない。
- `01M4D7RVKX` の残存理由は、sweep event と本番 DB の状態がないため個別に確定できていない（前段の記録どおり）。
- test-parallel が TMPDIR に 3 件の残骸を残す警告（exit 0）。どの試験が残すかは未調査。
- D4 の既定値（promote 10 本、rollback 3 本、定期 daily 7・weekly 4・総量 64 GiB）は実装の既定。本番の設定に載せるかは人が決める。

### 提案

- 配送後、docs/ops/local-disk-growth.md の 1〜6 を順に行い、効果を `df` と scratch lease の size で記録する。
- test-parallel が残す TMPDIR の 3 件を特定し、試験側で後始末する（別 WU）。

