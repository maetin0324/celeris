---
title: 健全性v2 G3 limits fix-wu-cap — WU の check の health だけの不合格が retry の上限を使い切ったら replan せず Blocked で止める
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 limits fix-wu-cap — WU の check の health だけの不合格が retry の上限を使い切ったら replan せず Blocked で止める

WorkUnit `fix-wu-cap`（task 01M70C8NHEANARRKZD9RGKZ87D）。final review の advisory (b)「WU の check の health だけの不合格で指摘が毎回変わり `max_retries` を使い切ると、worker_finish.rs の既存の自動 replan に入る。段の統合とは止まり方がそろっていない」を直す。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D4.4（health のために repair・replan を新しく足さない）。基準は `$CELERIS_WU_BASE`。

結論: 完了。WU の check の不合格が health だけで、WU の retry の上限（`task.budget.max_retries`）を使い切ったら、run の結果を質問 `health_repair_limit: WU <key> の check: …` にすり替え、WU と task を Blocked にする。WU を Failed にせず、自動 replan（計画の版の追加・planner の run）に入らない。retry の余地の判定は `execution_scheduler::retries_remain` の 1 か所で、`fn failed` と `settle_health_row` が共に呼ぶ。health でない失敗が混ざる不合格、上限の内の不合格、往復、Error の扱いは変えていない。新しい config・予算の欄は足していない。`worker_finish.rs`・`dispatcher.rs` は変えていない。`work_units.rs`（2181 行）は doc comment の 2 行を書き換えただけで、行数は増えていない。

## 実装前 brief
1. 根本原因: WU の check の health の行を扱う `Dispatcher::settle_health_row`（`health_review/check_item.rs`）は、Error（D3.4）と同じ指摘の往復（D4.3）だけを質問にすり替えていた。指摘が毎回変わる health だけの不合格は `Terminal::Error{retryable:true}` のまま `execution_scheduler::decide` → `failed` に渡り、retry の上限を使い切ると WU が Failed になる。すると `worker_finish.rs` の既存の自動 replan（`AutoReplanRequested`）に入る。段の統合は同じ使い切りを `phase_health_stop` が `health_repair_limit:` の質問で止めるので、2 つの経路の止まり方が違っていた。
2. 責務 owner: WU の check の health の行を run の結果に写す所は `check_item::settle_health_row`（Error と往復を既にここで質問にしている）。WU の retry の上限の判定は `execution_scheduler`（`fn failed`）。
3. 既存の再利用先: `execution_scheduler` の `wu.retries < limits.max_retries`（述語に出して共有した）、`check_item::HEALTH_REPAIR_LIMIT`・`stops_without_replan`、`repair_limit_question` の文（共通の `limit_question` に出した）、`only_health_fails`、`HealthRepeatSite::label`、既存の試験の土台（`work_unit_task`・`parallel_dispatcher`・`health_items`・`run_until_idle`）。
4. 選択肢: (a) `settle_health_row` で、health だけの不合格が `!retries_remain(wu, task.budget.max_retries)` なら質問にする（採用。Error・往復を質問にする既存の口と同じ所で、`worker_finish.rs`・`dispatcher.rs` を変えずに済む）。(b) `worker_finish.rs` の replan の分岐で health だけの失敗を見分ける（2000 行を超える file を増やし、check の結果の分類を 2 か所に持つので採らない）。(c) `execution_scheduler::failed` に health の分岐を足す（純粋関数の共通の口に health だけの分岐を足すことになる。規約 3）。上限の判定は `retries_remain` を呼び、`check_item.rs` に比較を書き写さない。上限の値は `worker_finish.rs` が `WuLimits.max_retries` に渡すのと同じ `task.budget.max_retries`（同じ run の `task` の行）。
5. 新しい状態・分岐・fallback・port: 状態・fallback・port は無し。分岐は `settle_health_row` の health だけの不合格の枝に 1 つ（往復でなく retry の余地が無いとき）。質問の先頭の語は既存の `health_repair_limit` で、`stops_without_replan` が既に知る語。
6. 撤去対象: WU の check の health だけの不合格が retry の使い切りで WU の Failed → 自動 replan に入る経路（この変更で通らなくなる）。設定・API・試験で消すものは無し（既存の試験はすべて残り、合格する）。
7. 不変条件: WU の check の health だけの不合格は、retry の上限を使い切っても replan しない（計画の版が増えない・planner の run が起きない）。WU と task は Blocked で、質問の先頭は `health_repair_limit:`。health でない失敗が混ざる不合格は、使い切りの後も今どおり WU の Failed → 自動 replan に進む。retry の上限の判定は `execution_scheduler::retries_remain` の 1 か所。試験は下の「試験」。

## 変更
- `crates/task-dispatch/src/execution_scheduler.rs`: `pub(crate) fn retries_remain(wu: &WorkUnitRow, max_retries: u32) -> bool`（`wu.retries < max_retries`）を足し、`fn failed` は `retryable && retries_remain(wu, limits.max_retries)` で呼ぶ（挙動は変えない）。
- `crates/task-dispatch/src/health_review/check_item.rs`:
  - `settle_health_row`: 不合格が health だけのとき、まず `health_repeat_stop`（往復）を見る。往復でなく `!crate::execution_scheduler::retries_remain(wu, task.budget.max_retries)` なら、`retry_limit_question` の質問を run の結果にし、検査の記録を空にする（Error・往復と同じすり替え）。
  - 使い切りの質問の文を `limit_question(site, used, summary)` の 1 か所にした。段の統合と review の `repair_limit_question` と、WU の `retry_limit_question` は数え（`used`）だけを作って呼ぶ。WU の質問は `health_repair_limit: WU a の check: health の修復が上限に達しました（WU の retry 2/2 回）。repair も replan もせずに止めました。直し方を comment で書くと再開します。指摘: health:deps.violation:…（evidence: …）`。段の統合と review の文は変えていない。
  - module の doc と `settle_health_row` の doc に、WU の retry の使い切りの止まりを書いた。
