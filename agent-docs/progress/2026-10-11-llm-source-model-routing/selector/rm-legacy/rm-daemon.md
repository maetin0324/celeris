---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
---
# rm-daemon: daemon の旧 routing 配線撤去

## 実施

- daemon の旧 shadow producer/sink と sidecar module・専用試験を削除。
- bootstrap の旧 dispatcher routing 設定、proxy state の fallback/shadow/sidecar 注入、admin reload の旧 wiring を除去。
- config の `[model_routing]` 設定型・検証は rm-config が扱うため残した。daemon 型への循環依存を作っていた sidecar control 欄のみ削除した。
- dispatcher に routing runtime を接続する前提の celeris lib 試験を削除。

## 後続葉への引き継ぎ

- task-dispatch と llm-proxy がまだ旧 routing API・欄を持つ場合、その削除は rm-dispatch / rm-proxy で行う。daemon は旧イベントを書き込まない。
- 新規 `routing_shadow_recorded` event の生成箇所は削除済み。履歴読取型と audit は hist の担当。

## 検証

- `cargo test -p celeris --lib`（短い TMPDIR）: 395 passed。
- `cargo check --workspace`: 成功。
- `cargo clippy --workspace -- -D warnings`: 成功。
- `git diff --check` と `sh "$CELERIS_WU_SCOPE_PATHS"`: 成功。
