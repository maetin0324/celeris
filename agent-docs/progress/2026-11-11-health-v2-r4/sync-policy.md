---
title: "健全性v2 R4 sync-policy: main の policy の変更を取り込み、high_risk の照合で待ちを判定する"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-13
---
# 健全性v2 R4 sync-policy: main の policy の変更を取り込み、high_risk の照合で待ちを判定する

完了日: 2026-11-13

## 概要

- operator が policy だけの branch で main に入れた commit「policy: R4 の lease・budget の移動先を high_risk と capability-map の
  owner path に足す」（main の merge commit「integrate R4 policy-paths: lease・budget の移動先を repo health の policy に足す
  （policy の branch、operator の hand merge）」）を含む main を、`git merge main` で WU の branch に取り込んだ。
- check 0（main の high_risk が移動先の 6 file を覆うか）は exit 0。待ちは解けた。
- `scripts/dev/repo-health/` と `agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json` は main と同じ（`git diff --quiet main` が 0）。
- `crates/celeris/tests/support/script_compare.rs` は衝突せず、main と同じ内容になった。

## 取り込んだ policy の正規表現

`scripts/dev/repo-health/profiles/celeris.toml` の `[[review.high_risk]]` に main で足された行（既存の行は消していない）:

| class | 足された正規表現 |
|---|---|
| lease | `^crates/task-dispatch/src/(run_supervisor(\.rs$\|/)\|run_settlement(\.rs$\|/)\|dispatcher/leases\.rs$)` |
| budget | `^crates/task-dispatch/src/(routing_runtime/quota(\.rs$\|/)\|routing_runtime/quota_estimate(\.rs$\|/)\|run_settlement(\.rs$\|/)\|dispatch_admission(\.rs$\|/)\|dispatcher/quota_book\.rs$)` |

policy-paths.diff は移動先ごとに 1 行だったが、operator は移動先の file がまだ追跡 file に無いため（profile の試験
`test_health_review_celeris_profile_points_at_existing_paths`）、class ごとに 1 行の選択肢にまとめ、旧い
`dispatcher/leases.rs`・`dispatcher/quota_book.rs` も選択肢に入れた。当たる file の集合は diff と同じ。record unit の
「policy の撤去」節では、この 2 行の旧い path の選択肢も撤去の対象に挙げる（人の決定 policy-paths の指示）。

## 衝突の解き方（両側の意図を保つ）

衝突は 11 file。own-maint・own-cluster・own-hooks が移した状態と、main の新しい機能が重なった。

- `HostMaintenance`（own-maint）と main の disk-aware-dispatch（ADR 2026-11-11-disk-aware-dispatch）:
  main が `Dispatcher` の `ScratchState` に足した `admission: disk_admission::Gate` を、`host_maintenance.rs` の `ScratchState` に移した。
  dispatcher は `self.maint.admission()`（`begin_pass`・`judge_run`・`started`・snapshot の `with_held`）で呼ぶ。
  試験だけが probe を差し替える `admission_mut()` は `#[cfg(test)]`。
- main の `min_free_disk_mb` → `min_free_disk_pct` の置き換えを `MaintenanceInputs` と `HostMaintenance` の
  `scratch_gc`・`seed_housekeeping`・`check_disk_space` に写した。空きの判定は main の `disk_admission::find_low_disk`、
  seed の hold の下限は `disk_admission::min_free_bytes` を使う。旧い `host_maintenance::free_disk_mb` は消した（呼び出し元が無くなった）。
- main の scratch-lock-tick-stall（target sweep の `try_lock`）を `HostMaintenance::run_target_sweep_task` に写した。
- `HostMaintenance::disk_watch_paths` は main の `disk_watch::paths_label` を呼ぶ（同じ処理を二重に持たない）。
- CoS の起動の門（main の cos-skill-sync・cos-fallback-timing の `Skip::gate`）は main の形で、空きの判定だけ `self.maint.disk_ready()`。
- `dispatcher.rs` の use: main の `check_slots::CheckLimits` を足し、own-cluster が外した `control_master_alive_blocking` は戻さない。
- `lib.rs`: own-cluster の `cluster_runtime` と main の `cos_context`・`cos_skill_gate`・`disk_admission` を両方残した。
- `docs/architecture-map.md`: own-* の owner の行（`ClusterRuntime`・`HostMaintenance`）と、main の行（KB の自動適用、
  `disk_admission`、`cluster_jobs_continuation`）を合わせた。`disk_admission` の行に `host_maintenance.rs`（`Gate` を持つ owner）を足した。
- 試験: main の新しい試験の `d.test_now` を own-hooks の `d.test_hooks.now` に、`d.scratch.admission` を `d.maint.admission()` に、
  `d.cluster_job_polls`・`d.cluster_job_poller` を `ClusterRuntime` の口（`job_polls_in_flight()`、試験用に足した `job_poller()`）に直した。

## 意味の衝突（cargo check と test-affected で見つけたもの）

- `cargo check --workspace --tests --keep-going`: 衝突の解消の後は `cluster_job_wait.rs` の 2 件（上の `ClusterRuntime` の口）だけで、直した後は exit 0。
- test-affected の 1 回目（取り込みの commit）で `cex_review_authority.rs` の 2 本が落ちた:
  `cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate`・
  `cex_integration_sync_conflict_schedules_one_repair_and_never_passes_the_conflict`。原因は main の
  ADR 2026-11-11-health-generic-review-input（R1・R6）で `[review]` の無い repo の reviewer も health の材料を受け、
  review.json に `health` の判定が無いと不合格になること。cex の偽 reviewer 2 つに main の `target_sync.rs` などと同じ空の判定
  （`"health": {"findings": []}`）を足した。試験の守る不変条件は変えていない。cex の試験 25 本は合格。

