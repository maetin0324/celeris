---
status: accepted
tasks: [01M7FYEHKMY34MM5Z75W2ESSGN]
---
# 研究 harness の締め切りの分類と、checkpoint から続けられない adapter を continuation から外す

- 日付: 2026-11-15
- 状態: D2・D3・D6 は実装済み（unit adr、task 01M7FYEHKMY34MM5Z75W2ESSGN）。D1・D4 は unit adapters、D5 は unit net が実装する。記録は [research の進捗](../progress/2026-11-15-health-v2-r6/research.md)
- 関係: [ADR 2026-11-10 adapter の結果契約](2026-11-10-adapter-result-contract.md) の (5)・D1・D2、[R1 の進捗](../progress/2026-11-10-health-v2-r1.md) の提案 2・3、ADR-0072 D7・D9・§6 (f)（予算切れ・yield・continuation の無効化）、ADR-0054 D1（継続 session の `sessions::SUPPORTED_ADAPTERS`）、ADR-0079（auto-leaf）、ADR-0027 D3・ADR-0029・ADR-0047 D4（研究 harness）、[ADR 2026-11-06 誤配置した report の回収](2026-11-06-misplaced-report-recovery-generalized.md)、[ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)

## 背景

研究 harness の 3 adapter（`crates/task-worker/src/{langmem,local_deep_research,paperqa}.rs`）は、監視 loop で wall clock の超過を `Terminal::Error{retryable: true, "wall clock exceeded"}` にしている。5 adapter（claude_code・codex・acp・pi・aider）は `adapter::deadline_terminal` で `BudgetExhausted{WallClock}` を返す。

dispatcher の `run_settlement::settle_terminal` は、`Error{retryable: true}` の文を `classify_budget_kind_from_text` で読み、「wall clock」を含めば `RunEnd::BudgetExhausted{WallClock}` にする（文言の安全網、ADR-0072 D7）。予算切れは continuation に回る終わり方（`RunEnd::is_continuable`）なので、`[execution] continuation` が有効なら、atomic task は `run_settlement::settle_task_run` が `Trigger::Continue{why: Continue}` にし、WU は `execution_scheduler::decide` が `needs_continuation` にする。どちらも checkpoint を合成して `CheckpointSaved` に残す。

研究 harness は checkpoint を読まない。langmem は `objective` だけを runner に渡し、local-deep-research は目的文・不合格の理由・前回の report だけを問いにし、paperqa は前置き（`preamble::render`）を問いに入れるが continuation の節（`preamble::continuation_section`）は描かず、取得・索引・回答を毎回最初から行う。このため「continuation」は実際には最初からのやり直しで、attempts を消費せず、進捗なしの上限（`no_progress_limit`）か固い上限（`continuation_hard_limit`）まで繰り返された。

## 決定

### D1. wall clock の超過は `BudgetExhausted{WallClock}` に揃える（unit adapters）

3 adapter の監視 loop を `adapter::deadline_terminal` に置き換える。wall clock の超過は `BudgetExhausted{WallClock, "wall clock exceeded", usage: None}`、idle の超過は `Error{retryable: true, "idle timeout"}`（5 adapter と同じ）。langmem は `[adapters.langmem].idle_timeout_secs` を `limits.idle_timeout` で上限を切った値として `deadline_terminal` の `idle_timeout` に渡す（ADR-0047 D4 の挙動を保つ）。paperqa は取得と回答の 2 段で同じ `start` を使う（壁時計は run 全体）。

理由: wall clock の超過は予算切れで、失敗ではない。締め切りの規則を 1 か所（`deadline_terminal`）にし、終わり方を構造化して返せば、dispatcher の文言の安全網に頼らずに済む（D5）。`WorkerFinished.end` は今も安全網で `BudgetExhausted{WallClock}` になっているので変わらない。変わるのは `runs/<run_id>/result.json` の 1 行（`error` → `budget_exhausted`）と、settle が作る outcome の文の元。D2・D3 の後は outcome の文は continuation の使えない枝の文になる。

### D2. checkpoint から続けられるかを adapter の性質 `WorkerAdapter::continues_from_checkpoint` で表す（実装済み）

`crates/task-worker/src/adapter.rs` の `WorkerAdapter` に `fn continues_from_checkpoint(&self) -> bool` を足した。性質の正本はこの method だけで、adapter の名前の一覧は新しく持たない。既定は `true`。

