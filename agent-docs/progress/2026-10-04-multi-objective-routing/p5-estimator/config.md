---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: config
status: done
completed: 2026-10-05
---

# Phase 5 estimator sidecar 設定

`[model_routing.estimator.sidecar]` を追加した。未設定では `enabled=false`、`shadow_only=true`、`send_prompt=false` で、既存の heuristic primary は変わらない。`shadow_only=false` は常に設定エラー。有効化には HTTP(S) endpoint、estimator id/version、4 次元の対象 allowlist、正の日次 request 上限が必要。endpoint は loopback または `network_allowlist` の host だけを許す。応答に含まれる外部依存 host の許可判定と prompt 対象判定を同じ設定型に置いた。prompt 送信には専用の 4 次元 allowlist が必要。

## 検証

- `cargo test -p celeris routing_sidecar_ --lib` → exit 0、2 passed
- `cargo test -p celeris --lib routing_ -- --nocapture` → exit 0、15 passed
- `cargo test -p celeris --lib cheap_local_first_ -- --nocapture` → exit 0、4 passed
- `cargo clippy -p celeris --lib -- -D warnings` → exit 0
- `git diff --check`・`cargo fmt --all -- --check` → exit 0

## 未解決事項

- sidecar への HTTP 呼び出し・response 依存先の実行時検証・日次上限の消費は後続の proxy-client / daemon-wire unit が担当する。この unit は設定読込時の検証と許可判定のみ。
