---
tasks: [01M5WC0AZ6MP312BFER7C8QKA0]
---
# ADR: collect-observed-out は not_submitted だけなら記録を書いて exit 0、測定基盤の class があれば非 0

- Date: 2026-10-26
- Status: Accepted

## Context

- 旧 nollm 通し（stub harness、無評価 LLM）で `celeris-campaign collect-observed-out` が exit 1 になった。出力は `failed` が k2 の全 trial で `classes: ["not_submitted"]`・`failure_class: model_failure`、`ok: false`。stub は cluster job を投げないので、台帳の `cluster_job_ids` が空になり、`observe::collect` が `NotSubmitted` を返す。
- `observe::collect_out` は `failed` が 1 件でもあれば `Err` を返し、`main.rs` が非 0 で終わる。`failed` には not_submitted と測定基盤の失敗（not_finished・fetch_failed・unobserved・identity_mismatch・artifact_changed・evaluator_asset）が区別なく入っていた。
- 一方で `grades.rs`（`GradeOutcome::is_observation_failure`）と `checker.rs` は ADR 2026-10-23-a-submit-audit-and-collect-classes D4/D6 で、not_submitted だけの cell をモデルの失敗（grader_failed）として数え、観測障害にも再収集・再採点の対象にもしない。collect の終了判定だけがこの契約と食い違い、モデルが job を投げなかっただけの正常な測定（stub 通しを含む）を収集の失敗として止めていた。

## Decision

- D1 終了判定: `collect_out` は失敗 trial のうち not_submitted 以外の class を 1 件でも持つものがあるときだけ `Err`（非 0）を返す。not_submitted だけの trial は失敗記録（`<out>/observed/<problem>/<Config>-rep<N>.collect-failure.json`、`celeris.observe-failure/1`）と `collect-log.jsonl` を従来どおり書き、終了は 0。not_submitted と基盤 class が同じ trial にある場合も、基盤 class が勝って非 0（D4 の優先と同じ）。
- D2 報告: 出力 `celeris.campaign-collect-out/1` の `failed` は従来どおり not_submitted も含めて全件を列挙する。`ok` は「基盤 class の失敗が 0 件」を表す（終了 0 と同値）。件数の欄 `not_submitted`（not_submitted だけの trial 数）と `blocking`（基盤 class を含む trial 数）を足す。
- D3 launcher: `measure-launcher.sh` の `cmd_grade` の警告は非 0 の意味（測定基盤の収集失敗。直して再収集・再採点する）に合わせる。not_submitted だけでは警告しない。
- 別案（stub が偽の job 記録を残す）は採らない。stub は job を投げていないので、記録を作れば評価側の観測の契約（台帳の job ID・scheduler の記録に結ぶ）を偽ることになる。

## Consequences

- 試験: `tests/launcher.rs` `collect_not_submitted_only_writes_failure_records_and_exits_zero`、`collect_not_submitted_mixed_with_infra_class_stays_nonzero`（A は scheduler 応答に job 記録が無い unobserved、B/C は not_submitted）。既存の `grade_collects_job_observations_bound_to_the_trial_and_rejects_every_mismatch` は、A が収集でき B/C が not_submitted の最終段で exit 0 を確かめる。
