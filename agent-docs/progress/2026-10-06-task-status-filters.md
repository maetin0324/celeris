---
title: タスク一覧の状態切り替えと件数表示
tasks: [01M475Z7T5E8PAPGBCE4XG2K7B]
status: done
updated: 2026-10-06
---
# タスク一覧の状態切り替えと件数表示

## 原因と修正

状態 chip はフォーム内の checkbox で、押しただけでは検索条件が送信されず、選択の強調も URL から決まっていた。このため押下直後は画面も一覧も変わらなかった。また `/tasks` の `validateSearch` が `status` を返さず、複数選択した URL でもルートの状態に残らなかった。

chip を `aria-pressed` 付きのボタンに変え、押下時に URL の状態条件を即時更新する。`validateSearch` は複数の `status` をカンマ区切りの正規形として保持し、画面では配列に戻す。状態を外す操作と「すべて」を用意し、検索・並び替えのフォーム送信でも選択済みの状態を保持する。件数は検索語や案件条件に関係しない DB 全タスクの数とし、範囲を画面に明示した。

`GET /api/v1/tasks/counts` は既存の `TaskStore::count_by_status`（SQLite の `GROUP BY status`）から集計値だけを返す。一覧の行は取得しない。web の件数 query は task list 系の key に置き、状態が変わる SSE で再取得する。gateway は `/api/tasks/counts` を既存の規則で中継する。daemon の複数 status 指定は `QueryParams::list` と `ListFilter.statuses` で OR として処理されることを API 試験で確認した。

API の契約は `gui-api.md`、生成した `api-v1.schema.json`、追加操作の `task-counts.openapi.yaml` に記録した。`web/api/generated/types.ts` も同じ schema から再生成した。CLI の `celerisctl show --json` はタスク詳細を返す既存契約を保ち、件数専用 API は CLI に追加しない。

`gui/app/celeris/types.ts` は今回再生成しない。共通 schema を再生成すると GUI 側にも `TaskStatusCounts` と `task_status_counts` が増えるが、このタスクの指示「gui/ は変えない」を優先した。GUI はこの新しい API を呼ばないため実行上の影響はない。GUI の生成型と共通 schema の完全一致は今回の範囲に含めず、GUI を変更する作業で `pnpm -C gui gen:types` を実行する。

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

## 再審査対応

前回の実装 commit `f8a9460a` はそのままに、進捗を `docs/progress/` から `agent-docs/progress/` へ移し、索引に必要な `title`・`tasks`・`status`・`updated` を付けた。`progress-index.sh` の一覧にこの文書が出ること、`check-doc-layout.sh`・`check-doc-links.sh` が通ることを確認した。`progress-index.sh --check` は既存の `2026-10-05-web-artifact-viewer.md`（title/updated 不足）と `2026-10-06-web-tabbar-first-screen.md`（tasks/updated 不足）で失敗する。今回の文書については警告なし。今回の差分は文書だけで、上記の UI・API の検証結果は前回の実装に対する結果である。
