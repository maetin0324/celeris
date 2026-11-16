---
title: "健全性v2 R4 settle-task: settle に atomic task の continuation と段の進み方の判断を足し、worker_finish.rs を適用だけにする"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 settle-task

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D3 の残り（atomic task の run と、WU の書き込みの後の段の進み方）を
  `crates/task-dispatch/src/run_settlement/task_run.rs` に置いた（`run_settlement.rs` から `pub(crate) use` で出す）。
- `settle_task_run(TaskRunInput) -> TaskRunSettlement` が決めるもの（旧 `finish_worker_result_with` の atomic の分岐の判断）:
  - クラスタ job の wait と子 task 待ち（checkpoint を残し、continuation・進捗なしに数えない。子待ちは `Continue{children_wait}`）。
  - `[execution] continuation = false` の run（`WorkerError{retryable:true}` に戻し、checkpoint を残さない）。
  - 進捗の比較・進捗なしの連続・continuation の回数と固い上限、質問の文（`execution_scheduler::no_progress_question`・
    `continuation_cap_question` と `ContinuationFacts` を呼ぶ。写していない）、`Continue{continue}` の文（Run #n）。
- `task_checkpoint_end`: 子待ちの checkpoint を `Waiting` にする（checkpoint の組み立ての前に要るので別の関数）。
- `task_reads_worker_decisions`・`hold_task_for_worker_decisions`: atomic task の run が worker の `decisions` を読む条件と、
  `needed_before: self` の保留の trigger と文（WU の `reads_worker_decisions`・`hold_for_worker_decisions` と同じ形）。
- `phase_step_after_write(units, unit_id, trigger) -> PhaseStep`: D3 の「WU の書き込みの後のもう 1 回の純粋関数」。
  `execution_scheduler::settle_phase` を呼び、task を遷移させないか（`stays_running`）・始める統合（`integration`）・質問を捨てるか・
  trigger の置き換え（advance・all done）・兄弟が走っている間に止まった別の unit（`deferred_unit`）を返す。
- `worker_finish.rs` に残したもの（入力を集めることと適用。順と transaction の範囲は変えていない）: events と委譲した子
  （`awaited_children`）の読み出し、checkpoint の組み立て（git・`checkpoint.json`・`yield`）、worker の決定の要求の読み出し、
  WU の行の読み直し（`work_units_for`）と遅れた unit の trigger（`deferred_work_unit_trigger`。events の追記を伴う）、統合の開始と
  task を遷移させないときの events の追記。
- `worker_finish.rs` から `execution_scheduler::no_progress_question`・`continuation_cap_question`・`ContinuationFacts`・`settle_phase`・
  `PhaseSettle` の参照を消した（`decide` は settle-wu で消えている）。残る `execution_scheduler::` の参照は WU の入力の型
  `ContinuationInputs` の組み立てだけ。
- WU と atomic task で同じだった tool の活動の抜き出しと worker の checkpoint の読み出しを、`worker_finish.rs` の free 関数
  `tool_activity`・`worker_checkpoint_input` の 1 つずつにした（`Dispatcher` の method ではない）。
- `finish_worker_result_with` は 926 行から 772 行になった（check の awk で数えた値）。
- ADR-0082 に付記（run_settlement の置き場・入口の順・`worker_finish` に残したもの）を足し、`docs/architecture-map.md` の
  「worker 起動・完了処理」と「run の終わりの判断」の行、`dispatcher.rs` の module map の `worker_finish`・`run_settlement` の行（同じ 1 行の中）を直した。

## 実装前 brief

1. 根本原因: atomic task の continuation（子待ち・進捗なし・固い上限）と段の進み方の判断が `finish_worker_result_with`（`Dispatcher`
   の method）の中で store・git の読み出しと副作用に混ざり、入力から提案を単体で試せない。worker 完了の経路が `execution_scheduler`
   の質問と段の関数を直接呼んでいた。
