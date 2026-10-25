---
title: IEEE recovery scheduler — scheduler raw/1 の writer・validator と trial への collector 結合（raw-collect）
tasks: [01M5SSBCP75CTVF2SHD0QQ9ZGG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# scheduler raw/1 の writer・validator と trial への collector 結合（raw-collect）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。
[ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) D3・実装付記 I6・I7 に従い、実装で決めた形を付記 I11 に足した。
実 scheduler・ssh・qsub・外部 network には通信していない（偽 transport のみ）。

## やったこと

- `crates/campaign/src/dagbench/pbs/raw.rs` を追加（`celeris.dagbench-scheduler-raw/1`、dataset=`scheduler_measured`）。
  - `RawWriter`: `create`（新規の出力先のみ）・`reopen`（再収集のみ）・`record_replies`（PBS 呼び出し 1 回 = `scheduler-events.jsonl` 1 行、seq・UTC・observer epoch・単調時計・identity・根拠参照、原票は sha256 付きで新規作成）・`collect_trial`（`QstatView` だけを受け、既存 `observe::collect` の `CollectClass` と `celeris.observed-jobs/1` を trial_key に結合して `trials.jsonl` に追記）・`write_submit_audit`（台帳から `celeris.submit-audit/1` を導出）・`write_run`（identity/pins、clock=wall、資源、予算、LLM calls=0、実 job 数）。
  - `validate`: run.json の版・pin・実 job 数・LLM calls、events の seq/時刻/単調時計/原票 hash、結合行の trial_key↔collector identity、intent↔job_ref、submit audit・observed 副記録の一致、成果物・grade の hash、欠測の理由、`derive_status` による結果の再計算を検査し、食い違いを拒否。
  - gone・取得失敗・履歴消失・採点入力欠落は null と理由で残し、`F`/`Exit_status=0` だけでは `validated_completed` にしない。照合できない投入は `unresolved`（分類 `Unobserved`、`NotSubmitted` にしない）。
- `metrics::compute` と `durable::verify` は新 raw の run.json を版で拒否する。
- 試験 `crates/campaign/tests/dagbench_pbs_raw.rs`（6 件）。

## 証拠

commit「phase 1: add scheduler raw writer/validator with collector binding」の HEAD で実行。

- `cargo test -p celeris-campaign --test dagbench_pbs_raw` → 6 passed（正常結合と validator 合格、再収集で qsub 1 のまま・消費台帳不変・原票と結合行は追記、欠測 5 種は null+理由で成功にしない、collector 不一致 6 種の拒否、原票改変/欠落・成果物改変・real_jobs/LLM calls 偽りの拒否、durable reader・metrics が新 raw を拒否）。
- `bash scripts/dev/test-affected.sh` → exit 0、2679 passed / 7 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → crates/campaign（dagbench/{durable,metrics}.rs、pbs/{mod,raw}.rs、tests/dagbench_pbs_raw.rs）と ADR・本進捗のみ。

## 未解決事項

- `process-events.jsonl`・`durable-wait.json`・worker/control 記録の raw への同梱と、実 scheduler 入口からの writer 呼び出しは wire-entry に残した。
- `celeris.dagbench-scheduler-metrics/1`（実時間の集計）は未実装。

## 提案

なし。
