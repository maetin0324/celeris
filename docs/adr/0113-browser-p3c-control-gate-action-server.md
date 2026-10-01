# ADR-0113: P3-C control gate を shim の action server に配線する

- Status: accepted
- Date: 2026-09-30
- 関連: ADR-0099 D3（control 状態機械）、ADR-0080 H3（認証区間）、ADR-0108 D4（shim → action socket）、ADR-0102 D6

## 文脈
`task_worker::browser_live::run_gated` と `ControlGate` は意味論の単体試験しかなく、harness の shim
（`browser_cli.py`）から出る agent の browser 操作は control 状態（store の `browser_control_state`）を
見ずに sandbox の action child へ書かれていた。human control・pause・auth_section・stopped 中でも
agent 操作が browser に届き得た。

## 決定
- D1: gate の位置は worker の `ActionServer::serve`（ADR-0108 D4 の private socket の受け口）とする。
  shim は同 UID で改ざんできるので、shim 内の検査に頼らない。検証済みの要求を `/session/actions` に
  書く直前に `run_gated` を通す。supervisor 自身の `__version__`（trusted、agent 操作ではない）は gate しない。
- D2: gate の状態は store が正。`StoreGate` は `BrowserWaitStore::browser_session_agent_action`
  （task-api の `agent/begin`・`agent/end` と同じ遷移を 1 IMMEDIATE トランザクションで行う）を呼ぶ。
  `Begin` は lease 期限切れを先に失効させ、`AgentRunning` 以外、または auth_section 中は拒否する。
  `End` で pause が収束する（実行中の操作が終わってから `Paused`、以後は `Blocked`）。
  他 controller（人の lease）が居る間は phase が `HumanControl` なので agent 操作は拒否される。
- D3: gate の入手は `EventSink::browser_control_gate(run_id, session_id)`。既定は `None` で、
  `None` のとき browser run は substrate 起動前に拒否する（fail closed）。dispatcher の `StoreSink` が
  `StoreGate` を返す。store の読み書き失敗も「操作を出さない」側に倒す。
- D4: `Stopped` を観測したら `SessionCloser` として action child に `close` を 1 度だけ書き
  （cancel の `spawn_close` と同じ upstream close）、その要求は出さない。
- D5: H3（認証区間中の LLM 観測停止）・approve_once・短い lease・ADR-0102 D6 の起動前拒否は変えない。
  gate は既存の拒否に追加で掛かるだけである。

## 帰結
- shim 経由の agent 操作は control 状態が `AgentRunning` かつ auth_section 外のときだけ browser に届く。
- 操作ごとに store 書き込みが 2 回増える（begin/end）。browser 操作の頻度では問題にならない。
- 試験: `crates/task-worker/tests/browser_control_gate_wire.rs`（実 SQLite store と実 shim）。
