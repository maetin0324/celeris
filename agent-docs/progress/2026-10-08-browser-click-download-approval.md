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

## 差し戻し対応（attempt 2、2026-10-08、commit 4bd3e88e）

レビュー（run 01M4CFVBZJR16FNPT9WBK4JH5F）の [needs-human]「承認対象に戻した click・download を fail-closed にして
よいか」に対し、CoS の一次対応（2026-10-08 00:55Z）は「人の objective は承認待ちを求めている。fail-closed への緩和は
人の決定が無い」。よって ADR D2 を「操作 intent 付きの承認待ち → 承認後に同じ session で一度だけ実行」に改訂して実装した。

- shim `browser_cli.py`: `request-approval <click|download> <@eN> <exact-HTTPS-origin> <purpose>`（検査の上
  `approval-request.json` を 1 回だけ書く、event `approval_request`）。承認が要る verb（`config.json` の
  `approval_actions`）は「request-approval を使え」と案内して exit 2。
- worker `browser.rs`: `read_approval_request`（policy で再検査: action が実効 `approval_actions` の業務 action、
  ref の形、exact HTTPS origin が許可 origin 内、purpose）→ `operation_wait`（`waiting_for_approval`、
  `operation = {action, args_digest: sha256("<action> <@eN>")}`、credential 無し、`resume_key =
  operation:<task>:<run>`）。run は `Question`、`BrowserRun.state = waiting_for_approval`。再開時は最後の
  `approved` の wait が credential_use 以外なら `approved_operation`（hash・revision・action・origin を照合）→
  wait の `session_id` を論理 session に、`harness_action_policy_with_approved` の policy で起動、
  `ActionServer` の `single_use` でその action を一度だけ通し、substrate の version 検査後に
  `consume_operation_approval`（新 `EventSink::browser_operation_approval_consume`、dispatcher `StoreSink` が実装）で
  一回の実行権を消費する。`BrowserContext.approval_actions` / `approved_operation` を prompt に出す
  （worker protocol schema 再生成）。launcher 経路（`browser_launcher_run.rs`）も同じ helper で対応し、
  `refuse_confidential` は credential_use 以外の approved を拒否しない。
- task-core: `harness_action_policy_with_approved` / `operation_approval_actions`、`ConsumedBrowserOperation` /
  `consume_operation_approval`。
- web `/browser/settings`: checkbox の説明を「押す前に毎回承認待ちになります。承認すると同じ session で一度だけ押します」
  （download も同様）に、期限の説明に受信箱の案内を加えた（レビューの指摘 2）。
- 文書: ADR D2・§3・§4 付記、`docs/SPEC.md` §3.6、`docs/ops/browser-department.md` §3、`docs/api/v1/gui-api.md`。

### 追加した試験

- task-core `browser_wait::tests::operation_approval_opens_without_credential_consumes_once_and_expires_at_thirty_minutes`
  （credential 無しの操作 wait → 未承認は消費不可 → approve_once → intent 食い違いは消費しない → 同じ run/session で
  一度だけ消費 → 29:59 では残り 30:00 で一度だけ期限切れ。時計は引数）
- task-core `browser::policy_tests::click_and_download_are_unapproved_by_default_and_approval_is_opt_in`（拡張:
  承認後の policy で click が戻る、credential_use・非承認 action は戻らない）
- task-worker `browser::tests::approval_request_is_bound_to_the_policy_and_resumes_only_under_the_same_policy`
  （要求の検証 7 系、wait の形、policy hash 束縛、prompt）
- task-worker `browser::tests::click_approval_opens_a_wait_and_resumes_the_same_session_once`
  （CELERIS_USERNS_TESTS=1。run 1: click 拒否・request-approval → Question・wait pending（click, digest）・
  task Blocked・substrate に click 無し・event に policy_block と approval_request。approve_once → run 2: 同じ
  session_id・click は 1 回だけ substrate に届き 2 回目は拒否・policy 外の scroll は shim が拒否・wait resumed）
- task-worker `browser::launcher_run::tests::single_use_action_reaches_the_launcher_once`
- shim `scripts/tests/test_browser_cli.py::test_request_approval_freezes_one_operation_and_approval_actions_hint`

### 証拠（attempt 2、最終 commit の tree）

| コマンド | 結果 |
|---|---|
| `cargo test -p task-core browser` | 120 passed |
| `cargo test -p task-worker browser` | 150 passed / 3 ignored |
| `CELERIS_USERNS_TESTS=1 cargo test -p task-worker -- click_approval_opens_a_wait launch_uses_generated_policy actions_and_domains_outside_task_policy single_use_action` | 4 passed |
| `python3 -m unittest scripts/tests/test_browser_cli.py` | 16 tests OK |
| `UPDATE_SCHEMA=1 cargo test -p task-worker protocol` / `-p task-api schema` | ok（worker protocol schema のみ差分） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh`（1 回目） | exit 100。4729 passed / 1 failed: `task-dispatch …phase_effect_ab_write_set_gate_avoids_conflict_and_repair`（browser と無関係の既存 A/B 試験。単独で 3/3 合格） |
| `bash scripts/dev/test-parallel.sh`（2 回目） | exit 0。`4730 passed, 0 failed, 14 ignored`、doctest ok、`tmp_leftovers: 0` |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web lint` | exit 0（警告 4 件は既存の `styles.css` `!important`） |
| `pnpm -C web test` | 82 files / 598 tests passed、server tests pass |
| `WEB_E2E_SCOPE=functional playwright test e2e/browser/settings.spec.ts` | 2 passed |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` / `sh scripts/dev/check-adr-numbers.sh` | ok |

## 未解決事項

- 承認は「この task の実効 policy・この論理 session・この action・一回」に束縛され、page の exact URL は wait に凍結しない
  （browser process は run ごとに新しく `@eN` は再生できないため、agent が同じ origin に戻って snapshot を取り直し、
  承認された action を一度だけ出す。束縛は allowed_domains と一回実行。ADR D2.4）。
- `task-dispatch` の `phase_effect_ab_write_set_gate_avoids_conflict_and_repair` は並列 gate の負荷で 1 回落ちた
  （単独 3/3 合格、再実行の gate は合格）。browser とは無関係。
- web に task 単位の browser policy の編集画面は無い（API `PUT /tasks/{id}/browser/policy` の `approval_actions` は従来どおり）。
  組織の grant 側の選択肢を `/browser/settings` に置いた。
- `gui/`（置き換え予定）の生成型は更新していない（gui は `approval_actions` を読まない）。
- 同時進行の task 01M4CDNAYX6J68WTX7SKF0DJ64（本番で browser 実行を使える状態にする）と browser 設定・task policy が
  重なる場合は、後から入る側が合わせる。

## 提案

- web の task 詳細に実効 browser policy（actions・approval_actions・allowed_domains）の表示を足すと、
  「この task では何が承認対象か」を人が確かめられる。
- 承認の束縛に exact URL（承認要求時の現在 page）を加えるなら、shim が現在 URL を知る経路（action server 経由の
  CDP 問い合わせ）が要る。必要になれば別 ADR。
