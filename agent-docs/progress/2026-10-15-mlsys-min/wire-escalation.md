---
unit: wire-escalation
status: done
completed: 2026-10-15
---
# wire-escalation

- `route_failure.rs`: `FailureOrigin` / `classify_failure`（adapter 失敗は常に TransientInfra、worker error は retryable または文面が timeout/429/5xx 等なら infra、それ以外と check 失敗・review 差し戻しは Content）、`Dispatcher::decide_route_failure`（policy-core の `escalation_decision` を呼ぶ。backoff は `infra_backoff_delay`＋注入時計 `now_unix_fn`。明示 model 指定の Content 失敗は昇格せず `ReportFailure`）。
- `route_launch.rs`: `admit_by_availability`（Available のみ起動、Unknown は Hold。0 扱いしない）。
- 試験 `dispatcher/tests/route_failure_escalation.rs`（5 件、時計は注入、待機なし）。既存試験は無変更。route_select.rs・config は未変更。
- 証拠: `bash scripts/dev/test-parallel.sh` 5315 passed / 0 failed、`cargo clippy --workspace -- -D warnings` exit 0。

## 未解決・提案
- 本 unit は判断関数の配線まで。worker_finish の実際の再試行・再計画経路から `decide_route_failure` を呼ぶ接続と、attempts（昇格回数・infra 再試行回数）を events から導く部分は wire-mode 統合後に行う（dispatcher.rs の既存 `infra_backoff` と二重管理にならないよう整理が要る）。
