# ADR 2026-10-08: browser の click・download を常時承認から外し、承認待ちの期限を 30 分にする

- 日付: 2026-10-08
- 状態: 実装済み（2026-10-08。task 01M4CDS3E4PA3F974MX20YERX3。**人の決定 2026-10-08**: click と download は許可された
  domain の中なら承認なしで実行してよい・毎回の承認はやめる、承認待ちの期限は 5 分 → 30 分、credential_use の毎回承認は
  変えない・standing approval も入れない）
- 上書きするもの: [ADR-0080](0080-browser-phase2-policy-broker-approval.md) D4 の「待機期限は既定・上限 5 分」と
  D5 の「Phase 2 では全 `click` と `download` を承認対象とする」。ADR-0080 のそれ以外（credential_use の一回承認、
  登録待ち 24 時間、attestation、intent の束縛、standing approval を設けない）は変えない
- 関連: [ADR-0078](0078-browser-execution-capability.md) D3/D4、ADR 2026-10-05-browser-department-web-live-view D2.0
  （task policy の origin 交差）、[ADR-0113](0113-browser-p3c-control-gate-action-server.md)（agent action の control gate と記録）、
  [ADR-0110](0110-browser-p4b-h3-shared-cdp-trusted-selector.md)（credential_use 承認の固定値）

## 1. 文脈

### 1.1 人の決定（2026-10-08）

1. click と download は、許可された domain の中なら承認なしで実行してよい。毎回の承認はやめる。
2. 承認待ち（`waiting_for_approval`）の期限を 5 分から 30 分にする。
3. credential_use（ID・password を使う login）の承認はこれまでどおり毎回人が行う。standing approval も入れない。

### 1.2 現状（2026-10-08、base `c8f0d4dd` のコードで確認）

- `crates/task-core/src/browser.rs` の `BrowserAction::ALWAYS_APPROVED` は `[Click, Download, CredentialUse]`。
  `EffectiveBrowserPolicy::derive` は task policy の `approval_actions` にこれを常に足し、`check_narrowing` は
  モデル由来の編集でこれを外せないようにしている。実効 policy の `approval_actions` は policy hash・API の表示・
  `requires_approval()` に出る。
- **実行時の click・download の承認待ちは、旧実装にも無い。** `harness_action_policy()` は `approval_actions` を見ず、
  `credential_use` 以外の全 action（click・download を含む）を harness の allow に入れる。worker の action server
  （`crates/task-worker/src/browser_action.rs`）は allow と control gate（ADR-0113）だけで判定する。
  `WaitingForApproval` の wait を開くのは credential_use の経路（`crates/task-worker/src/browser.rs`、登録済み credential
  の使用承認）だけである。つまり宣言（常時承認）と実行時（承認なしで実行）が食い違っていた。
- `crates/task-core/src/browser_wait.rs` の `APPROVAL_WAIT_MAX_SECS = 5 * 60`（既定・上限）。`NewBrowserWait::ttl()`
  が `ttl_secs` をこの上限に clamp し、期限切れは `browser_waits_expire(now)`（時計は引数）が tick から一度だけ終端化する。
- 組織の grant（`BrowserCapability`）には `approval_actions` が無く、web の `/browser/settings` にも承認の項目は無い。
  web の承認画面（`browser-waits-panel.tsx`）は `deadline` を絶対時刻で表示し、「5 分」の文言は持たない。

## 2. 決定

### D1. 常時承認は credential_use だけ

`BrowserAction::ALWAYS_APPROVED = [CredentialUse]`。click・download を承認対象にするかどうかは次の和集合で決まり、
既定（どちらも空）では承認なしである:

- task policy の `approval_actions`（既存。`allowed_actions` の部分集合。`PUT /tasks/{id}/browser/policy`）
- 組織の browser grant の `approval_actions`（**新設**。`BrowserCapability.approval_actions: Vec<BrowserAction>`、
  既定は空。`PATCH /org/{id}/browser-settings` の `approval_actions` と web の `/browser/settings` の
  「毎回の承認が要る操作」checkbox〈click・download〉で変える）

実効 policy の `approval_actions` = （task ∪ grant ∪ ALWAYS_APPROVED）∩ 実効 `actions`。`check_narrowing` は
現行実効 policy の `approval_actions` を外す編集を引き続き拒否する（grant 由来も含めて「承認を外す」のは管理者経路）。
grant の `approval_actions` に `credential_use` を書いても意味は変わらない（常に承認）。

