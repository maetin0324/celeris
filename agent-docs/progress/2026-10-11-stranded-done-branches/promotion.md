---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# promotion の決定的な読み取り view

`task-ops::stranded` に `PromotionView` / `PromotionStatus` と `promotion_view` を追加した。agent-platform 以外は `not_applicable`。指定された `releases_dir` 内の `promoted.json` がある release から `promoted_at` が最も新しいものを選び、manifest の完全 SHA を取得して `git merge-base --is-ancestor <task-head> <release-sha>` で `promoted` / `promotion_pending` を決める。取り込み状態の保存・書き換えは行わず、promotion は独立して読める。

## 検証

- `cargo test -p task-ops stranded_promotion_ --lib` — 成功（1 件）。一時 Git repo と偽 release directory で pending、promoted、対象外 repo を確認。
- `cargo clippy --workspace --all-targets -- -D warnings` — 成功。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 成功。変更 path は `crates/task-ops/src/stranded.rs` のみ。

## 未解決事項

- なし。
