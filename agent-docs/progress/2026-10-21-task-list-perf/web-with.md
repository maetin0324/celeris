---
task: task-list-perf
wu: web-with
status: done
completed: 2026-10-21
---
# web-with: 一覧の取り込み状態を明示して頼む

- `web/features/tasks/task-list-query.ts`: `taskListPath` に `with=integrations` を常に付け、query key にも `with` を含める。task-list-view（初回取得・続きの読み込みとも）はこれを使う。
- board（`boardTasksPath`）と wait-reason は付けない。試験 `board-model.test.ts` に `with=` が付かないことを追加。
- overview-view は `GET /tasks/:id`（詳細。api-with 後も integrations を常に付ける）を使い、一覧を取らないため変更なし。
- `web/api/generated/schema.json` を `gen:types` で再生成（types.ts は差分なし）。

## 証拠
- `pnpm -C web typecheck`: exit 0
- `vitest run`: 663 passed
- 試験: task-pages.test.ts（path に `with=integrations`、key に含む）、board-model.test.ts（board に with なし）

## 本番への移行
config 変更なし。
## 提案
なし。
