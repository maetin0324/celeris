---
wu: observed-wiring
task: 01M59KY5FTRJTJ80PACNW3K533
status: done
completed: 2026-10-19
---
# observed-wiring: PBS 評価側観測の限定配線

判断材料は外部資料にある。ここには確定した成果だけを書く。変更はすべて `crates/campaign`。

## 標準収集呼出し（後続実行者向け）
`celeris_campaign::observe::collect_for_trial(transport, out, TrialKey{problem, config, rep}, refs, expect, artifact, raw_paths)`
- `refs`: Celeris 台帳の `cluster_job_ids`（`scheduler:cluster:job_id`）を `JobRef::parse` したもの。trial の自己申告・PID は使わない。
- `expect`: 固定の scheduler・cluster と case の `experiment.conditions`（queue・ncpus）。
- `transport`: read-only の `SchedulerTransport`（`qstat -x -f` 相当の本文を返す）。固定応答は `origin=synthetic` として記録され本番証拠と区別される。
- 出力: `<out>/observed/<problem>/<Config>-rep<N>.json`（`celeris.observed-jobs/1`）。`grade-out` と `grade --observed` が既存のまま読む（grader 側は無変更）。
- 拒否（記録を書かない）: 未実行（refs 空）、PID 形の id、投入先が case の固定と違う、取得元が投入先と違う、応答の job id 不一致、未終了（job_state≠F）、Exit_status 欠落、raw 欠落。exit_status・条件は観測のまま記録し、合否は通常 grader が case と照合する。raw の sha256 は評価側が成果物から計算する。

## 未解決・提案
- 実 scheduler 用の `SchedulerTransport` 実装（ssh での read-only `qstat -x -f`）は未作成。実投入・remote 操作を伴うため本 WU の範囲外。実行前に別途用意する。
- task-api の木 detail（`cluster_job_wait`）は job の状態のみで Exit_status を持たないため、終了記録は scheduler 照会が必要。
- 合成記録（origin=synthetic）を受入側で本番証拠と区別して拒否する扱いは acceptance-cli 側で決める。

## 証拠
- `cargo test -p celeris-campaign --test short_ml_observation`: 6 passed（成功/exit≠0/条件違い/raw tamper/job id のみ自己申告/未実行・未終了・不一致・欠落の拒否）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-parallel.sh`: 5832 passed / 1 failed（`a_program_pin_and_watch::watch_stop_and_restart_…` は高負荷時の既知のタイミング依存。単独再実行で 4 passed）。
