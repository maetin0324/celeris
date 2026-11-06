---
tasks: [01M6RJTQQ629D0NP953708JMS4]
---
# ADR 2026-11-06: chat run の起動境界を store で守る（旧行の判別・attempt の usage 欠測の永続化）

- 状態: 採用。[ADR 2026-11-06-cos-run-launch-accounting-and-drain](2026-11-06-cos-run-launch-accounting-and-drain.md)（以下「親 ADR」）D1・D4・D5 の store 側の細目を決める。親 ADR の決定は変えない。
- 範囲: `task-core` の chat 型・migration・`run_store` だけ。worker の process 起動と `record_attempt` の接続、予算の和の SQL、公開 view の API 文書は後続の task が行う。

## 親 ADR が決めていない点

1. **旧い走行中の行を起動前と誤認しない**。親 ADR D4-4 は「`launched_at` が NULL の行は spawn していない」と書く。
   これは `launched_at` を spawn の前に書く新しい daemon が作った行にだけ成り立つ。migration の時点で走っていた行
   （旧 daemon が作り、起動済みかもしれない）は `launched_at` が NULL でも spawn 済みの可能性がある。親 ADR D1-4 は
   既存行を埋めないと決めたので、行ごとに「境界を記録する世代か」を区別する印が要る。
2. **attempt の usage 欠測を run に残す場所**。親 ADR D4-1 は「起動した attempt のうち 1 つでも usage が無ければ run 全体を
   費用不明にする」と決めたが、`usage_json` は 1 つで、最終 telemetry（`chat_run_finish_with_telemetry`）が丸ごと上書きする。
   欠測を `usage_json` の NULL だけで表すと、既知 attempt の usage があるときに欠測を表せず、最終 telemetry が欠測を消す。

## 決定

### S1 境界の印 `launch_tracked`
- `chat_runs.launch_tracked INTEGER NOT NULL DEFAULT 0`。`chat_run_claim_next` が作る行だけ 1。migration は既存行を 0 のままにする。
- `prelaunch_reason` を書けるのは `launch_tracked = 1` かつ `launched_at IS NULL` の行だけ。`launch_tracked = 0` の行は
  `launched_at`・`prelaunch_reason` が共に NULL のままで、予算では「境界不明」（費用不明）として扱う。
- 再起動・stop の回収は `launch_tracked = 0` の行を起動前として閉じない（`interrupted`・`stopped` で閉じ、理由は通常の文面）。

### S2 attempt の記録
- `attempts_recorded INTEGER NOT NULL DEFAULT 0` と `attempts_usage_missing INTEGER NOT NULL DEFAULT 0` を足す。
- store API `chat_run_record_attempt_usage(run_id, Option<&Usage>)` は 1 attempt につき 1 回呼ぶ。`Some` なら既知 usage を `usage_json`
  へ足し合わせ（欄ごとに、片方が欠けた欄は None。`context_tokens` は最後の観測）、`None` なら `attempts_usage_missing` を 1 増やす。
- 1 回でも `record_attempt_usage` が呼ばれた run（`attempts_recorded > 0`）では、`chat_run_finish_with_telemetry` は `usage_json` を
  上書きしない（attempt の記録が正本）。呼ばれていない run は従来どおり渡された usage を書く。skill_reads・first_output_at は従来どおり。
- 欠測は消えない: `attempts_usage_missing > 0` の run の `cost_state` は、`usage_json` が既知でも `unknown`。

### S3 `cost_state`
- `not_launched`: `finished_at` あり・`launch_tracked = 1`・`launched_at` NULL・`prelaunch_reason` が列挙のどれか。
- `known`: `finished_at` あり・`usage_json` が読める・`attempts_usage_missing = 0`。
- それ以外は `unknown`（走行中の行、旧行、境界不明、読めない reason を含む）。旧 event（`cost_state` 欄なし）の読み込みも `unknown`。

### S4 順序と通知
後続の起動側が守る順序・失敗時の動作・通知 payload は `chat_run_mark_launched` などの rustdoc と
`agent-docs/progress/2026-11-06-phase5/g7-run1/backend/durable-contract.md` に書く。通知は既存の `run` event（payload は更新後の `ChatRun`）で、
初回の `mark_launched` と終端の 1 回だけ追記し、再送は追記しない。
