---
tasks: [01M4X1AKJ2CG1F7S5YTMYFJ3B0]
wu: web
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# web: クラスタ画面にローカルマシン行と設定フォームを出す

設計は [ADR 2026-10-14-local-machine-settings](../../adr/2026-10-14-local-machine-settings.md)（D1・D3・D4）。API は backend の葉（[backend.md](backend.md)）。crates/ は変えていない。

## 変更
- `web/features/ops/clusters-screen.tsx`
  - 状態表（PC の表・スマホの一覧）の先頭に `GET /clusters` の `local` の行（`local（<hostname>）`、badge「このマシン」）。
    一覧の件数は `items.length + 1`。リモートが 0 件でも表と操作の節を出す。
  - 操作の節の先頭に `LocalMachineCard`: 使用中（`in_use / max_concurrency`）、作業ディレクトリ・同時実行数の実効値と
    source（`cli`=CLI の --workspace-root／`settings`=画面で保存した値／`config`=設定ファイル）、上書き中は設定ファイルの値も、
    反映の時期の badge（`new_tasks`=新しい task から反映、`next_tick`=次の tick から反映）、scratch の root（表示のみ）、最終更新。
  - フォーム: 作業ディレクトリ・同時実行数（空欄 = 上書きなし。placeholder に設定ファイル値）。「ローカルの設定を保存」で
    `PUT /api/clusters/local/settings` に全置換の本文（空欄は `null`）、「上書きを消す」で両方 `null`。
    同時実行数は 1〜256 の整数だけを送り、外れたら送らずに文言を出す。サーバの 422（path の検証など）は `detail` を alert で出す。
    成功時は応答（変更後の `LocalMachineView`）の各欄の反映の時期を `local-settings-applied` に出す。
  - **restart_required の表示**: backend の `apply` は `new_tasks` / `next_tick` の 2 値。再起動が要るのは
    `workspace_root.source == "cli"`（CLI の `--workspace-root` が store より優先。ADR D3）のときで、この欄は
    「daemon の再起動が必要」（warning）と「CLI の上書きを外して daemon を再起動した後に効く」の注記を出す。
    `apply == "restart_required"` が来た場合も同じ表示にする。CLI の値は store の上書きではないので入力の初期値にしない。
  - 403 で画面の操作を止める既存の挙動にローカルのボタンも従う。
- `web/features/ops/clusters-screen.test.tsx`（新規、6 件）: 表示（値・source・反映）、source の語彙、
  保存の成功（PUT の path・method・本文と反映の表示）、空欄 = null、検証エラー（範囲外は送らない・422 の文言）、
  restart_required（`apply` と `source: cli`）。
- e2e: `web/e2e/support/fake-daemon.mjs` に `local` の状態と `PUT /clusters/local/settings`（422 の検証つき）、
  `web/e2e/parity/ops.spec.ts` に「parity: /clusters ローカルマシンの作業ディレクトリと同時実行数」（範囲外の入力 →
  相対 path の 422 → 保存して反映の表示と値 → 上書きを消す、PUT 3 回）、`web/e2e/admin/ops-runtime.spec.ts` の
  route 応答に `local` を足し、ローカル行・403 で保存が無効・360px の件数を確かめる。
- 生成型: `corepack pnpm@12.6.0 -C web gen:types` は差分なし（`LocalMachineView` 等は api 葉で生成済み）。gui/ に生成型は無い。

## 証拠
- `corepack pnpm@12.6.0 -C web install --offline` → Done
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0
- `corepack pnpm@12.6.0 -C web exec vitest run features/ops/clusters-screen.test.tsx` → 6 passed
- `corepack pnpm@12.6.0 -C web test` → exit 0（vitest 93 files / 656 tests passed、node --test 込み）
- `corepack pnpm@12.6.0 -C web lint` → exit 0（既存の warning 4 件、変更 file には無し）
- `pnpm build` 後 `playwright test e2e/parity/ops.spec.ts e2e/admin/ops-runtime.spec.ts` → 19 passed
- `node web/scripts/screenshots.mjs --only /clusters --widths 390,1440`（build 後）→ 2 枚。先頭にローカル行と設定カードが出ることを目視で確認
  （画像は run の artifacts/screenshots。リポジトリには置かない）。

## 未解決事項
- なし（Rust の全体検査は closeout の葉）。

## 提案
- なし。
