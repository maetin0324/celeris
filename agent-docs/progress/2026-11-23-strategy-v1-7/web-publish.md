---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: web-publish
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 7 web-publish: publish manifest の画面（claims → evidence → 再現条件 → limitations・未解決 → 人の署名）

## 結論
- 新しい route `/tasks/$id/publish/$manifestId?version=` に publish manifest の画面を足した（ADR 2026-11-23-project-observability-publish D3・D4）。
  読むのは `GET /api/v1/tasks/{id}/publish/{manifest_id}?version=`（`task_api::publish::manifest_status`）だけで、署名の状態・違反・根拠の無い claim は
  daemon が導いた値をそのまま出す。画面は状態の正本を持たない。
- 並びは「1. 主張（claims）→ 2. 根拠（evidence）→ 3. 再現条件 → 4. limitations・未解決 → 5. 人の署名」。5 段の前に「強い警告」の節を置き、
  色だけでなく文字で出す: unsupported の claim（claim ごとに 1 行）、file が記録の sha256 と違う、検査の違反、未署名、失効（`stale`・`superseded`）、
  「署名しない」の回答、概要と同じ強い警告（`manifest_blocking_question`・`manifest_confidential`）。
- evidence の参照（`task_core::strategy::Reference` の 6 種）は、案件の概要の link の部品（`OverviewLink`）に写して出す。commit → その task の変更の画面
  （`/tasks/$id/changes`）、成果物 → task の成果物の viewer（`?tab=artifacts`）、task → task の詳細、URL → 新しい tab（https・`noopener noreferrer`、
  外部・untrusted と表示）、knowledge → path と知識の検索、chat → `/?thread=`。
- 署名は既存の決定の回答（`POST /api/decisions/{id}/answer`、`features/chat/data/client.ts` の `answerDecision`）で `sign` か `reject` を送るだけ。
  操作は「この版に署名する」「署名しない」の 2 つで、どちらも既存の `ConfirmDialog` を挟む。確認の文は
  「対象: publish/<id>/v<n>.json（sha256:<64 桁>）。Celeris は外に送らない。公開は人が行う。…」。公開・送信の操作は画面に無い。
- 署名の操作を出す条件: gateway の session が login 済み、署名の状態が `unsigned`、決定が `open`、検査の違反が 0 で file が記録と同じ。
  それ以外は理由の文を出す（失効・より新しい版・回答済みは task の決定の画面へ）。最終の判定は daemon（403・409・422）。
- 失敗の文: 403 は「署名は人だけができます（publication_requires_human）。」、gateway が 403 を置き換えた 502 `daemon_auth` は
  「daemon が要求の主体を拒みました（daemon_auth）」、409 `publish_manifest_stale` と 422 `publish_manifest_invalid` は code を添えて読み直しを促す。
- 案件の概要の O14「公開の準備度」の各 entry に「manifest <id> v<n> を読む」の link（`data-link="manifest"`）を足した（`project-overview-panel.tsx`）。
- gateway の SPA の path（`web/server/spa-routes.js`）に `/tasks/$id/publish/$manifestId` を足した。足す前は、直接開いた manifest の URL に
  gateway が index.html を 404 で返していた（画面は描かれるので e2e では気づかず、`spa-routes.test.mjs` が routeTree との差で落とした）。

## 変えた file
| file | 種類 | 内容 |
|---|---|---|
| `web/features/publish/manifest-model.ts` | production | 文書の項目の型、5 段の順、強い警告、参照から link への写し、署名できるかの判定、確認の文、失敗の文、GET の query |
| `web/features/publish/manifest-view.tsx` | production | `ManifestScreen`（route の画面）と `ManifestPanel`（props だけで描く本体）・署名の節 |
| `web/routes/tasks.$id.publish.$manifestId.tsx` | production | route（`version` の search） |
| `web/routeTree.gen.ts` | 生成 | `vite build` の router plugin で再生成 |
| `web/features/projects/project-overview-panel.tsx` | production | O14 の entry から manifest の画面への link |
| `web/server/spa-routes.js` | production | gateway の SPA の path に manifest の route |
| `web/e2e/support/fake-daemon.mjs`・`.d.mts` | test | `createFakeDaemon({ publishManifest: true })` の manifest・署名の決定・回答の口・概要の O14・制御口 |
| `web/e2e/support/fixture-gateway.ts` | test | 戻り値に `daemonUrl`（制御口と、gateway の置き換えを通さない要求に使う） |
| `web/e2e/work/publish-manifest.spec.ts` | test | e2e 5 件 |
| `web/features/publish/manifest.test.tsx` | test | vitest 8 件 |
| `web/e2e/support/fake-daemon.test.ts` | test | fixture の試験 1 件 |
| `web/server/spa-routes.test.mjs` | test | `isSpaRoute` の受ける path・拒む path に manifest の path |

