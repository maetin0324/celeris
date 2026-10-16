# ADR: campaign の通信障害・cancel 失敗で追跡と予約を失わない（executor 失敗の分類）

- Date: 2026-10-16
- Status: Accepted
- 決定者: 監査修正 4（上位依頼 01M530W6D1C60YJZ107ESWA58M）
- 関連: [破壊的変更を是とする](2026-10-11-breaking-changes-by-default.md)、`crates/campaign`（ledger・runner・executor）

## Context

`drive_async` は submit/track/poll/usage の Err を一律 `DriveErr::Exec` にし、その試行を error として記録し、
外部 task を best-effort で cancel（失敗は無視）して台帳を閉じていた。POST 応答喪失・poll の通信断でも
tokens/cost=None・wall=0 の Error を残し、外部で動き続ける task を追跡対象から外し、予約も解除してしまう。

## Decision

- D1: executor の失敗は `ExecError { kind, msg }`。`ErrKind`:
  - `NotSent`（確実な未投入・再試行可）: 行は planned のまま、次回の run が同じ冪等キーで再試行する。error 記録を作らない。
  - `Refused`（確実な未投入・再試行しても通らない。HTTP 4xx・設定不備）: その試行だけ error、台帳は cancelled で閉じる。
  - `Unknown`（投入結果不明。POST 応答喪失・5xx）: 台帳 state を `submit_unknown` にする。再 submit しない。
    次回以降は冪等キーの照会（lookup）で見つけた task を追う。見つからない間は `submit_unknown` のまま保ち続ける
    （自動で再投入しない。人の確認が要る行は `ledger.by_state.submit_unknown` に出る）。
  - `Interrupted`（投入済みの task との通信断。track/poll/usage/cancel）: 台帳の task_id・冪等キー・既観測 usage・
    開始時刻・予約を保ったまま `comm_failures`・`last_error` を更新して次回の poll に回す。消費 0 の完了にも error にもしない。
  分類のないエラー（`From<String>`）は保守的に `Unknown`。投入後の phase（track/poll/usage）は種類にかかわらず通信断として扱う。
- D2: cancel は先に `cancel_pending` を台帳へ永続化し、`cancel` の成功と `Executor::stopped`（外部 task・子 task・job が終端）の
  確認が揃って初めて `cancelled`（terminal）にする。失敗・未確認は隠さず `cancel_pending` のまま残り、予約も解除しない
  （`unfinished()` と `is_referenced` に残るので in-flight 数・workspace 保持の対象）。run の先頭で、新しい仕事より先に
  `cancel_pending` 行の cancel を再試行する。cancel の再試行は trials.jsonl の記録を増やさない。
- D3: 台帳行に `started_at`（planned にした時刻）・`observed_usage`（最後に観測した累積。tokens None は欠測）・
  `reserved`（投入前に確保した実効上限 Cap）・`comm_failures`・`last_error` を足す。旧行は serde 既定値（None/0）で読める。
  再起動（`Ledger::open`）後も、`task_id` を持つ行は同じ task を追い、`submit_unknown` は照会で再接続する。
- D4: 旧 `DriveErr::Exec` と「Err なら cancel して Error を保存」の経路を消した。`drive_async` は `Driven::{Done, Cancelled,
  Deferred, Refused}`。`ArmDispatch` は `usage`/`stopped` も委譲する（以前は usage が既定の None で、実行中の上限監視が効かなかった）。

## 範囲外・残り

- 予算の予約と実消費の整合（global/arm/case）と欠測の扱いは budget-reserve WU、継続監視 driver と cluster job の walltime・
  終了確認は watch-driver WU の担当。`Executor::stopped` の実 job 確認（Celeris 側は木の終端のみ、cluster job は人手手順記録）は
  それらで強める。HarnessExecutor の `stopped` は既定（cancel が終端を確定させるので true）。
