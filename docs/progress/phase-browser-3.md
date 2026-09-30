# Phase browser-3: Browser Identity・live proxy・takeover

---
tasks: [01M3PBAVFAYPDWMQMDBXPTE2V8, 01M3Q2FPRCF34F00PBZSMNSZE8, 01M3SM0WN346ABGF42QV02RZTP]
---

- 状態（2026-09-30、[追跡表](phase-browser-acceptance.md) と一致）: **P3-A は一部達成**（P3-A-1〜7 合格: 保管・期限・削除・失効・混入拒否・試験 admission での復元・本番 admission の SameUid 拒否。P3-A-8「本番 admission での identity 復元の成功」は後続 task `01M3SPN8H05EJ3DHPVEGEYTMEH`、決定 sep-uid=a）。**P3-B は達成**（P3-B-1〜4 合格）。**P3-C は達成**（P3-C-1〜6 合格、worker 側 control gate の run loop 配線 ADR-0094 を含む）。本番未昇格。
- 更新: 2026-09-30（control gate 配線の統合を反映、task 01M3SM0WN346ABGF42QV02RZTP）。前回更新: 2026-09-29（HEAD 17e0777 + docs）
- ADR: [0081](../adr/0081-browser-phase3-control-lease.md)（制御 lease）/ [0082](../adr/0082-browser-phase3-live-proxy-acl.md)（live proxy ACL）/ [0083](../adr/0083-browser-phase3-identity-contract.md)（identity 契約）

## 行ごとの成果と検査

| ID | 単独の成果・検査 | 入ったもの | 検査（コマンド → 結果） | 判定 |
|---|---|---|---|---|
| P3-A Browser Identity | project/origin 単位の暗号化・期限/削除/失効、他 identity 混入拒否 | task-core `browser_identity.rs`（束縛・期限 既定 7 日/上限 30 日・失効で世代を上げる・混入拒否）、credentiald の project+origin 鍵による XChaCha20Poly1305 封緘（AAD に束縛）と削除時の鍵消去（e168f57）、store の identity metadata（4a53db3、schema 34）、task-api の登録/一覧/失効/削除（5e900e8・40e742f・8940ad3）、GUI の一覧・失効・削除（a45159a） | `cargo test -p e2e --test api_scenarios phase3_` → exit 0、3 passed（`phase3_identity_register_revoke_delete_and_trusted_local_restore_denied` を含む）。`cargo test -p celeris-credentiald identity` → exit 0、8 passed。`cargo test -p task-core --lib browser_identity` → exit 0、10 passed（前回記録） | 一部達成（[追跡表](phase-browser-acceptance.md) P3-A-1〜7 合格、P3-A-8 本番復元成功は後続 `01M3SPN8H05EJ3DHPVEGEYTMEH`）。保管側は満たす。復元は P4-A の deliver_state（2026-09-30）で試験 admission のみ成功、本番は SameUid 拒否。trusted local での復元は `isolation_required` で拒否することを e2e で確認 |
| P3-B live proxy | task/run ACL、WS 接続/再接続、frame/status/tabs/url/console と永続 event、他 task 拒否、cookie/token 非記録 | task-core `browser_live.rs`、store の live event（4a53db3）、task-api の task/run 単位 live grant と event 書き込み・`last_seen` からの再接続読み出し（fde7e38）、worker の live emitter・認証区間の送出停止（d5aefa5・96fce7b）、GUI relay の task-api 経由認可と永続 event の WS 配信（b7ec98d・f2d5a1c） | `cargo test -p e2e --test api_scenarios phase3_` → exit 0、3 passed（`phase3_live_grant_is_task_scoped_scrubbed_and_reconnects_from_last_seen`: 他 task の grant 拒否、cookie/token の scrub、last_seen からの再接続）。`cargo test -p task-api browser_` → exit 0、11+1+1 passed | 達成（[追跡表](phase-browser-acceptance.md) P3-B-1〜4 合格） |
| P3-C takeover | pause 収束・controller lease 排他・takeover/resume/stop、切断・競合・二重 action 試験 | task-core `browser_control.rs`、store の control 状態と `stop_task`（ec83529）、task-api の pause/takeover/renew/resume/stop と task cancel の同期（6f18bb1・3be95ac）、worker の control gate のモデル（`browser_live::run_gated`／`InMemoryGate`、d5aefa5・96fce7b）に加え、**run loop への配線を完了**（ADR-0094、2026-09-30）。gate の位置は worker `ActionServer::serve`（shim の private action socket の受け口）で、検証済み要求を `/session/actions` に書く直前に `run_gated` を通す。gate の状態は store が正で、`EventSink::browser_control_gate(run_id, session_id)` 経由で `StoreGate`（`BrowserWaitStore::browser_session_agent_action` の begin/end）を取得する。既定 `None` は fail closed（substrate 起動前拒否）。`Stopped` 観測時は `SessionCloser` として action child へ 1 度だけ close。GUI の takeover/resume/stop 導線（a45159a） | `cargo test -p e2e --test api_scenarios phase3_` → exit 0、3 passed（`phase3_control_converges_rejects_competition_and_cancel_stops`: pause 収束、他 controller の拒否、古い version の `VersionConflict`、同じ idempotency key の二重 action、task cancel で `Stopped`）。`cargo test -p task-worker --test browser_control_gate_wire -- --nocapture`（実 SQLite store + 実 shim）→ exit 0、**5 passed**（`gate_wire_human_control_blocks_agent_until_resume`・`gate_wire_auth_section_blocks_agent_until_left`・`gate_wire_pause_converges_after_in_flight_then_blocks`・`gate_wire_stopped_closes_session_once_and_never_runs_actions`・`gate_wire_running_actions_reach_the_browser`、2026-09-30 再検査） | 達成（[追跡表](phase-browser-acceptance.md) P3-C-1〜6 合格、run loop 配線を含む） |

