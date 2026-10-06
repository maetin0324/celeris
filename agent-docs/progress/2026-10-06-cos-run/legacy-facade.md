# CoS legacy facade

---
tasks: [01M47J2YP9QKYVD40D6NW09X1B]
---

旧 Console、`POST /org/cos/messages`、MCP `console_instruct` は旧 message と受付用 task ID を確保したうえで、scope ごとの既定 legacy thread に user message を積む。受付用 task は draft にとどめ、CoS の実行は chat queue が担う。非 CoS ノードの対話経路は従来どおり。

`POST /console/new-conversation` は互換既定 thread の参照先を切り替える。旧 thread と新 UI の thread は残す。`GET /console` と SSE は新 chat message を併せて読み、移行済みの `legacy_message_id` と旧返信の `run_id` で重複を除く。`console_action_runs` は変更していない。

検証: `cargo test --workspace cos_chat_legacy_`（9 件。Console SSE の新着も含む）、`cargo test -q -p task-api --test console --test console_instruct`（各 13 件）、旧 Console の GUI 単体試験 4 ファイル（72 件）が通過。`cargo clippy --workspace -- -D warnings`、GUI の `react-router typegen`・`tsc -b`・変更 4 ファイルの Biome check、`scripts/sync-gui-docs.sh --check` も通過。GUI 依存は本体 repo の既存インストールを読み取り専用のリンクで参照し、Vite の一時ディレクトリだけ worktree 内に置いた。リンクは試験後に削除した。
