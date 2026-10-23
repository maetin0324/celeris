---
title: "研究評価再監査 2/P0: 独立採点と人手・待機・再作業集計を同じ成功契約に揃え、共有作業と CSV を直す"
tasks: [01M5KVHDWYTACKJKBNKWQXGX0M]
status: done
updated: 2026-10-23
completed: 2026-10-23
---
# 成功契約と human-op 集計の整合

ADR: `agent-docs/adr/2026-10-23-campaign-success-contract.md`。定義: `docs/ops/eval-raw-record-definitions.md` §4・§5・§7、手順: `docs/ops/eval-human-ops-runbook.md`。

着手時の main は監査対象と同じ `13d64acd17806d73a1797bffbf831efc9cb5ab6a`。5 群の所見はどれも main で未解消だった（解消済みで変更を省いた項目は無い）。
main 反映 commit はまだ無い（確認時の main `d7cb0fd4` に対し `git merge-base --is-ancestor 9bbf6827 main` は exit 1）。人の取り込み後に決まる。
この branch `celeris/01M5KVHDWYTACKJKBNKWQXGX0M` の commit（base `d7cb0fd4` からの履歴）:
- `9bbf6827` 成功契約・human-op・共有作業の訂正・B の待機/自動 retry・CSV quoting の実装（本体）。
- `2b15bd6d` Jev e2e 試験を試験の task の request だけで数える（範囲外の flaky 修正）。
- `165d76e1` 偽 webhook の HTTP 要求前 EOF を通知に数えない（範囲外の flaky 修正と回帰試験）。
- `cd689927`・`f08ce9db` 第 3 回検証・再提出の記録（誤って `docs/progress/` に置いた。今回 agent-docs のこのファイルへ統合し削除）。
- 今回の commit（attempt 2 of 3 の差し戻し対応）: 未観測の人の時間・介入・再作業を 0 にしない修正と本ファイルの統合。
過去の報告にあった `d1ef3181`・`e5fedc86`・`4a0f8f93`・`9dd77c10`・`801279e0`・`d7687881` は別 worktree の履歴の hash で、この branch の祖先ではない（上の 5 commit が同じ変更）。

## 変更
- `crates/campaign/src/success.rs`（新）: 計画 cell ごとの判定（trial_no 最小・`SuccessClass`・`needs_regrade`・未実行・記録数）と out dir からの `load`。
  checker の `recompute_graded` はこれで集計する。
- `human_ops_join.rs`: runner の out dir を渡すと計画 cell をすべて行にし、`executor_status`（最初の試行）・`validated_success`・`success_class`。
  export の `wait` 行 → `event_waits`、`run.is_retry` → `auto_retries`、`celeris_event_ref` の照合 → `event_refs`。旧 `outcome`（最後の試行）・`failure_reason` は削除。
- `human_ops_report.rs`（schema `/2`）: 完遂 = validated success、共通 filter `counts_as_human_op`、観測窓（cutoff か試行終了）を介入・時間・再作業に共通適用、
  観測源の無い待ち・job は欠測、`celeris_wait`・`auto_retries`・`corrections`・`success_contract`・`cost_scope`。暫定 `failure_cause` は削除。
- `human_ops.rs`: `--scope shared`、`human-op correct`（`--retract` / `--set-field`）、`resolve_with_history`（retracted を外し補正履歴を返す）。
- `task-core` `decision_log_human_waits` と `celerisctl campaign export` の `wait` 行（Question/Approval）。
- `crates/campaign/src/csv.rs`（新）と `export::csv_table`: RFC 4180 の quoting。`export::COST_SCOPE` を table.json に出す。

## 受け入れ条件と証拠
0. 共有契約: `crates/campaign/tests/success_contract.rs::checker_export_and_human_ops_reach_the_same_conclusion_per_cell`（A 合格・B model_failure・C env_failure の同一 fixture で
   checker totals・export row・`success::load`・human-op report の arm/試行が同じ）。
1. 旧 main-3 再集計（旧採点のまま。task 1 の grader 変更による再採点ではない）: A = executor passed / `model_failure` → validated 0/1、B = `executor_cancelled`（case_budget）→ 0/1。
   checker（`--measurement-id mlsys-main-3`）・export・report が一致。旧 report は A を `completion: true`、protocol の trial-start attempt=2 を rework 1、窓の外の status_check を介入 3 と数えていた。
2. shared/correction: `tests/human_ops_report.rs::shared_env_lock_fix_is_counted_once_with_correction_history`。実データでは main-3 の human-ops の写しに
   `correct --logical-op pilot/env-lock-regen --retract` と `record --scope shared` を追記 → A0/B0 の人の時間 71→0、shared_overhead 71 秒・補正履歴 4 件、元の行は byte 一致の prefix。
3. B event: `tests/human_ops_report.rs::b_waits_auto_retries_and_event_refs_join_from_export_rows`、`task-core` `human_waits_pair_question_and_approval_events_of_the_campaign_tree`。
   main-3 B は自動 retry 2 run（`is_retry`）、Question/Approval 0 件（event 観測あり）。A は event_waits/auto_retries が欠測として残る。
   未観測と観測済み 0 の区別（差し戻し対応）: `human_ops_report.rs` の `TrialEval` が op 記録数・attempt 記録数を持ち、op 記録の無い試行の
   `interventions`・`human_time`、attempt 記録の無い試行の `rework` は `source=missing`・値 null。arm 集計は `human_time_missing_trials`・
   `rework_missing_trials` に数えて秒数・件数に 0 を足さない。shared_overhead も op が無ければ null。
   試験 `tests/human_ops_report.rs::cutoff_or_job_only_records_leave_human_time_and_rework_unobserved`（cutoff+job だけの A0 は 3 項目とも欠測、
   protocol op だけの B0 は観測済みの 0）。定義書 §5、ADR D5 付記。
   main-3 は A0/B0 とも status_check・protocol の op 記録があるので観測済みのまま（新コードの再生成 `report-corrected-v3/` は arms・shared_overhead が前版と一致）。
