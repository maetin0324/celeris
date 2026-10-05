# CoS chat migration と wire 型

- WorkUnit: `model`
- ADR: `agent-docs/adr/2026-10-05-cos-chat-home.md` D1/D2
- migration 番号の追加直前走査: 1,341 refs、162 worktree、最大 0049、0050 未使用。
- `0050_cos_chat.sql`: D1 の会話・添付・CoS 受信箱/操作/通知経路・冪等受付・upload 予約表、`node_sessions` の 4 列、部分 UNIQUE、FTS5 と同期 trigger を追加。0048/0049 はこの branch にまだ存在しないので予約版数に記録した。
- chat wire: T/M/R/A/Card/E と SSE data、chat REST の request/response を serde + JsonSchema で定義。書込み JSON body は未知フィールドを拒否する。
- 試験: `chat_migration_*` は新規 DB と 47 版からの upgrade、制約・FTS 同期・旧 session 維持を確認。`chat_model_*` は ADR 例の round-trip と未知フィールド拒否を確認。
- schema の登録と `docs/api/v1` の生成は後続の `schema` WorkUnit が担当。

検証: `cargo test -p task-core chat_` 5/5、`cargo test -p task-core --quiet` 725/725、`cargo test -p celerisctl --test no_migrate --quiet` 4/4。旧版 DB を再現する 2 試験には 0050 の巻き戻し手順を足した。
`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`、base からの差分範囲検査も合格。`docs/api/v1` の変更はない。
