---
tasks: [01M43XZ299ZQYR8M4F65B97FEB]
unit: providers
status: done
completed: 2026-10-04
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

## providers（secrets・MCP を含む）

### 変えたもの（commit 7107180b・6daaeea8・80efd3df・b0fe0ad6）
- `web/features/ops/providers-form.ts`: 送る前の検査 `validateProviderForm`（id 必須・concurrency は 0 以上の整数）、403/401 の判定 `isDenied` と理由文 `deniedMessage`、実行枠の状態の写像 `providerState`（重い順: 認証失敗・起動失敗（danger）→ 休止中（warning、理由と期限）→ 利用制限中（warning）→ 接続確認済み（success）→ 未確認（neutral））。
- `web/features/ops/providers-screen.tsx`
  - 節「adapter / harness の実行枠」の先頭に状態の表（`Table aria-label="実行枠の状態"`: 実行枠・状態・道具・tiers・同時実行・前回の確認）。状態は文字の Badge。
  - card（`li aria-label="プロバイダ <id>"`・h3 は保つ）に状態の Badge と DataList（状態の補足・前回の確認・認証情報・これまでの run）。認証情報は env の名前だけで値は出さない。接続を確認した結果は card と表の badge に反映する。
  - 追加・変更 form は `noValidate`。各入力に可視 label、項目の下の error（`aria-invalid` + `aria-describedby`）、送信失敗時は入力順で最初の error へ focus。422 は id 欄（変更は concurrency 欄）に結ぶ。
  - 403/401 が 1 つでも出たら、保存・接続を確認・削除・追加を disabled にし、`role="alert"` の理由文へ `aria-describedby` で結ぶ。GET の 403 は FetchFrame（subject「プロバイダ」）。
  - LLM source の行に有効・到達性の Badge（届かない時は理由）。追加を primary、削除を destructive にした。
  - 節の名前は `aria-label="プロバイダ一覧"` のまま（見出しの「adapter」を名前に流すと parity の `getByLabel("adapter")` と重なるため Section primitive にしなかった）。
- `web/features/ops/secrets-section.tsx`（/accounts の下段）
  - secret は値も fingerprint も出さず、「保存あり / 未設定」の Badge と DataList（値: 表示しません・更新日時・使っている所 used_by）だけ。
  - 「置き換え」→ 行内の password 入力 →「値を置き換える」で ConfirmDialog（対象・影響する設定・次の run から使われる・前の値には戻せない・更新日時で確かめる）。空なら dialog を開かず項目の error と focus。送ったら入力を空にする。
  - 追加 form で既にある id は上書きせず、置き換えへ案内する error を出す。空欄は項目ごとの error と focus。
  - 403/401 で保存・置き換え・削除を止め、理由文を出す。
- `web/features/ops/mcp-clients.tsx`: summary を名前 + 状態の Badge（有効 / 失効）にし、scopes・最終利用・作成・失効日時を DataList、呼び出しの成否を Badge にした。空の一覧の文言。
- `web/features/ops/providers-llm-source.ts`・`web/routes/providers.tsx` は変更不要（色・任意値なし、loader なし）。
- 試験: `providers-screen.test.tsx` に 5 件（状態の写像・表の badge・form 検査・secret の行に値が無い・MCP の状態）、新規 `web/e2e/admin/ops-config.spec.ts` に 6 件（providers の状態の変化、追加 form の error・focus・422、providers の 403 停止、secret の値が DOM に無い・置き換えの ConfirmDialog・削除の確認文、secret の 403 停止、MCP の badge）。403 は fixture に無いので `page.route` で browser の要求に 403 を返している。

### 受け入れ条件との対応と残る差
- secret の削除は ConfirmDialog ではなく `window.confirm`（確認文に影響する設定と戻せないことを書いた）。理由: 編集できない `web/e2e/parity/ops.spec.ts` が「secret を削除」・プロバイダ「削除」を `page.on("dialog", accept)` の native confirm で進めるため、ConfirmDialog にすると parity が落ちる。置き換えは ConfirmDialog。accounts 葉の account 削除と同じ扱い。

### 自己レビュー（ui-ux-quality-gate、Light）
- 主な仕事: どの実行枠・secret・client が使えるかを読む → 確認・保存・置き換え。状態を表と card の先頭の badge に置き、詳細と操作を card に寄せた。
- 赤線: secret 値・fingerprint を出さない、破壊操作は確認つき、状態は文字（色だけにしない）、403 で送信を止め理由と次の手を出す、360px で表は名前つき枠の中だけ横スクロール（axe・mobile-audit で横溢れ 0）。
- 残り: 表と card で同じ状態を 2 回出す。実行枠が多い運用では card を開閉式にする余地（提案）。

### 検査結果（providers 葉の最終、HEAD b0fe0ad6）
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0
- `corepack pnpm@12.6.0 -C web lint` → exit 0（既存 warning 4 件は styles.css 等で差分外）
- `corepack pnpm@12.6.0 -C web test` → exit 0（vitest 48 files / 307 tests、server node --test pass 42 / fail 0）
- `corepack pnpm@12.6.0 -C web build` → exit 0
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/ops.spec.ts e2e/admin/ops-config.spec.ts e2e/a11y/axe.spec.ts e2e/parity/mobile-gate.spec.ts -g "ops-config|P4-|accounts|providers"` → 18 passed（parity ops 8・ops-config 6・axe/mobile-gate の /accounts・/providers 4）
- `corepack pnpm@12.6.0 -C web mobile-audit --only /providers`・`--only /accounts` → 各 1 path x 4 widths ok
- FRONTEND_CONTRACT の生の色・任意値 grep を task の 8 file（accounts 側を含む）に当てて 0 件

### screenshot
- before: `<WU artifacts>/before-providers/_{providers,accounts}-{360,390,412,1440}.png`
- after: `<WU artifacts>/after-providers/_{providers,accounts}-{360,390,412,1440}.png`
  （WU artifacts = `/local/celeris/data/workspaces/01M43XZ299ZQYR8M4F65B97FEB/wu/providers/artifacts`）

### fixture 要望（web/e2e/support/ は変えていない）
- fake-daemon の providers に `last_check: auth_failed`・`cooldown`・`throttled`・`env_keys` を持つ実行枠を足すと、表の状態を全種 screenshot と e2e で見られる。今は claude-main（未確認）1 件。
- secrets の初期値に `used_by` を持つ secret を 1 件置くと、置き換え・削除の影響文（使っている所）を e2e で見られる。今は空。
- MCP clients に `revoked_at`・`last_used_at` を持つ client、calls に `ok: false` の行を足すと「失効」「失敗」の badge を見られる。
- 403 を返す経路（provider check・secret PUT）があれば `page.route` を使わずに確かめられる。
- LLM sources に `reachable: false` と `unreachable_reason` の source を足すと「届かない」の badge と理由を見られる。

### 提案
- `web/e2e/parity/ops.spec.ts` の担当（ops-runtime 葉）に、secret とプロバイダの削除手順を ConfirmDialog の確認 button を押す形へ変えてもらえば、削除も ConfirmDialog に移せる（各 1 箇所の差し替えで済む）。
- 設定系の状態語（接続確認済み・休止中・認証失敗・失効など）は StatusBadge の写像に無いので Badge（tone + 文字）で出した。StatusBadge に設定系の写像を足すかは components 担当で決めたい（accounts 節と同じ提案）。
