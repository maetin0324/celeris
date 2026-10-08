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

### D2. 承認対象に戻した click・download は fail-closed

実効 policy の `approval_actions` に入った業務 action は **harness の action allow から外す**
（`harness_action_policy()`）。worker の action server はその action を allow 外として拒否し（既存の固定 code、
ページ本文を返さない）、実行しない。ADR-0080 D4 の「操作 intent を凍結して承認待ち（Blocked）にし、承認後に同じ
session で再開する」耐久の操作承認は、旧実装にも無く、本 ADR でも作らない（人の決定は「承認をやめる」であり、
戻す選択肢は fail-closed で残せば足りる）。必要になれば別 ADR で `operation` intent 付きの wait を設計する
（wait の列 `operation_intent_id` / `action` / `args_digest` は既にある）。

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
  `approval_actions` に click を入れると `requires_approval(Click)` が真で harness allow から click が消える。
  credential_use は task/grant が空でも常に `approval_actions` に入る。
- task-core `browser_wait/tests.rs`: `APPROVAL_WAIT_MAX_SECS == 1800`、`ttl_secs` 無しの承認待ちの deadline が
  開いた時刻 + 30 分、`browser_waits_expire(now + 29 分)` は空・`now + 31 分` で一度だけ期限切れ（時計は引数で差し替え）。
- task-worker: action allow に無い click を action server が拒否する（既存の allow 判定）。
- web: `/browser/settings` の checkbox を入れて保存すると PATCH 本文に `approval_actions` が載る（e2e、fake daemon）。

## 4. 付記（実装突き合わせ、2026-10-08）

- `crates/task-core/src/browser.rs`: `ALWAYS_APPROVED`、`BrowserCapability.approval_actions`、`derive` の和集合、
  `harness_action_policy` の除外。
- `crates/task-core/src/browser_wait.rs`: `APPROVAL_WAIT_MAX_SECS`。
- `crates/task-api/src/handlers/org.rs`: `BrowserSettingsPatch.approval_actions`。schema（`docs/api/v1`、web の生成型）再生成。
- `web/features/browser/browser-settings-screen.tsx`: 「毎回の承認が要る操作」checkbox。`web/e2e/support/fake-daemon.mjs`
  も `approval_actions` を受ける。
