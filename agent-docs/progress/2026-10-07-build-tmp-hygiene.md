---
title: ビルド成果物と /tmp の残骸が際限なく溜まる問題の再発防止
tasks: [01M4B4J92KBR73EQA5S7FWB21G]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# ビルド成果物と /tmp の衛生

ADR `agent-docs/adr/2026-10-07-build-tmp-hygiene.md`（状態: 実装済み。実装との突き合わせは ADR 末尾の付記）。葉ごとの進捗は `2026-10-07-build-tmp-hygiene/<key>.md`。

## 葉一覧と証拠

| 葉 | 状態 | 証拠（詳細は各葉の進捗） |
|---|---|---|
| adr / main-sync / repair-design-1 | 完了 | ADR 作成。repair は binary 未 build のみ（code 変更なし） |
| target-sweep | 完了 | `target_sweep_` 26 件 ok。D1 全項目の対応表は [target-sweep.md](2026-10-07-build-tmp-hygiene/target-sweep.md) |
| run-tmpdir | 完了 | `run_tmpdir_` 10 件 ok（task-dispatch 3・task-worker 7）。[run-tmpdir.md](2026-10-07-build-tmp-hygiene/run-tmpdir.md) |
| rust-test-tmp | 完了 | `check-test-tmp-leftovers.sh -p task-core/celerisctl/celeris/task-worker` 全て `no leftovers`。[rust-test-tmp.md](2026-10-07-build-tmp-hygiene/rust-test-tmp.md) |
| web-e2e-tmp | 完了 | `mkdtemp` 直書き 0 件、vitest `tmp-dir.test.ts` 2 passed、`tsc -b`・biome error 0。[web-e2e-tmp.md](2026-10-07-build-tmp-hygiene/web-e2e-tmp.md) |
| tmp-fix | 完了 | task-dispatch・task-api・celeris で `no leftovers`（修正前は 7・8 件残り）。[tmp-fix.md](2026-10-07-build-tmp-hygiene/tmp-fix.md) |
| disk-watch | 完了 | `disk_watch_` 13 件 ok。[disk-watch.md](2026-10-07-build-tmp-hygiene/disk-watch.md) |
| close-out | 完了 | `docs/ops/build-tmp-hygiene.md`（運用手順）、ADR を実装済み＋突き合わせの付記、`docs/architecture-map.md` に 3 行 |

## 最終検査（close-out 葉、統合前の本ブランチ HEAD）

- `bash scripts/dev/test-parallel.sh` → exit 0。nextest 4710 passed / 0 failed / 14 ignored（binaries 160）、doctest exit 0、`tmp_leftovers: 0`。
- `cargo clippy --workspace -- -D warnings` → exit 0、警告なし。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。

## 未解決事項

- 有効化は人: cron `target-sweep` の作成・`resume`、config の調整、`/tmp` の既存残骸の掃除（`docs/ops/build-tmp-hygiene.md`）。本番には触っていない。
- remote run の TMPDIR は範囲外（ADR §4）。`RunTmpCleanupFailed` event と adapter ごとの TMPDIR 保持試験（D2.4）は未実装。
- `celerisctl target sweep` は config を読まない（既定 roots・既定値）。
- `release.sh` の sweep は次回 release から効く。playwright e2e 自体は未実行。
- `clippy --all-targets` は既存 2 件（`model_role_assignments_consistency.rs`、`undeclared_artifacts/tests.rs`）で落ちる。範囲外。

## 提案

- ADR-0133 の種類一覧に `disk`・`disk_full` を書き足す。
- `celerisctl target sweep` に `--config` を足し、daemon と同じ規則で dry-run できるようにする。
- `TargetSweepRan` に失敗理由欄、`RunTmpCleanupFailed` event を足す。
