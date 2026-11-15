---
title: "健全性v2 R6 console-api: 常に null の API 欄 ScratchCacheView.stats と Console の途中目標の節を消す"
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 console-api

完了日: 2026-11-15

結論: `ScratchCacheView.stats`（ADR-0129 で廃止、値は常に `None`）と Console の `milestone` ブロック（ADR-0079 R5a 9）を消した。
生成 schema（`docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`・`types.ts`）は作り直し、もう一度生成しても差分は 0。
決定 r6-write-set は a（運用者が選んだ。人ではない）で、変えた path はすべて R6 の write set に入る。

commit（件名）: 「health-v2 R6 console-api: ScratchCacheView.stats と Console の途中目標の節を消す」と、この進捗 file の commit。
基準: `$CELERIS_WU_BASE`（この WorkUnit の base）。

## 項目ごとの判定

| 項目 | 判定 | 根拠の検索 |
|---|---|---|
| `task_ops::daemon::ScratchCacheView.stats` | 消した | `grep -rn "ScratchCacheStats\|scratch-cache-stats\|ScratchCacheView" crates web/features web/lib web/routes web/test web/e2e scripts docs` で、読み手は daemon.rs の定義と生成 schema・types.ts だけ。値を入れる経路は無い（ADR-0129 で cache server を撤去） |
| `ScratchCacheStats`（型） | 消した | `stats` 欄だけが参照していた（上と同じ検索）。B0 の dup-candidates.json の canonical も「stats 欄・ScratchCacheStats・SCRATCH_CACHE_STATS_SCHEMA を消す」 |
| `SCRATCH_CACHE_STATS_SCHEMA` | 消した | 定義の 1 か所だけで、参照は 0 件（同じ検索） |
| `ConsoleBlock::Milestone`・`console.rs` の節 3・`proposed_milestones` | 消した | `grep -rn "ConsoleBlock::Milestone\|MilestoneReviewView" crates`: 作るのは `side_blocks` の節 3、読むのは `cursor_of` だけ。途中目標は ADR-0079 D13 で凍結し、R5a 9 が「Console の読み手は R5b / R6 で外す」と決めた |
| web の `console-labels.ts`・`console-view.tsx` の `case "milestone"` | 消した | `grep -rn -i milestone web/features/console web/test web/lib`: この 2 か所だけ。生成型から `kind: "milestone"` が消えるので残すと tsc が落ちる |
| `task_ops::milestone_review::find`（と `PROJECT_SCAN`） | 消した | `grep -rn "milestone_review::" crates`: 呼び手は 0 件。R5a 9 は「案件ページと Console」が読むとしたが、どちらも使っていない |
| `task_ops::milestone_review` module（`latest_proposal`・`review_state`） | 残した | `crates/task-api/src/handlers/projects.rs` の 219・224 行が `GET /projects/{id}?include_frozen=true` の履歴表示に使う。objective の「過去を読む経路は残す」に当たる |
| `MilestoneReviewView`（API 型） | 残した | `handlers/projects.rs` の `MilestoneView.review` が使う |
| 試験 `approvals_milestones_and_questions_carry_their_state` | 途中目標の部分だけ外し、`approvals_and_questions_carry_their_state` に改名 | 消した契約は Console の `milestone` ブロック（ADR-0079 D13）。認可・質問の assert（`kind = question` の text・answered・node_id・run_id、`kind = approval` の id・未決、回答後の `answered = true`）は変えていない。途中目標の欄が無いことは `ConsoleBlock` の型で決まるので、無いことを確かめる試験は足していない。凍結した行を既定で隠すことは `project_plan.rs::project_detail_hides_frozen_milestones_by_default` が引き続き確かめる |
| `docs/api/v1/api.md` の Console の節（§3.98・§3.99・表 87 行目） | 書き換えた | `kind` は 8 種になった。範囲の説明と SSE の種類から途中目標を外した |

変えていないもの: events・migration・`task_core::Milestone` と store の途中目標の読み取り・`GET /projects/{id}?include_frozen=true`・
worker の `context.milestone_review`（R5a 9 が別に挙げた読み取り。この unit の範囲外）。

## 証拠（提出時の HEAD で実行）
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::` で schema を作り直した。`cd web && node scripts/gen-types.mjs` で生成型を作り直した。
- 再生成の差分 0: `cargo test -q -p task-api --lib schema::` → 2 passed（`committed_schema_matches_generated` を含む）。
  `node scripts/gen-types.mjs` の後の `git diff --exit-code -- web/api/generated docs/api/v1` → exit 0。
- 消した名前の残り: `grep -rn "ConsoleBlock::Milestone\|SCRATCH_CACHE_STATS_SCHEMA\|ScratchCacheStats\|\"kind\": \"milestone\"" crates web/api web/features docs/api` → 0 件。
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・
  task-ops・task-worker と web。nextest は「6908 tests run: 6908 passed (8 slow), 23 skipped」。web は tsc -b が通り、vitest は
  「Test Files 103 passed / Tests 714 passed」。`repo_health_gate: all checks passed`。
  - `milestone_review::find` を消す前の 1 回目（load average 約 29）は task-worker の browser の 2 件
    （`launcher_credential_post_login_consent_page_is_held_with_its_reason`・`daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages`）
    が落ちた。この unit は task-worker も browser の経路も変えていない。2 件だけを `cargo nextest run -p task-worker -E …` で走らせると
    2 passed。提出時の HEAD の 2 回目は全件 passed。
- `cargo nextest run -p task-api -E 'test(approvals_and_questions_carry_their_state) | test(committed_schema_matches_generated) | test(project_detail_hides_frozen_milestones_by_default) | binary(unmapped_suite_a) & test(/^console::/)'` → 13 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0（1 回目は import の折り返しで落ち、`cargo fmt --all` で直した）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の 11 path とこの file を
  `slices-check.py --write-set R6 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` に通した → 「write-set R6: ok」、exit 0。
- 保護する領域の差分: `git diff --name-only $CELERIS_WU_BASE` の中に campaign・routing-eval・jev-classifier・script-runtime・
  control-ts・migrations・events.rs・script_guard・credentiald の path は 0 件。

## 未解決
- 無し。

## 提案
- `ScratchStatus.sccache`（`ScratchSccacheView`）と `ScratchStatus.cache`（`ScratchCacheView`）も ADR-0129 で廃止し、値は常に `None` の
  API 欄として残っている。doc comment の理由は「旧 client との互換」だが、旧い仕組みを同じ変更で消す方針
  （ADR 2026-10-11-breaking-changes-by-default）に合わない。この unit の objective は `stats` 欄だけなので残した。
  消すかどうかは R6 の close か後続の task で判断してほしい（消すと生成 schema の JSON がまた変わる）。
