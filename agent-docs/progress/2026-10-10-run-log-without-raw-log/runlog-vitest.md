---
tasks: [runlog-vitest]
---

# run ログ生ログなし表示の Vitest

追加した試験:

- `RunLogScreen stdout availability > does not enable or fetch raw stdout when files.stdout is false, and shows protected progress and result`
  - `files.stdout: false` で `useRunLog` の stdout polling が無効（`enabled=false`）になり、fetch が発生しないこと、生ログを保存しない案内、worker_progress、result の summary/question を表示することを確認。
- `RunLogScreen stdout availability > keeps the stdout log path and rendered output when files.stdout is true`
  - `files.stdout: true` で stdout hook が有効になり、従来のログ表示を維持することを確認。
- `task overview run link > labels a run without stdout as progress`
  - stdout のない run を「進捗を開く」と表示することを確認。
- `task overview run link > keeps the run id link when stdout is available`
  - stdout のある run は run id 表示を維持することを確認。

実行結果:

- install: `COREPACK_ENABLE_NETWORK=0 corepack pnpm@12.6.0 -C web install --prefer-offline` — exit 0
- test: `corepack pnpm@12.6.0 -C web exec vitest run features/runs features/tasks` — exit 0、9 test files / 45 tests passed
