# /browser/settings の site policy と credential grant 設定

---
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

2026-10-08、web-site-policy 葉を実装・検証済み。

- ログイン先の一覧と追加・編集フォームに policy_id、exact_origin、login_url、password_selector、任意の submit_selector を表示。編集中は ID を固定し、空の submit selector は null で送る。削除と未保存入力の破棄は ConfirmDialog を通す。
- browser-execution grant の credential_use と credential_policy_ids を編集。登録済みログイン先から選択し、既に grant が参照している未登録 ID も表示して解除できる。使用のたびに本人の承認が必要な旨を表示する。
- site policy と grant の入出力は web/api/generated の SitePolicyList、SitePolicyPutBody、SitePolicyPutResult、BrowserSettingsPatch、OrgNode を使用。保存・削除後は active query を再取得する。409 site_policy_in_use は参照解除と承認待ちの解消を案内し、変更を自動再送しない。grant の 422 は従来の項目別エラー表示を維持する。
- UI の編集は owner session があるときだけ表示。gateway の PUT /browser/site-policies/:id、DELETE /browser/site-policies/:id、PATCH /browser/settings も owner session・same-origin・CSRF を検査し、CSRF は daemon に送らない。通常の /api relay から同 API を変更する迂回も、符号化 path を含めて拒否する。/browser/settings の GET/HEAD は SPA 表示を維持。
- ADR D5 の GET /api/browser/readiness を読み、daemon の点検項目と説明を日本語の状態 badge で表示。preflight 葉の公開 response（items: status/check/detail）に合わせた読み取り型を使用する。現在の base の生成 schema に readiness は未収録であり、この葉は crates/・生成 schema を変更しない。点検 API 自体は preflight 葉との工程統合で利用可能になる。
- docs/frontend/DESIGN.md・FRONTEND_CONTRACT.md に従い Input、Button、Section、DataList、Badge、ConfirmDialog、FetchFrame を使用。44px の checkbox label と操作領域、エラー・再取得・空状態を用意。

## 検証

すべてこの専用 worktree で実行。本番 host・本番 DB の操作は行っていない。

| コマンド | 結果 |
|---|---|
| corepack pnpm@12.6.0 -C web typecheck | 成功 |
| corepack pnpm@12.6.0 -C web lint | 成功。styles.css の既存 reduced-motion !important 警告 4 件 |
| corepack pnpm@12.6.0 -C web test | Vitest 83 files / 601 passed、gateway 78 passed、失敗 0 |
| node --test web/server/browser-settings.test.mjs web/server/browser-live.test.mjs | 最終 gateway 変更後に再確認、20 passed |
| corepack pnpm@12.6.0 -C web e2e browser/settings.spec.ts | 3 passed。追加・編集・削除の入力と送信、再取得、credential grant の送信、既存設定・入力エラーを確認 |
| corepack pnpm@12.6.0 -C web mobile-audit --only /browser/settings | 4 幅（360/390/412/1440px）で成功 |
| 上記 E2E の編集フォーム検査 | 4 幅で axe serious/critical 0、ページ横溢れ 0、操作・checkbox label の44px確認 |
| corepack pnpm@12.6.0 -C web build | 成功 |
| corepack pnpm@12.6.0 -C web check:boundaries | 成功 |
| corepack pnpm@12.6.0 -C web check:secrets | 成功 |
| git diff --check / crates/ 差分検査 | 成功、crates/ 差分 0 |
| CELERIS_WU_BASE からの計画の差分範囲検査 | 不合格。必要な gateway 3 ファイルが許可範囲から欠落（下記） |

Vitest は追加・置換・削除と grant 有効化/無効化の送信内容、active QueryObserver の再取得、失敗時の非再送、owner CSRF 不在時の拒否、フォームの既存値、点検結果の表示を確認する。
実画面 E2E は loopback の偽 daemon と page.route の site-policy/readiness fixture を使用する。
本番 API の Rust 試験は先行 site-policy-api 葉、workspace Rust の全体 gate は close-out 葉の担当。

実行ログと 4 幅の設定保存前後・site policy 編集中の screenshot は run の成果物ディレクトリ
`/local/celeris/data/workspaces/01M4CDNAYX6J68WTX7SKF0DJ64/wu/web-site-policy/artifacts/`
の web-*.log、settings-e2e.log、gateway-test.log、mobile-audit.log、settings-screenshots/ に保存。

## 範囲チェックの再確認

2026-10-08 の再実行で、計画の範囲チェックが exit 1 になることを確認した。
`web/server/browser-live.js`、`web/server/relay.js`、`web/server/browser-settings.test.mjs`
が許可範囲から欠落している。ADR D3 は編集操作に owner session を要求する。
browser-live.js は owner session・same-origin・CSRF を検査して daemon へ転送し、
relay.js は通常 API relay による本人確認の迂回を拒否する。browser-settings.test.mjs は
その拒否と正常な転送を検証する。画面だけでの検査は直接 HTTP 要求で迂回できるため、
これらを削除して範囲チェックを通すことはできない。

同じチェックの許可パターンに上記 3 ファイルを追加する必要がある。
既存実装を維持し、plan_issue として再計画を要求する。done は返さない。
この再実行で typecheck・lint（既存の警告 4 件）・test（Vitest 601 件、gateway 78 件）は成功。
crates/ 差分は 0、git diff --check も成功。本番の操作は実施していない。

## 再計画後の確認（2026-10-08, run 01M4CQCK71R9V4HX6CZBZ2TZ6B）

再計画で範囲に gateway 3 ファイル（browser-live.js・relay.js・browser-settings.test.mjs）が加わった。
実装は 913c8dc5 のまま変更なし。CELERIS_WU_BASE からの変更は web/ の 9 ファイルとこの進捗ファイルだけで、すべて範囲内。

| コマンド | 結果 |
|---|---|
| corepack pnpm@12.6.0 -C web typecheck | exit 0 |
| corepack pnpm@12.6.0 -C web lint | exit 0（既存の警告 4 件） |
| corepack pnpm@12.6.0 -C web test | exit 0。Vitest 83 files / 601 passed、gateway 78 passed / 0 failed |
| git diff --name-only $CELERIS_WU_BASE HEAD -- crates/ | 0 件 |

未解決: readiness API は preflight 葉との統合で有効になる。Rust の全体 gate は close-out 葉の担当。
