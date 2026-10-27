# 委譲した子が未完了の間、親は子待ちにして continuation を数えない

- 日付: 2026-10-18
- 状態: 採用・実装済み

## 背景
Pairs PRA-37 で、親が子 4 件へ委譲した直後に yielded→continue を 3 回回し、「continuation 3 回 / 進捗なし 2 回」で人に質問した。
人が「子の完了を待つ」と答えても 1 分後に同じ質問が出た。親の作業は子の完了待ちで、続きの run に進むものは無く、LLM run と受信箱を浪費した。

## 決定
- D1: atomic の親の run が yield／予算切れで終わったとき、委譲した子に未完了（terminal でない）が 1 件でもあれば、
  `Trigger::Continue{why: ChildrenWait}`（reason `children_wait`）で `ready` に戻す。checkpoint は `end = waiting` で残す
  （クラスタ job の wait と同じく進捗なしの窓から外れる）。continuation の上限・進捗なしの判定は行わない。
- D2: `dispatch_ready` は、直近の遷移が `children_wait` で子が未完了の親を dispatch しない。子が全て terminal になれば次の tick で dispatch する。
- D3: `consecutive_continuations` は `children_wait` で窓を切る（子待ちの前の continuation は数え直す）。
  子が全て終わって初めて進捗判定と質問の対象になる。
- 旧い挙動（子待ちでも continue を数えて質問する）は削除した。

## 付記（2026-10-27）: 失敗した子の再実行を待つ
### 背景
D2 は「子が terminal でない間」だけ親を止めていた。`failed` は terminal なので、子が失敗した時点で親が起きる。
BenchFS 01M5WARE6D56 では子 01M5WC8PPW が `failed` のまま修正の昇格を待っていた。親は run 13〜17 を空回りし、
「進捗なしが 2 回」の質問を出した。人が「子の再実行を待て」と答えても、親は起きて同じ問いを繰り返した。

### 決定
- A1: 親が待つ子は `task_ops::delegate::child_wait::awaited_children` が決める（dispatcher の `dispatch_ready` と、
  run の終わりの子待ち判定の両方が使う）。
  - 終端でない子は D1/D2 のとおり待つ。dispatch の判定では、直近の遷移が `children_wait` であるか、
    親の最後の run より後に「待て」の指示があるときに待つ（再実行中の子を待つ）。
  - `failed` の子は次のどれかがある間だけ待つ。
    1. 未解決の escalated 項目: CoS の受信箱（`cos_inbox_items`）にある、その子の `failed` 項目（key `failed-<子の id>-…`）が
       `escalated` の状態にある（`SqliteStore::cos_triage_failure_escalated`）。`fallback`（CoS が使えず機械的に人へ回した項目）は
       数えない。数えると、CoS が止まっている間は ADR-0021 の親のやり直し（`child_failed`）がすべて止まるため。
    2. 子の最後の `failed` 遷移より後の「待て」の指示: 親への回答（`Event::Answered`）か、親・子への comment。
       comment は人のもの、または run に紐付かない node のもの（CoS・operator・lead）に限る。
  - 印の無い `failed` の子は従来どおり terminal で、親を起こす。
- A2: 「待て」の判定は、決定的な語の照合で行う（`child_wait::is_wait_instruction`）。
  「再実行を待」「完了を待」「wait for the rerun」などの語を含み、かつ「待たず」「don't wait」などの否定を含まない文を
  指示と読む。LLM で意味を解釈しない（DESIGN 原則 1）。
- A3: 待つ間は親を dispatch せず、待ちを `TaskWait::DelegatedChildren`（M7）として見せる。待ちの間に終わった親の run は
  D1 と同じく `children_wait` になり、continuation と進捗なしの判定に数えない。
- A4: 子が reopen・retry で動いている間は、A1 の「終端でない子」として待つ。子が完了したら親を起こす。
  子が再び失敗した場合、前の指示はその失敗より古いので効かず、親は起きる（新しい escalated 項目や指示があれば、
  その間はまた待つ）。
- 旧い挙動（`failed` の子は印があっても terminal として親を起こす）は削除した。その挙動を固定した試験は無かった。
  新しい挙動は次の試験で固定する: `parent_stays_asleep_for_a_failed_child_while_told_to_wait_for_its_rerun`（dispatcher）、
  `awaited_children_holds_a_failed_child_only_while_told_to_wait` / `a_wait_instruction_before_the_latest_failure_does_not_hold` /
  `wait_instruction_matches_wait_phrases_and_rejects_negations`（task-ops）、
  `cos_triage_failure_escalated_matches_only_the_open_item_of_that_task`（task-core）。