- `crates/task-dispatch/src/dispatcher/work_units.rs`: `on_work_unit_checks_finished` の doc comment に retry の使い切り（D4.4）を足した（同じ 2 行の書き換え）。

## 試験
`crates/task-dispatch/src/dispatcher/tests/health_limits_integration.rs` に 2 件足した。試験の repo の base に commit した `health_enforce` の stub の検査器を使い、外部ネットワークに出ず LLM を呼ばない。時刻に依らず、dispatcher が静まるまで tick を回す出来事待ち（`run_until_idle`）で決まる。

- `health_limits_work_unit_retry_cap_blocks_without_replan`: 守る不変条件は「WU の check の health だけの不合格は、指摘が毎回変わって retry の上限（`max_retries` = 2）を使い切っても replan せず、WU と task が Blocked になる」。adapter が run ごとに `FP` を 1, 2, 3 にし、指摘の fingerprint が毎回変わる。確かめること: health の評価が 3 回（`deps.violation:…1`・`…2`・`…3`）で終わる、`WorkUnitChecksFailed`（retry）が 2 件、WU の `retries` が 2、WU が Blocked（reason Question）、task が Blocked、質問が 1 件で先頭が `health_repair_limit: WU a の check: `、`WU の retry 2/2 回` と 3 回目の指摘を含む、`HealthFindingsRepeated` が無い、`AutoReplanRequested` が無い、`ExecutionPlanned` の数が増えない、planner の run（`WorkerStarted` の role Planner）が 0、有効な計画の版が 1。既存の `health_limits_work_unit_different_findings_still_retry` は retry が続くことと往復で止めないことしか見ず、使い切りの後の行き先（replan か Blocked か）を検出できない。`health_limits_work_unit_same_fingerprint_blocks` は上限の内の往復の経路しか通らない。直す前のコード（`settle_health_row` の新しい枝を `false &&` で外した状態）では、この試験が WU a の状態の assert（Blocked でない）で落ちることを確かめた（他の 4 件の WU の試験は合格のまま）。
- `health_limits_work_unit_mixed_failure_cap_keeps_replan`: 守る不変条件は「health でない検査の失敗（`false`）が混ざる不合格は、retry の上限を使い切った後も `health_repair_limit:` で止めず、今どおり WU の Failed → 既存の自動 replan に進む」。確かめること: WU a の `WorkUnitChecksFailed` が 2 件を超える（retry 2 回の後の 3 回目の失敗）、`AutoReplanRequested { key: "a", source: WorkUnit }` がある、events に `health_repair_limit:`・`health_findings_repeated:` が無い、`HealthFindingsRepeated` が無い。既存の `health_limits_work_unit_mixed_failure_keeps_retry` は上限の内の retry しか見ず、使い切りの後の行き先を検出できない。

既存の試験は消していない・弱めていない。

## 変更（commit の件名）
- G3 limits fix-wu-cap: WU の check の health だけの不合格が retry の上限を使い切ったら replan せず Blocked で止める
- G3 limits fix-wu-cap: 進捗に提出時の検査の結果を書く

## 証拠（提出時の HEAD で実行。最後の同期の後に再実行する）
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更は上の「変更」「試験」の 4 file とこの記録だけ。計画の範囲の check（正規表現）は exit 0
- `grep -qE 'pub\(crate\) fn retries_remain' crates/task-dispatch/src/execution_scheduler.rs && grep -qE 'retries_remain\(' crates/task-dispatch/src/health_review/check_item.rs && ! grep -nE 'retries *(<|>=?) *[A-Za-z_.]*max_retries' crates/task-dispatch/src/health_review/check_item.rs` → exit 0
- `cargo nextest run -p task-core -p task-ops -p task-dispatch --no-tests=fail -E 'test(/health_limits_/) | test(/health_enforce_/)'` → exit 0、90 passed
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-affected.sh`（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）→ exit 0。nextest 3687 passed・11 skipped、check-health は `repo_health_gate: all checks passed`
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/progress-index.sh --check` → いずれも exit 0

## 未解決
- integration repair の WU（`task_core::is_integration_repair_unit`）の WU の check で health だけの不合格が retry を使い切った場合も、この直しで Blocked（`health_repair_limit:`）になる。前は WU の Failed から `integration_repair_exhaustion` に入っていた。WU の check の往復（D4.3）も前から同じ所で質問にしていたので扱いはそろっているが、この組み合わせを通す試験は無い。
- ADR 2026-11-08 の limits の付記と `limits.md` は、WU の check の使い切りの止まり（`health_repair_limit: WU <key> の check:`）をまだ書いていない。範囲外（ADR と `limits.md` はこの WU の範囲に無い）なので、reclose が改める。

## 提案
- reclose で、ADR 2026-11-08 の limits の付記の (c)（使い切り）に「WU の check は WU の retry の上限（`max_retries`、判定は `execution_scheduler::retries_remain`）を使い切ったら `health_repair_limit: WU <key> の check:` の質問で Blocked。replan しない」を足し、`limits.md` の未解決から advisory (b) を消す。