4. filter/窓: `tests/human_ops_report.rs::protocol_attempts_and_out_of_window_ops_are_not_counted` と既存 fixture（cutoff 後の op を介入に数えない）。
5. CSV: `tests/success_contract.rs::csv_round_trips_json_cells_with_multiple_fault_keys`（comma・quote・改行・fault_breakdown 2 key、parser で列数と JSON 一致）。
6. cost scope: `export::COST_SCOPE`（CoS call・共有 overhead を含まない、list/cash は final-accounting の別欄）を table.json・report.json・定義書 §7 に記載。
7. 本ファイル。

## 実行したコマンドと結果
- `bash scripts/dev/test-affected.sh` → exit 0、6296 tests run: 6296 passed, 16 skipped（13 crate）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- attempt 2: 決定的 check の再実行で範囲外の `celeris daemon::classify_tick::e2e::unregistered_key_waits_notifies_then_resumes_once_and_classifies` が負荷下で落ちた（Jev request 総数 3、期待 1。単独では通る）。同じ入力は classifier の cache と `max_retries = 0` で再送されないので、総数が増えるのは別 subject の分類と判断し、試験の task の `instruction` を持つ request だけを数える `JevServer::count_for` に直した。直後に `bash scripts/dev/test-affected.sh` → exit 0、6296 passed / 16 skipped。clippy・fmt も exit 0。
- main-3 再集計（読み取りのみ。測定 root は書き換えていない）:
  `celeris-campaign check --manifest $M/out/campaign.json --out $M/out --measurement-id mlsys-main-3` → ok=true・complete=true、A (n1, passed1, final_passed0, grader_failed1)、B (n1, cancelled1)。
  `celeris-campaign export ... --dest <artifacts>/export --records $M/runs/exp-db/records.jsonl` → complete、table.csv 3 行とも 43 列。
  `celeris-campaign human-op report --file … --events $M/out --events $M/runs/exp-db --out …`。

- 第 3 回（`165d76e1`）: 偽 webhook が HTTP 要求前の EOF を空の通知として記録していたのを直した（受信記録を応答前に確定）。`cargo test -p celeris --test credential_wait_notify` → 2 passed。
  `bash scripts/dev/test-affected.sh` → exit 0、6297 passed / 16 skipped。clippy・fmt exit 0。
- 再提出（run 01M5M0TR319JS7HBDS6GTEN1H8）: コード変更なしで再検査、test-affected 6303 passed / 17 skipped、clippy・fmt exit 0（制御プレーンの再実行でも合格）。
- 今回（attempt 2 of 3 の差し戻し対応）: `cargo test -p celeris-campaign --test human_ops_report --test success_contract` → 5 passed + 2 passed。
  最終検査の結果は下の「最終検査」節。

## 保存した raw・grade・集計
- 入力（不変）: `/local/celeris/data/measure/main-3/out/{trials,grades,human-ops,ledger}.jsonl`、`runs/exp-db/records.jsonl`。sha256 は成果物の `inputs.sha256`。
- 再集計の出力: run の成果物 dir `/local/celeris/data/workspaces/01M5KVHDWYTACKJKBNKWQXGX0M/artifacts/main-3-reaggregate/`
  （`check.json`・`export/table.{json,csv}`・`export/final-accounting.json`・`report-original/`・`human-ops.corrected.jsonl`・`report-corrected/`・
  今回のコードで再生成した `report-corrected-v3/`）。前版は上書きしていない。
- 入力 6 file の sha256 は今回も `inputs.sha256` と全件一致（`sha256sum -c`）。訂正台帳は元 human-ops を byte 一致の prefix として保持、CSV 全 3 行 43 列。

## 未解決
- `--measurement-id` を付けない check は、現行の期待 lock の measurement id（`mlsys-main-3-043d5f52e4da`）と保存 lock（`mlsys-main-3`）の命名差で lock 不信用になり採点を外す。旧 raw の再集計では id を明示する。
- export の `wait` 行は Question/Approval だけ。`BrowserWaitOpened`・blocked 区間・`PlanApprovalRequested` は未導出（定義書 §4 に明記）。
- 実 daemon の campaign export に `wait` 行が出ることは store の単体試験と fixture までで、本物の B 試行の export では未確認（main-3 は wait 0 件）。
- タスク 1（K4 採点/入力 archive）は main に未反映のため、新 K4 の採点→集計一致は未確認。

## 次の測定の開始可否
- 集計側（成功契約・human-op・CSV）は次の測定に使える。K4 pilot の開始はタスク 1 の grader/archive が main に入ってから、同じ out dir で checker と report の一致を確かめた上で判断する（現時点では開始不可: タスク 1 未完）。

## 本番への移行
- config・DB の変更なし。human-op report の schema が `/2` に変わるので、既存の report は生記録から再生成する。

## 提案
- `success::load` を `celeris-campaign check` の `--measurement-id` 省略時にも使えるよう、保存 lock の measurement id を期待 lock 側で受ける既定を検討する。
