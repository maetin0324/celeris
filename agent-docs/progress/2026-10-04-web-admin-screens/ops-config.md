---
tasks: [01M43XZ299ZQYR8M4F65B97FEB]
unit: accounts
status: running
completed:
---
# ops-config: accounts・providers（secrets・MCP 含む）の表・form・secret の扱い

差分の基点は ac78fad613ad。この file は accounts 節を accounts 葉が書き、providers 節を providers 葉が追記する。

## accounts

### 変えたもの（commit 72bb4d9c）
- `web/features/ops/accounts-screen.tsx`
  - account ごとの card（`li aria-label="アカウント <id>"` は保つ）に、状態の Badge（必ず文字付き）と DataList（状態の補足・adapter・認証情報・最終確認・実行中の run・これまでの run）を置いた。
  - 状態の写像 `accountState`（重い順）: 除外中（danger）→ 認証失敗・起動失敗（danger、last_check）→ 休止中（warning、cooldown と期限）→ 利用制限中（warning）→ ログイン待ち（info）→ 未ログイン（warning、「期限切れか未作成」）→ ログイン済み（success）。
  - 認証情報は値を DOM に出さず「保存あり（値は表示しません）」「保存なし」だけ。
  - 403/401 の操作結果が 1 つでも出たら、追加・確認・ログイン開始・削除・コード送信・取り消しを disabled にし、`role="alert"` の理由文（「この操作を行う権限がありません…」）へ `aria-describedby` で結ぶ。一覧の GET の 403 は既存の FetchFrame（permission-denied、subject「アカウント」）が出す。
  - 追加 form と認証コード入力は `noValidate` にし、空欄・422 などの失敗を項目の下の error（`aria-invalid` + `aria-describedby`）に出して、その入力へ focus を戻す。403 は項目の error にせず上の理由文に回す。
  - 主操作（アカウントを追加・コードを送る）を primary、削除を destructive にした。削除の確認文に「元に戻せません」を足した（parity e2e が `dialog.accept()` で進むため window.confirm のまま）。
  - 生の palette 名 `border-neutral-300` を token（`border-border bg-surface`）に替えた。入力欄の枠は `@layer base` の `--color-input` のまま（入力欄に色 class を付けない）。
- `web/features/ops/accounts-screen.test.tsx`（新規）: 状態の写像、badge と認証情報の文言、blocked 時の disabled 3 件と aria-describedby 3 件。
- `web/routes/accounts.tsx` は変更不要（loader なしのまま）。

### 自己レビュー（ui-ux-quality-gate、Light）
- 主な仕事: 「どの account が使えるか・何をすれば使えるか」を読む → 確認・ログイン・削除。状態を先頭の badge と 1 行目の補足に置き、操作は card の下に寄せた。
- 色だけで伝えない（badge は文字）、secret 値なし、破壊操作は確認つき、403 で送信を止め理由と次の手（管理者に確認）を出す。
- 残り: 360px では DataList が縦積みで card が長い（6 行）。account 数が多い運用では、狭い幅で「状態・認証情報・最終確認」以外を折り畳む余地がある（提案）。

### 検査結果
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0
- `corepack pnpm@12.6.0 -C web lint` → exit 0（既存の warning 4 件は styles.css 等で今回の差分外）
- `corepack pnpm@12.6.0 -C web test` → exit 0（vitest 48 files / 302 tests、server node --test pass 42 / fail 0）
- `corepack pnpm@12.6.0 -C web build` → exit 0
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/ops.spec.ts` → exit 0（8 passed）
- `corepack pnpm@12.6.0 -C web e2e e2e/a11y/axe.spec.ts e2e/parity/mobile-gate.spec.ts -g accounts` → 2 passed
- `corepack pnpm@12.6.0 -C web mobile-audit --only /accounts` → 1 path x 4 widths ok
- FRONTEND_CONTRACT の生の色・任意値 grep を `web/features/ops/accounts-screen.tsx web/routes/accounts.tsx` に当てて 0 件

### screenshot
- before: `<WU artifacts>/before-accounts/_accounts-{360,390,412,1440}.png`
- after: `<WU artifacts>/after-accounts/_accounts-{360,390,412,1440}.png`
  （WU artifacts = `/local/celeris/data/workspaces/01M43XZ299ZQYR8M4F65B97FEB/wu/accounts/artifacts`）

### fixture 要望（web/e2e/support/ は変えていない）
- fake-daemon の accounts に、状態の各種（`last_check: auth_failed`・`cooldown`・`excluded_reason`・`logged_in: false`）の account を足すと、screenshot と e2e で状態の badge を全種見られる。今は「ログイン済み」1 件だけ。
- 403 を返す経路（例: 特定 id の POST /accounts/:id/check が 403）があると、操作停止と理由表示を e2e で確かめられる。今は vitest（静的描画）でだけ確かめている。

### 提案
- StatusBadge は Celeris の状態語（task/WU/run）に限られるため、account の状態は同じ Badge（tone + 文字）で出した。設定系の状態語（接続・期限切れ・失敗）を StatusBadge 側の写像に足すかは components 担当で決めたい。