fake-daemon の fixture（task T1・manifest `io-paper`）: 署名の状態は本物の `task_core::publish::signoff_state` と同じ順（より新しい版 → 回答無し →
`reject` → file の変化 → 署名済み）で、読むたびに導く。`POST /api/v1/decisions/DPUB<n>/answer` は、主体が人でなければ 403
`publication_requires_human` で何も記録しない。制御口は `POST /__fixture/publish/principal {principal}`・`modify {version}`・`new-version`、
`GET /__fixture/publish/answer-log`。応答は `ManifestView`・`DecisionList`・`DecisionOutcome`・`ProjectOverview` の schema に合う（fixture の試験で確かめる）。

## 試験
守る不変条件と、既存の試験で検出できない理由: base には manifest の画面・route・概要からの link が無く、既存の試験はどれも manifest を描かない。

e2e `web/e2e/work/publish-manifest.spec.ts`（実の gateway と偽 daemon、外部ネットワークに出ない）:
1. 概要の公開の準備度から開き、5 段の順に読み、unsupported の claim と未署名を強い警告で出し、公開の操作が無い
   （O14 の印「要判断」→ link → URL `?version=1`、節の順 `claims・evidence・reproduction・limitations・signoff`、強い警告が 5 段より前、
   `unsupported_claim`（c2）・`unsigned`・`manifest_blocking_question`、名前に「公開」「publish」を含む button・link が 0、button は署名の 2 つだけ、
   変更の要求 0 件）
2. evidence から commit・成果物・文献・chat・task へ移る
3. 署名: dialog が sha256 と『外に送らない』を示し、人の回答 1 件で署名済みになる（dialog の文に `publish/io-paper/v1.json（sha256:111…）` と
   「Celeris は外に送らない。公開は人が行う。」、署名後は `signed`・「回答: human:web」、画面が送った変更の要求は
   `/api/v1/decisions/DPUB1/answer` の 1 件だけ、概要の O14 は「確認済み」）
4. 失効: 署名の後に file が変わると `stale`（`signoff_stale`・`file_changed` の警告、今の file の sha256、署名の操作が消える、概要の O14 は
   「矛盾・反証」と強い警告 `manifest_invalid`）、新しい版が出ると前の版は `superseded`（「より新しい版 v2」）で、v2 は `unsigned` で署名できる
5. 403: 人以外の主体（operator）の署名を daemon が `publication_requires_human` で拒む。gateway 経由では 502 `daemon_auth` の文、daemon の 403 が
   そのまま届くと「署名は人だけができます（publication_requires_human）。」を dialog に出し、状態は `unsigned` のまま、回答の記録は 2 件とも 403
- どの test も、直接開いた manifest の URL の応答が 200 であることを確かめる。

vitest `web/features/publish/manifest.test.tsx`（接頭辞 `publish_manifest_ui_`）:
`publish_manifest_ui_sections_follow_claims_evidence_reproduction_limitations_signoff`・
`publish_manifest_ui_unsupported_claim_and_unsigned_are_strong_warnings`・
`publish_manifest_ui_stale_superseded_and_invalid_are_strong_warnings_and_block_signing`・
`publish_manifest_ui_sign_controls_need_login_and_open_decision`・
`publish_manifest_ui_confirm_text_has_sha256_and_no_publish_action`・
`publish_manifest_ui_signoff_failure_names_publication_requires_human`・
`publish_manifest_ui_references_drill_down_to_commit_artifact_literature_chat`・
`publish_manifest_ui_overview_publish_readiness_links_to_manifest`。
fixture の試験 `web/e2e/support/fake-daemon.test.ts`: `publish_manifest_ui_fixture_is_schema_valid_and_derives_signoff_like_the_daemon`。

変更前に落ちること（Run #1 で確認）: 画面の file（`web/features/publish/manifest-model.ts`・`manifest-view.tsx`、route、`routeTree.gen.ts`、
`project-overview-panel.tsx`）を base（`git merge-base` の commit、`integrate wu/proposal-e2e (phase web)`）に戻し、fixture と spec はそのまま残して実行した。
- `corepack pnpm@12.6.0 -C web exec playwright test e2e/work/publish-manifest.spec.ts` → 5 failed（`expect(locator).toBeVisible() failed`）
- `corepack pnpm@12.6.0 -C web exec vitest run features/publish` → 失敗（`Cannot find module '/features/publish/manifest-model'`、Test Files 1 failed）
- fixture の試験は fixture だけを見るので、変更前でも通る（画面の回帰ではなく fixture の形と状態の導出を守る）。
- `spa-routes.test.mjs` の `spa route patterns match the declared route tree` は、route を足して `spa-routes.js` を直す前に 1 件落ちた（test-affected の 1 回目）。

