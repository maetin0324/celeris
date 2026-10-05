# browser run と待ちへの導線

---
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
---

## 変更

- task 詳細の概要に browser run ごとの状態・lease badge・待ち件数・Live View へのリンクと、その場で答えられる待ち panel を追加した。待ちがあるときは上部の「次の操作」から `#browser-waits` へ移れる。スマホの区画切替でも概要を開く。
- 受信箱の `browser_wait` に理由別 badge と run 画面へのリンクを追加した。`InboxItem` に run ID が無いため、公開の待ち一覧を wait ID と task ID で照合する。照合できないときは task 詳細の `#browser-waits` に移る。
- 承認画面にも未決の browser 待ちと run 画面へのリンクを追加した。shell・ScreenFrame・`web/styles.css`・gateway・gui・crates は変更していない。
- inbox model の badge・リンク先を Vitest で固定し、受信箱 → run、task 詳細 → Live View、task 詳細での decision、承認画面 → run を Playwright で確認した。

## 検査

| コマンド | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web test` | 合格（Vitest 428、gateway 52） |
| `corepack pnpm@12.6.0 -C web typecheck` | 合格 |
| `corepack pnpm@12.6.0 -C web lint` | 合格（既存の warning 5 件） |
| `corepack pnpm@12.6.0 -C web check:boundaries` | 合格 |
| `WEB_E2E_SCOPE=functional ... playwright test e2e/browser/links.spec.ts` | 2 件合格 |
| `WEB_E2E_SCOPE=functional ... playwright test e2e/work/inbox-notifications.spec.ts e2e/work/reports-approvals.spec.ts e2e/parity/tasks.spec.ts e2e/parity/task-detail.spec.ts e2e/parity/inbox.spec.ts --grep-invert '空白帯が無く.*360x800\|stale の / [0-9]+: scroll 前'` | 25 件合格、fixture screenshot 2 件 skip |
| `corepack pnpm@12.6.0 -C web mobile-audit --only /tasks/T1` | 360/390/412/1440px 合格 |

変更前は WU base `6de936dc26e5` を `/tmp` に展開して build し、変更後はこの worktree を build して、同じ browser fixture を撮影した。画像は WU の `artifacts/screenshots/{before,after}/` にあり、task・inbox・approvals の各 4 幅、計 24 枚。変更後の 4 幅で横溢れが無いことも Playwright で確認した。

## 未解決事項

- main 由来の `home-layout.spec.ts:26`（360x800）と `home-stale-viewport.spec.ts` はこの葉の範囲外。上記 functional 検査では指定の `--grep-invert` で除外した。