| adapter（id） | 値 | 理由 |
|---|---|---|
| 既定値 | `true` | prompt を `claude_code::build_prompt` 系で組む adapter は continuation の節（前の run の checkpoint）を描く。目的文だけを入力にする adapter が例外として `false` を宣言する |
| claude_code（`claude-code`） | `true`（既定） | `build_prompt` が continuation の節を描く。継続 session も持てる |
| codex（`codex`） | `true`（既定） | 同上 |
| acp（`acp`） | `true`（既定） | `build_prompt_with_skill_list` が同じ節を描く。継続 session も持てる |
| pi（`pi`） | `true`（既定） | `build_prompt_with_skill_list`（CoS の対話 run は別の入力） |
| aider（`aider`） | `true`（既定） | `build_prompt` |
| fake（`fake`） | `true`（既定） | protocol 準拠の worker で、`RunRequest` の `context.continuation` を受け取る |
| browser_specialist（`browser-specialist`） | 内側の harness の値 | 内側へ転送する（daemon は acp を包むので `true`） |
| langmem（`langmem`） | `false` | 入力は `objective` だけで、前置きを足さない（ADR-0047 D4） |
| local_deep_research（`local-deep-research`） | `false` | 問いは目的文・不合格の理由・前回の report だけ（ADR-0029、Phase 109） |
| paperqa（`paperqa`） | `false` | continuation の節を描かず、取得・索引・回答を毎回最初から行う（ADR-0035） |

`sessions::SUPPORTED_ADAPTERS`（`claude-code`・`codex`・`acp`）は、続けられる adapter（値が `true`）の部分集合である。継続 session（会話の resume）を持てる adapter は checkpoint からも続けられるが、逆は成り立たない（pi・aider は session を持たず、checkpoint の節で続ける）。2 つは別の性質なので 1 つの表にまとめない。`SUPPORTED_ADAPTERS` を trait の性質に移すかは本 ADR の範囲外（研究の進捗の提案）。

dispatcher は run の adapter の性質をこう引く。worker 完了の経路（`dispatcher/worker_finish.rs` の `finish_worker_result_with`）が、`RoutingRuntime::run_continues_from_checkpoint(events, run_id)` を呼ぶ。この method は `WorkerStarted.adapter`（events が正本。orphan の確定〈`finalise_from_result_json`〉でも同じ）から adapter の id を取り、宣言された harness の adapter（`RoutingRuntime` の harness adapter の表）から同じ id のものを引いて性質を返す。同じ id の adapter は同じ型なので性質も同じ。記録が無い run と、宣言に同じ id の adapter が無い run（設定の再読み込みで harness が消えた run）は `false` にする。続けられると確かめられない run を continuation に回すより、最初からのやり直し（retry）の方が常に正しいからである。これは新しい切替 flag でも互換の分岐でもなく、性質を引けない run の分類である。

worker 完了の経路は、continuation の入力を「`[execution] continuation` かつ性質が `true`」にする。対象は atomic task の `TaskRunInput.continuation_enabled`、WU の `WorkUnitRunInput.continuation_enabled`、WU の checkpoint を残すかの判定 `run_settlement::work_unit_saves_checkpoint` の `continuation` の 3 つ。`run_settlement` には adapter・harness で分ける分岐を足さない。`run_settlement` は「continuation が使えない run」を今と同じ入力で受け、理由（設定か性質か）を知らない。`run_settlement` と `worker_finish.rs` には adapter・harness の名前を書かない。

Yielded も同じ扱いにする。研究 harness は `yield` を返さないが、性質が `false` の adapter の `Yielded` は予算切れと同じく continuation に回さない。

### D3. 外した run の終わり方と、その後の retry（実装済み）

- atomic task: `settle_task_run` の既存の枝（continuation が使えない run）が、`Trigger::WorkerError{retryable: true}`、outcome `error(retryable=true): <終わり方> (continuation disabled)`、`saves_checkpoint: false` を返す。`WorkerFinished.end` は `BudgetExhausted{WallClock}` か `Yielded` のまま残る。task は attempts を 1 つ使って `ready` に戻り、次の run は checkpoint の無い新しい run（最初からのやり直し）になる。`max_retries` を使い切れば `failed`（既存の `WorkerError` の経路）。routing の試行の履歴（`task_core::is_budget_outcome`）は outcome の文に「budget」を含むので、これまでどおり予算切れとして数える。
- 委譲した子を待つ run（`children_pending`）とクラスタ job の wait（`RunEnd::Waiting`）は continuation ではないので変えない。研究 harness は委譲しない（ADR-0027 D3）し、wait を返さない。
- WU: `run_settlement/work_unit.rs` の `settle_work_unit` の既存の枝を直した。これまでの枝は WU の行を変えずに `WorkerError{retryable: true}` を返していた。WU は `running` のまま残り、task だけが `ready` に戻るので、`next_work_unit` は次の WU を選べず（`Stuck`「既に実行中の WorkUnit があります」）、task が止まった。`[execution] continuation = false` の WU でも同じだった（dispatcher の試験が無かった）。直した後は、continuation が使えない run の予算切れ・yield を `RunEnd::Failed{retryable: true}` として `execution_scheduler::decide` に渡す。WU は WU の retry（`ready`、`retries` +1、task は `Trigger::Continue{why: WorkUnitRetry}`、reason `work_unit_retry`、outcome「WorkUnit <key> を最初からやり直します（n/max）」）になり、`task.budget.max_retries` を使い切れば WU の失敗の経路（`settle_work_unit_failure` の自動 replan か質問）に進む。これは WU の idle timeout など他の retryable な失敗と同じ表し方で、continuation（`Continue{why: Continue}`、reason `continue`、WU は `needs_continuation`）とは別である。checkpoint は残さない。分岐の条件は変えていない（分岐を足していない）。

