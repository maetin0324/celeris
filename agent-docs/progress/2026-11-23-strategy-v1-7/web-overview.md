---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: web-overview
status: done
completed: 2026-11-23
---
# 戦略層 v1 子 7 web-overview: 案件の詳細に概要の panel

## 結論
- 案件の詳細（`/projects/$id`）の「概要」の節の下に「案件の概要（14 項目）」の節を足した。`GET /api/v1/projects/{id}/overview` を読み、
  ADR 2026-11-23-project-observability-publish D1.3 の 14 項目を決めた順（O1 原目的 〜 O14 公開の準備度）で並べる。
- 印は 4 語を文字と色で区別する（確認済み=success・推定=info・要判断=warning・矛盾・反証=danger）。記録の無い項目は印を付けず「記録無し」と出す。
- 強い警告（`warnings[]` の `strong: true`）だけを panel の先頭にまとめ、「強い警告（項目）: 文」と文字で出す（色だけに頼らない）。通常の警告は O13 危険の件数に出る。
- drill-down の link は既存の route だけに向ける: `log` → `/tasks/$id/runs/$runId`、`notebook` → `/tasks/$id?tab=artifacts`（成果物の viewer）と
  `/tasks/$id/files`、`artifact` → `/tasks/$id?tab=artifacts`、`git` → `/tasks/$id/changes`（task が無ければ repo と 40 桁の sha を文字で出す）、
  `literature` → `https://` の URL だけを新しい tab（`rel="noopener noreferrer"`）、それ以外は path を文字で出して `/knowledge`、`chat` → `/?thread=`、`task` → `/tasks/$id`。
- 概要に無い欄は写さない（ADR D1.1）。task の status は `GET /projects/{id}` の `tasks[]` から引き、request の本文・予算の上限・Roadmap・task の木は
  同じ画面の既存の節（概要・自動運営・途中目標と仕事・成果物）が出す。panel は状態を持たず、query は `projectKeys.detail(id)` の下の key にした。
- `crates/` の差分は 0。新しい DB 表・列・migration・event・状態機械・Manager・trait は無い。

## 変えた file
- `web/features/projects/project-overview-panel.tsx`（新規）: `ProjectOverviewSection`（query）と `ProjectOverviewPanel`（表示）、`entrySummary`・`warningText`・`strongWarnings`・`OverviewLink`。
- `web/features/projects/project-detail-view.tsx`: 節を 1 つ足した（既存の controller・roadmap・authority・成果物の節は並べたまま）。
- `web/features/projects/project-queries.ts`: `projectOverviewQuery`。
- `web/e2e/support/fake-daemon.mjs`: `projectOverviewFixture()`（schema `ProjectOverview` で検証してから返す）を rich profile の `/api/v1/projects/P1/overview` に登録。
  link は rich の T1・R1・chat-history に向け、7 種の link・印 4 種・記録無し・強い警告 5 件・通常の警告 1 件を含む。
- `web/e2e/work/project-overview.spec.ts`（新規）、`web/features/projects/project-overview-panel.test.tsx`（新規）。

## 試験
守る不変条件: 14 項目を決めた順で出す・印 4 種を文字で区別する・記録の無い項目に印を付けない・強い警告だけを先頭にまとめる・link は既存の route と https の URL だけ。
既存の試験は案件の詳細の request・status しか見ないので、これらを検出できない。base には panel も route の fixture も無いので、下の試験は変更前に全部落ちる（import 先の file が無い・`project-overview-panel` の test id が無い）。
- vitest（`web/features/projects/project-overview-panel.test.tsx`、6 件）: `project_overview_panel_lists_fourteen_items_in_adr_order`・
  `project_overview_panel_marks_are_distinct_text_and_unrecorded_items_have_no_mark`・`project_overview_panel_collects_only_strong_warnings_at_the_top`・
  `project_overview_panel_links_drill_down_to_existing_routes_and_https_only`（http の URL は link にしないことを含む）・
  `project_overview_panel_reads_task_status_from_project_detail`・`entry_summary_formats_item_details`。
- e2e（`web/e2e/work/project-overview.spec.ts`、2 件）:「概要は 14 項目を決めた順に出し、印を文字で区別し、強い警告だけを先頭にまとめる」と
  「drill-down: log・Notebook・作業ツリー・Git・文献・chat の thread・task へ移る」（各 link を押して URL と移り先の見出しか test id を確かめる。外部の URL は押さず属性だけを見る）。