### D2. 承認対象に戻した click・download は操作 intent 付きの承認待ちになり、承認後に同じ session で一度だけ実行する

（2026-10-08 差し戻し対応で改訂。初版は「fail-closed で拒否」だったが、人の objective は「approval_actions に入れれば
承認待ちになること」を求めており、fail-closed への緩和は人の決定が無い。）

1. **要求**: 実効 policy の `approval_actions` に入った業務 action は harness の action allow から外す
   （`harness_action_policy()`）。shim はその verb を「承認が要る」と返し（`config.json` の `approval_actions`）、
   agent は `request-approval <click|download> <@eN> <exact-HTTPS-origin> <purpose>` で要求して止まる。shim は
   action が `approval_actions` にあること・ref の形・origin が正規の exact HTTPS で許可 origin の中にあること・
   purpose（1〜500 文字）を検査し、session に 1 回だけ `approval-request.json` を書く（秘密は無い）。
2. **wait**: run の終わりに supervisor がその file を policy で再検査し（`read_approval_request`）、
   `waiting_for_approval` の wait を開く。`operation` = `{intent_id, action: click|download, args_digest:
   sha256("<action> <@eN>")}`、`origin`・`purpose` は要求の値、`credential` 無し、`resume_key` =
   `operation:<task>:<run>`。task は `Blocked`、run は `Question`、`BrowserRun.state` は `waiting_for_approval`。
   期限は D3 の 30 分。受信箱・承認画面は既存の wait 表示（operation と purpose）で出る。
3. **再開**: 人の `approve_once` で task は `ready` に戻り、次の run は最後の wait が `approved` で operation が
   `credential_use` 以外なら、policy hash・revision が一致し、action が今も実効 `approval_actions` にあり、origin が
   許可 origin の中にあるときだけ（`approved_operation`）、**wait の `session_id` を論理 session として** 起動する。
   harness policy はその action を加えた allow（`harness_action_policy_with_approved`）で、action server は
   その action を **一度だけ**通す（`single_use`。browser に渡した時点で allow から外れる）。承認の一回の実行権は
   substrate の version 検査の後、`consume_operation_approval`（store の wait の intent と突き合わせ →
   `browser_wait_consume`、CAS・期限・resume_key）で消費する。prompt は承認された操作（action・origin・purpose）
   を agent に伝える。`deny`・期限切れは task を `failed` にする（ADR-0080 D4 のまま）。
4. **束縛の範囲**: 承認は「この task の実効 policy・この論理 session・この action・一回」に束縛される。browser process は
   run ごとに新しく、snapshot の `@eN` は再生できないので、agent が同じ origin に戻って snapshot を取り直し、承認された
   action を一度だけ出す。page の同一性（exact URL）は wait に凍結しない（allowed_domains と一回実行で束縛する）。
5. **経路**: daemon 経路（`browser.rs`）と launcher 経路（`browser_launcher_run.rs`）の両方で同じ helper
   （`read_approval_request` / `operation_wait` / `approved_operation` / `resumed_policy_bytes`）を使う。launcher 経路の
   `refuse_confidential` は、approved の wait が credential_use 以外の operation なら拒否しない（registered・
   credential_use の approved は従来どおり拒否）。
6. **監査**: `request-approval` は shim の `approval_request` event（`browser.approval_request: success`）と wait の
   `BrowserWaitOpened` に残る。承認後の click/download は control gate と session 記録に従来どおり残る（D4）。

### D3. 承認待ちの既定・上限は 30 分

`APPROVAL_WAIT_MAX_SECS = 30 * 60`。`ttl_secs` の clamp・`browser_waits_expire` の終端化・一度だけの失敗化は変えない。
登録待ち（`waiting_for_auth`、24 時間）は変えない。credential lease の TTL（既定 60 秒・上限 300 秒・承認の残期限
との最小値）も変えない。web・通知は `deadline`（絶対時刻）を表示しているので、表示側に固定の分数は無い。

### D4. 監査は変えない

承認なしで実行した click・download も、これまでどおり control gate を通って session 記録（agent action の開始・終了）と
`browser_session_live` の event に残る（何を・いつ・どの origin で）。本 ADR は記録の経路を変えない。

### D5. 文書

ADR-0080 D4/D5 の該当文は本 ADR が上書きする（ADR-0080 本文は書き換えず、本 ADR への参照を付ける）。
`docs/api/v1/gui-api.md` の期限と `BrowserCapability`/`BrowserSettingsPatch` の欄、`docs/ops/browser-department.md` §3
の `approval_actions`、`docs/SPEC.md` §3.6 に browser の承認の既定を 1 文足す。CoS の skill（`config/skills/cos-operator`）
には browser 承認の説明が無いので変えない。

