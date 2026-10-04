---
title: draining の旧 daemon は run の終わりで手を離し、WU 検査・統合は新 daemon に渡す（ADR-0040 付記 2026-10-04）
tasks: [01M443KCS4K7XX3XCF9KBH5BYT]
status: done
updated: 2026-10-04
---

# PROGRESS — ライブ引き継ぎ後の WU 検査の手放しと WU 検査の進み具合

正本: [ADR-0040 付記 2026-10-04](../adr/0040-self-improvement-deploy.md)（「draining の旧 instance は run の終わりで手を離す」）。

## Phase 1（完了 2026-10-04、単一 phase）

人の指摘（2026-10-04）: 18:12 UTC の昇格（08c66fa6a54b → 864f5d29b07c）で、旧 daemon が draining のまま worker run
01M440WBF9D82CAX1WT8H3SB05 の後の WU 受け入れ検査（旧版の遅い web e2e）を流し続け、drain が終わらなかった。WU 検査は
events にも GUI にも出なかった。

### 変えたこと

- task-core: 追記の event `WorkUnitCheckStarted` / `WorkUnitCheckFinished`（`IntegrationCheck*` と同じ欄 + `run_id`）と
  `WorkUnitChecksHandedOff { work_unit_id, key, run_id }`。状態は変えない。
- task-dispatch:
  - `on_worker_finished`: draining（`accepting_new_work == false`）で検査付き WU の `Done` が届いたら
    `hand_off_work_unit_checks`（result.json を残す・lease を検査の分延ばす・印・quota 解放）で返し、検査を spawn しない。
  - `hand_off_checks_in_hand`: draining になった後の最初の tick で手元の検査を abort して同じく手放す。
  - 段の最後の WU が済んだときの `start_integration` は active のときだけ（draining は照合 → 新 active の dispatch に任せる）。
  - `take_over_handed_off_checks`: active の毎 tick、照合より前に、印のある手元に無い running の WU run を
    `finalise_from_result_json` で確定し直す（検査は新 instance で走る）。
  - `spawn_work_unit_checks` は `run_integration_checks`（`ObservedIntegration.run_id = Some`）で検査を流し、
    1 件ごとの event とログ（`<task_dir>/work-unit-checks/<wu_key>/<ms>-<i>.log`）を残す。
- task-ops: `check_progress`（WU の行）が WU 検査の event も読む。task-api: `check-log` が `WorkUnitCheckStarted` も読む。
  `EVENT_TYPES` に 3 語（59 → 62）。
- 文書・型: `docs/api/v1/gui-api.md` と `gui/docs/celeris-api-v1.md`（§3.126.19 と `check_progress` の説明）、
  `docs/api/v1/{api-v1,event}.schema.json`（`UPDATE_SCHEMA=1`）、`gui/app/celeris/types.ts`・`web/api/generated/*`（gen:types）、
  `web/api/realtime/{event-kinds,invalidation-map}.ts`。
- GUI（gui/）: WU の行の検査表示は統合で入れたものがそのまま葉の WU にも出る。コメントを更新し、偽 celeris の
  fixture（`gui/scripts/lib/celeris-fixture.mjs`）の running の葉 `api` に `check_progress` と check-log を足して監査対象にした。

### 受け入れ条件と証拠

0. ADR-0040 付記: `agent-docs/adr/0040-self-improvement-deploy.md` 末尾「付記 2026-10-04: draining の旧 instance は run の
   終わりで手を離す（WU 検査の引き継ぎ）」D1〜D3。
1. 試験（`crates/task-dispatch/src/dispatcher/tests/drain_hand_off.rs`、fake adapter の gate と `echo >> marker` の check）:
   - `a_draining_instance_leaves_the_work_unit_checks_to_the_new_active`: 旧 instance は検査を始めず（印ファイル無し・
     `WorkUnitCheckStarted` 無し）`WorkUnitChecksHandedOff` 1 件で in_flight 0、新 active が検査して WU done・
     `WorkerFinished` 1 件・`runs` completed。
   - `a_draining_instance_does_not_start_the_phase_integration`: 旧 instance は統合を始めず、新 active が統合する。
   - `work_units::draining_dispatcher_hands_off_work_unit_checks_in_hand_to_the_new_active`（F5-fix2 の試験を新しい規則へ
     書き換え）: 検査の途中で draining → 次の tick で in_flight 0・印、新 active がやり直して 1 回だけ完了。
   - 修正前の確認: hand-off を無効にした状態で 3 本とも失敗（`draining の旧 instance が WU の検査を流した`・
     `draining の旧 instance が検査を抱えたまま`（left 1 right 0）・`draining の旧 instance が統合した`）。修正後は通過。
2. WU 検査の event と出力の末尾:
   - `drain_hand_off` の 1 本目が `WorkUnitCheckStarted/Finished`（run_id・index・total・exit）とログ内容を確認。
   - `task-api/tests/files.rs::work_unit_check_log_reads_a_leaf_acceptance_check`（実行中の tail・index=0 の exit/所要時間・
     events 一覧の種別名）、`task-ops view::tests::work_unit_check_progress_shows_the_running_acceptance_check`。
   - gui: `corepack pnpm typecheck` exit 0、`corepack pnpm test` exit 0（91 files / 1301 tests）、`corepack pnpm build`
     exit 0、`corepack pnpm mobile-audit` exit 0（routes=28 violations=0）。web: `corepack pnpm typecheck` exit 0。
3. `bash scripts/dev/test-parallel.sh` exit 0（nextest 3891 passed、12 skipped、doctest ok）。
   `cargo clippy --workspace -- -D warnings` exit 0。

### 未解決事項

- draining になる前に始まっていた段の統合は止めず、終わるまで drain が延びる（git の途中で止めない。ADR 付記「採らない」）。
- 手放した run を新 instance が確定するとき、アカウント（quota の割り当て）は `None` になる。
- 本番への反映は昇格が要る（人の操作）。この版が current になった次の昇格から、旧側（この版）が手を離す。今回の
  旧 daemon（08c66fa6a54b）には効かない。

### 提案

- 統合も「draining 中に始まっていなければ始めない」は入ったが、始まっていた統合を冪等に中断・再開できるようにすれば
  drain を run の終わりだけで閉じられる（merge 前と検査中で分けて止める）。
