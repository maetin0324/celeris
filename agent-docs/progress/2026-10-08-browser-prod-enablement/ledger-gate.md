---
task: browser-prod-enablement
wu: ledger-gate
status: done
completed: 2026-10-08
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

# ledger-gate: 台帳の未配置・古い version で browser task を投入時に止める

ADR [2026-10-08-browser-prod-enablement](../../adr/2026-10-08-browser-prod-enablement.md) の D1.4（判定）と D2（gate）の中核。

## したこと
- `crates/task-worker/src/browser_ledger.rs`（新規）: `ledger_status(path, expected_release, host)` が D1.4 の表の code を返す
  （`missing`・`invalid`・`stale_release`・`stale_agent_browser`・`agent_browser_missing`・`no_conformant_backend`、task ごとの
  `ledger_lacks_credential` は `LedgerStatus::task_code`）。台帳の `generated_for` を読む（`ConformanceLedger` に欄を追加。
  `deny_unknown_fields` は残す）。`configure_conformance(path, release)`（OnceLock。env は使わない）と、`(mtime, size)` が変わった
  ときだけ計算し直す `LedgerWatch`（host の `agent-browser --version` もそのときだけ起動、5 秒で打ち切り）。
  `conformance_record_path()` は env の上書き → configure された path の順。
- `crates/task-core/src/browser_prerequisite.rs`（新規）: `BrowserPrerequisiteCode`（固定の code と人向けの文）。
  `Trigger::BrowserPrereqBlock`（Ready|Running → Blocked、reason `browser_prerequisite`、attempts 不変）・
  `Trigger::BrowserPrereqResume`（Blocked → Ready、reason `browser_prerequisite_resolved`）。
  `Event::BrowserPrerequisiteBlocked { code, message }`・`Event::BrowserPrerequisiteResumed { code }`。migration なし。
- dispatcher（`dispatcher/browser_prereq.rs` 新規、`dispatch_run.rs`・`dispatcher.rs`・`worker_finish.rs`）:
  - gate は `dispatch_one` の backoff の後・write-set／lease の前。対象は `requests_browser`・Execute・planner でない run。
  - tick の dispatch の前に、`browser_prerequisite` で止めた task を見直す。台帳全体の code は台帳が変わったときだけ、
    task ごとの code（`ledger_lacks_credential`・`browser_policy_missing`）は 30 tick ごとにも見る。揃えば Resume、
    code が変わっただけなら Blocked event を 1 回足す。止まっている間は再追記しない。
  - worker が `browser conformance record unavailable|invalid` を返した atomic の run は `InfraRequeue` にせず
    `BrowserPrereqBlock`（outcome `browser_prerequisite: <code>: adapter: …`）。
- schema 再生成（`UPDATE_SCHEMA=1`: docs/api/v1/api-v1.schema.json・event.schema.json）、web/gui の生成型、
  `web/api/realtime/event-kinds.ts`・`invalidation-map.ts`、`docs/api/v1/gui-api.md` の event 一覧と説明。

## 証拠
- `cargo test -p task-worker --lib browser_ledger_gate` → 7 passed（missing・invalid・stale_release／stale_agent_browser／agent_browser_missing・ok と ledger_lacks_credential・no_conformant_backend・watch の再計算・probe）。
- `cargo test -p task-dispatch --lib browser_ledger_gate` → 4 passed:
  - `browser_ledger_gate_missing_ledger_blocks_before_dispatch_without_infra_requeue`（40 tick で遷移は `browser_prerequisite` の 1 回だけ、adapter 0 回、event 1 件）
  - `browser_ledger_gate_stale_release_blocks_and_resumes_after_regeneration`（台帳の作り直しで `browser_prerequisite_resolved` → `dispatch`）
  - `browser_ledger_gate_placed_ledger_dispatches_and_worker_ledger_error_is_not_infra`（配置済みは `dispatch`。worker の台帳失敗は `infra_requeue` にならず、変わらない台帳では往復しない）
  - `browser_ledger_gate_ignores_tasks_without_browser_skill`
- `cargo test -p task-core --lib browser_ledger_gate` → 1 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 4726、failed 0、ignored 14）。1 回目は数を固定した試験 3 件（trigger 数 29→31、EVENT_TYPES 74→76）が落ち、期待値を直した。
- `cargo clippy --workspace -- -D warnings`・`--tests` → exit 0。
- web: `pnpm typecheck` exit 0、`pnpm vitest run api/realtime` 60 passed。gui: `pnpm typecheck` exit 0。

## ADR との差（実装の明確化）
- `Trigger::BrowserPrereqBlock` は `{ code }` を持たない（code は event に載せる。reason は固定）。
- `no_conformant_backend` の判定は `BROWSER_BACKEND_IDS`（acp を含む 3 つ）のどれも公開能力で certify されないとき（ADR は claude-code・browser-specialist の 2 つと書く。上位集合なので本番の判定は変わらない）。
- WU（計画のある task）の run の worker 側の台帳失敗は従来どおり（WU の経路は触っていない）。gate 自体は Ready の task の 1 本目で効く。

## 未解決・後続の葉へ
- daemon（`crates/celeris`）から `configure_conformance(<release dir>/browser/conformance.json, Some(sha12))` を呼ぶ配線は ledger-release 葉（D1.4 の release dir 解決）。それまで本番の gate は env が無いので `missing` で止める（従来の infra_requeue の代わり）。
- 受信箱（`build_attention` の reason `browser_prerequisite`）、`TaskDetail.browser.prerequisite`、`GET /api/v1/browser/readiness` はこの葉では未実装。API 型と web の表示を伴うので preflight／web 葉で足すことを提案する（`LedgerWatch::status()` と `BrowserPrerequisiteCode::message()` をそのまま使える）。
- `browser_policy_missing` は code だけ用意した（D4 の task-policy-auto 葉が gate から使う）。

## v2 replan（2026-10-08, run 01M4CG55NBVM704Q8GE7CNCS2K）
前回の失敗は範囲 check が再生成物 `gui/docs/celeris-api-v1.md` を許していなかったことだけ。実装は作り直さず、生成物が最新かを確かめた。
- `UPDATE_SCHEMA=1 cargo test -p task-api schema`（3 passed）・`UPDATE_SCHEMA=1 cargo test -p task-worker protocol`（1 passed）・`pnpm -C gui gen:types`・`pnpm -C web gen:types` → `git status` 差分ゼロ。
- `cargo test -p task-dispatch browser_ledger_gate_` → 4 passed。`cargo test -p task-worker browser_ledger` → 7 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 4726、failed 0、ignored 14、tmp_leftovers 0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
