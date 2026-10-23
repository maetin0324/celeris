---
tasks: [01M5KVHDY26YCNYGZKDTANGJX6]
---
# ADR: A の受理済み job ID を submit wrapper の受理記録から台帳へ運び、収集の失敗を分類して研究の失敗と分ける

- Date: 2026-10-23
- Status: Accepted

## Context

- `HarnessExecutor`（A arm）の `TaskRef` は task_id と pid だけで、`cluster_job_ids` は常に空だった。`track` は既定の clone、台帳は `TaskRef` の job ID を写すだけなので、A の実験 case は `collect-observed-out` が「no job was submitted」で必ず落ちた。`tests/launcher.rs` の試験は台帳に job ID を手で追記しており、この欠落を隠していた。
- `observe::collect` は失敗を文字列の列でしか返さず、収集に失敗すると観測記録が無いだけになる。`grade-out` は観測なしの実験証拠を一律に不合格（failure_class=model_failure）にしていたので、ssh・qstat の不通（測定基盤の失敗）がモデルの失敗に数えられ得た。

## Decision

- D1 受理記録: 試行内の制御された submit wrapper `scripts/campaign/celeris-qsub` を置く。実 submit command（`CELERIS_SUBMIT_CMD`。例 `ssh sirius qsub`）に `-v CELERIS_TRIAL_KEY=<冪等キー>` を付けて投入し、scheduler が受理して返した job id だけを `CELERIS_SUBMIT_AUDIT` に `celeris.submit-audit/1` の 1 行（trial_key・scheduler・cluster・job_id・accepted_at）で追記する。受理されなかった投入は記録しない。
- D2 executor → 台帳: `HarnessExecutor::submit` は `CELERIS_SUBMIT_AUDIT`（sandbox では試行 state dir の `submit-audit.jsonl`、無ければ key dir）と `CELERIS_TRIAL_KEY` を command の env に入れ、path を `meta.json` に残す。`track` は受理記録からこの trial の行だけを `scheduler:cluster:job_id` として順序どおり重複なく参照に足す（別 trial の行・PID 形・壊れた行は採らない）。runner は `Done` を確かめた直後にもう一度 `track` し、終了直前の受理を台帳（`Ledger::record_ref`）に取り込む。toolchain env は `CELERIS_SUBMIT_AUDIT`・`CELERIS_TRIAL_KEY` を設定できない。
- D3 同定契約: 台帳の job ID は成功根拠ではなく照会先にすぎない。collect は scheduler から記録を取り直し、`Variable_List` の `CELERIS_TRIAL_KEY` があれば台帳行の冪等キーと一致を要る。A は記録が試行内の wrapper から来るので印を必須にする（印が無い・別 trial の印は identity 不一致）。B/C の job ID は Celeris の cluster wait（評価側）が正で、印があれば同じ照合を受ける。
- D4 収集の分類: `observe::CollectClass` = `not_submitted`・`not_finished`・`fetch_failed`・`unobserved`（scheduler が job を知らない・記録や raw の欠け・台帳行なし）・`identity_mismatch`・`artifact_changed`・`evaluator_asset`。failure_class への写像は not_submitted → model_failure、fetch_failed・evaluator_asset → env_failure、他は undetermined。job の非 0 終了（研究の失敗）は収集の失敗ではなく、観測記録を作り `job_status: job_failed` と印す（合否は grader が決める）。
- D5 記録と再収集: 失敗は `<out>/observed/<problem>/<Config>-rep<N>.collect-failure.json`（`celeris.observe-failure/1`、trial identity・issues・failure_class）に最新を書き、試みは成功・失敗とも `<out>/observed/<problem>/collect-log.jsonl` に追記する。収集し直して成功すれば失敗記録は消え、履歴は残る。既存の観測記録は取得の一時失敗で消さない。
- D6 grade: `grade-out` は観測記録が無いとき、identity が一致する失敗記録を読み、`grader::apply_observation_failure` で実験証拠の不合格の原因を分類に合わせる（env_failure・undetermined は model_failure に数えない）。grade 行に `observation`（`observed` / `collect_failed`＋分類 / `none`）を足す。注入した workflow fault（`faults.jsonl`）とは別の欄で、混同しない。GradeBinding は `ObservationFailure(Vec<CollectIssue>)` で収集分類・詳細を保持する。空の issues、未知分類、failure_class との矛盾、final_pass と収集失敗の同居は拒否する。check/export は `observation_failed`（cell 数）と `observation_failures`（分類別 cell 数、複数分類は重複可）を JSON/CSV に出し、grader_env_failure と分離する。not_submitted 以外は complete=false とし、原因を直して再収集・再採点する案内を出す。fault_breakdown は注入状態 × observation_failed を独立に数える。
- 対象外: 受理応答を失った投入の照合（reconciliation）・exactly-once・新しい durable wait は足さない。wrapper を通らない投入は台帳に入らず、その試行の実験証拠は観測なし（not_submitted）になる。

## Consequences

- 運用者は A の cap-env toolchain に `celeris-qsub` を ro mount で PATH に置き、`CELERIS_SUBMIT_TARGET=pbs:<cluster>` と `CELERIS_SUBMIT_CMD` を与える（cap-env は A と worker の toolchain の一致を要るので worker 側にも同じ値が入るが、B/C は `CELERIS_SUBMIT_AUDIT` が無いので wrapper を使えない）。A の prompt で qsub の代わりに `celeris-qsub` を使うよう指示することは別の判断（case 資産）。
- 試験: `tests/launcher.rs` `grade_collects_job_observations_bound_to_the_trial_and_rejects_every_mismatch`（台帳の手編集を削除し、wrapper → track → 台帳 → collect → grade の通常経路）、`tests/harness_exec.rs` `submit_wrapper_records_accepted_job_ids_and_track_carries_them_without_duplicates`、`tests/short_ml_observation.rs` `collection_outcomes_are_classified_and_fetch_failures_are_not_model_failures`。