理由: 研究 harness の予算切れは最初からのやり直しなので、continuation と数えず retry として attempts（WU は `retries`）を使う。retry の上限が回数を決め、人に聞く前に WU の失敗の経路（replan）に乗る。

### D4. `read_result_file` を 3 adapter のどの時点で通すか（unit adapters）

研究 harness の engine は `artifacts/result.json` を書かない。3 adapter は engine の出力（`CELERIS_RESULT` 行・`report.md`・`answer.md`）から result.json を自分で書く（ADR-0027 D3）。このため誤配置の回収の対象になる「worker が別の場所に書いた result.json」は存在しない。

3 adapter は、成功の枝で `artifacts/result.json` を書いた直後に `result_report::read_result_file` を 1 回だけ通し、終端を `interpret_result_text` で作る（結果契約の正本が 1 つの解釈で終端を決める。ADR 2026-11-10 D1）。期待の置き場に file があるので `misplaced_report` は探索しない。書き込みに失敗したら `read_result_file` を呼ばずに `Error{retryable: true}`（文言は adapter が決める）にする。今は warn だけで `Done` にしているが、reviewer が読む file の無い run を `Done` にしない。締め切りの枝（D1）と engine の失敗の枝（非 0 の exit・出力の欠け・venv の欠け）は result.json を書かず、`read_result_file` も呼ばない。

### D5. 安全網の撤去の条件（unit net が判断して実装する）

責任者は同じ task の unit net。条件を満たさないものは残し、残す理由を unit net の進捗に書く。

1. `dispatcher/worker_task.rs` の `misplaced_report` の呼び出しのうち `Ok(Done | Error)` の枝: `Ok` を返す全 adapter が、正常終了の時点で自分で `read_result_file` を通すか、終端を result.json から作らない（`fake`・`run_subprocess` は stdout の protocol で終端を受ける）とき消せる。5 adapter は R1 で通した。研究 3 本は D4 の後に通す。D4 の後に dispatcher の探索が届くのは研究 harness の `Error`（engine が書かない file の探索で、見つかるのは無関係な file だけ）と `fake` だけになる。unit net は `fake` の扱いを決め、`Ok` の枝を消す。
2. 同じ呼び出しのうち `Err(AdapterError::Other)` の枝: adapter が `read_result_file` に届く前に `Err(Other)` で落ちる経路で、worker が result.json を書き終えている run があるときだけ残す。claude_code の `RESULT_JSON_MISSING_MARKER` は `read_result_file` の後なので対象外。unit net が `Err(Other)` を返す箇所を数え、該当が無ければ枝ごと消す。
3. `run_settlement::classify_budget_kind_from_text`（`settle_terminal` の `Error` の枝）: 予算切れを意味する `Error{retryable: true}` を返す経路が無くなったとき消す。残る経路は、研究 3 本（D1 で消える）、`subprocess::run_subprocess` の wall clock（`"wall clock exceeded"`。`deadline_terminal` に寄せれば消える）、pi・aider・acp が context・turn の超過を文で返す経路（claude_code・codex は `looks_like_context_exceeded` で構造化済み）。protocol 準拠の worker は `budget_exhausted` の message で構造化して返せる。消した後の `Error` は `Failed{retryable}` だけになる。
4. `task_core::is_budget_outcome`（`retry_policy` の試行の履歴）: outcome の文を読み、構造化した予算切れ（outcome に「budget」を含む）も同じ関数で数えている。安全網だけの関数ではない。試行の履歴が `WorkerFinished.end` を読むように変えたときに消せる。`end` の無い古い event（ADR-0072 E1 より前）は events が追記専用で書き換えられないので、unit net は「`end` が無い run を `WorkerError` と数える」か「`end` が無い run だけ文を読む」かを決める。後者は互換の分岐なので、選ぶなら設計の記録で承認を得る。

### D6. auto-leaf の WU（実装済み）

