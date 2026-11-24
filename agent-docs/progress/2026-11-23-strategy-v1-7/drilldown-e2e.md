---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: drilldown-e2e
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 7 drilldown-e2e: drill-down の e2e を本物の backend が出した概要の JSON で流す

## 結論
- final review（v3）の不合格の条件 0 のうち、e2e の側を直した。v3 の e2e『drill-down: log・Notebook・作業ツリー・Git…』は、
  `web/e2e/support/fake-daemon.mjs` の手書きの `projectOverviewFixture` の link を押していた。本物の daemon はその形を返さなかった。
- 偽 daemon に `createFakeDaemon({ projectOverview: "drilldown" })` を足した。この構成では、`GET /api/v1/projects/P1/overview` に
  `crates/task-api/tests/fixtures/project_overview_drilldown.json`（drilldown-api の試験 `project_overview_drilldown_fixture_matches_generated` が
  本物の route の応答から生成した golden）をそのまま返す。file は repo の根からの相対で読み、写しを `web/` に置いていない。
- golden の link の先の応答を、golden の label に合わせて足した（同じ構成のときだけ）:
  - `T2`（実験の task。run `R1`）の詳細・`runs/R1/stdout`・成果物 `analysis/scc.ipynb`・作業ツリー・変更（repo `agent-platform`、head は golden の sha、取り込み済み）
  - `T3`（run の無い実験の task）の詳細
  - `T1` の成果物 `strategy/arch-v4-cycle/v1.json`（golden の sha256）
  - chat の thread `TH1` と、外部 LLM（`mcp:chatgpt`）の案 `M1`（版 1）
- `web/e2e/work/project-overview.spec.ts` の drill-down の試験は golden から link を引いて押す。移る先: `/tasks/T2/runs/R1`（log の本文が出る）、
  `/tasks/T2?tab=artifacts`（Notebook の行が出る）、`/tasks/T2/files`、`/tasks/T2/changes`（O6 と O9 の task 付きの Git の 2 か所）、
  `/tasks/T1?tab=artifacts`（戦略の成果物）、`/?thread=TH1`（O8 と O12 の chat）、`/tasks/T2`（task）。文献は https・新しい tab・noopener の属性を見る。
- 手書きの `projectOverviewFixture` の link は、同じ項目の key で golden に現れる kind だけにした。消した link は 3 種:
  O7 `latest_findings` の `artifact`・`log`、O12 `pending_human_decisions` の `task`（entries と warnings の両方）。
  O11 `next_verification` にあった知識の path の `literature` は O6 `refutations` に移した（本物の backend は証拠の
  `Reference::Knowledge` を path だけの `literature` にする（`crates/task-ops/src/project_overview.rs` の `Reference::Knowledge` の枝）。
  golden は O6 に `literature` を持つ）。手書きの fixture は 14 項目・印・強い警告の試験に残す。
- `crates/` は変えていない（`git diff --quiet "$CELERIS_WU_BASE" -- crates/` が exit 0）。

## 実装前 brief
1. 根本原因: 偽 daemon の P1 の概要が手書きの値で、本物の route の応答と結ばれていなかった。e2e は本物が返さない link（v3 の時点で log・
   Notebook・task 付きの Git）を押して通った。
2. 責務 owner: 偽 daemon の fixture（`web/e2e/support/fake-daemon.mjs`）と e2e の spec（`web/e2e/work/project-overview.spec.ts`）。
   backend の形の正本は drilldown-api の golden とそれを生成する Rust の試験。
3. 既存の再利用先: drilldown-api の golden と生成試験、`validateFixture`（schema の検査）、`richFixtures` の T1 の詳細（T2・T3 の詳細の土台）、
   `createChatFixture` の thread の seed、`startFixtureGateway` の options。
4. 選択肢: (a) golden を `web/` に写す → 写しが本物とずれうるので不採用（Objective も禁じる）。(b) P1 の概要を常に golden にする → 14 項目・印・
   強い警告 5 件と publish の O14 の試験が golden に無い状態に頼るので不採用。(c) option で P1 の概要を golden に差し替え、link の先を足す → 採用。
5. 新しい状態・分岐・fallback・port: 偽 daemon の option `projectOverview`（`"fixture"` 既定・`"drilldown"`）だけ。試験の構成の選択で、production の
   分岐ではない。`publishManifest` との併用と rich 以外の profile は起動時に例外にする（P1 の概要を 2 つの fixture が奪い合わないため）。
6. 撤去対象: v3 の drill-down の試験（手書きの fixture の link を押すもの）を置き換えた。手書きの fixture の、golden に無い (項目, kind) の link 3 種を消し、
   知識の path の文献は golden が `literature` を持つ O6 に移した。
