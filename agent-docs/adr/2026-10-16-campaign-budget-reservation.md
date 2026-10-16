# ADR: campaign の予算は『確定消費 + in-flight の予約』で数え、予約を下回る残りでは投入しない

- Date: 2026-10-16
- Status: Accepted
- 決定者: 監査修正 4 budget-reserve（上位依頼 01M530W6D1C60YJZ107ESWA58M）
- 関連: [campaign-budget-cap](2026-10-16-campaign-budget-cap.md)、[campaign-comm-failure-tracking](2026-10-16-campaign-comm-failure-tracking.md)、`crates/campaign/src/budget.rs`

## Context

上限判定は完了済みの記録（`arm_use` / `tokens_charged_total`）だけを見ていた。in-flight の試行は消費 0 に見え、
投入済み・追跡中の試行が複数あると global / arm の残りを過大に見積もって投入しすぎる。再起動後は記録から作り直すため同じ誤りが続く。

## Decision

- D1: 残予算 = 上限 − 確定消費 − in-flight の予約。`budget::spend` は trials.jsonl の記録（確定）と台帳（予約）から
  global / arm 別に毎回作り直す純関数で、再起動後も同じ値になる。
- D2: 予約は投入前に台帳へ永続化する実効上限 `reserved`（= min(per_case, arm の残り)）。1 行の予約額は
  `max(reserved, 既観測の累積)`。tokens 欠測（観測できない・usage 更新待ち・usage 取得失敗）は 0 にせず予約のまま。
  確定した記録の tokens 欠測は従来どおり per_case 相当を課金する。
- D3: 新規投入（台帳に予約が無い行）の前に、global の残り（tokens・wall）が予約額以上かを見る（`budget::admit`）。
  下回るとき、他に in-flight があれば `Defer`（次の run が回収してから再判定。`reserve_deferred` に数える）、
  何も無ければ `Exhausted`（budget_stop。有料実行 0 件）。arm の残りは同 arm の他の予約を差し引く。追跡中の行は予約済みなので gate しない。
  対象は非同期 executor かつ `stop_on_budget`（台帳を持つ経路）。同期 executor は従来どおり事後判定。
- D4: 予約の解除は台帳が terminal（done / cancelled）になった後だけ。cancel した行は停止確認（`cancel_pending` → `cancelled`）後。
  記録済みでも台帳が未完了の行は確定消費として二重に数えず、予約側（max(予約, 観測)）で 1 回数える。
  未投入の拒否（executor_error）は何も消費していないので数えない。
- D5: summary の `budget` に `reserved_in_flight`・`remaining_tokens`・`reserve_deferred`・`overshoot_formula` を出す。

## 上限保証と想定最大超過

- 保証: 確定・予約の合計は、各投入時点で `max_tokens` / `max_wall_secs` 以内（残り >= 予約を投入条件にするため）。
  wall は task の wall 上限（`min(trial_timeout, 予約 wall)` を投入時に渡す）で実行側も硬く抑える。
  tokens は実行側の硬い上限が無い（task に token 上限を渡していない）ため、超過は監視間隔に依存する。
- 想定最大超過（tokens）= `max_parallel × (poll_interval + 終端確認までの時間) × 消費速度`
  （`budget::max_overshoot_tokens`、実時間を測らず値を渡す）。例: 並列 1・poll 60 秒・終端確認 120 秒・消費 1,000 tokens/秒
  なら 180,000 tokens。case 30 万 / arm 130 万 / global 300 万の枠に対し、並列 1 では超過の上限は 1 試行ぶんの上の式だけ。
  runner の poll 間隔は 1 回の run（走査）で、継続監視 driver（watch-driver WU）の間隔がそのまま `poll_interval` になる。

## 範囲外

- task への token 上限の送出、継続監視 driver、cluster job の終了確認は別 WU。

## 付記（通し受け入れ試験での実装突き合わせ、監査修正 6b）

- 実 HTTP の通し試験（`crates/celeris/tests/campaign_acceptance_e2e.rs` の budget 停止）で、runner の gate
  `budget::admit` が `Exhausted`（in-flight 無しで残り < 予約）を `budget_stop` として記録する一方、`checker` の
  budget-stop 検査は「確定消費 >= max_tokens」を要求していたため、正当な予約停止の記録を
  「cancelled for budget although budget remained」として export が拒否することが分かった。
- 修正: `checker::budget_gate_closed`（残り < `per_case`、wall も同様）を閉じた gate の条件にする。残りが 0 になるまで
  待つ必要は無い。`per_case` は実効上限（`min(per_case, per_arm の残り)`）以上なので、runner が止めた記録を誤って拒否しない。
