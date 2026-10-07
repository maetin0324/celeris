---
tasks: [01M47C1EBX17QKSS3AD2TX1SHK]
---
# web のブラウザ実行課 設定画面

`/browser/settings` を追加し、browser 画面から入れるようにした。`GET /api/org` の `browser-execution` node を読み、`PATCH /api/org/browser-execution/browser-settings` で許可 origin、harness、予算、credential policy ID と identity ID の対応を一括保存する。追加・削除した origin とその他の編集値は確認 dialog に出し、確定後に送る。サーバーの 422 は field と message を見て各節に示し、保存が失敗した場合は画面の入力を保つ。scheme と wildcard の入力補助も置いた。

保存成功後は API 応答の `updated_at` と、この画面での保存操作に対応する actor `admin` を表示する。管理 API は actor `admin` と before/after を `org_browser_events` に保存するが、その履歴を読む API はまだない。そのためページを再読込した後の過去の actor は表示せず、「この画面で保存した変更」と明記した。現在値の更新時刻は再読込後も表示できる。履歴一覧を実装するには読み取り API が要るが、この子 task の `crates/` 無変更という範囲では追加しない。

fixture は loopback 2 件の初期値と管理 API の成功・422・監査 event を持つ。`web/e2e/browser/settings.spec.ts` は編集成功、確認前の未送信、actor と時刻、scheme・`*`・public suffix wildcard の拒否と event 不変を確かめる。4 幅の保存前後のスクリーンショットは run の成果物ディレクトリ `settings-shots/` に保存した。360px で axe の serious/critical は 0、4 幅で横スクロールは 0。

検証: `corepack pnpm@12.6.0 -C web test`、`typecheck`、`lint`、`build`、`WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 exec playwright test e2e/browser/settings --workers=1`。
