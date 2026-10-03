---
task: review-sync-fix
wu: container-fix
status: done
completed: 2026-10-03
---
# container-fix: container・非 claude-code adapter の run で session を作らない判定順

## 変更の要点
- `crates/task-dispatch/src/sessions.rs` の `decide_continuation`: #5（adapter が `claude-code` 以外 → `AdapterUnsupported`）と #8 の container（→ `SurfaceUnsupported`）を #1（役割）の直後に移した。従来は #2 の `IndependentWu`（WU の最初の run）と #3 の `NotContinuation` が先に返り、どちらも `starts_session()` が true なので、container run に `--session-id` が渡り `node_sessions` に WU の continuation 行が作られていた（container の `CLAUDE_CONFIG_DIR` は read-only mount。ADR-0140 D3 違反）。
- 2 つの理由は `starts_session()` が false（既存）なので、dispatcher（`dispatcher/continuation_session.rs`）は session を作らず保存もせず、従来どおり `--no-session-persistence` で走らせる。dispatcher 側の変更は無し。
- retire 規則: 早期に返す分岐では、保存 session が**この WU のもの**のときだけ retire（使えない session を残さない）、別 WU の session には触れない（#2 の規則を保つ）。
- cwd が変わった場合の `SurfaceUnsupported`（#8 の後半）は従来の位置のまま。

## 試験
- `sessions/tests.rs`: `session_resume_container_and_adapter_are_decided_before_first_run_and_not_continuation`（WU の最初の run・別 WU の session・continuation でない run・continuation の 4 通りで、container も codex も session を作らない理由になり、reviewer は RoleFresh が先）。
- `dispatcher/tests/session_resume.rs`:
  - `session_resume_container_no_session`: `[run] mode = "container"` の dir repo、偽の runtime 検出（`detect_with(.., |_| Ok(()))`、podman を起こさない）。WU a の budget → yield → done と b・c の全 run で `context.session` が無く argv 相当は `--no-session-persistence` のみ、checkpoint 前置きは残る、全 WU の `work_unit_session_current` が None、`runs.session_id` が全部 None、判断の行は 5 本とも `fresh (reason=surface_unsupported)`。
  - `session_resume_reviewer_stays_fresh`: compound task（planner = claude-code）+ Reviewer 条件。planner・reviewer の run は session 無し、WU a の continuation は resume、reviewer の後も WU a・b の session 行は現役（retire されず同じ id）、reviewer/planner の `runs.session_id` は None、判断の行は WU の 3 本だけ。
- 偽 adapter（id `claude-code`）と in-memory store のみ。実 claude・外部ネットワークは使わない。

## 証拠
- 修正前の `sessions.rs` に戻して `cargo test -p task-dispatch --lib session_resume_container_no_session` → FAILED（container の初回 run に session が付く）。修正後 → ok。
- `cargo test -p task-dispatch --lib session_resume` → 26 passed / 0 failed
- `cargo test -p task-dispatch` → 594 passed / 0 failed（lib）、4 passed（integration）
- `cargo clippy -p task-dispatch -- -D warnings` → exit 0（`--all-targets` でも exit 0）
- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace` → 3770 passed / 0 failed
- `cargo clippy --workspace -- -D warnings` → exit 0

## 未解決事項
- atomic task（`current_wu` が None）の continuation の resume は並行 WU atomic-resume の範囲。

## 提案
- なし
