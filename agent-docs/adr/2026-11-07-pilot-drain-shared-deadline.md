---
tasks: [01M6TER3DG6Z9EXM9PN8X9GRJD]
---
# ADR 2026-11-07: pilot の共有の期限は承認の wall の総量から数える（drain の付記）

- 状態: 採用。[ADR 2026-11-06-cos-run-launch-accounting-and-drain](2026-11-06-cos-run-launch-accounting-and-drain.md) D3-1 を 1 点だけ改める。
- 決めた人: task 01M6TER3DG6Z9EXM9PN8X9GRJD の worker。

## 決定
1. D3-1 の式「承認の開始時刻 + 試行数 × limits.max_wall_secs」をやめ、**承認の開始時刻 + limits.max_wall_secs** にする。
   `deadline_at = min(試行の開始 + --max-wall-secs, 承認の開始時刻 + limits.max_wall_secs, expires_at)`。
   理由: judge の `wall_cap` は全試行の wall の和を `limits.max_wall_secs` と比べる（`pilot_gate.py`）。承認の wall は全体の総量で、
   試行数を掛けると 1 回目に使った分が 2 回目の余裕に化け、`wall_cap` を超えた後に試行を始められる。
2. runner（`long-horizon-pilot-llm.sh`）は最初の試行の前に承認の開始時刻を 1 度だけ取り、全試行の driver に
   `--approval-started-at`・`--approval-wall-secs`・`--approval-expires-at` で渡す。実 LLM は承認の file から、config-stub・stub は
   `--total-wall-secs` で総量を渡す（渡さなければ共有の期限は試行の上限だけ）。
3. 期限を過ぎて始まる試行は、daemon を起こして facts を書く（試行を欠かさない）。`wait_end` は直ちに wall 上限で終わり、
   facts の `drain.wall_limit_hit` を true にする。judge はその試行の `drain` 検査を unknown にする。
4. facts の `drain` は `deadline_at`・`waited_secs`・`timed_out`（件数）・`timed_out_runs`（run_id・state・finished・usage・reason）・
   `wall_limit_hit`。`timed_out_runs` の usage 欠測は `usage.missing` の `cos_drain_timeout:<件数>` に数え、tokens・cache・USD は null。
5. drain は、未完了の run が 0 になってからも 2 秒（`--drain-settle-secs`）読み直し、その間に新しい run が現れたら待ち直す
   （最後の wake・off の直後に始まる CoS run の競合）。起動前に終わった run（`launch_tracked=1`・`launched_at` 無し・reason が列挙内）は
   未完了に数えない。旧記録・列挙外の reason・終わっていない run は数える（fail closed）。

## 結果
- 試行をまたいで wall が総量を超えない。期限超過は未完了 run と理由が facts に残り、judge は unknown。
- config-stub の G7 は pending、`cos_usage_null`・`cos_drain_timeout`・`cos_not_launched` は null のまま（実 LLM の合格を主張しない）。
