# refs-crates: crate 内 docs 参照の追従

- tasks: [01M3YBGM64RYPEY9NZANF79A0M]

## 変更

- task-worker 以外の crates 内の API・運用・ガイド・ADR・進捗参照を新配置へ更新した。
- `docs/api/` と `docs/protocol/` の生成 schema の参照は維持した。
- `docs/DESIGN.md` の crate 内参照を `docs/SPEC.md` に変更した。
- テスト中のパス例も agent-docs 配置へ合わせた。
- `cargo fmt` の差分（`crates/celerisctl/src/commands/mcp.rs`、本文は docs パス変更のみで整形は無関係）を適用。
- 型の doc comment（`docs/gui/api.md`→`docs/api/v1/gui-api.md`、`docs/knowledge.md`→`docs/guides/knowledge.md`、`docs/adr/0074-...`→`agent-docs/adr/0074-...`）の更新で生成 schema の description が変わったため、`UPDATE_SCHEMA=1 cargo test -p task-api --lib` と `UPDATE_SCHEMA=1 cargo test -p task-core --lib store::tests::event_row_schema_matches_committed` で `docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json` を再生成した（memory: 型の doc comment を変えると生成 schema の description も変わる）。

## plan_issue（2 回目の run）

前回の run はここまでの変更で done を返したが、celeris の事後 check（差分範囲を
`crates`・`CLAUDE.md`・`AGENTS.md`・`.claude`・`scripts`・`web`・`gui`・
`docs/architecture-map.md`・`docs/README.md`・`agent-docs/README.md`・`agent-docs/progress`
に限定するもの）が不合格だった。原因は `docs/api/v1/api-v1.schema.json` と
`docs/api/v1/event.schema.json` への差分で、この 2 ファイルはその範囲 check の
除外リストに入っていない。

この WorkUnit の Objective・受け入れ条件 0/2 と、この事後 check は両立しない:
- `docs/gui/api.md`・`docs/knowledge.md`・`docs/adr/0074-...` を指す `///` doc comment
  （`crates/task-api/src/types.rs`・`knowledge.rs`、`crates/task-ops/src/gate.rs`・`plan.rs`・`add.rs`、
  `crates/task-core/src/quota.rs` 等、`#[derive(JsonSchema)]` の対象型・フィールド）は
  受け入れ条件 0（check-doc-links.sh）を満たすために新配置へ直す必要がある。
- 直すと生成 schema の description 文字列が変わり、受け入れ条件 2
  （`schema::tests::committed_schema_matches_generated` / `store::tests::event_row_schema_matches_committed`）
  を通すには `UPDATE_SCHEMA=1` での再生成が要る。
- 再生成すると `docs/api/v1/*.schema.json` に差分が出て、事後 check（範囲外）に落ちる。
- 逆に再生成しない／該当 doc comment を直さないと、受け入れ条件 0 か 2 のどちらかが落ちる。

3 つの要求（受け入れ条件 0・受け入れ条件 2・事後 check の差分範囲）を同時に満たす編集は無い
（確認済み: 現在のブランチで実際に両方の状態を作って再現した）。事後 check 側の除外リストに
`docs/api/v1`（必要なら `docs/protocol` も）を加える判断が要る。Objective 本文の
「docs/api/ と docs/protocol/ の生成物の場所は変えない」は置き場所の話であり、
中身（description）まで固定する意図ではないと読んでいる。

現在のリポジトリの状態は、受け入れ条件 0・1・2 をすべて満たす側（schema 再生成済み）に揃えてある
（下の検証を参照）。事後 check だけが不合格。

## 検証

- `cargo fmt --all -- --check`: ok
- `sh scripts/dev/check-doc-links.sh crates/celeris crates/celeris-credentiald crates/celeris-mcp crates/celerisctl crates/llm-proxy crates/scratch-cache crates/task-api crates/task-core crates/task-dispatch crates/task-ops`（task-worker 以外の全 crate）: ok
- `cargo check --workspace --tests`: 成功
- `cargo test -p task-ops -p task-core -p task-api --lib --no-fail-fast`: task-api 72 passed/2 ignored、task-core 625 passed、task-ops 400 passed、0 failed
- `git diff --check`: ok