## 3. 試験

- task-core `browser/policy_tests.rs`: 既定の policy で `requires_approval(Click/Download)` が偽・harness allow に
  click/download が入る・`approval_actions` は `{credential_use}`（credential_use 許可時）だけ。task policy か grant の
  `approval_actions` に click を入れると `requires_approval(Click)` が真で harness allow から click が消え、
  `harness_action_policy_with_approved(Click)` で戻る（credential_use・承認対象でない action は戻らない）。
  credential_use は task/grant が空でも常に `approval_actions` に入る。
- task-core `browser_wait/tests.rs`
  `operation_approval_opens_without_credential_consumes_once_and_expires_at_thirty_minutes`: credential 無しの
  操作 wait を開く → 未承認では消費不可 → `approve_once` → intent の食い違いは消費しない → 同じ run/session で
  一度だけ消費 → 30 分で一度だけ期限切れ（時計は引数）。
- task-worker `browser_tests.rs`
  `approval_request_is_bound_to_the_policy_and_resumes_only_under_the_same_policy`（`approval-request.json` の
  検証・wait の形・policy hash の束縛・prompt）、`click_approval_opens_a_wait_and_resumes_the_same_session_once`
  （CELERIS_USERNS_TESTS=1。run 1: click 拒否 → `request-approval` → `Question`・wait pending・task Blocked・
  substrate に click は届かない。承認 → run 2: 同じ session_id、click は 1 回だけ substrate に届き 2 回目は拒否、
  wait は `resumed`）。`browser_launcher_run_tests.rs` `single_use_action_reaches_the_launcher_once`。
- shim `scripts/tests/test_browser_cli.py`
  `test_request_approval_freezes_one_operation_and_approval_actions_hint`。
- task-core `browser_wait/tests.rs`: `APPROVAL_WAIT_MAX_SECS == 1800`、`ttl_secs` 無しの承認待ちの deadline が
  開いた時刻 + 30 分、`browser_waits_expire(now + 29 分)` は空・`now + 31 分` で一度だけ期限切れ（時計は引数で差し替え）。
- task-worker: action allow に無い click を action server が拒否する（既存の allow 判定）。
- web: `/browser/settings` の checkbox を入れて保存すると PATCH 本文に `approval_actions` が載る（e2e、fake daemon）。

## 4. 付記（実装突き合わせ、2026-10-08）

- `crates/task-core/src/browser.rs`: `ALWAYS_APPROVED`、`BrowserCapability.approval_actions`、`derive` の和集合、
  `harness_action_policy` の除外、`harness_action_policy_with_approved` / `operation_approval_actions`（D2）。
- `crates/task-core/src/browser_wait.rs`: `APPROVAL_WAIT_MAX_SECS`、`ConsumedBrowserOperation` /
  `consume_operation_approval`（D2.3）。
- `crates/task-worker/src/adapter.rs`: `EventSink::browser_operation_approval_consume`（`task-dispatch` の
  `StoreSink` が実装）。
- `crates/task-worker/src/browser.rs`: `ApprovalRequest` / `read_approval_request` / `operation_wait` /
  `approved_operation` / `shim_approval_actions` / `resumed_policy_bytes`、`BrowserContext.approval_actions` /
  `approved_operation`（worker protocol schema 再生成）、prompt、`approval-request.json` → wait、再開時の policy・
  session・消費。`browser_launcher_run.rs`: 同じ helper と `refuse_confidential` の緩和。
- `crates/task-worker/src/browser_action.rs`: `ActionServer::start(.., single_use, ..)`（一回だけの action）。
- `crates/task-worker/src/browser_cli.py`: `request-approval` verb、`config.json` の `approval_actions`、承認が要る
  verb の案内文、`approval_request` event（`forward_events` の許可 operation に追加）。
- `crates/task-api/src/handlers/org.rs`: `BrowserSettingsPatch.approval_actions`。schema（`docs/api/v1`、web の生成型）再生成。
- `web/features/browser/browser-settings-screen.tsx`: 「毎回の承認が要る操作」checkbox（文言は D2 の実挙動:
  承認待ち → 承認後に同じ session で一度だけ）。`web/e2e/support/fake-daemon.mjs` も `approval_actions` を受ける。
