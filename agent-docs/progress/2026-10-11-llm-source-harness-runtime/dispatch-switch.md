---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: dispatch-switch: worker protocol に選択 route を追加
updated: 2026-10-11
---
# dispatch-switch: worker protocol に選択 route を追加

## 完了

- `RunRequest` に `SelectedLlmRoute` を追加。harness/source/model/account ID、OpenAI 互換 endpoint の URL/API format、credential の環境変数名または account directory を秘密値なしで載せられる。
- workspace 内の `RunRequest` literal を新フィールドに追随。
- `UPDATE_SCHEMA=1 cargo test -p task-worker schema` で `docs/protocol/worker-protocol.schema.json` を更新。

## 検証

- `cargo test -p task-worker --no-run` — 成功。
- `UPDATE_SCHEMA=1 cargo test -p task-worker schema` — 成功（2 tests）。
- `cargo check -p task-dispatch -p task-worker` — 成功。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 成功。許可範囲内の変更。

## 検証（この run・受け入れ条件）

- 0. `RunRequest.llm_route` と docs/protocol の schema の一致: `cargo test -p task-worker --lib committed_schema_matches_generated` — `ok. 1 passed`（schema 再生成後の差分なし）。
- 1. `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告 0）。
- 1. `cargo test --workspace --no-run` — exit 0（workspace の全 test target が compile する。RunRequest literal の追随は全 crate 済み）。
- 2. この記録を status: done にして commit。

## 移した作業（この WorkUnit の範囲外）

dispatcher の切替は後続の WorkUnit へ移した。この WorkUnit は protocol の欄の着地だけを担当する。
- `harness-adapters`: dispatcher の harness 単位 adapter 表と RoutePick から起動入力を作る helper。
- `run-switch`: 通常 run（`dispatch_run.rs`）を `select_route` に切替。
- `review-switch`: reviewer run（`review_spawn.rs`）を `select_route` に切替。

Run #4・#5 で確認した事実（後続の WU への引き継ぎ）: 現状の `dispatch_run.rs` と `review_spawn.rs` は選択後も旧 provider ID を adapter・監査・run 記録・RunEntry に使う。selector だけを差し替えても選択 model は実行に届かないため、後続で route 伝播を含めて切り替える。

## Run #4

- `cargo check -p task-dispatch -p task-worker` — 成功（dev profile）。
- dispatcher の route selection API と legacy selection 後段を調査。現状は旧 provider id/tier を adapter・監査・run 記録・run entry で再利用しており、selection 呼び出しだけの差し替えでは選択 model を実行に使えないことを確認。
- この run ではそのための dispatcher 変更はまだ行っていない。次 run は RoutePick から harness kind adapter と具体 model/account を一貫して作る切替に集中する。

## 次の作業

この WU の次は無い（完了）。dispatcher の切替は `harness-adapters` → `run-switch` / `review-switch` の順で行う。

## Run #5

- 開始時に `git status --short` / `git diff --stat` で前回の未コミット変更を確認。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 成功。許可範囲内の変更。
- `dispatch_run.rs` の選択後は adapter model、provider quota、legacy audit、RunEntry が同じ provider ID に依存し、`review_spawn.rs` も旧選択後に同じ形で run を作ることを確認。selector のみ置換する実装は具体 model の適用と selection 記録を満たさないため、後段の route 伝播も含む実装が必要。
- この run では dispatcher 実装・追加検証に未着手。
