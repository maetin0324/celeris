# PROGRESS — taskd

現在地: **Phase 119、Phase E6、Phase F4b まで本番反映（release c51837427ac5、schema 28）。F5-1 dogfood の 3 回目を準備中**。以後の追記は `docs/progress/phase-F.md` へ。

詳細な履歴と証跡は下記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## 目次

- [Browser capability Phase 1](progress/phase-browser.md) — ADR-0078、既存 harness + agent-browser、管理者 grant・session・監査・dashboard 導線。完了 gate 2026-09-28（test 2639 passed、GUI 1154 passed）。本番未昇格。

- [Phase 1–50（Phase 0 の初期記録を含む）](progress/phase-001-050.md)
- [Phase 51–100](progress/phase-051-100.md)
- [Phase 101–150](progress/phase-101-150.md)
- [Phase E](progress/phase-E.md)
- [Phase F](progress/phase-F.md) — 最終報告: [ADR-0074 Phase F 最終報告](execution-parallel-report-2026-09-28.md)（2026-09-28）
- [Phase K（知識ベース）](progress/phase-K.md) — K-1（知識の置き場の整理と配置ガード。案件の `slug` = migration 0029）完了 2026-09-28（worktree、main 未 merge）
- [Phase G（ビルドキャッシュの 2 層化、ADR-0075）](progress/phase-G.md) — G0（設計）完了 2026-09-28。G1（scratch pool + semantic GC + celerisctl / metrics）完了・本番反映 2026-09-28。G2（sccache L1 の配線 + `CARGO_INCREMENTAL=0`）完了 2026-09-28。G3（L2: webdav の階層 cache server + flusher + L2 の GC + 監視）完了 2026-09-28（worktree、main 未 merge。cache server の有効化は人）。G3-fix1（継いだ `RUSTC_WRAPPER` / `SCCACHE_*` を run と checks から外す）完了 2026-09-28（worktree）。SD-1（release / verify の所要時間の短縮: 共有 target・GUI の段の skip・本番依存の cache・verify の所要時間）完了 2026-09-28（worktree）。SD-2（release の gate の `cargo-test` をテストバイナリ並列に: cargo-nextest 0.9.146、214 s → 115 s〈実行 202 s → 78 s〉）完了 2026-09-28（worktree）。SD-3（`truncate_phase_report` を挙動不変で O(n²) → O(S log n) に: 該当テスト 45.9 s → 0.02 s、並列 gate 69 s → 58 s）完了 2026-09-28（worktree）

各 Phase の詳細・証跡・申し送りは上記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## Phase F5-1 dogfood（再レビュー対応）

worker・review の完了を JoinHandle で明示同期し、実時間の待機回数に依存しない検証へ変更。
[実装と検証の記録](progress/phase-F.md#f5-1-review-repair)を参照。