## ワークスペース全体の検査（2026-09-29）

- `cargo test --workspace` → 1 回目 exit 101（`api_enforces_token_host_and_workspace_boundaries_without_leaking_env_values` が `celerisctl replay` の `status replayed=Running stored=Ready` で 1 件失敗。Phase 3 の試験ではない）。同じ試験の単独再実行 `cargo test -p e2e --test api_scenarios api_enforces_token` → exit 0、1 passed。
- `cargo test --workspace --no-fail-fast`（再実行）→ **exit 0、2929 passed / 0 failed / 7 ignored**（`test result` 108 行）。
- `cargo clippy --workspace -- -D warnings` → **exit 0**、warning 0。

## 未解決・後続

- **P3-A の利用（browser session への identity の復元）は P4-A（container / 別 UID / egress 制限）の後**。`Isolation::Isolated` を名乗れる経路は P4-A まで作らない（ADR-0083 D3）。Phase 3 の復元 API は常に `isolation_required` を返す。
- **agent-browser 0.38.1 の `--restore` / `--state` / `--profile` は使わない**。0.38.1 の restore は origin 単位の絞り込みが無いと仮定しており（未検証）、allowlist を解除して対応することはしない。復元は broker が run 開始前に行う別経路とし、配線（P4-A の後）の前に固定 version 上の負例で確かめる（ADR-0083 D4）。
- `api_enforces_token_host_and_workspace_boundaries_without_leaking_env_values` の replay 不一致が高負荷時に一度出た。単独・再実行では通る。タイミング依存の疑い。
- GUI の検査（pnpm）は各 WorkUnit（gui-live・gui-ctl）で実行済み。本 WorkUnit では再実行していない。

## fix-replay: e2e `replay` の不一致（Running vs Ready）

- 原因: Phase 3 の状態遷移の退行ではなく、`task_ops::replay` の**非原子的な読み取り**。`store.list(None)` で tasks を読んだ後、タスクごとに別の読み取りで `events_for` を読んでいたため、daemon 稼働中に呼ぶ試験（`api_enforces_token_host_and_workspace_boundaries_without_leaking_env_values`）で、その間に daemon が lease を取る（Ready→Running の `Transitioned` を書く）と「古い tasks 行（Ready）× 新しい events（Running）」を突き合わせて偽の MISMATCH を出した。Phase 3 の e2e 追加で負荷とタスク数が増え、窓に当たりやすくなった。
- 修正: `TaskStore::tasks_with_events` を追加し、`SqliteStore` では tasks と events を 1 つの読み取りトランザクション（WAL スナップショット）で読む。`replay` はこれを使う。sleep・リトライ・`#[ignore]` は使っていない。

## 再試行（task 01M3Q2FPRCF34F00PBZSMNSZE8, 2026-09-29）: 認証区間を control・event 経路へ配線

前回ブランチ（65c5ae7）を merge して引き継ぎ、reviewer 指摘の欠落だけを塞いだ。

