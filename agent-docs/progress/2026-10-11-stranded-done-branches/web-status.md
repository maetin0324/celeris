# Web stranded integration status

tasks: [01M4N33RH4ER5SRZ902QMR1PQJ]

## 実装

- task 一覧と詳細で integration の `unintegrated` / `integrated` / `discarded` / `held` を区別し、promotion の `promotion_pending` は「昇格待ち」と表示する。
- 詳細と受信箱の未取り込み item に merge / discard / hold / reopen 操作を追加。discard と hold は理由を要求し、discard は `confirm: true` を送る。
- task-api の integration route を用い、既存 `task_integrations` と `integration_repair` は操作に使わない。
- 操作 UI のボタンと入力は `min-h-11 min-w-11` を確保。

## 確認

- `corepack pnpm@12.6.0 -C web install --prefer-offline --frozen-lockfile && corepack pnpm@12.6.0 -C web typecheck && corepack pnpm@12.6.0 -C web lint && corepack pnpm@12.6.0 -C web test` 成功（vitest 678 tests、server 93 tests）。lint は既存 `styles.css` の `!important` 警告 4 件のみ。
- 状態回帰試験を `web/features/tasks/task-integration.test.tsx` に追加。
