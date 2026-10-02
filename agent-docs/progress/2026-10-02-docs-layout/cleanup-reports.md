---
title: docs 再構成 — agent-docs/reports/・notes/ の報告を実装と照らして整理（cleanup-reports）
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# docs 再構成 — agent-docs/reports/・notes/ の報告を実装と照らして整理（cleanup-reports）

完了日: 2026-10-02。対象は `agent-docs/reports/` 5 本と `agent-docs/notes/` 1 本の計 6 本。
規則は ADR-0128 D2（日付付き報告は今の実装と食い違うなら削除）と D9（削除の記録）。残した報告の中身は書き換えていない。

## 判断（全 6 本）

| 判断 | パス | 照らしたもの | 根拠 |
|---|---|---|---|
| 残す | `agent-docs/reports/browser-live-relay.md` | `gui/app/celeris/browser-live-relay.server.ts`・`browser-live.server.ts`・`gui/server.js`、`gui/scripts/browser-live-e2e.sh`、`gui/e2e/g14-browser-live.spec.ts`、`gui/test/unit/browser-live-relay.test.ts`、`scripts/browser-auth-login-check.py`、`crates/task-worker/src/browser_credential.rs:415` | 書かれた設定名 `CELERIS_GUI_LIVE_VIEW_UPSTREAM`・`503 live_view_relay_unavailable`・dashboard の既定 port 27848・`/api/exec` の拒否・再実行の台本と試験・`export_worker_settings_for_real_browser_check` がいずれも今のコードにある。食い違いは見つからなかった |
| 残す | `agent-docs/reports/model-routing-2026-09-20.md` | `crates/task-core/src/model_routing.rs`（`select_tier` の 3%・10%・30% の閾値）、`crates/task-dispatch/src/accounts.rs`（`measured_remaining`・観測 300 秒・利用率 0.97）、`tier_models`（`crates/celeris/src/config/`）、`credential_refs`・`unavailable_reason`（`crates/task-api/src/admin.rs`）、`crates/celerisctl/tests/worker_run.rs`、`scripts/check-model-routing.mjs`、`agent-docs/gui/model-routing/*.png` | 設計（階層ごとのモデル ID、残量による階層の制限、固定 account_id、秘密参照）は今の実装どおり。ずれは 1 点だけで、`config/celeris.model-tiers.example.toml` には今は実行 ID（例 `claude-fable-5-1`）が入っており「全 6 件を実行モデル ID 未確認として記載」は当時の記述。報告の主張（仕組み）には効かないので残す |
| 削除 | `agent-docs/reports/execution-architecture-2026-09-24.md` | `crates/task-dispatch/src/dispatcher.rs`（今 2113 行、`dispatcher/` に分割）、ADR-0072・ADR-0074・ADR-0079 | ADR-0072 の前の現状調査（main `d792c63` の行番号つき）。「Task と worker run は実質 1:1」「`running: HashMap<TaskId, RunEntry>`（D:1279、19,485 行）」など、ExecutionPlan / WorkUnit の導入と ADR-0079 の分割で今の実装と食い違う。設計の結論は ADR-0072 にある |
| 削除 | `agent-docs/reports/execution-decomposition-report-2026-09-25.md` | ADR-0072・ADR-0074、`execution-parallel-report` §5「E6 報告の問題の行方」 | E6 dogfood の時点の分析。挙げた問題 1〜7 は Phase F で実装済み（review_timeout・merge_base の repair、artifact 走査、runs 索引、codex の cache read、quota 指標、planner の短縮、WU の並列）で、報告の「現状」は今と食い違う。gate 既定の提案も ADR-0079 の木の分解で置き換わった |
| 削除 | `agent-docs/reports/execution-parallel-report-2026-09-28.md` | ADR-0079（§冒頭「D3 全体（案件計画）を廃止」、T3・D13）、`crates/task-api/src/project_plan.rs` | ADR-0074 Phase F の最終報告。中心の 1 つの「案件計画」（`POST /projects/{id}/plan {mode: milestones}`、`celeris.project-plan/1`）と 3 層固定は ADR-0079 で廃止され、今の `POST /projects/{id}/plan` は 410 Gone を返すだけ（`project_plan.rs` の冒頭）。並列 WU など残った部分の決定は ADR-0074 本文にある |
| 削除 | `agent-docs/notes/build-cache-tiering-input-2026-09-28.md` | ADR-0075、`crates/scratch-cache/`（`l2.rs`）、`crates/celeris/src/cache_server.rs`、`crates/celeris/src/config/scratch.rs`、`crates/celerisctl/src/commands/scratch.rs` | ADR-0075 の入力メモ。人の方針（target は scratch、sccache の L1/L2、非同期の L2 書き込み）は ADR-0075 に取り込まれ、案 (b) の cache server として実装された。メモの「Celeris への当てはめ」節の事実（sccache は未導入、`CARGO_TARGET_DIR` を手で指定、F5-fix 進行中）は今と食い違う |

`agent-docs/notes/` は空になり、ディレクトリは git から消えた。`agent-docs/README.md` の `notes/` の説明は、今後の作業メモの置き場の定義なので変えていない。

## 削除の記録（ADR-0128 D9）

最後の commit は削除の前に `git log -1 --format=%h -- <旧パス>` で取った。内容は `git show <commit>:<旧パス>` で取り出せる。

| 処理 | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 削除 | agent-docs/reports/execution-architecture-2026-09-24.md | - | ADR-0072 前の現状調査。Task:run 1:1・dispatcher.rs の行番号が今の実装と食い違う | a42f9a54 |
| 削除 | agent-docs/reports/execution-decomposition-report-2026-09-25.md | - | E6 時点の分析。挙げた問題は Phase F で実装済み、gate 提案は ADR-0079 で置き換わった | a42f9a54 |
| 削除 | agent-docs/reports/execution-parallel-report-2026-09-28.md | - | 案件計画（ADR-0074 D3）は ADR-0079 で廃止。決定は ADR-0074 本文にある | a42f9a54 |
| 削除 | agent-docs/notes/build-cache-tiering-input-2026-09-28.md | - | ADR-0075 の入力メモ。方針は ADR-0075 と実装に入り、現状の記述（sccache 未導入など）は食い違う | a42f9a54 |

## 取り込み

- 全体のリンク検査を通すため、兄弟 WU `cleanup`（done、未統合）のブランチを merge した（3b0fbd77）。作業前の base（ad454132）では
  `sh scripts/dev/check-doc-links.sh` が 46 件の壊れたリンクで exit 1 だった（README.md・docs/guides・docs/ops・agent-docs/progress など、
  この WU の範囲外）。`cleanup` がそれらと `model-routing-2026-09-20.md` の相対リンク 4 件を直している。統合時は同じ commit なので衝突しない。

## 証拠

- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0（削除の後）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → 4 件の違反、exit 1。削除した 4 本が tsv では `agent` の行のまま
  （新パスが追跡されていない）。tsv はこの WU では変えない約束なので、land-verify で 4 行を `delete`・新パス `-` に直す
- `cargo test --workspace`・`cargo clippy --workspace -- -D warnings` の結果は下の「検査」

## 未解決

- `scripts/dev/docs-layout.tsv` の 140・141・142・166 行（上の 4 本）を land-verify で `delete` に直す。
- ADR-0072・ADR-0074・ADR-0075・ADR-0079 と `agent-docs/PROGRESS.md` に、削除した報告の旧パス（`docs/…` の字のまま）が残る。
  ADR は経緯の記録なので書き換えず、上の表の commit で辿る。

## 提案

- なし。
