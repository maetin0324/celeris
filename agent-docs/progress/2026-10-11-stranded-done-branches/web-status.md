# Web stranded integration status
---
tasks: [01M4N33RH4ER5SRZ902QMR1PQJ]
---

## 実装

- task 一覧と詳細で `unintegrated`（未取り込み）・`integrated`（取り込み済み）・`discarded`（捨てた）・`held`（保留）を区別し、独立した promotion の `promotion_pending` を「昇格待ち」と表示する。詳細には repo・branch/head・最終確認時刻・理由も表示する。
- task 詳細と受信箱から merge/discard/hold/reopen を実行する。詳細は未取り込み時に merge/discard/hold、捨てた・保留時に reopen を表示する。
- 詳細は `IntegrationView.repo`（名前）を URL エンコードし、`POST /api/v1/tasks/{id}/integrations/{repo}/{merge|discard|hold|reopen}` に送る。`repo_id` は識別キーであり route の引数ではない。
- 受信箱は `answer.native.path` の merge route を使い、他の操作は末尾の `merge` を置換する。汎用 inbox answer API、既存 ADR-0043 の changes API、integration_repair は利用しない。
- discard/hold は空白のみの理由を拒否し、生成型 `ReasonBody` に従って `{reason: string}` だけを送る。`ReasonBody` は未知フィールドを拒否するため、旧実装の `note` と `confirm` は送らない。根拠は `crates/task-api/src/integrations.rs` の `ReasonBody`・`resolve_repo`。
- 破棄は共通 ConfirmDialog で対象・影響・reopen による復帰・確認先を提示してから送信する。入力とボタンは 44px 以上、送信中は再送を無効化する。
- merge の応答が `integrated: false` の場合は「取り込みを依頼しました」と表示し、完了とは扱わない。状態の再取得は既存 SSE の integration event による invalidation を利用する。
- crates/ と API 生成型は変更していない。

## 回帰試験と検証（attempt 3）

`web/features/tasks/task-integration.test.tsx` は 10 件。既存の Chromium/Vite fixture 基盤を使い、実コンポーネントのボタン押下で発生した HTTP の method・path・body を検証する。repo 名と ID を別にし、詳細には日本語と空白を含む名前、受信箱には別 repo の native route を与える。両入口の merge/discard/hold、held/discarded と受信箱の reopen、空白理由の拒否、破棄確認のキャンセルを確認する。4 状態と独立した昇格待ちの表示も試験する。

成功した検査:

- `corepack pnpm@12.6.0 -C web install --prefer-offline --frozen-lockfile && corepack pnpm@12.6.0 -C web typecheck && corepack pnpm@12.6.0 -C web lint && corepack pnpm@12.6.0 -C web test` — Vitest 93 files / 686 tests、server 93 tests。lint は既存 styles.css の `!important` 警告 4 件のみ。
- acceptance の `unintegrated` / `discarded` テスト文字列検査。
- `corepack pnpm@12.6.0 -C web check:boundaries` / `build`。
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/tasks.spec.ts e2e/parity/inbox.spec.ts e2e/a11y/axe.spec.ts` — functional 12 passed、任意の screenshot 試験 2 skipped。この指定では axe は対象外となるため次のコマンドで別途検証した。
- `corepack pnpm@12.6.0 -C web e2e a11y/axe.spec.ts --grep 'S4 /(tasks|inbox)'` — 7 passed。
- `corepack pnpm@12.6.0 -C web mobile-audit --only <path>`（`/tasks`・`/tasks/T1`・`/inbox`）— 各 360/390/412/1440px で成功。
- 新しい取り込み fixture 自体も同じ 4 幅で axe serious/critical 0 件、44px target と横溢れなしを確認。確認ダイアログの前後を撮影した。

ログと確認画像は run の成果物ディレクトリ `/local/celeris/data/workspaces/01M4N33RH4ER5SRZ902QMR1PQJ/artifacts/` の `web-test.log`・`web-build.log`・`web-e2e.log`・`web-axe.log`・`integration-shots/` に保存。
