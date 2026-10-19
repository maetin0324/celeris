---
title: "task-dispatch: 分類 call 単位の版メタデータ受け渡し契約"
tasks: [01M59W8DEFMMP2C2SX5VE4KTKG]
unit: board-meta
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# board-meta（task-dispatch 内のみ）

## 決めたこと
- `CallProvenance { response_model, observed_snapshot }`（既定は全 None）を追加し、`BoardCall.provenance` に載せる。`BoardItem`/`ClassificationEntry` は不変（struct literal を壊さない）。
- `ClassificationBoard::put_with_provenance(subject, item, provenance)` は `put` と同じ採番（`next_call_id`/`call_no`）・同じ lock 内で原子的に結び付ける。従来 `put` は provenance 欠測（None）。
- 読み出しは `get_call` / `take_unrecorded`（BoardCall 経由）。subject だけの `get` は版を返さないので別 call と取り違えない。
- 公開: `task_dispatch::CallProvenance`（既存 `BoardCall` と同じ再 export。celeris は `task_dispatch::` を直接参照する規約）。新しい採番系なし。

## 証拠
- `cargo test -p task-dispatch --lib lane_assist`: 14 passed（新試験 `board_call_provenance_is_bound_to_call_id_and_missing_is_none`、既存 board 試験含む）。
- `bash scripts/dev/test-parallel.sh`: exit 0、5840 passed / 15 skipped。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決・後続
- 実際の値を入れるのは daemon-board（celeris）、record への反映は dispatch-wire。実 API 成功は未検証。