| 指摘 | 入ったもの | 検査 → 結果 | 判定 |
|---|---|---|---|
| (1) worker が認証区間の開始/終了を control 状態へ | `EventSink::browser_auth_section`（supervisor 専用）。browser.rs の credential 注入の前に `true`、注入と policy 切替の後に `false`。記録できなければ fail closed。書き込みは task-api `auth-section` endpoint と同じ唯一の store op `SqliteStore::browser_control_auth_section`（dispatcher の StoreSink は `BrowserWaitStore::browser_session_auth_section` 経由）。`BrowserControl::leave_auth_section` を追加 | `cargo test -p task-api --test browser_e2e auth_section` → exit 0、1 passed（実 substrate shim・credentiald bridge・fake harness。区間中 auth_section=true・takeover は `auth_section_active`、終了後 false、store の control 状態も false） | 満たす |
| (2) API と GUI の両方で takeover/renew を拒否 | `auth-section` endpoint が `{"active": bool}` を受ける（既定 true）。拒否は core（`BrowserControl::takeover/renew`）→ HTTP 409 `auth_section_active`。GUI `browser-control.server.ts` は task-api の control 状態を読み、`auth_section` が true なら command を task-api へ送らずに 409、GET 後に区間が始まった競合では task-api の 409 をそのまま返す | `cargo test -p e2e --test api_scenarios phase3_auth_section` → exit 0、1 passed（保持中 lease の取り上げ、renew/takeover の 409、終了後の takeover 成功）。`cd gui && pnpm exec vitest run test/unit/browser-control.test.ts` → 4 passed（takeover・renew の 409 と未送信、task-api 409 の中継、区間外の転送）。`pnpm test` → 83 files / 1236 passed（1 回目は高負荷で失敗し、再実行で全件 pass） | 満たす |
| (3) 実 forward_events の停止 | `forward_events` を 1 本に統一（`forward_events_live` を削除）。`LiveEmitter` が本番の run loop で作られ（`EventSinkLive` → `EventSink::browser_live` → store の live event）、区間中は progress・artifact・live event を出さず行を消費する（溜めない） | `cargo test -p task-worker browser_auth_section` → exit 0（progress 0・artifact 0・live 0、区間後の行だけ転送）。e2e の区間中 worker 出力件数の差 0 | 満たす |
| (3') テスト専用の未配線経路 | `forward_events_live` に加え、テストからしか作られていなかった `BrowserLive`／`CliCloser`（`#[cfg_attr(not(test), allow(dead_code))]` 付き）と、それだけを試していた 2 試験を削除。gate の意味論の試験は `browser_live::tests`（`run_gated`）に残る | `grep -rn "BrowserLive\|CliCloser\|forward_events_live" crates` → 0 件 | 満たす |

- 最終（本 run の 2 回目、`cargo fmt` 後）: `cargo test --workspace` → exit 0、2929 passed / 0 failed。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo test --workspace auth_section` → exit 0（e2e 1・browser_e2e 1・task-core 2・task-worker 2 passed）。
- 1 回目（前の commit 時点）: `cargo test --workspace` → exit 0、2931 passed / 0 failed（test result 92 行、0 件の行を除く）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- 未解決（2026-09-29 時点）: P3-A の identity 復元は P4-A 後（ADR-0083 D3、`isolation_required` のまま）。worker 側の control gate（human control／pause 中に agent の browser 操作を止める）の run loop への配線は未（agent の操作は harness 子プロセスの shim から出るため、store の control 状態を shim が読む経路が要る）。削除した `BrowserLive`／`CliCloser` はこの配線の代わりにならない。

## control gate 配線の解消（2026-09-30、ADR-0094、task 01M3SM0WN346ABGF42QV02RZTP）

- 上記の未解決（worker 側 control gate の run loop 配線）を解消した。gate は worker `ActionServer::serve`（ADR-0088 D4 の shim → action socket の受け口）に置き、shim は同 UID で改ざんできるため shim 内検査には頼らない。検証済み要求を `/session/actions` に書く直前に `run_gated` を通す（supervisor 自身の `__version__` は gate しない）。
- gate の状態は store が正。`StoreGate` は `BrowserWaitStore::browser_session_agent_action`（task-api の `agent/begin`・`agent/end` と同じ遷移を 1 IMMEDIATE トランザクションで行う）を呼ぶ。`Begin` は lease 期限切れを先に失効させ、`AgentRunning` 以外または auth_section 中は拒否。`End` で pause が収束する。gate の入手は `EventSink::browser_control_gate(run_id, session_id)` で、既定 `None` のときは fail closed（substrate 起動前拒否）。`Stopped` 観測時は `SessionCloser` として action child へ close を 1 度だけ書く。詳細は [ADR-0094](../adr/0094-browser-p3c-control-gate-action-server.md)。
- P3-A の identity 復元（`isolation_required` 拒否と、その後の成功経路）は別 WorkUnit（deliver-state）が P4-A/P4-B で扱う。phase-browser-4.md 側の該当行を参照。
- 検査（2026-09-30、ワークスペース全体への統合後）: `cargo test --workspace` → exit 0、**3055 passed / 0 failed / 11 ignored**（114 test binary）。`cargo clippy --workspace -- -D warnings` → exit 0、警告 0。`cargo test -p task-worker --test browser_control_gate_wire -- --nocapture`（実 SQLite store + 実 shim）→ exit 0、5 passed（human control／pause／auth_section 中に agent 操作が実行前に止まり、解除後に再開することを確認。テスト専用の未配線経路ではなく `ActionServer::serve` の本番経路を通す）。
