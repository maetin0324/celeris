---
title: 旧 gui/ の provider 画面と provider API 呼び出しを消す（gui-purge）
date: 2026-10-12
unit: gui-purge
adr: agent-docs/adr/2026-10-11-llm-source-harness-selection.md
status: partial
---

# gui-purge: 旧 gui/ の provider 前提を消す

## 完了した範囲

- `gui/app/lib/routing-catalog.ts` と `gui/test/unit/routing-catalog.test.tsx` を削除（呼び出し元は `/providers` だけだった）。
- `gui/e2e/g4・g5・g5-a11y・g8・g9` の `/providers` 訪問を外した。
  - g4: multi-account の provider 画面の試験を、`/tasks/<id>/runs` の usage 合計が 0 より大きいことを見るだけの試験に置き換えた（画面上の行との突き合わせは消えた）。`/daemon` と `/providers` のバナー確認は `/accounts` に替えた。
  - g5・g5-a11y: 対象ページを `/accounts` に替えた。
  - g8・g9: provider の追加・削除の手順を消し、アカウントの追加・ログイン・確認・削除の手順だけ残した。
- `gui/scripts/lib/celeris-fixture.mjs`: `providers` route、`GET /api/v1/providers` mock、`GET /api/v1/llm/routing/catalog` mock、`GET /config` の `providers: []` を削除。
- `gui/test/mock-celeris/fixtures.ts`: `routingCatalogView` と import、チェック表示の `providers` 語を削除。
- `gui/test/unit/client.test.ts`: PATCH・DELETE の試験を実在の路（`/projects/{id}`、`/llm/sources/{source}/accounts/{id}`）へ替えた。
- `gui/app` のコメント: `/providers` と `providers-admin.server.ts`・`routes/providers.tsx` を指す文言を現行の画面・file に直した。`accounts.tsx` の未使用 import（`Tier`）を削除。
- `gui/docs/celeris-api-v1.md`: §3.19・§3.24〜3.27 を削除の印に置き換え、route 表の provider 行に削除の印、監査表の provider 行を削除、`/providers` を指す文を直した。改訂履歴（2026-09-16 の行）と `docs/adr/`・`PROGRESS.md` は履歴なので変えない。
- `gui/docs/DESIGN.md`: §4.5 の見出しと表、route 表の `providers` 行、受け入れ条件の `/providers` を `/accounts`・`/llm/sources`・`/accounts` に替えた。

## 検査（2026-10-12、TMPDIR=/local/x/gp）

- `corepack pnpm@11.27.0 install --offline`: Done（exit 0）
- `corepack pnpm@11.27.0 typecheck`: exit 0
- `corepack pnpm@11.27.0 test`: Test Files 92 passed (92)、Tests 1307 passed (1307)、exit 0
- `corepack pnpm@11.27.0 build`: `✓ built in 966ms`、exit 0
- `npx biome check .`: 変更したファイルは通る。残りの format 違反は本 unit の範囲外の既存のもの（`app/components/task-detail/OverviewTab.tsx`、`app/root.tsx`、`app/routes/notifications.tsx`、`test/unit/notifications.route.test.ts`、`test/unit/notifications.test.ts`）。exit 1。
- `grep -rn "/providers\|api/v1/providers" gui/app gui/test gui/e2e gui/scripts`: 1 件（`gui/test/fixtures/run-log/claude-code-real.jsonl` の記録済み run ログの本文）。実行記録の本文なので変えない。
- `git diff -- crates web`: 差分なし。

## 未解決（この unit では直せていない）

1. **e2e の celeris 設定 fixture が現行 celeris に拒否される。** `crates/celeris/src/config/mod.rs:417` は `providers` / `providers.d` / `providers_include` を含む config を「legacy provider configuration is no longer supported」で読み込みエラーにする。次の file が `[[providers]]` か `providers_include` を使っており、このままでは `scripts/celeris.sh fixture …` で起動した celeris が立ち上がらない:
   - `gui/test/celeris/{multi-account,celeris,auth,accounts,delegation,clusters,org,unroutable}.toml.tmpl`、`gui/test/celeris/g14-browser-live.toml.tmpl`
   - `gui/scripts/celeris.sh`（fixture の説明文と `providers_include` の記述）
   - 置き換えは `[llm_sources.<id>]`・`[harnesses.<id>]`・`[route_defaults.<harness>]`（config の新形）。multi-account の fallback 試験（acct-a の throttle → acct-b）をどう表すかは設計判断が要るため、人か次の unit で決める。
2. `gui/docs/celeris-api-v1.md` の §3 以外（`GET /daemon` 例の `providers` 配列、`GET /config` の `providers[]` の説明、§4 の `[[providers]]` 設定の説明、`providers_include` の節）にはまだ旧 provider 語が残る。API の応答例と設定の節は、crates の config 形を決めてから直すのが安全。
3. e2e（Playwright）は実 celeris を起動する必要があり、この環境では未実行。g4 の `/accounts` バナー確認、g5-a11y の `/accounts` の a11y 検査は未確認。

## 提案

- 次の unit（web-purge や closeout）の前に、`gui/test/celeris/*.toml.tmpl` と `scripts/celeris.sh` の fixture を新 config 形へ移す unit を 1 つ切る。その unit の check に `scripts/celeris.sh fixture accounts` と `start` が通ることを入れる。

## 再実行（前回の check 不合格の対応、2026-10-12、TMPDIR=/local/x/ui）

