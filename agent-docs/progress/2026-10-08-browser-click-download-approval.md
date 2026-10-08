---
title: browser の承認方針を変える — click・download を常時承認から外し、承認待ちの期限を 30 分にする
tasks: [01M4CDS3E4PA3F974MX20YERX3]
status: done
updated: 2026-10-08
completed: 2026-10-08
---
# browser の click・download 承認と承認待ち期限（人の決定 2026-10-08）

ADR `agent-docs/adr/2026-10-08-browser-click-download-approval-policy.md`（状態: 実装済み。ADR-0080 D4/D5 を上書き）。

## 現状の確認（base `c8f0d4dd`）

- `BrowserAction::ALWAYS_APPROVED` は実効 policy の `approval_actions`・hash・`check_narrowing` にだけ効いていた。
  **実行時の click/download の承認待ちは旧実装にも無い**: `harness_action_policy()` は `approval_actions` を見ず、
  worker の action server は allow と control gate だけで判定する。`WaitingForApproval` の wait を開くのは
  credential_use の経路だけ。つまり宣言と実行時が食い違っていた（ADR §1.2）。
- 承認待ちの期限は `APPROVAL_WAIT_MAX_SECS`（5 分）。期限切れは `browser_waits_expire(now)` で時計が引数。
- web の承認画面・通知は `deadline`（絶対時刻）を表示していて、「5 分」の固定文言は無い。

## やったこと

- task-core `browser.rs`: `ALWAYS_APPROVED = [CredentialUse]`。grant に `approval_actions: Vec<BrowserAction>`
  （既定 空）を新設し、実効 `approval_actions` = (task ∪ grant ∪ ALWAYS_APPROVED) ∩ 実効 actions。
  `harness_action_policy()` は `approval_actions` の業務 action を allow から外す（承認対象に戻した操作は fail-closed）。
- task-core `browser_wait.rs`: `APPROVAL_WAIT_MAX_SECS = 30 * 60`。登録待ち 24 時間・lease TTL は不変。
- task-api `handlers/org.rs`: `BrowserSettingsPatch.approval_actions`（`PATCH /org/{id}/browser-settings`）。
  schema 再生成（`docs/api/v1/api-v1.schema.json`、`docs/protocol/worker-protocol.schema.json`、`web/api/generated/*`）。
- web `/browser/settings`: 「毎回の承認が要る操作」の checkbox（click / download、既定どちらも外れ、credential_use は
  常に承認と明記、承認待ち 30 分の説明）。確認 dialog にも載る。`fake-daemon.mjs` が `approval_actions` を受ける。
- 文書: `docs/SPEC.md` §3.6 に既定を 1 文、`docs/api/v1/gui-api.md`（期限 30 分・承認対象の規則）、
  `docs/ops/browser-department.md` §3（`approval_actions`）、ADR-0080 に上書きの参照。
  `config/skills/cos-operator/SKILL.md` には browser 承認の説明が無いので変更なし。
- 監査（受け入れ 0 の「event・session 記録に残る」）: 経路は変えていない。承認なしの click/download も control gate
  （`browser_session_agent_action`）を通り、`browser_control_ops.rs` の既存試験（agent action の開始・終了の記録）が覆う。

## 試験（追加・変更）

- `task-core browser::policy_tests::click_and_download_are_unapproved_by_default_and_approval_is_opt_in`
  （既定で承認なし・harness allow に入る／task か grant の `approval_actions` に入れると承認対象になり allow から消える／
  hash が変わる／grant 由来の承認は model 由来の編集で外せない）
- `task-core browser::policy_tests::credential_use_is_always_approved_without_standing_approval`
- `task-core browser_wait::tests::approval_wait_defaults_and_caps_at_thirty_minutes_and_expires_once`
  （既定 30 分・clamp・29 分 59 秒では期限切れず 30 分で一度だけ・登録待ちは 24 時間。時計は引数で差し替え）
- 既存の 5 分前提を 30 分へ: `browser_wait::tests::{approve_once_consumes_once_and_deny_fails_task,
  expiry_terminates_exactly_once_and_cancel_closes_waits}`、`task-api browser_waits::expired_wait_is_gone_and_unconfigured_api_refuses_human_actions`
- web e2e `web/e2e/browser/settings.spec.ts`: checkbox 2 つが既定で外れ、download を入れて保存すると PATCH 後の
  `browser.approval_actions == ["download"]`。

## 証拠（コマンドと結果）

| コマンド | 結果 |
|---|---|
| `cargo test -p task-core browser` | 119 passed, 0 failed |
| `UPDATE_SCHEMA=1 cargo test -p task-api schema` / `UPDATE_SCHEMA=1 cargo test -p task-worker protocol` | 再生成、ok |
| `cargo test -p task-api --test browser_waits` | 6 passed |
| `cargo clippy --workspace -- -D warnings` | exit 0（warning なし） |
| `bash scripts/dev/test-parallel.sh`（1 回目） | 4715 passed / 2 failed（worker protocol schema drift、browser_waits の 6 分 offset）→ 修正 |
| `bash scripts/dev/test-parallel.sh`（2 回目） | exit 0。`4717 tests run: 4717 passed, 13 skipped`、doctest ok、`tmp_leftovers: 0` |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web lint` | exit 0（警告 4 件は既存） |
| `pnpm -C web test` | vitest 82 files / 594 tests passed、server tests pass |
| `WEB_E2E_SCOPE=functional playwright test e2e/browser/settings.spec.ts` | 2 passed |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` / `sh scripts/dev/check-adr-numbers.sh` | ok |

## 未解決事項

- **click/download を承認対象に戻したときの実行時の挙動は fail-closed（action server が拒否）で、承認待ち wait は開かない。**
  ADR-0080 D4 の「操作 intent を凍結して Blocked にし、承認後に同じ session で再開する」耐久の操作承認は旧実装にも無く、
  本 task では作っていない（ADR D2）。Objective の試験項目「approval_actions に入れれば承認待ちになること」は
  「承認対象になり、承認なしでは実行されない」の形で確かめた。
- web に task 単位の browser policy の編集画面は無い（API `PUT /tasks/{id}/browser/policy` の `approval_actions` は従来どおり）。
  組織の grant 側の選択肢を `/browser/settings` に置いた。
- `gui/`（置き換え予定）の生成型は更新していない（gui は `approval_actions` を読まない）。
- 同時進行の task 01M4CDNAYX6J68WTX7SKF0DJ64（本番で browser 実行を使える状態にする）と browser 設定・task policy が
  重なる場合は、後から入る側が合わせる。

## 提案

- 承認対象に戻した click/download を「拒否」ではなく「承認待ち（operation intent 付き wait）→ 承認後に同じ session で
  実行」にするなら、別 ADR で設計する（wait の `operation_intent_id` / `action` / `args_digest` 列は既にある。
  worker の action server から wait を開くと task が Blocked になり lease が外れるので、run の寿命管理を先に決める）。
- web の task 詳細に実効 browser policy（actions・approval_actions・allowed_domains）の表示を足すと、
  「この task では何が承認対象か」を人が確かめられる。
