---
task: 01M4TK1VVQPV0YJZ9XEM43RXXM
wu: schema-sync
status: done
completed: 2026-10-13
---

# schema-sync: RoutingMode の読み取り専用 variant による schema drift

## 原因

legacy-events で `RoutingMode` に読み取り専用の `Legacy` variant を足した。
`#[schemars(skip)]` を付けていたが、schemars は skip 付き variant があると enum を
`oneOf[enum]` に変える。その結果、

- `docs/api/v1/api-v1.schema.json` の `RoutingMode` が `enum` から `oneOf` に変わった（生成と不一致）
- `docs/api/v1/event.schema.json` も同じ `oneOf` に変わったまま commit されていた（task-core の `event_row_schema_matches_committed` が落ちた）

## 選んだ直し方

- `RoutingMode` の `JsonSchema` derive を外し、`schema_name` と `json_schema` を手で書いた
  （`{"type":"string","enum":["shadow","enforce"]}`）。Legacy は API schema に出さない。
- 付随: `#[schemars(skip)]` 属性は残さない（手書き impl で不要になり、derive が無いと属性自体が
  エラーになるため削除）。`#[serde(skip_serializing)]` は残す（Legacy を書き出さない）。
- 検討して退けた案:
  - `#[schemars(skip)]` を外す → `legacy` が API schema に現れる（`const: "legacy"`）。退けた。
  - 生成結果をそのまま採る → 同上で `legacy` が API に出る。退けた。

## 再生成

- `UPDATE_SCHEMA=1 cargo test -p task-core --lib event_row_schema_matches_committed`
  で `docs/api/v1/event.schema.json` を再生成（差分は `RoutingMode` の 1 箇所のみ）。
- `docs/api/v1/api-v1.schema.json` は手書き impl 後の生成結果が既存 commit と一致したので再生成不要。
- `web/api/generated/` は `node scripts/gen-types.mjs` の結果が既存と同一（差分なし）。

## 確認コマンド と結果

- `cargo test -p task-api --lib committed_schema_matches_generated` → ok（1 passed）
- `cargo test -p task-core --lib` → ok（912 passed, 0 failed）
- `cargo test -p task-core --lib legacy_routing_event` → ok（1 passed）
- `cargo test -p celeris --test legacy_event_startup` → ok（1 passed）
- `cargo test -p task-api --lib` → ok（81 passed, 0 failed, 2 ignored）
- `cargo clippy -p task-core -p task-api -- -D warnings` → 警告なし
- `cd web && node scripts/gen-types.mjs` の前後で `diff -r` 差分なし

## 未解決事項

- なし（この WU の範囲で）。
- 提案: `#[schemars(skip)]` を variant に付ける型は、schema が oneOf に化ける。読み取り専用 variant を持つ
  enum は、同じ問題を避けるため手書き JsonSchema にする方針を次回から取る。
