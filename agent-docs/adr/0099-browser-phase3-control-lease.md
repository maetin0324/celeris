# ADR-0099: Browser Phase 3 の制御 lease（pause / takeover / resume / stop）

---
tasks: [01M3PBAVFAYPDWMQMDBXPTE2V8]
---

- 日付: 2026-09-29
- 状態: **Accepted・実装済み（2026-09-29）**。状態機械（task-core）・store の永続化・task-api の制御 API と task cancel の同期・worker の control gate・GUI の takeover/resume/stop 導線。e2e `phase3_control_converges_rejects_competition_and_cancel_stops`
- 関連: [ADR-0078](0078-browser-execution-capability.md) D3 補足・D6 補足・D8 P3-C、[ADR-0080](0080-browser-phase2-policy-broker-approval.md) H2・H3・D6

## 範囲

本 ADR が決めるのは P3-C の「誰がいま browser を操作してよいか」の規則だけである。
P3-A（Browser Identity）と P3-B（live proxy）の契約は本 ADR に含めない。別の ADR で決める。

## D3. 制御 lease の規則

実装は `crates/task-core/src/browser_control.rs`（I/O と時計を持たない。`now` は呼び出し側が渡す）。

1. 状態は `AgentRunning` / `Pausing` / `Paused` / `HumanControl` / `Stopped`。
2. **pause の収束**: pause は新しい agent 操作を止める。実行中の操作が残る間は `Pausing` で、全て終わって初めて `Paused` になる。`Paused` の前の takeover は `NotConverged` で拒否する。
3. **lease の排他**: 人の lease は同時に 1 つ。持ち主以外の takeover・操作・resume は拒否する。人が lease を持つ間、agent の操作は通らない。
4. **短い lease**: 既定 60 秒、1 回の上限 300 秒（ADR-0080 H2 と同じ値）。延長は持ち主の `Renew` だけで、1 回で上限を超えない。
5. **自動再開しない**: 切断・lease 切れでは `Paused` に戻るだけで、agent は再開しない。再開は明示の `Resume` だけ。
6. **resume の再確認**: fresh snapshot と policy / origin の再確認が済んでいない `Resume` は拒否する。
7. **競合と二重 action**: 全ての制御要求は `expected_version` と `idempotency_key` を持つ。version が古ければ `VersionConflict`。同じ key の再送は状態を変えずに前回の結果を返す（lease も延びない）。同じ key を別の command に使えば拒否する。
8. **認証区間**（ADR-0080 H3）: credential を注入したら session の終わりまで戻らない。人が lease を持っていれば取り上げ、以後の takeover と延長を拒否する。
9. **stop**: lease を失効させ `Stopped` にする。以後はどの操作も通らない。

## 未実装（後続）

- 実装済み: 状態の永続化（store）、制御 API（task-api）と task cancel の同期、worker の browser 実行への配線（human control 中は agent の操作を止める、stop で session を閉じる）、GUI の導線。
- 残り: 実 browser（agent-browser）を相手にした WS 切断・再接続の試験。現在の試験は task-api 経由の e2e と worker の fake harness まで。
