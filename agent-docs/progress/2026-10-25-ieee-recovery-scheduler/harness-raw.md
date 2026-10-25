---
title: IEEE recovery scheduler — raw の復帰時間・再作業・人介入(null)、壁時計/論理時計の分離、新 raw だけからの再集計（harness-raw）
tasks: [01M5T2P8MAW1564BFKSXXV33YG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# raw の復帰時間・再作業・人介入(null)、壁時計/論理時計の分離、新 raw だけからの再集計（harness-raw）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。ADR 付記 I14 (4)(6) を実装した。

## やったこと

- `crates/campaign/src/dagbench/pbs/recovery.rs`（新規）: `recovery-trials.jsonl`（`celeris.recovery-trial/1`）の
  `RecoveryRecord`。`trials.jsonl` の行とは `trial_key` と `binding_seq` で結合（`TrialBinding`・`derive_status` は不変）。
  controller の incarnation（pid・starttime・exit・kill）、親の独立読み直し（`saved_wait` の file は測定 dir 相対 + sha256）、
  停止中の原票 seq、壁時計 `wall_*_ms`（親の `Instant`）と論理時計 `logical_*_secs`（`FileClock`、`clock_kind=file_fake`）を
  別の欄に、`rework`（intent 台帳の `sent` 行・消費台帳から導く）、`human_intervention`/`rework.redone_local_nodes` は
  null + `missing` の理由（0 にしない）。`scheduler_class_of`（結合行から）と `derive_recovery` を writer と validator が共用し、
  unknown・fetch_failed・unresolved は `unobserved`、mismatch は `rejected`（成功にしない）。`RejectReason` 型も置いた（配線は harness-reject）。
- `raw.rs`: `RawWriter::record_recovery`、`validate` が `recovery-trials.jsonl` を raw だけから再検算（導出欄・台帳由来欄・
  saved wait の hash・停止中原票の参照・normal に停止欄なし・null に理由）。`RawReport.recovery`。
- `proc.rs`: `ProcTrial::run` が各時点の壁時計と論理時計を取り、`collect_trial` の後に記録を追記。`ProcReport.recovery`。
  `ControllerRun` は recovery.rs へ移し proc で re-export。
- `pbs/metrics.rs`（新規）: `regenerate(raw, out)` が `raw::validate` を通った raw だけから
  `scheduler_metrics.json`（`celeris.dagbench-scheduler-metrics/1`）・`scheduler_cells.tsv`・`scheduler_trials.tsv`・
  `scheduler_wall.tsv`・`scheduler_logical.tsv` を新規 dir に作る（既存 dir・raw の内側は Err、旧 durable raw などは版で Err）。
  出力に時刻・絶対 path を入れないので同じ raw（と写し）から byte 一致。`validated_completed` は unknown/unresolved/rejected を数えない。
- `tests/dagbench_harness.rs`: ignored 入口 `regen_from_env`（`DAGBENCH_REGEN_RAW=<測定 dir>`・`DAGBENCH_REGEN_OUT=<新規 dir>`）と
  試験 `harness_pbs_raw_` 4 本（再生成の一致と raw 不変・null の保持・unknown 非成功・時計欄分離）。

## 証拠（提出時の HEAD で実行）

| 検査 | 結果 |
|---|---|
| `cargo test -p celeris-campaign --test dagbench_harness harness_pbs_` | exit 0、10 passed（うち `harness_pbs_raw_` 4） |
| `bash scripts/dev/test-affected.sh` | exit 0、2695 passed / 8 skipped |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

既存資産: 再生成の試験は raw dir の全 file の sha256 一覧が regenerate 3 回と拒否 3 件の前後で不変であることを assert。
main-3・K4・既存 raw の path は触らない（試験は tempdir だけ）。

## 未解決事項（後続 unit へ）

- I14 (1) の「`run.json` を作成時 1 回にして `real_jobs` を再集計へ移す」は未実施。`run.json` は全 trial の後に
  `RawWriter::append(..).write_run(..)` で閉じる形のまま（`dagbench_pbs_raw`・`dagbench_pbs_e2e` が依存）。
  `regenerate` は閉じた測定だけを受け、`real_jobs` は validator が台帳と一致を確かめた値を写す。harness-close で突き合わせる。
- ADR との名前の差: 入口は objective に従い `regen_from_env` / `DAGBENCH_REGEN_RAW`・`DAGBENCH_REGEN_OUT`（ADR は
  `regenerate_from_env` / `DAGBENCH_RAW`・`DAGBENCH_METRICS_OUT`）。`wall_submit_to_wait_saved_ms` は親が投入時刻を実時間で
  見られないため `wall_spawn_to_wait_saved_ms`（spawn 起点）にした。`wall_recovery_ms` は kill → 再開 controller の exit（消費の後）。
- Err で終わる trial（(5) の拒否）に `recovery_status=rejected` の行を残す配線は harness-reject。
- validator は既存どおり intent 台帳を lock file 経由で読む（閉じた raw では内容不変を試験で確認）。

## 提案

- ADR 付記 I14 (4)(6) の欄名・入口名を上の実装名に合わせる（harness-close）。