2. 責務 owner: 判断は `run_settlement`（D3 の表）。質問の文と段の状態の正本は `execution_scheduler`。適用は `worker_finish.rs`。
3. 既存の再利用先: `execution_scheduler::{no_progress_question, continuation_cap_question, ContinuationFacts, settle_phase, PhaseSettle}`、
   `task_ops::derive::{consecutive_continuations, latest_progress_checkpoint, no_progress_streak, progress_checkpoints_since_answer}`、
   `task_core::checkpoint_shows_progress`、`run_settlement::describe_run_end`。
4. 選択肢: (a) `settle` 1 つに atomic task の入力も入れる（checkpoint の組み立てと子の読み出しが `settle` の `run_end` で決まるので、
   先に読めない）、(b) settle-wu と同じく同じ module に atomic task の段の関数を置き、読み出しは適用側が提案の欄を見て行う。
   (b) を採った。段の進み方は WU の行を書いた後でないと決まらないので、D3 のとおり別の純粋関数にした。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `worker_finish.rs` の atomic の分岐の判断の本体（進捗の比較・上限の比較・質問の文の組み立て・子待ちと無効時の trigger）、
   atomic の決定の要求の保留の文、`settle_phase` の match と `PhaseSettle` の参照、WU と atomic で重複した活動の抜き出しと worker の
   checkpoint の読み出し。
7. 不変条件: 既存の worker 完了の試験と cex_ の試験が同じ assertion で通る（test-affected）。新しい関数の境界は下の試験。

## 読み出しと書き込みの順

- 変えていない。atomic の run の events・runs の番号・委譲した子・checkpoint の組み立ては今と同じ条件と順で行い、continuation の回数と
  直前の checkpoint は同じ events から `settle_task_run` の中で数える（旧も `[execution] continuation` が有効なときだけ数えた。新も同じ）。
- WU の行の読み直しと `deferred_work_unit_trigger` は、今と同じく runs 索引の終了の書き込みの後、token 予算の判定の前に行う。

## 足した試験（`crates/task-dispatch/src/run_settlement/task_run/tests.rs`）

守る不変条件と既存の試験で検出できない理由は各試験の doc comment に書いた。

- `settle_task_run_waits_for_children_and_cluster_jobs_without_counting`
- `settle_task_run_disabled_continuation_retries_without_a_checkpoint`
- `settle_task_run_asks_on_no_progress_before_the_hard_limit`
- `worker_decisions_hold_an_atomic_run_only_when_done`
- `phase_step_after_write_keeps_the_task_running_or_replaces_the_trigger`

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh`: exit 0（約 710 秒）。3897 tests run: 3897 passed（9 slow）、11 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0。verdict=pass、fail_grade=0、worsened=0、new=1（settle-end・settle-wu と同じ件数。中身は未確認）。
- `cargo nextest run -p task-dispatch -E 'test(run_settlement) | test(cex_) | test(structure_guard_)'`: exit 0、58 passed（settle-wu の 53 に
  この unit の 5 本）。
- cex_ の試験と ADR 2026-11-11 は変えていない（`git diff --quiet "$CELERIS_WU_BASE" -- …` は exit 0）。
- 計画の check（`worker_finish.rs` が `execution_scheduler::(decide|no_progress_question|continuation_cap_question|settle_phase)` を
  呼ばない・run_settlement が質問の関数を呼ぶ・`finish_worker_result_with` が 772 行・ADR-0082 と地図が run_settlement を指す）: exit 0。
  純粋さの grep と名前に decide を含む関数の grep: exit 0。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` に計画の正規表現を当てる）: exit 0（変更 path 8 件、範囲外 0）。
  `sh scripts/dev/progress-index.sh --check`: exit 0。
- `dispatcher/` の行: `worker_finish.rs` は 205 行減・88 行増、`dispatcher.rs` は module map の 2 行を同じ行の中で直した。`Dispatcher` の
  method は足していない。

## 未解決事項

- 無し。

## 提案

- settle-record: `questions_of_run`・`stop_at_token_budget`・`phase_step_after_write` は、どれも WU と atomic task の分岐の後の trigger を
  見る。統合後に、この 3 つを 1 つの「書き込みの後の段」の提案にまとめるかを見る（今は読み出しの位置が 3 つで違うので分けた）。
- run の metrics（`RunMetrics`）は時計と知識の使用の file から値を集めるだけで判断が無いので、`worker_finish.rs` に残した。
