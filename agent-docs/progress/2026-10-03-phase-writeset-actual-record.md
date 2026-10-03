---
title: Phase 5 actual-record — actual write-set 記録（ADR-0130 D2 の実装上の明確化）
tasks: [01M3YE0JTQEYBFDTV3HCHR4G9J]
status: done
updated: 2026-10-03
---
# Phase 5 actual-record: actual write-set 記録（ADR-0130 D2 の実装上の明確化）

D2 の配線（`task-ops/changes.rs` の `run_start_head` / `committed_write_set`、`task-dispatch/dispatcher/write_set_record.rs`）で決めた細部。ADR-0130 の不変条件は変えない（この葉は docs/adr を変えない範囲のため、明確化はここに記録し、verify 葉で ADR へ取り込むか判断する）。

- **run 行の基点**: WU の run も atomic の run も、`run_write_sets` の `base_sha` は dispatch 直前に固定した**その run の開始 HEAD**（worktree が未作成ならブランチ先端、ブランチも無ければ切り出す base）。WU の `base_commit..HEAD` は done 時の `work_unit_write_sets` だけに使う。continuation で WU の run が複数あっても run 行は互いに重ならず、WU の累積は snapshot が持つ。
- **採取の時点**: `finish_worker_result_with` の中で、v2 WU の自動 commit（`commit_work_unit`）の後、`WorkUnitTransitioned` を書いた後に採る。planner run は採らない。
- **開始 HEAD はプロセス内メモリ**（`Dispatcher.run_write_bases`）。daemon の再起動を跨いだ run は `unavailable`（理由 `run start HEAD unknown`）とし、推測した基点で埋めない。
- **remote / shared / 非 Git**: remote の repo を含む task は `task_workspaces_for` が手元の Git worktree を作らない（手元は rsync の写し）。「同期後の手元 worktree で取る」対象の Git worktree が無いので、D2 のとおり repo ごとに `unavailable`（理由 `no local Git worktree`）を残す。
- **案件の repo を選ばない旧来の 1 worktree**（`task.repos` が空）は RepoId が無いので記録しない（D3 の照合も repo ID 単位のため）。
- `dirty`（未コミットの編集・追跡外ファイル）は `incomplete`。未コミットの path は `paths` に入れない。git の失敗はすべて warn と `unavailable` の行で、run の遷移・attempts に影響しない。
- 着手時 main = ae780a91。`git merge-tree --write-tree --name-only HEAD main` は exit 1、衝突候補 10 件（task-api query.rs と tests、task-core cluster_job/tests.rs・store/migrations.rs・store/tests.rs、task-ops delivery.rs と tests、task-worker tests/browser_shared_cdp.rs、docs/PROGRESS.md、gui inbox.tsx）。いずれもこの葉の変更 file ではない（task branch 側の既存差分由来）。
