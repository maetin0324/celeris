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
