---
title: P1-G 自律復帰とイベントの正本
tasks: [01M6N7XY9W412TA8G2WV0726Z0]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P1-G 自律復帰とイベントの正本

controller が `blocked` で止まった案件を、止まった原因が解けた根拠があるときだけ自動で 1 回再評価するようにした。人の pause・cancel、未回答の `human_required` の問い、危険な予算超過は自動で解かない。人の回答待ちの毎時の再評価をなくし（24 時間で 24 回 → 1 回）、案件 CoS の chat run の token と起動数を予算と計測に入れた。

## main と既存実装の照合
- 基準は `git merge-base HEAD main`。A1〜A9・P0-A の K29〜K40・A9 findings・arrival・`docs/ops/project-controller.md` を確認した。
- 既にあったもの（再実装しない）: 許可の作成と同じ transaction の `authority_unblock_tx`（`pc/authority-required` だけ再開）、答えのある問いを聞き直さない `authority_guard`、event の wake の冪等（`UNIQUE(project_id, wake_key)` と案件別 cursor）、効果の門の `human_epoch` 照合、A6 の guard。
- 足りなかったもの: brief の保存口の transaction と wake（phase 1 で対応）、chat run の予算（phase 1）、`blocked` からの自動の再開の分類、日次予算の期限、人の待ちの毎時の評価（TS 6 時間・答えのある問い 1 時間・cron 毎時）、却下・取り消しの wake、CoS の brief で TTL が延びる経路（前回の review の懸念）。
- 旧 milestones API（410）、研究 K2/K4・IEEE Access の凍結評価、本番の config・DB・daemon には触れていない。

## 変更（commit 件名）
- `phase 1: make project brief wakes atomic and account for CoS chat usage`
- `phase 2: resume blocked project controllers on resolved causes and cut human-wait re-evaluation`
- `phase 2: pin the pilot cost cap test to a dangerous overrun`
- `delivery-repair: wait for dispatched runs in the script compare test helper`（配送の release 準備の失敗の修正。下の「配送の修正」）

中身（決定は [ADR 2026-11-05-phase3-p1g](../../adr/2026-11-05-phase3-p1g.md) の 1〜10）:
- `crates/task-dispatch/src/dispatcher/project_resume.rs`（新規）: 最後の判断の行から止まった原因を分類し、解消の根拠があれば 1 つの transaction の CAS で再開する。判断の行（`pc/resume/<原因>`、`celeris.project-resume/1`、`basis`）に根拠を残す。
  - 問い（TS の escalate）: 問いの key の許可・提案の却下・（objective_change は）依頼文の変更。
  - 同じ失敗: 人の操作か、失敗した root 自身の完了。
  - TTL: 外からの入力（cron・停滞・CoS の `brief:`・daemon の `authority-denied:` は数えない）。
  - 日次予算: 次の UTC 00:00 の期限、その日の使用量の数え直し、`Dangerous`（token 2 倍以上など）は戻さない、直近 7 日 3 回まで、起票の予算は有効な許可が要る。戻さないときは `budget_hold` の notice を 1 回。
  - 検証落ち・判定／効果の失敗・分類できない止まりは人だけ。
- `project_guard.rs`: 予算の guard が期限を書く。同じ失敗の問いの key に解消の時刻を入れる（解消の後の再発で新しい問いを 1 回）。TTL の入力から brief を外す。
- `project_wake.rs`: tick の段に再開を入れる（`Boundary::AfterResume`）。guard の問いの notice が既にあるときは問いを増やさずに `blocked` に揃える。止まりの notice の末尾を原因ごとに書く。`blocked` と人だけの待ちの案件に cron の照合の wake を積まない。
- `project_decider_ts.rs`: 答えのある問いの待ちを 1 日に。断られた起票の許可の問いは聞き直さず root も作らない。
- `control-ts/src/domains/project_controller.ts`: `humanWaitSecs` を 1 日、今日の CoS 入力の上限の待ちは `dailyLimitRecheckSecs`（6 時間、従来の値）。
- task-core: `project_controller_resume`・`project_controller_budget_hold`・`project_run_usage_between`・`project_chat_starts_between`・`is_external_input`、TTL の SQL から brief を外す。`ProjectControllerView.budget_today`。
- task-ops `project_budget`: `usage_on_day`・`severity`・`secs_until_next_day`。
- task-api: 人の却下と人の取り消しで同じ transaction に `human_epoch` と wake。`GET …/controller` の `budget_today`。
- web: 案件の画面に今日の使用量（token・CoS の起動・CoS への依頼・起票）、blocked の案内の文。
- 試験の台本: `scripts/dev/test-project-recovery.sh`。文書: ADR、`docs/ops/project-controller.md`（「予算は日付が変わっても自動では戻らない」を新しい規則に置き換えた）、`docs/architecture-map.md`。
- migration は不要（予約 0116 は未使用）。

