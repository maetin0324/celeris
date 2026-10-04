# refs-crates: crate 内 docs 参照の追従

- tasks: [01M3YBGM64RYPEY9NZANF79A0M]

## 変更

- task-worker 以外の crates 内の API・運用・ガイド・ADR・進捗参照を新配置へ更新した。
- `docs/api/` と `docs/protocol/` の生成 schema の参照は維持した。
- `docs/DESIGN.md` の crate 内参照を `docs/SPEC.md` に変更した。
- テスト中のパス例も agent-docs 配置へ合わせた。
- `cargo fmt` の差分（`crates/celerisctl/src/commands/mcp.rs`、本文は docs パス変更のみで整形は無関係）を適用。
- 型の doc comment（`docs/gui/api.md`→`docs/api/v1/gui-api.md`、`docs/knowledge.md`→`docs/guides/knowledge.md`、`docs/adr/0074-...`→`agent-docs/adr/0074-...`）の更新で生成 schema の description が変わったため、`UPDATE_SCHEMA=1 cargo test -p task-api --lib` と `UPDATE_SCHEMA=1 cargo test -p task-core --lib store::tests::event_row_schema_matches_committed` で `docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json` を再生成した（memory: 型の doc comment を変えると生成 schema の description も変わる）。

## plan_issue（2 回目の run、解消済み）

前回の run は schema 再生成を含めた状態で done を返したが、celeris の事後 check
（差分範囲が `docs/api/v1/*.schema.json` を除外していた）で不合格になった。
今回の run の Objective 本文に「これらの schema の差分は範囲内」と明記されたため、
この plan_issue は解消済み。3 回目の run はコードを変更せず、前回の変更をこの
WorkUnit 専用ブランチにコミットし（`c115d1f3`）、受け入れ条件 0〜3 を再確認しただけ。

## 検証

- `cargo fmt --all -- --check`: ok
- `sh scripts/dev/check-doc-links.sh crates/celeris crates/celeris-credentiald crates/celeris-mcp crates/celerisctl crates/llm-proxy crates/scratch-cache crates/task-api crates/task-core crates/task-dispatch crates/task-ops`（task-worker 以外の全 crate）: ok
- `cargo check --workspace --tests`: 成功
- `cargo test -p task-ops -p task-core -p task-api --lib --no-fail-fast`: task-api 72 passed/2 ignored、task-core 625 passed、task-ops 400 passed、0 failed
- `git diff --check`: ok
