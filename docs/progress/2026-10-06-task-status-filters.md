---
tasks: [01M475Z7T5E8PAPGBCE4XG2K7B]
---
# タスク一覧の状態切り替えと件数表示

## 原因と修正

状態 chip はフォーム内の checkbox で、押しただけでは検索条件が送信されず、選択の強調も URL から決まっていた。このため押下直後は画面も一覧も変わらなかった。また `/tasks` の `validateSearch` が `status` を返さず、複数選択した URL でもルートの状態に残らなかった。

chip を `aria-pressed` 付きのボタンに変え、押下時に URL の状態条件を即時更新する。`validateSearch` は複数の `status` をカンマ区切りの正規形として保持し、画面では配列に戻す。状態を外す操作と「すべて」を用意し、検索・並び替えのフォーム送信でも選択済みの状態を保持する。件数は検索語や案件条件に関係しない DB 全タスクの数とし、範囲を画面に明示した。

`GET /api/v1/tasks/counts` は既存の `TaskStore::count_by_status`（SQLite の `GROUP BY status`）から集計値だけを返す。一覧の行は取得しない。web の件数 query は task list 系の key に置き、状態が変わる SSE で再取得する。gateway は `/api/tasks/counts` を既存の規則で中継する。daemon の複数 status 指定は `QueryParams::list` と `ListFilter.statuses` で OR として処理されることを API 試験で確認した。

API の契約は `gui-api.md`、生成した `api-v1.schema.json`、追加操作の `task-counts.openapi.yaml` に記録した。`web/api/generated/types.ts` も同じ schema から再生成した。CLI の `celerisctl show --json` はタスク詳細を返す既存契約を保ち、件数専用 API は CLI に追加しない。

## 表示確認

変更前後の `/tasks` を 360/390/412/1440px で撮影した。画像は run の `before-tasks/` と `after-tasks-final/` にある。スマホ幅では chip の横スクロール、下部タブ、一覧の先頭行が共存する。デスクトップ幅では全状態の件数が一覧の前に見える。

## 検証

| コマンド | 結果 |
|---|---|
| `cargo test -p task-api --test list_and_detail task_counts_group_all_statuses_without_list_filters` | 成功 |
| `pnpm -C web typecheck` | 成功 |
| `pnpm -C web test` | 成功（Vitest 363 件、server tests 成功） |
| `pnpm -C web e2e web/e2e/work/task-status-filters.spec.ts` | 成功 |
| `WEB_E2E_WORKERS=2 pnpm -C web e2e` | 成功（functional 243 件成功、8 件 skipped） |
| `bash scripts/dev/test-parallel.sh` | 成功（3,943 件成功、13 件 ignored） |
| `cargo clippy --workspace -- -D warnings` | 成功 |
| `cargo fmt --all -- --check`・`git diff --check`・web の Biome 対象ファイル検査・`check:boundaries` | 成功 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`bash scripts/dev/check-doc-links.sh` | 成功 |

途中の functional E2E では、変更後の checkbox 前提試験、空状態の URL、検索入力と scroll の戻りで失敗した。試験の操作を新しいボタンに更新し、空状態の URL を正規化し、検索入力と件数エラー表示の高さを安定させた後、全 243 件が通過した。