## 受け入れ条件と証拠
すべて提出時の HEAD で実行。raw は `p1g/run/`（`recovery.jsonl` 31 行、`human_wait_measure.jsonl`、`summary.json`、`nextest.log`）と `p1g/human_wait_before.*`。

| 条件 | 試験（K） | 結果 |
|---|---|---|
| 0: restart/crash | `project_recovery_k29_brief_wake_survives_crash_at_each_stage`（8 境界）、`..._k29_resume_survives_crash_right_after_its_transaction` | 各境界で brief の wake 1 件・root 1 件・未処理 0。再開の行 1 件、再起動後の評価 1 回 |
| 0: 同じ event 2 回 | `project_recovery_k30_same_event_twice_is_one_wake_and_one_evaluation` | wake と評価は増えない。同じ内容の brief の 3 日後の再送も wake 0 |
| 0: 3 日の時計飛び | `project_recovery_k31_three_day_clock_jump_folds_into_one_evaluation` | root の終わり 5・brief 5・外の応答 5 と 72 時間の cron が評価 1 回に畳まれる。問い 0・root 0・TTL の止まり 0 |
| 0: 同じ失敗 | `project_recovery_k32_same_failure_stops_again_after_human_clear`、`..._k32_failed_root_completing_clears_the_block` | 止まっている間は解けない、問い 1。人の解消で 1 回再評価、再発で新しい問いを 1 回（計 2）、root 0。失敗した root 自身の完了でも再開、別の root の完了では再開しない |
| 0: brief/authority 同時編集 | `project_recovery_k33_concurrent_brief_and_authority_edits_invalidate_the_decision`（commit の前・後） | 古い判断の root 0、編集ごとに wake 1、次の評価は最新の `human_epoch` を読み root 1 |
| 0: clear と deny | `project_recovery_k34_answer_clears_once_and_records_the_basis`、`..._k34_request_change_clears_objective_question`、`..._k35_deny_has_no_effect_and_is_not_asked_again`、task-api `project_recovery_human_answers_write_wake_with_the_edit` | 許可で 1 回再評価し根拠（grant id）が行に残る。却下は効果 0・問いは増えない・root 0。却下・人の取り消しは同じ tx で wake、CoS の取り消しは数えない |
| 0: 7 日以上の virtual clock | `project_recovery_k36_eight_day_composite_keeps_the_limits` | 8 日（192 時間）で評価 13、wake 16、cron の wake 5、問い 3、root 0、CoS 入力 0、再開 3（同じ失敗・問い・予算） |
| 1: pause/cancel | `project_recovery_k37_pause_and_cancel_are_not_lifted_by_time_or_answers` | 8 日・brief・外の応答があっても phase は paused/cancelled、評価 0、root 0 |
| 1: 未解決 human_required | `project_recovery_k37_unresolved_human_required_stays_blocked`（explicit_human・objective_change・standing_permission） | 8 日・外の応答・brief・root の完了・別の key の許可で解けない。評価 0、問い 1 |
| 1: 危険な予算超過 | `project_recovery_k37_dangerous_budget_overrun_stays_blocked`、`long_horizon_pilot_cost_cap_stops_until_human_resets` | 期限なし、8 日で評価 0。止めた後に危険になった場合も翌日に戻さず `budget_hold` の notice 1 |
| 2: human wait の削減 | `project_recovery_k38_human_wait_evaluations_in_24_hours`（同じ file を基準の tree でも実行） | 24 時間の評価: 本物の TS の開いた問い 24 → 1、答えのある問い 24 → 1 |
| 2: CoS chat run の予算と計測 | `project_recovery_k39_chat_tokens_stop_the_guard_and_are_measured`、`p1g_chat_tokens_and_starts_are_project_scoped_and_daily`、task-api の `budget_today` | chat run の token で guard が止まる（判定 0、問い 1）。`chat_starts` が guard の記録と `GET …/controller` に出る |
| 2（日次予算の期限） | `project_recovery_k40_*` 4 件 | 期限は次の UTC 00:00、当日は戻らず翌日に評価 1 回。許可なしの起票の予算は戻さず root は増えない。自動の再開は 7 日で 3 回まで。同じ日の置き直しでは問いを増やさずに止まり直す |
| 前回の懸念 | `project_recovery_cos_brief_does_not_extend_the_human_wait_ttl` | CoS の要約を 12 時間ごとに更新しても TTL で 1 回止まる。外の応答で再開 |
| 3 | `test -f agent-docs/progress/2026-11-05-phase3/p1g.md` | exit 0 |

