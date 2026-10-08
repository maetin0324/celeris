---
title: WorkUnit 開始時 snapshot の実装・検証
tasks: [01M4E0E7SX4Q8BJ50N0K08174R]
status: done
updated: 2026-10-08
---
# WorkUnit 開始時 snapshot の実装・検証

[ADR](../adr/2026-10-08-work-unit-scope-snapshot.md) に従い、worker 起動前に範囲検査専用の snapshot を保存し、
worker・事後 check へ `CELERIS_WU_BASE` と未追跡一覧・全差分補助を渡した。
統合用の `base_commit` は維持し、retry・check 引き継ぎは WU id ごとの同じ snapshot を読む。
planner の雛形、[checks ガイド](../guides/work-unit-checks.md)、[worker protocol](../../docs/protocol/worker-protocol.md) を更新した。

## 検証

- `cargo test -p task-dispatch wu_base`: 7 passed、0 failed。
  直列実行で先行の未 commit 成果を除外し、先行の未追跡 file への範囲外編集は拒否する。
  worker と check の変数一致、retry の固定基点、専用 worktree 分離、実 index・HEAD の保存、
  未追跡の追加・変更・削除・symlink・commit 後比較、git GC 後の再利用も確認した。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`・`git diff --check`・文書リンク検査: exit 0。
- `cargo test -p task-dispatch phase_effect_ab_review_sync -- --nocapture`: 2 passed、0 failed。
- planner の check 雛形試験: 1 passed、0 failed。
- 全体検査で未作成 cwd の remote WU を snapshot 取得が妨げる回帰を発見し、
  remote を対象外にし、未作成の非 git ディレクトリを許す修正を追加した。remote continuation の単独再検査は合格。
- `bash scripts/dev/test-parallel.sh`: 修正前は 4777 passed / 67 failed / 14 ignored。
  修正後の `cargo nextest run -p task-dispatch --no-fail-fast --test-threads 4` は
  900 passed / 0 failed。全体再検査（`bash scripts/dev/test-parallel.sh -p task-dispatch` は
  内部の `--workspace` により全 workspace を検査）は 4780 passed / 64 failed / 14 ignored、exit 100。
  task-dispatch の失敗は 0。長い TMPDIR により broker の Unix socket path が SUN_LEN 上限を超える
  試験などが残り、全体合格とは扱わない。
- ADR 番号検査: exit 0。architecture-map 検査には既存の placeholder と省略 path 3 件の失敗が残る。

本番 host・本番 DB・daemon 再起動、元のアカウント上限 task の計画・成果には触れていない。
旧計画の生の未追跡一覧追加式は自動変更せず、補助を使う雛形への変更が必要である。