## 証拠（提出時の HEAD で実行）

基準は `$CELERIS_WU_BASE`。結果は下の「検査の結果」に書く。

- check 0: `git diff --quiet main -- scripts/dev/repo-health agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json && python3 -c …`
  （計画の check 0 と同じ式。lease 2・budget 4 の移動先）
- `cargo check --workspace --tests --keep-going`
- `bash scripts/dev/test-affected.sh`
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --all -- --check`

### 検査の結果

- check 0: exit 0（6 つの移動先すべてが main の high_risk の lease・budget に当たる。policy の 2 file は main と差分 0）。
- `cargo check --workspace --tests --keep-going`: exit 0（warning は main と同じ `script_exhaustive.rs` の 1 件だけ）。
- `bash scripts/dev/test-affected.sh`: exit 0、781 秒（load 15〜19）。nextest 8408 件すべて合格（23 skipped）、web の unit 713 件・
  134 件合格、`repo_health_gate: all checks passed`、`test-ts-verify: 20 passed, 0 failed`。
  1 回目（cex の直しの前の取り込みの commit）は exit 100（8406 passed・2 failed、上の cex の 2 本）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## WU の check の health の再評価（3 回目の run）

2 回目の run で申告した不合格（下の「経過」）は、Celeris の規則の側で直った。WU の check と段の統合の health の base を、取り込んだ
review target の分だけ進める規則（ADR 2026-11-13-health-base-merged-target、task 01M79CHBRY）が本番に入ったので、
人（外部 operator の運用 session）の回答どおり、コードは変えずに同じ HEAD を新しい規則で評価し直した。

- target は repo の既定 branch（`refs/heads/main`。この task は木の子でない）。T は `git merge-base --all HEAD refs/heads/main` の 1 件で、
  取り込んだ main の commit（main の merge commit「integrate R4 policy-paths: lease・budget の移動先を repo health の policy に足す
  （policy の branch、operator の hand merge）」の後の commit）。start は WU の基点「integrate wu/own-hooks (phase owners-edge)」。
- start と T はどちらも相手の祖先でないので、main の `health_review::target_base` と同じ手順（`git merge-tree --write-tree` の tree を、
  親 start・T、固定の作者・日付・本文で `git commit-tree`）で base の commit を作った。merge-tree は exit 1（11 file の衝突。
  検査器の file には当たらない）なので、ADR D3 のとおり base = T の 2 回目の検査も走らせた。検査器はどちらも base の版を
  `git archive` で一時 dir に出して `python3 -I -B repo_health.py --repo … --base … --head HEAD --policy-version 1 --out …` で起こした。
- 結果（提出の前の HEAD。この節を足した commit の前）:
  - 1 回目（base = start と T の merge）: exit 0、verdict pass、fail_grade 0、new 0、worsened 0、`policy.changed` は false。
  - 2 回目（base = T）: exit 0、verdict pass、fail_grade 0、new 1（warning の `migration.remnant`。`crates/task-dispatch/src/host_maintenance.rs`
    の `legacy` の語。own-maint が移した `scratch_gc::legacy_paths` の呼び出しで、main の側にはこの file が無いので new になる。fail 級でない）、worsened 0。
- 2 回目の run の fail 級 5 件（`policy.changed`・`progress.index`・`size.responsibility-growth` 2 件・`source-size`）は、どちらの base でも出ない。

## 経過（2 回目の run の申告）

- WU の check の health（`celeris:repo-health/1`）は、旧い規則ではこの unit の目的（main を取り込む）と両立せず、成果をどう直しても通らなかった。
  2 回目の run（attempt 1 of 3）で原因を確かめ、done を返さず計画の問題（plan_issue）として申告した。
  - 原因: 旧い規則の WU の health の base は `wu.base_commit`（main を取り込む前の段の基点
    「integrate wu/own-hooks (phase owners-edge)」）で、取り込んだ main の commit の変更も候補の変更として数えた。
  - fail-grade の 5 件はどれも main の commit の変更から出た:
    `policy.changed`（main の policy の commit が変えた profile・capability-map・検査器）、`progress.index`（main の進捗 5 file の hash が
    段の基点の祖先でない）、`size.responsibility-growth` 2 件と `source-size`（main の disk-aware-dispatch・host-check-slots が
    `dispatcher.rs` と `dispatch_one` に足した行）。

## 未解決事項

- `crates/celeris/tests/script_exhaustive.rs:23` の unused import の warning（`ScriptGenerationControl`）は main と同じ file の既存の warning。
  この unit では変えていない。
- main はこの取り込みの後も進んでいる（health base の規則の task など）。policy の 2 file（`scripts/dev/repo-health/` と
  capability-map.json）は今の main とも差分 0。取り込みの後の main の変更は、review 前の main 同期で入る。

## 提案

- 2 回目の run の提案（main の取り込みを基点の無い unit で行う計画の直しと、health の base に取り込んだ target を入れる Celeris の直し）のうち、
  Celeris の直しが ADR 2026-11-13-health-base-merged-target で採られ、本番に入った。この run の再評価は合格したので、計画の直しは要らない。