## 実行したコマンド（提出時の HEAD）
- `bash scripts/dev/test-project-recovery.sh --out agent-docs/progress/2026-11-05-phase3/p1g/run` → exit 0、27 tests passed、K38 before 24 / after 1（2 件とも）。
- K38 の修正前: 基準の commit の一時 worktree に `project_human_wait_measure.rs` だけを足し、その tree の control-ts で作った bundle で同じ試験を実行 → exit 100（期待どおり assert で失敗）、raw は評価 24 回（`p1g/human_wait_before.jsonl`・`.nextest.log`）。
- `bash scripts/dev/test-affected.sh`（`pnpm_config_store_dir="$PWD/.pnpm-store"`、`CELERIS_SCRIPT_RUNNER_CACHE="$CARGO_TARGET_DIR/p1g-runner-cache"`）→ exit 0。Rust 7655 passed・21 skipped、web 709 passed、gateway 131 passed、test-ts-verify 20 passed、bundle SKIP 0、doc test と文書検査も合格。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-project-controller-replay.sh` → exit 0（18 replay 行・4 resource 行）。
- `bash scripts/dev/test-long-horizon-bench.sh` → exit 0（v2 raw の照合を含む）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。`bash scripts/dev/check-doc-links.sh` → exit 0。

## 失敗ケースと対処
- 基準の tree を同じ `CARGO_TARGET_DIR` で build した後、cargo が基準の rlib を新しいものとして使い、`test-long-horizon-bench.sh` が基準の path の fixture を探して失敗した（exit 1）。変更した source を touch して作り直し、再実行で合格。修正前の測定を同じ target で行うときは、戻った後に変更 file を touch する。
- 最初の test-affected（phase 2 の最初の commit）で `long_horizon_pilot_cost_cap_stops_until_human_resets` が 1 件失敗（旧い「翌日も戻らない」を固定していた）。超過を上限の 2 倍以上（危険な超過）に変え、元の意図（人の置き直しまで止まる）を保った。
- 新しい試験の作成中の失敗: crash の後に同じ dispatcher を使い続けた（panic で controller の実行時の状態が戻らない。再起動として作り直した）、task の状態を `json` 列なしで書き換えた、K36 の上限の数え方。いずれも試験側の誤りで直した。

## 配送の修正（delivery-repair、2026-11-05）
- 失敗: release 準備の `script-bundle-tests` の段で `celeris::script_compare_route_priority` の `script_compare_route_priority_dispatch_match_rollback_and_replay` が 1 件失敗した（`86 tests run: 85 passed, 1 failed`、`kinds = ["harness_selected"]`）。同じ準備の `cargo-test` の段では合格していた。
- 原因: 試験の helper `Env::dispatch`（`crates/celeris/tests/support/script_compare.rs`）は compare だけを待ち、run（fake harness の `sh`）の終わりを待たなかった。負荷で前の run が残ると、次の task の route が account の同時実行の上限（`max_runs_per_account` の既定 2）で `AccountConcurrency` として断られ、`llm_route_selected` が出ない。P1-G の変更とは関係ない時間依存の不具合。
- 再現: helper の fake harness の先頭に `sleep 3` を足すと、同じ試験が決定的に失敗した（`route refused: … rejections: [{reason: AccountConcurrency}]`）。
- 修正: `Env::dispatch` が compare の後に `wait_runs_finished` で dispatcher の `in_flight()` が 0 になるまで tick で完了を取り込む（出来事待ち、上限 120 秒は保険）。`sleep 3` の harness でも合格（`1 test run: 1 passed`）。`sleep` は戻した。
- release gate と同じ段: `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_BUNDLE_DIR=$TMPDIR/bundle bash scripts/selfdeploy/script-bundle-gate.sh test $PWD $TMPDIR/bundle` → exit 0、`86 tests run: 86 passed, 0 skipped`、`test ok (SKIP 0)`。
- 本番の release 準備の再実行は配送側が行う（この run ではしていない）。

## 未解決
- 実 LLM の確認と 24〜72 時間の実稼働は未実施（人の gate。A9 の手順）。
- `pc/no-progress` の止まりは問いの回答でだけ解ける。進捗の増加での自動の再開は入れていない。
- 案件の一時停止 → 再開で `blocked` が `active` に戻る既存の挙動は変えていない（人の操作として扱う）。
- 「TS policy だけの変更は verify で cargo 0、p50<=30 秒・p95<=60 秒」は、この task が Rust を変えるため測定の対象外。TS の定数の変更だけを切り出した verify の測定はしていない。

## 次の gate
- 本番への promote と config の変更は人が行う。config の欄の追加・削除は無いので、本番 config の書き換えは不要。
- 実 LLM の pilot（外部 operator）で、予算の翌日の再評価と問いへの回答からの再開が通知 1 件のまま動くかを確かめる。

## 提案
- `blocked` の理由と次の自動の再評価の条件を `GET …/controller` に構造化して出す（今は判断の記録と notice の文だけ）。

## 本番への移行
- migration・config の変更は無い。release の promote で有効になる。既存の `blocked` の案件は、最後の判断が予算の guard で `next_wake_at` が無い（この変更の前に止まった）ものは自動では戻らない。問いの key に許可がある・却下済みの案件は、promote の後の最初の tick で 1 回再評価される。