auto-leaf（ADR-0079）は、自動で leaf にした WU の continuation と context の rollover を数え、上限を超えたら人に決定（`leaf_too_large`）を求める。上限は continuation と進捗なしの上限の代わりに効く（`WuLimits`）。

checkpoint から続けられない adapter の run は、auto-leaf の回数を測らない（worker 完了の経路は `auto_counts` を作らない）。決定の要求も checkpoint も作らず、D3 の WU の retry に回す。理由: この run の予算切れは continuation ではなく retry で、上限は `max_retries` と WU の失敗の経路が受け持つ。auto-leaf の決定の材料にする checkpoint を、それを読めない adapter のために残す意味も無い。研究 harness は context の rollover を報告しないので、実行中の rollover の監視（`AutoLeafWatch`）にも掛からない。

残る限界: 同じ WU を続けられない adapter と続けられる adapter の両方で走らせた場合、後の run の `auto_leaf::counts` は前の run の `WorkerFinished.end`（予算切れ）を continuation と数える。adapter が run ごとに変わる WU はまれで、数えすぎる方向（早めに人に聞く）なので、本 ADR では変えない。

## 試験

| 試験 | 置き場 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|---|
| `research_deadline_continues_from_checkpoint_per_adapter` | `crates/task-worker/src/adapter_tests.rs` | D2 の表（10 adapter の値、browser-specialist は内側の値を返す） | 既存の試験はこの性質を持たず、上書きを消しても既定値を変えても通った |
| `research_deadline_atomic_run_neither_continues_nor_saves_a_checkpoint` | `crates/task-dispatch/src/dispatcher/tests/research_deadline.rs` | 続けられない adapter の atomic task の run は、文言の `Error`・`BudgetExhausted{WallClock}`・`Yielded` のどれでも reason `continue` の遷移と `CheckpointSaved` を残さず、`end` は予算切れ・yield のまま、outcome は `(continuation disabled)`。続けられる adapter の同じ run は continuation に回る | 既存の continuation の試験は性質を持たない偽アダプタだけを使い、研究 harness の run が continuation に回ることを検出できなかった |
| `research_deadline_work_unit_run_neither_continues_nor_saves_a_checkpoint` | 同上 | WU の経路で同じ 3 つの終わり方。WU は `needs_continuation` にならず、task は止まらずに idle まで進む（WU の retry と失敗の経路） | 同上。加えて、continuation が使えない WU の run を dispatcher で走らせる試験が無く、WU が `running` のまま task が止まる不具合（D3）を検出できなかった |
| `research_deadline_session_adapters_continue_and_research_adapters_do_not` | 同上 | `sessions::SUPPORTED_ADAPTERS` の adapter は `true`、研究 3 本は `false` で session も持たない | 2 つの性質の関係を見る試験が無かった |

変えた試験: `run_settlement::work_unit::tests::continuation_disabled_keeps_the_unit_unless_the_auto_leaf_limit_stops_it` を `continuation_disabled_retries_the_unit_unless_the_auto_leaf_limit_stops_it` に改名し、「WU の行を変えない」の確認を「WU は `ready`、task は `work_unit_retry`」に替えた。旧い契約は D3 のとおり task を止めるので廃止した。auto-leaf の上限で止める後半は変えていない。

## 代替案

- dispatcher に研究 harness の名前の一覧を置く: 名前の一覧が 2 つ目の正本になり、adapter を足すたびに直す場所が増える。性質は adapter が知っている。
- `RunFinishInput` などで性質を `run_settlement` に渡して分岐を足す: 「continuation が使えない run」の枝は既にあり、入力を変えれば同じ結果になる。`run_settlement` に adapter の事情を持ち込む理由が無い。
- 起動時に性質を run の in-memory の記録（`RunEntry`）に持たせる: daemon の再起動後の確定（orphan）では記録が無い。`run_supervisor` は lease の高 risk の path でもある。events の `WorkerStarted.adapter` で足りる。
- 既定値を `false` にする: protocol を話す adapter は全部 `build_prompt` 系で continuation の節を描くので、6 adapter と試験用の偽アダプタの全部に `true` を書くことになる。例外は目的文だけを入力にする 3 本で、例外が宣言する方が表が小さい。
- 研究 harness の wall clock を `Failed{retryable}` にする: wall clock の超過は予算切れで、routing の試行の履歴も予算切れとして数えている。締め切りの規則は `deadline_terminal` の 1 つにする。
- 続けられない run にも auto-leaf の上限を効かせる: retry を continuation と数えることになり、読めない checkpoint を残す（D6）。

## 状態の記録

- 2026-11-15: unit adr が D1〜D6 を書き、D2・D3・D6 と試験を実装した。D1・D4 は unit adapters、D5 は unit net が実装し、実装との食い違いをこの ADR に付記する。