7. 不変条件と試験:
   - 手書きの fixture の (項目, kind) は golden に含まれる → `project_overview_fixture_links_are_kinds_the_golden_has_for_the_same_item`。
     手書きの fixture が 7 種の kind をすべて残していることも見る（link を全部消して空で通ることを防ぐ）。
   - drilldown の偽 daemon は golden を書き換えずに返し、golden のすべての link の先が 200 で、label（task・run・成果物の name・repo と head の sha・
     thread と message と版）が一致する → `project_overview_drilldown_daemon_serves_the_golden_and_every_link_target`。
   - 画面が golden の link で既存の画面へ移る → e2e『log・Notebook・作業ツリー・task 付きの Git・文献・chat の thread・task へ移る』。
   - 既存の試験ではこれらを検出できない理由: v3 の e2e は手書きの値を押すだけで、手書きの値と本物の応答の対応を誰も確かめていなかった。

## 試験の変更の理由（規約 10）
- e2e の drill-down の試験を書き換えた。守る契約（概要の link で既存の画面へ移る）は同じで、押す link を手書きから golden に替えた。
- v3 の drill-down の試験にあった知識の path の `literature`（`projects/demo.md` と「知識を検索」から `/knowledge` へ移る）の確認は、
  14 項目の試験に移した。golden は O11 に `literature` を出さないので、手書きの fixture ではこの link を O6 に置いた。
  golden に知識の path の文献は無いので、この確認だけは手書きの fixture に対する確認のまま残る（下の『未確認』）。
- 手書きの fixture から消した link（O7 の `artifact`・`log`、O12 の `task`）を押す試験はもともと無く、14 項目の試験の他の確認は変わらない。

## 変更前の再現（負の試験）
- 手書きの fixture の `latest_findings` の `log` と `pending_human_decisions` の `task` を戻して unit 試験を流した（commit 前の作業ツリーで実行）:
  `corepack pnpm@12.6.0 -C web exec vitest run e2e/support/fake-daemon.test.ts -t project_overview_fixture_links` → exit 1、
  `Received: ["latest_findings:log", "pending_human_decisions:task"]`。戻した file は元に戻した。

## 検査（提出時の HEAD で実行）
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline` → exit 0
- `corepack pnpm@12.6.0 -C web exec vitest run e2e/support/fake-daemon.test.ts` → exit 0、21 passed（新しい 2 件を含む）
- `corepack pnpm@12.6.0 -C web exec playwright test e2e/work/project-overview.spec.ts e2e/work/publish-manifest.spec.ts e2e/chat/proposal-review.spec.ts`
  → exit 0、14 passed（project-overview 2・publish-manifest 5・proposal-review 7。偽 daemon に対する実行）
- `corepack pnpm@12.6.0 -C web exec biome check <変えた 4 file>` → exit 0（警告 2 件は既存の行: `progressLine` の未使用・reduce の spread）
- `bash scripts/dev/test-affected.sh`（基点 `$CELERIS_WU_BASE`。対象は web の typecheck と unit 試験）→ exit 0。tsc -b exit 0、
  vitest 108 files・773 passed、node --test 110 passed・fail 0、repo-health verdict=pass（fail_grade 0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/` → exit 0（crates/ の差分 0）

## commit（件名）
- strategy-v1 7 drilldown-e2e: 概要の drill-down の e2e を本物の backend の golden で流す
- strategy-v1 7 drilldown-e2e: 知識の path の文献を O6 に移し、14 項目の試験で確かめる
- strategy-v1 7 drilldown-e2e: WU の進捗を書く

## 未確認・引き継ぎ
- 本物の daemon と本物の gateway をつないだ画面の drill-down は未確認（e2e は偽 daemon に対する実行。偽 daemon が返す概要は本物の route の golden）。
- golden の再生成（`UPDATE_FIXTURE=1`）で label や link の数が変わると、unit 試験と e2e が落ちる。そのときは `drilldownFixtures` の link の先を合わせる。
- 知識の path の `literature` link（本物の backend は証拠の `Reference::Knowledge` から出す）を含む場面は golden に無い。14 項目の試験は
  手書きの fixture の O6 のこの link を押している。golden の生成試験に Knowledge の参照を持つ証拠を足せば、golden で確かめられる。
- Notebook は成果物の一覧の行まで確かめた。`.ipynb` の本文の表示は既存の成果物の viewer の範囲で、この WU では見ていない。

## 親の目的への寄与
- observability（人が概要から生の資料へ辿れること）: 確認済み — 本物の backend の出した概要の link で、log・Notebook・作業ツリー・task 付きの Git・
  文献・chat の thread・task の画面へ移ることを e2e で示した（偽 daemon 上。本物の daemon と gateway での確認は未確認）。
- 改善 A〜D: この WU は直接は寄与しない。
- 新しい Manager・trait・状態機械・DB 表・Agent サービスは足していない。Project.request と Roadmap は書き換えていない。本番昇格・publish・外部公開はしていない。
