---
task: browser-prod-enablement
wu: policy-missing
status: done
completed: 2026-10-08
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

# policy-missing: 保存 policy の無い browser task を browser_policy_missing で止め、受信箱に出す

部署 reviewer の差し戻し（criterion 1: `BrowserPolicyMissing` を出すコードが無く、保存 policy の無い browser task が
`browser policy rejected` で infra_requeue を繰り返す。ADR 付記が未実装の受信箱 reason を「実装した」と書いていた）への修正。

## 実装

- `crates/task-dispatch/src/dispatcher/browser_prereq.rs`
  - `browser_task_code(&Task)`（dispatch 前の gate と再開の両方）: 台帳より先に、保存 policy が無い・
    `browser_task_policy_get` が Err・task の `requirements.browser` と交わらない（`task_run_policy` が Err）なら
    `BrowserPolicyMissing`。`Ready → Blocked`（reason `browser_prerequisite`、attempts 不変、event 1 回）。
  - `worker_error_code`: worker の失敗文の分類。台帳由来に加え `browser policy rejected` を `BrowserPolicyMissing` にする。
- `crates/task-dispatch/src/dispatcher/worker_finish.rs`: 分類に `worker_error_code` を使う（infra_requeue に回さない）。
- `crates/task-ops/src/inbox.rs`: `AttentionItem::BrowserPrerequisite { task, code, message, at }`。`build_attention` が
  直前の遷移の reason が `browser_prerequisite` の blocked task について、最後の `BrowserPrerequisiteBlocked` の message で出す。
- `crates/task-ops/src/human_inbox.rs`: 共通形では既存の `browser_wait` 種類へ写す（id `browser_prerequisite-<task>-<code>`、
  選択肢・native なし、link `/tasks/<id>`）。web の種類一覧は変えていない。
- schema 再生成: `docs/api/v1/api-v1.schema.json`、`web/api/generated/{schema.json,types.ts}`、`gui/app/celeris/types.ts`。
- ADR `agent-docs/adr/2026-10-08-browser-prod-enablement.md` の付記を訂正（D2 の受信箱、D4 の browser_policy_missing）。

## 試験（接頭辞 `browser_policy_missing_`）

- task-dispatch `dispatcher::tests::browser_policy_missing`（4）: 保存 policy 無しで blocked・adapter 未呼び出し・event 1 回・
  41 tick で再追記なし／policy の書き込み後の見直しで `BrowserPrerequisiteResumed` → `browser_prerequisite_resolved` → `dispatch`／
  task の origin と交わらない policy も止まる／worker の `browser policy rejected`（grant と交わらない）が infra_requeue でなく
  `browser_prerequisite: browser_policy_missing: …` で止まる。fake adapter・`LedgerWatch::fixed` と一時 file の台帳。
- task-ops `inbox::tests::browser_policy_missing_inbox_item_carries_message_and_disappears_after_resume`（1）。
- 既存の `browser_ledger_gate_` の helper を `pub(super)` にして共有（挙動は変えていない）。

## 証拠

- `cargo nextest run -p task-dispatch -p task-ops -E 'test(browser_policy_missing_)'` — 5 passed。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-api -E 'test(schema)'` — 4 passed（再生成）。task-worker protocol は差分なし。
- `bash scripts/dev/test-parallel.sh` — exit 0。passed 4798 / failed 0 / ignored 14、doctest exit 0。
- `cargo clippy --workspace -- -D warnings` — exit 0。`cargo fmt --all --check` — exit 0。
- web: `pnpm run typecheck` exit 0、`pnpm run test` exit 0（vitest 625 passed + server node tests）、`pnpm run lint` exit 0（既存の warning 4）。
- gui: `pnpm typecheck` exit 0。

## 未解決

- 担当の grant と交わらないことは dispatcher の gate では見ない（worker の `admit` だけ）。この形で止まった task は
  30 tick ごとの見直しで ready に戻り worker の `admit` で再び止まる（LLM・attempts は使わないが遷移と event が周期で増える）。
- 実機（本番 daemon）での確認は未実施。

## 提案

- `PUT /tasks/{id}/browser/policy` が `BrowserTaskPolicySet` event を書けば、見直しを「policy が変わったときだけ」にでき、
  上の周期の往復を無くせる。
- 共通形の受信箱に `browser_prerequisite` 専用の種類を足すと、web の badge（今は「ブラウザの承認待ち」）を正確にできる。
