---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# stranded 処分操作

`task-ops::stranded` に discard / hold / reopen を追加した。discard と hold は空白だけの理由を拒否し、actor・時刻・理由を状態行に記録する。reopen は held/discarded だけを unintegrated に戻し、既存 store の append-only event に記録する。

merge 操作は task branch の現在 head が記録 SHA と一致することを再検査する。既に default branch の祖先なら integrated を記録し、反復呼び出しは追加 event を作らない。未到達なら git merge を独自実装せず、既存 `integration_request_record` 経路に依頼を追記する。

## 検証

- `cargo test -p task-ops stranded_ --lib` — 成功（4 件）。git fixture で検出後の統合、既に祖先の場合の冪等性、理由必須と discard/hold/reopen の履歴を確認。
- `cargo clippy --workspace --all-targets -- -D warnings` — 成功。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 成功。変更は `crates/task-ops/src/stranded.rs` のみ。
- `git diff --check` — 成功。

## 未解決

- 未到達 branch の明示 merge は既存 integration request を作る。delivery policy 対象外 task を reviewer-run 付き delivery に載せる API/CLI 配線は後続 WorkUnit の担当。
