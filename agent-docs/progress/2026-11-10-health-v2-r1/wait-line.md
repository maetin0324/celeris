---
title: 健全性v2 R1 wait-line — finish_run の Waiting の run dir の行を共通試験で固定する
tasks: [01M6TYVJSZDSCB697193J01FYW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R1 wait-line — finish_run の Waiting の run dir の行を共通試験で固定する

完了日: 2026-11-10

WorkUnit `wait-line`（[ADR 2026-11-10 harness adapter の結果契約と実行共通処理](../../adr/2026-11-10-adapter-result-contract.md) の (4) 4b・付記 (i)）。final review の criterion 1 の差し戻しを直す。基準は `$CELERIS_WU_BASE`（slices-check と repo-health は `git merge-base HEAD main`）。

## 差し戻しの内容
unit result-contract は claude_code の `result_wait_becomes_terminal_waiting` を「契約の写し」として消し、代わりを「dispatcher の `cluster_job_wait` の試験」とした。reviewer の指摘どおり、この代わりは誤りだった。
- 消した試験は、adapter が `Terminal::Waiting` で終えた run の `runs/<run_id>/result.json` に `{"type":"wait",...,"jobs":[...]}` の 1 行が書かれること（`crates/task-worker/src/subprocess.rs` の `finish_run` → `write_result_json` の `Terminal::Waiting` → `WorkerMessage::Wait` の写像）を確かめる唯一の試験だった。
- この行は daemon の再起動と orphan の引き継ぎで、task-dispatch の `dispatcher.rs` の `terminal_from_run_dir` が `task_worker::adapter::wait_terminal` で `Terminal::Waiting` に戻す。
- dispatcher の `cluster_job_wait` の試験は scripted adapter が `Terminal` を直接返すので、run dir の行を書きも読みもしない。run-common の `run_common_finish_run_maps_provider_failure_to_adapter_error` も `Error` と `Done` しか見ない。

## 変更
- 件名「R1 wait-line: finish_run の Waiting の run dir の行を共通試験で固定し、claude_code の結線の試験を戻す」の commit
  - `crates/task-worker/src/subprocess/tests.rs` に共通試験 `run_common_finish_run_writes_wait_line_for_waiting` を足した。
  - `crates/task-worker/src/claude_code/tests.rs` に結線の試験 `result_wait_becomes_terminal_waiting` を戻した（base の試験の前半だけ。不正な wait と question の優先は `result_contract_` の試験が見るので戻していない）。
  - `crates/task-worker/src/result_report_tests.rs` の `RunContext` を `RunContext { cos_chat: Some(Default::default()), ..Default::default() }` で組むようにした（clippy `field_reassign_with_default`）。
  - 進捗（この file、[R1 の進捗](../2026-11-10-health-v2-r1.md)、[result-contract.md](result-contract.md)、[run-common.md](run-common.md)）と ADR の付記 (i)。
- `subprocess.rs` は変えていない。試験は写像の欠陥を見つけなかった。`crates/task-dispatch/` も変えていない（R1 の write_set の外）。

## 試験
| 試験 | 置き場 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|---|
| `run_common_finish_run_writes_wait_line_for_waiting` | `subprocess/tests.rs`（`subprocess::tests`） | `finish_run` に `Terminal::Waiting`（cluster・jobs 2 件・scheduler pbs・poll_secs・timeout_secs・checkpoint・summary・usage）を渡すと `Ok` の `RunOutcome` が同じ `Waiting` を持ち、run dir の `result.json` が改行で終わる 1 行の JSON で `type=wait`・`kind=cluster_job` と各欄を持つ。その行を dispatcher の `terminal_from_run_dir` と同じ手順（`serde_json::Value` に読み、`usage` を `task_core::Usage` に読み、`adapter::wait_terminal`）で読むと元と同じ request・checkpoint・usage に戻る | 共通試験の `run_common_finish_run_maps_provider_failure_to_adapter_error` は `Error`・`Done` だけを見る。dispatcher の `cluster_job_wait` の試験は scripted adapter が `Terminal` を直接返すので run dir の行を通らない。adapter の結線の試験は行の一部（type・kind・jobs）しか見ない。`write_result_json` の Waiting の枝が欄を落としても、どれも落ちない |
| `result_wait_becomes_terminal_waiting` | `claude_code/tests.rs` | stub の claude が `artifacts/result.json` に wait を書くと、結果契約を通って `Terminal::Waiting`（cluster・jobs・scheduler・poll_secs・timeout_secs・summary・checkpoint・stream の usage）になり、`runs/run-wait/result.json` に `type=wait`・`kind=cluster_job` の行と jobs 2 件が残る | 結果契約の試験（`result_contract_priority_order`・`result_contract_usage_passthrough`）は `interpret_result_text` を直接呼び、adapter が `finish_run` に `Waiting` を渡す結線を通らない。claude_code の他の結線の試験は `Done` だけ |

変異の確認: `write_result_json` の `Terminal::Waiting` の枝で `checkpoint: checkpoint.clone()` を `checkpoint: None` に変えると、`run_common_finish_run_writes_wait_line_for_waiting` が落ちた（`subprocess/tests.rs` の `line["checkpoint"]` の比較）。`result_wait_becomes_terminal_waiting` は checkpoint の欄を見ないので通った（結線の試験の役目は終端の種類と行の有無）。確認の後、`subprocess.rs` は `git checkout` で元に戻した（`git diff --quiet` で差分ゼロ）。

`crates/task-worker/src` の `#[test]`・`#[tokio::test]` は `$CELERIS_WU_BASE` の 1191 から 1193 になった（`git merge-base HEAD main` の 1192 から +1）。

## 検査（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
test-affected・clippy・nextest は件名「R1 wait-line: finish_run の Waiting の run dir の行を共通試験で固定し、claude_code の結線の試験を戻す」の commit（clean な tree）で実行した。その後の commit（件名「R1 wait-line: 進捗に提出時の HEAD での検査の結果を書く」）は進捗の文書だけを変えたので、文書の検査・fmt・slices-check・check-health をその HEAD でもう一度実行し、同じ結果だった。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。nextest 5855 passed（22 skipped）、400 秒。repo-health は `verdict=pass`・`fail_grade=0`・`new=0`・`worsened=0`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0（`result_report_tests.rs` の警告 0 件）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo nextest run -p task-worker -E 'test(/run_common_|result_contract_|result_wait_becomes_terminal_waiting/)'` → exit 0、13 passed（`run_common_` 6 件、`result_contract_` 5 件、`read_result_file_recovers_strays_only_for_runs_that_require_the_file`、`result_wait_becomes_terminal_waiting`）。
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-adr-numbers.sh` → exit 0（414 files。`--refs --base $CELERIS_WU_BASE` も exit 0）。`sh scripts/dev/progress-index.sh --check` → exit 0。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。`python3 scripts/dev/check-human-text.py` → exit 0（0 findings）。
- `bash scripts/dev/check-health.sh` → exit 0（`repo_health_gate: all checks passed`）。
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0（`write-set R1: ok`）。基準を `git merge-base HEAD main` にしても exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/task-worker/src/{claude_code/tests,result_report_tests,subprocess/tests}.rs`、ADR、R1 の進捗、`2026-11-10-health-v2-r1/` の `result-contract.md`・`run-common.md`・`wait-line.md` だけ。

## 未解決
- `cargo clippy --workspace --all-targets` は `crates/task-core/src/chat/store_tests.rs` の `useless_conversion` で落ちる（R1 の write_set の外。既存）。task-worker の分（`result_report_tests.rs`）はこの unit で直した。

## 提案
無し。
