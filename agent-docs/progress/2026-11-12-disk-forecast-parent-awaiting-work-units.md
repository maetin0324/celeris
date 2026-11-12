---
title: 空きの見込みで、WU を待っているだけの親 task の target を予約しない
tasks: [01M78D03YNZ21WM22D5754G7TB]
status: done
completed: 2026-11-12
updated: 2026-11-12
---
# 空きの見込みで、WU を待っているだけの親 task の target を予約しない

完了日: 2026-11-12

設計: [ADR 2026-11-11-disk-aware-dispatch](../adr/2026-11-11-disk-aware-dispatch.md) の付記（2026-11-12、親の数え方）。

## 実装前 brief

1. 根本原因: 走査の分類（`task_worker::scratch::classify`）が `running` の task の owner `task-<id>` を一律に
   `task running` とし、`target_written_now` が増分を予約する owner に数えた。親が WU を待っているだけで target を
   持たないと、大きさ不明の増える owner として repo の見込み全体（本番 70.9 GB）を予約し、WU の owner の増分
   （66.9 GB）と合わせて WU の run 1 本で 137.8 GB になった。症状は dispatch の判定（`disk_admission`）の held に出た。
2. 責務 owner: 増分を予約するかを DB の状態から決める分類（`task_worker::scratch::classify` と `target_written_now`）。
3. 既存の再利用先: `StatusLookup`（store の lookup）、store の `work_units_for`、WU の `branch`（自分の worktree を切った WU に
   だけ `acquire_work_unit_lease` が `Running` と同じ transaction で入れる）、判定の `Gate::judge_run`（`Owner::of_run`）と
   通した分（`admitted`）、`task reviewing` の分類。
4. 選択肢: (a) daemon の `Gate` が走らせている run の owner から増分を数え直す。走査の status（`GET /api/v1/metrics/scratch`、
   `celerisctl scratch status`）と判定で増分が食い違い、ADR の「増分は DB の状態、書き手は daemon」の分け方も崩れる。
   (b) 走査の分類で親が WU を待っているだけかを DB から決める。(b) を採った。
5. 新しい状態・分岐: 分類の理由 `task waiting for work units`（P0 のまま）、`StatusLookup::task_work_units` と
   `UnitTargetUse`（WU の状態と Task の worktree を共有するか）、`task_awaits_work_units`。fallback・切替の flag は無し。
6. 撤去対象: 「WU を待つ `running` の親は増分の予約には数える」の数え方（ADR D1 の文、`disk_admission.rs` の説明）。
   旧い数え方を固定する試験は無かった（既存の試験は WU の無い task の `task running` だけを見る）。
7. 不変条件: 親が `running` で自分の worktree の WU が 1 本走るとき、走っている target の増分はその WU の owner の分だけ。
   Task の worktree を共有する WU が走る親と `reviewing` の親は親の owner を数える。WU の無い task の `running` は従来どおり
   数える。「詰まらない」規則・release-build の常時の予約・`forecast_disk_ratio` は変わらない。試験は下の「変更」。

## 変更

- `phase 1: 空きの見込みで WU を待つだけの親の target を予約しない` — 次を入れた。
  - `crates/task-worker/src/scratch/gc.rs`: `running` の task が WU を待っているだけ（終わっていない WU があり、`Running` の
    WU のどれも Task の worktree を共有しない）なら理由を `task waiting for work units`（P0）にし、`target_written_now` に
    入れない。WU が共有するかは branch の有無（子 task の unit は共有しない。`UnitTargetUse::of`）。`StatusLookup` に
    `task_work_units` を足した（`StoreLookup` は `work_units_for`、`NoDb` は空）。
  - 試験（守る不変条件と、既存の試験で検出できない理由は各試験の doc に書いた）:
    `task_worker::scratch::tests::a_running_parent_is_written_only_while_a_run_shares_its_target`、
    `a_unit_shares_the_task_target_when_it_has_no_branch_of_its_own`、
    `task_dispatch::scratch_gc::tests::a_parent_waiting_for_its_work_unit_reserves_only_the_work_unit_growth`（pool の走査から
    status の `admission` まで。本番の形で増分が WU の 66 GB だけ、Task の worktree を共有する WU が走ると親の 70 GB、
    `reviewing` の親も 70 GB）。Task の worktree を共有する WU の run の判定（owner `task-<id>` で見込みを数える）は既存の
    `disk_admission::tests::a_shared_worktree_work_unit_is_judged_by_the_task_owner` が確かめる。
  - 旧い数え方に戻したとき（`classify` の新しい分岐を無効にした一時的な変更）、新しい分類の試験と走査の試験の 2 つが
    `left: "task running" / right: "task waiting for work units"` で落ちることを確かめ、元に戻した。
  - ADR 2026-11-11-disk-aware-dispatch: D1 の「走っている target の増分」と「詰まらない」の親の文を改め、付記
    （2026-11-12、親の数え方）を足した。`disk_admission.rs` の module doc とコメント、`ScratchOwnerForecastView.grows` の
    説明を合わせ、API schema（`docs/api/v1/api-v1.schema.json`、`web/api/generated/schema.json`）を作り直した
    （`UPDATE_SCHEMA=1`、`node scripts/gen-types.mjs`。`types.ts` は差分なし）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `cargo nextest run -p task-worker -p task-dispatch -E 'test(/running_parent|shares_the_task_target|only_running_and_reviewing|parent_waiting_for_its_work_unit|target_sweep_scratch_cap|classify_follows|disk_admission|disk_forecast/)'` → 24 tests run, 24 passed（修正後）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6741 tests run, 6741 passed, 23 skipped。web の tsc -b と unit 103 files / 712 tests passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）
- 上の 4 つは commit `phase 1: 空きの見込みで WU を待つだけの親の target を予約しない` の上で実行した。その後の commit
  `phase 1: 進捗に検査の結果を書く` はこの進捗ファイルだけを変える（fmt と progress-index は再実行した）。

## 本番への移行

config の変更は無い。本番の `[scratch] forecast_disk_ratio = 0.3`（2026-11-12 17:29Z に operator が設定）はそのまま残す。
この変更を含む release の後、`GET /api/v1/metrics/scratch` の `owners` で、WU を待っている親の `reason` が
`task waiting for work units` になり、`admission.owners` に親の行が無い（target の無い親）か `grows: false` であることを
人が確かめる。

## 未解決

- 前に自分の worktree を切った WU が後で並列 1 に倒れて Task の worktree で走ると、branch が残るので親は
  `task waiting for work units` のままになる。その run の見込みは判定で通した分（`Gate` の `admitted`）が run の終わりまで
  数えるので数え落としは無いが、daemon の再起動の後に走り続ける run があれば数えない（未確認。再起動で run を引き継ぐ
  経路の有無を確かめていない）。

## 提案

- 無し。