## 検査（提出時の HEAD で実行）
| 検査 | コマンド | 結果 |
|---|---|---|
| e2e | `corepack pnpm@12.6.0 -C web exec playwright test e2e/work/publish-manifest.spec.ts e2e/work/project-overview.spec.ts` | 7 passed（manifest 5・概要 2）。spa-routes の修正の後に manifest の 5 件を再実行して 5 passed |
| typecheck と web の unit 試験 | `bash scripts/dev/test-affected.sh`（対象は web の typecheck+unit） | exit 0。`tsc -b` 通過、vitest 108 files・771 tests passed、`node --test server/*.test.mjs` 110 pass・0 fail、repo_health_gate pass |
| lint | `corepack pnpm@12.6.0 -C web exec biome check <変えた file>` | exit 0。`fake-daemon.mjs` の警告は base と同じ 2 件（増えていない） |
| 範囲 | `sh "$CELERIS_WU_SCOPE_PATHS"` を許可 path（`web/`・ADR・`agent-docs/progress/2026-11-23-strategy-v1-7/`）に照らす | 範囲外 0 |
| crates/ の差分 | `git diff --quiet "$CELERIS_WU_BASE" -- crates/` | 差分 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |

## 測定（LoC、基準は `git merge-base HEAD main` ではなく WU の base の commit `integrate wu/proposal-e2e (phase web)`、`git diff --numstat`）
- production: +622 −0（`manifest-view.tsx` 372、`manifest-model.ts` 210、`project-overview-panel.tsx` 21、route 18、`spa-routes.js` 1）
- test: +879 −2（fake-daemon の fixture 293・`.d.mts` 17・fixture の試験 39、e2e spec 248、vitest 269、fixture-gateway 4/−1、spa-routes の試験 9/−1）
- 生成: `routeTree.gen.ts` +22
- 純増の理由: 画面が base に無い新しい機能（manifest の表示と署名の操作）だから。新しい状態の正本・store・Manager・trait・状態機械・DB 表・event の種類は足していない。
  署名の状態は daemon の値を表示するだけで、署名は既存の決定の回答、参照の link は概要の部品、確認は既存の `ConfirmDialog` を使う。

## commit（件名）
- strategy-v1 7 web-publish: publish manifest の画面（claims→evidence→再現条件→limitations・未解決→人の署名、unsupported・未署名・失効の強い警告、sha256 と『外に送らない』の確認で既存の決定の回答に署名）、概要の公開の準備度からの link、fake-daemon の manifest と署名の fixture、e2e publish-manifest.spec.ts と publish_manifest_ui_ の試験
- strategy-v1 7 web-publish: gateway の SPA の path に /tasks/$id/publish/$manifestId を足し、直接開いた manifest の URL を 200 で返す（spec で確かめる）
- strategy-v1 7 web-publish: spa-routes の試験の整形
- strategy-v1 7 web-publish: 進捗を書く

## 未確認・引き継ぎ
- 「人の login のときだけ署名の操作を出す」（ADR D4）は、gateway の session（`GET /api/session` の `authenticated`）で判定した。Web は BFF の token の
  daemon の主体（`human:web` か、token の無い構成の `human:local`）を読めない。最終の拒否は daemon（非人は 403、`human:local` も拒む。publish-api の試験）。
  root の route は未 login なら `/login` へ移すので、未 login のまま manifest の画面は描かれない。password 付きの gateway での e2e は書いていない（未確認）。
- 本物の gateway（`web/server/relay.js`）は daemon の 401・403 を 502 `daemon_auth` に置き換える。gateway 経由では画面に
  `publication_requires_human` の文が出ず `daemon_auth` の文になる（web-proposals と同じ件）。spec は両方の表示を分けて確かめた。gateway を変えるかは
  integrate-web か close で人が決める。
- chat の参照の link は概要の部品と同じく `/?thread=` へ移る（message の位置へは移らない）。`message` の search を足すと概要の e2e の URL の期待も変わるので、
  この WU では変えていない。
- 本物の daemon に対する画面の確認（本番のデータの manifest）はしていない。偽 daemon の fixture は本物の応答の schema と状態の導出の順に合わせた。
- Celeris は署名を受けて外へ何も送らない。この WU は公開・送信の操作を足していない（e2e で button・link の名前と変更の要求を確かめた）。

## 親の目的への寄与
- observability（確認済み）: 人は概要の「公開の準備度」から manifest を開き、主張・根拠・再現条件・limitations・未解決・署名を 1 画面で順に読める。
  根拠の無い主張・未署名・失効は文字の強い警告で出る。根拠から commit・成果物・文献・chat・task へ移れる（e2e 2・3・4）。
- publish は人だけ（確認済み、範囲は Web の表示）: 画面の署名は既存の決定の回答だけで、人以外の主体の回答は daemon が 403 で拒み、画面はその理由を出す（e2e 5）。
  backend の拒否そのものは publish-api の試験が示す。
- 改善 A〜D への直接の寄与は無い（この WU は表示と人の署名の操作だけ）。