前回の不合格の原因は 3 つ。

1. gui lint が 6 file の format 違反で落ちていた。違反はすべて本 unit が触っていない file（`app/root.tsx`、`app/components/task-detail/OverviewTab.tsx`、`app/routes/notifications.tsx`、`test/unit/notifications.test.ts`、`test/unit/notifications.route.test.ts`、`scripts/check-resume-recovery.mjs`）で、`CELERIS_WU_BASE` 時点で既に違反していた（2c69314e の gen-types 以降）。`biome check --write` で整形だけ行った（挙動の変更なし）。
2. grep 検査が `gui/docs/celeris-api-v1.md` の provider 行（`` `/providers` ``）と `gui/test/fixtures/run-log/claude-code-real.jsonl` の 1 行を拾った。
   - API 表の provider 行 5 本（旧 22・27〜30）を削除。§3.19 の本文は削除の一文に置き換え済み。
   - jsonl は実 run のログだが、Edit 記録の中の `types.ts` の複写に含まれる生成コメントの path 1 つだけを `/api/v1/…/{id}/check` に伏せた（JSON は有効。run-log の試験は行の種類と Edit の path だけを見るので結果は変わらない）。
   - `gui/docs/DESIGN.md` の route 一覧から `/providers` を削除。
3. `crates/` と `web/` の検査の第 1 項（`git diff --quiet $(git merge-base HEAD main) -- crates/`）は計画の問題。詳細は下の「計画の問題」。

## 検査（再実行、同一の chain）

- `corepack pnpm@11.27.0 -C gui install --offline`: Already up to date（exit 0）
- `typecheck`（react-router typegen && tsc -b）: exit 0
- `lint`（biome check .）: exit 0（info 2 件のみ。`useTemplate` の unsafe fix 提案で、既存の script）
- `test`（vitest run）: Test Files 92 passed (92)、Tests 1307 passed (1307)、exit 0
- `build`（react-router build）: `✓ built in 1.70s`、exit 0
- `! grep -rnE '/api/v1/providers|"/providers|/providers"|`/providers|providers-admin' gui/app gui/test gui/e2e gui/scripts gui/docs/celeris-api-v1.md`: exit 0（一致 0 件）
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ && git diff --quiet "$CELERIS_WU_BASE" -- web/`: exit 0（差分ゼロ）

## 計画の問題

- 検査 `git diff --quiet $(git merge-base HEAD main) -- crates/` が成り立たない。`merge-base(HEAD, main)` は `4ef38473` で、`CELERIS_WU_BASE`（`c6a6c285`）は main の祖先でない。そのため、この unit の変更とは関係なく、`4ef38473..` の crates は 341 file 変わっている（先行の impl-v2・closeout2 の統合が branch にあり、main にはまだ入っていない）。コードをどう直しても通らない。
- 提案: この検査の第 1 項を外す、または main に統合された後の基点（例: main の HEAD）で比べる。`CELERIS_WU_BASE` 基準の 2 項は通っている。

## 未解決（前回からの持ち越し）

- e2e の celeris 設定（`gui/test/celeris/*.toml.tmpl`、`gui/test/celeris/g14-browser-live.toml.tmpl`、`gui/scripts/celeris.sh`）は `[[providers]]` / `providers_include` を使い、現行の crates（`crates/celeris/src/config/mod.rs:417`）は読み込みエラーにする。`scripts/celeris.sh fixture …` が起動しない。新形（`[llm_sources.*]`・`[harnesses.*]`・`[route_defaults.*]`）への移行は、multi-account の fallback 試験の表し方を決める判断が要るため、この unit では触っていない。
- e2e（Playwright）は実 celeris を要するためこの環境では未実行。
- `providerLlmSourceDisplay` の名前、`accounts.tsx` の `provider-tier-models-heading`、`USED_BY_SCOPE_LABEL` の「プロバイダ」は provider の語を残す内部名・表示。機能の provider 画面ではないので残した。

## 再々実行（2026-10-12、TMPDIR=/local/x/ui）

worktree の前回成果を確認した（変更は残っていた。commit はされていない）。

- 受け入れ 0（provider の grep）: 一致 0 件、exit 0。
- 受け入れ 1（gui の typecheck・lint・test・build）: typecheck exit 0、lint exit 0（info 2 件のみ）、vitest 92 files・1307 tests passed、build exit 0。
- 受け入れ 2（`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/`）: exit 0（差分ゼロ）。
- 前回の不合格の第 1 項 `git diff --quiet $(git merge-base HEAD main) -- crates/` は still exit 1。原因は前回と同じ計画の問題（merge-base 4ef38473 が branch base c6a6c285 の先行統合を含まない）。コードでは直せないので、計画側で直す必要がある。

## v3 の確認（2026-10-12、TMPDIR=/local/x/ui）

- 受け入れ 0（provider の grep、gui/app・test・e2e・scripts と docs/celeris-api-v1.md）: 一致 0 件、exit 0。
- 受け入れ 1: install --offline exit 0、typecheck exit 0、lint exit 0、vitest 92 files・1307 tests passed、build exit 0。
- 受け入れ 2（`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/`）: exit 0。
- 検査は merge-base 基準を外した。e2e の celeris 設定 fixture（gui/test/celeris/*.toml.tmpl・scripts/celeris.sh）は gui-fixtures の範囲なので触っていない。