## 検査（提出時の HEAD で実行）
- `corepack pnpm@12.6.0 -C web exec playwright test e2e/work/project-overview.spec.ts` → 2 passed。
- 回帰の確認で既存の spec を名指しで実行: `e2e/parity/projects.spec.ts`・`e2e/parity/project-controller.spec.ts`・`e2e/work/projects.spec.ts`・
  `e2e/work/narrow-layout.spec.ts`・`e2e/states/rich-data.spec.ts`・`e2e/parity/shell.spec.ts`・`e2e/parity/runs-files.spec.ts`（新しい spec と合わせて）→ 34 passed。
  nfr の `e2e/a11y/axe.spec.ts`・`e2e/parity/mobile-gate.spec.ts` を `-g "projects|案件|P1"` で実行 → 10 passed（`/projects/$id` の axe serious/critical 0、4 幅で横溢れ 0 を含む）。
- `corepack pnpm@12.6.0 -C web exec vitest run features/projects/project-overview-panel.test.tsx` → 6 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（対象は web の typecheck+unit。tsc -b 通過、vitest 106 files・751 passed、server の node --test 110 pass、
  repo-health verdict=pass。new 5・worsened 2 の指摘はどれも前の backend の WU の crates（`task-core/src/publish.rs` など）で、この WU の file は無い）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `git diff "$CELERIS_WU_BASE" -- crates/ | wc -l` → 0。`sh "$CELERIS_WU_SCOPE_PATHS"` → 上の 6 file と、この進捗の file。
- `corepack pnpm@12.6.0 -C web exec biome check`（変えた file）→ 指摘 0（fake-daemon.mjs の既存の警告 2 件は触っていない行）。

### 再実行（計画 v3、提出時の HEAD で実行）
最初の run の後に計画の check『`git diff --quiet $(git merge-base HEAD main) -- crates/`』が落ちた。基点の main の merge-base は、
統合先 branch に先に入った backend 段（publish-api・proposal-api・overview-api）の crates/ の差分 35 file を数えていた。
この WU の差分に crates/ は無い。計画 v3 が基点を `$CELERIS_WU_BASE` に直したので、成果は作り直さずに検査だけを再実行した。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/` → exit 0。`sh "$CELERIS_WU_SCOPE_PATHS"` → 上の 6 file と、この進捗の file。
- `corepack pnpm@12.6.0 -C web exec playwright test e2e/work/project-overview.spec.ts` → 2 passed、exit 0。
- `corepack pnpm@12.6.0 -C web exec vitest run features/projects/project-overview-panel.test.tsx` → 6 passed。
- `bash scripts/dev/test-affected.sh`（対象は web の typecheck+unit）→ exit 0。tsc -b 通過、vitest 106 files・751 passed、server の node --test 110 pass、
  repo-health verdict=pass（new 5・worsened 2 は前回と同じ）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 回帰の既存 spec（上の 34 件と nfr の 10 件）はこの再実行では流していない。この WU の file は最初の run から変わっていない。

## 測定（LoC、基準 `$CELERIS_WU_BASE`、`git diff --numstat`）
- production（web）: +371 / −1（panel +352、detail view +8、queries +11/−1）。
- test: +520 / −2（vitest +218、e2e spec +118、fake daemon の fixture +184/−2）。
- 純増の理由: 新しい画面の節で、置き換える旧い仕組みが無い（ADR D4）。

## commit（件名）
- strategy-v1 7 web-overview: 案件の詳細に概要の panel（14 項目・印・強い警告・drill-down）、overview の fixture と project-overview の e2e・unit 試験
- strategy-v1 7 web-overview: 進捗を書く
- strategy-v1 7 web-overview: 計画 v3 の check の再実行の結果を進捗に足す

## 未確認・引き継ぎ
- chat の link は `/?thread=` だけに移る。`message` の search の欄は web-proposals の WU が `web/routes/index.tsx` に足すので、統合の後に
  `message` を足すかは integrate-web か close で決める（この WU は route の型の外の欄を書かない）。
- `log` の link は backend がまだ出さない（overview-api の未確認）。Web は 7 種すべてを扱い、e2e は fixture の `log` で遷移を確かめた。
- literature の `path`（knowledge の page）へ直接移る route は無い（ADR D1.4 の未確認）。今は path を文字で出し `/knowledge` の検索の画面へ移す。
- artifact の link の sha256 と今の file の照合（「上書きされた」の表示）はしていない。成果物の viewer（`?tab=artifacts`）の `sha256_matches` で読める。
- 本番の大きさの案件での表示の速さは測っていない。

## 親の目的への寄与
- observability（確認済み、e2e と unit 試験で）: 人は案件の詳細の 1 画面で 14 項目・印・強い警告を読み、log・Notebook・Git・文献・chat の thread へ移れる。
- 改善 A〜D への直接の寄与は無い（読むだけの画面）。概要に出る値の正しさは overview-api の backend の試験に依る。
