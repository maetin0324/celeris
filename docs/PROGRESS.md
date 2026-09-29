# PROGRESS — taskd

現在地: **Phase 119、Phase E6、Phase F4b まで本番反映（release c51837427ac5、schema 28）。F5-1 dogfood の 3 回目を準備中**。以後の追記は `docs/progress/phase-F.md` へ。

詳細な履歴と証跡は下記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## 目次

- [Phase 1–50（Phase 0 の初期記録を含む）](progress/phase-001-050.md)
- [Phase 51–100](progress/phase-051-100.md)
- [Phase 101–150](progress/phase-101-150.md)
- [Phase E](progress/phase-E.md)
- [Phase F](progress/phase-F.md) — 最終報告: [ADR-0074 Phase F 最終報告](execution-parallel-report-2026-09-28.md)（2026-09-28）。F6: 既存の Task / 案件を後から分解の経路に入れる（`POST /tasks/{id}/execution/decompose`・MCP `task_decompose`・retry の再判定）、案件の名前・説明の編集（2026-09-28、未昇格）
- [Phase K（知識ベース）](progress/phase-K.md) — K-1（知識の置き場の整理と配置ガード。案件の `slug` = migration 0029）完了 2026-09-28（worktree、main 未 merge）
- [Phase G（ビルドキャッシュの 2 層化、ADR-0075）](progress/phase-G.md) — G0（設計）完了 2026-09-28。G1（scratch pool + semantic GC + celerisctl / metrics）完了・本番反映 2026-09-28。G2（sccache L1 の配線 + `CARGO_INCREMENTAL=0`）完了 2026-09-28。G3（L2: webdav の階層 cache server + flusher + L2 の GC + 監視）完了 2026-09-28（worktree、main 未 merge。cache server の有効化は人）。G3-fix1（継いだ `RUSTC_WRAPPER` / `SCCACHE_*` を run と checks から外す）完了 2026-09-28（worktree）。SD-1（release / verify の所要時間の短縮: 共有 target・GUI の段の skip・本番依存の cache・verify の所要時間）完了 2026-09-28（worktree）。SD-2（release の gate の `cargo-test` をテストバイナリ並列に: cargo-nextest 0.9.146、214 s → 115 s〈実行 202 s → 78 s〉）完了 2026-09-28（worktree）。SD-3（`truncate_phase_report` を挙動不変で O(n²) → O(S log n) に: 該当テスト 45.9 s → 0.02 s、並列 gate 69 s → 58 s）完了 2026-09-28（worktree）

各 Phase の詳細・証跡・申し送りは上記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## Phase F5-1 dogfood（再レビュー対応）

worker・review の完了を JoinHandle で明示同期し、実時間の待機回数に依存しない検証へ変更。
[実装と検証の記録](progress/phase-F.md#f5-1-review-repair)を参照。

## Web GUI Phase 0（2026-09-29、設計・移行計画）


- 判断と実装方針: [ADR-0081](adr/0081-web-spa-frontend.md)。現行 GUI の全 42 route、操作・認証・通知・SSE・file viewer・mobile 要件と Phase gate は [feature parity matrix](web/feature-parity.md)、Phase 1〜7 の session 単位の作業・受け入れ条件・検証方法・人の判断点は [implementation plan](web/implementation-plan.md) を参照。計画で参照する遅延 baseline は main `06e9a03cffe8` 時点で 5 秒遅延時約 15 秒、10 秒時約 30 秒、遅延 5 秒 + tick 2 秒で `/tasks` → `/tasks/:id` は 120 秒後も未完了。詳細と再現手順は WU `baseline` の成果物に記録。
- GUI 不変 gate: `git diff --quiet 06e9a03cffe8 -- gui ':!gui/docs/adr/0002-frontend-stack.md'` → exit 0。GUI の差分は旧 ADR `gui/docs/adr/0002-frontend-stack.md` の supersede 追記だけ。
- Rust gate: `cargo test --workspace` → exit 101（sccache 起動時 `Operation not permitted`、rustc コンパイル開始前）。`cargo clippy --workspace -- -D warnings` → exit 101（指定 `CARGO_TARGET_DIR` 内の `.cargo-build-lock` を read-only filesystem のため開けず）。どちらもコード検査に到達せず、コード起因か判定できていない。
- GUI gate（`gui/`）: `pnpm typecheck` / `pnpm test` / `pnpm build` は各 exit 1。pnpm 11.27.0 の依存事前確認がユーザー cache の SQLite database を開けず、各コマンドの実処理は開始しなかった。テスト数は未取得。main との比較も未実施。
- 未解決と提案: sccache と `CARGO_TARGET_DIR` が書き込み可能な環境で Rust 2 gate を再実行し、pnpm store が利用できる環境で GUI 3 gate と main 比較を再実行してテスト件数を記録する。今回の GUI 差分 gate `git diff --quiet 06e9a03cffe8 -- gui ':!gui/docs/adr/0002-frontend-stack.md'` は exit 0。旧 ADR 追記を含む GUI 全体の差分は新 ADR-0081 に supersede として記録済み。ADR・parity・計画の相互リンクを確認済み。
