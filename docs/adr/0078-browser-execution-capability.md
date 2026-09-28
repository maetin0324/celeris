# ADR-0078: 既存 harness に agent-browser execution capability を付与する

---
tasks: [01M3MBV3AKXZGEG5RXR60XC62J, 01M3MFS5T52FXA63W4V10XGC4S, 01M3MZKB3DFYJNBH015MJGQ0BT]
---

- 日付: 2026-09-28
- 状態: **Accepted: Phase 1 MVP の境界と契約。Proposed: Phase 2〜4 の実装計画**
- 関連: ADR-0061（coding harness adapter）、ADR-0069（routing 四層）、ADR-0040 D5（検証済みリリースと人による昇格）

## 文脈

ブラウザ操作を既存 coding harness へ付与する。Celeris は browser agent、DOM 操作、クリック戦略、agent loop を自作しない。OpenCode を初期 harness、Claude Code を明示切替先として、Vercel agent-browser を共用する。将来 Codex や Browser Use へ routing を拡張できるが、browser を固定の worker/genre にしない。

既存 `WorkerAdapter`、`RunRequest/RunContext`、`EventSink`、task artifacts を接合面とする。OpenCode ACP と Claude Code は `claude_code::build_prompt` を共用するため、harness 外の supervisor で session と lifecycle を管理できる。

## D1. 採用範囲と責務

| 部品 | 担当 |
| --- | --- |
| OpenCode ACP / Claude Code | task推論、agent loop、次に行うブラウザ操作の判断 |
| agent-browser **0.38.1** | browser起動・DOM/ref・click・snapshot・download、action/domain policy、content boundaries、dashboard/stream |
| Celeris | capability grant/routing、task/run/session対応、既存 policy 形式の生成、秘密を受け取らない監査、成果物登録、終了処理、GUI導線 |
| 将来 CredentialBroker | provider認証、origin/task/session/TTLに束縛したlease、承認・失効・audit。DOM操作はagent-browserに委ねる |

基準 source は release tag commit `aff6125c023b810ea3f2e5deec5379e9a4270bdc`。起動時に `agent-browser --version` を確認して未対応版を拒否する。Phase 1 は local trusted harness の**公開・未認証ページ**を対象とする。browser auth、persistent profile、秘密の入力、remote/container capability run は未実装である。

Celeris CLI shim は固定 command grammar の転送と監査だけを行う。ref 解決や navigation 戦略を再実装しない。任意 flags/path/eval/auth を受け付けず、操作の可否は agent-browser の policy を併用する。

### D1 補足: 固定版と調査の証拠範囲

2026-09-28 に手元の npm 配布物 `agent-browser/package.json` と native CLI `--version` で **0.38.1** を確認した。同梱 `README.md`、`skill-data/core/references/{session-management,streaming,trust-boundaries}.md`、`plugin --help` を契約確認に使う。配布物のソースは JS launcher と導入スクリプト等であり、native Rust 本体は同梱されていない。上記 tag commit と native binary の再現ビルド一致は未確認。D4/D7 の内部 policy 評価の注意は既存検証に基づく前提として維持し、更新時には同梱文書だけで安全性を判定せず負例を再実行する。

ローカル CLI で OpenCode **1.18.31** の ACP/run/session/skill・plugin コマンド、Claude Code **2.1.283** の print/stream-json/session/skills/MCP・plugin オプションを確認した。両者の会話 session と browser auth state は別物であり、browser session の分離は Celeris と substrate の責務とする。Browser Use の配布物・一次資料は手元に見つからず、その skill/CLI、auth state、policy、stream、credential provider の互換性は**未確認**。これを Phase 1 の依存や安全性根拠にしない。

agent-browser 自身にも `chat`/dashboard AI Chat があるが、採用する loop は既存 harness のものだけである。同梱 skill の広い操作例（fill、cookie import、restore 等）は MVP の許可ではなく、Celeris の制限された CLI 契約が優先する。調査表・候補比較は当該 WorkUnit の成果物に置き、本節は採用契約の証拠範囲だけを記録する。

## D2. Capability と routing 契約

Phase 1 の公開 schema は `task-core/src/browser.rs`:

```json
{
  "browser": {
    "allowed_domains": ["example.com", "*.example.org"],
    "live_view_url": "https://browser.example.org/"
  }
}
```

`Profile` と `EffectiveProfile` に optional `browser` を追加する。grant は組織管理者が与え、ancestry から継承、子 profile で grant 全体を置換する。`allowed_domains` は空不可、明示 host または先頭 `*.` のみ。`live_view_url` は optional であり、credentials/query/fragment のない HTTPS dashboard URL に限る。HTTPS 構文は reverse proxy の認証設定を保証しない。

task の `skills: ["browser-enabled"]` は能力の**要求**であり grant ではない。`matching::decide` は有効な grant を持つ node のみ候補とする。`add::build_task` は既定 adapter を `acp`、明示 `claude-code` を許可し、未対応 adapter を拒否する。capability を失う silent fallback はしない。worker 起動時にも profile と adapter を再検証する。

profile/browser-specialist は通常の org profile の拡張点として残す。Phase 4 の backend selector は capability の実行要件と harness の提供能力を照合し、browser専用 backend も同じ task/run/event 契約に接続する。

### D2 補足: 将来 backend の適合契約（Proposed）

browser-specialist は通常の org profile として `browser-enabled`、長時間実行予算、許可 origin、利用可能 harness を管理者が設定する。profile 名だけでは credential grant を与えない。将来の selector は `isolated_session / action_policy / network_policy / credential_lease / human_control / live_view` の要求と backend が検証済みの能力を照合する。Browser Use 等は既存 agent loop を含む backend として `WorkerAdapter` に接続し、二重に loop を実装しない。不足能力があれば routing を拒否し、fallback 時も同じ制約と新しい run/session を要求する。

## D3. Session、state、task lifecycle

```json
{
  "task_id": "<TaskId>",
  "run_id": "<execution/run-id>",
  "session_id": "celeris-<task/run hash>",
  "state": "RUNNING",
  "live_view_url": "https://browser.example.org/"
}
```

`BrowserRun` は DB credential record を持たず、既存 `Event::BrowserUpdated { browser }` から再構成する。session ID は task ID と run ID から一意に生成し、retry/parallel execution は別 session。`--session` で独立 browser を起動、`--restore`, `--state`, `--profile`, CDP attach を指定しない。run-private config を `--config` で明示し home/cwd 自動設定を避け、subprocess の inherited `AGENT_BROWSER_*` を除去して Celeris namespace を指定する。

| State（公開 enum） | Phase 1 の意味 / 後続 |
| --- | --- |
| RUNNING | harness実行中の capability。browser は最初の操作で遅延起動され得る |
| WAITING_FOR_AUTH | schema予約。Phase 2でbroker grant/auth待ちを永続化 |
| WAITING_FOR_APPROVAL | schema予約。Phase 2で承認requestを永続化 |
| WAITING_FOR_HUMAN | harnessが既存 `Terminal::Question` を返した状態。MVPではbrowserを閉じ、通常のtask人間待ちに委ねる |
| COMPLETED | harnessが `Terminal::Done` へ終了し、session close の成功を確認 |
| FAILED | その他のterminal/error。session終了処理を行う |

MVP の waiting は live browser の持ち越しを保証しない。再開runは新session。正常終了は `close`、cancel/drop も session固有 cleanup を試みる。close失敗は非retryable errorとFAILEDへ写し、upstream idle timeoutを5分に設定する。daemon/host crash後の全session reconcile、durable lease/wait、pause/resume は後続対象。GUIはrunのactive状態も照合し、古いRUNNING eventだけでLive Viewを表示しない。

Phase 2 は外部待ちの意図と期限をdurable recordへ保存し、worker slotを解放して再投入する。upstream confirmationには短いtimeoutがあるため、承認待ちの間ずっとpending CLIを保持せず、Celeris承認後に新しい操作/leaseを開始する。retryはidempotency keyで重複実行を防ぐ。

### D3 補足: durable wait と状態遷移（Proposed / Phase 2 以降）

`BrowserWait { wait_id, task_id, run_id, session_id, reason, approval_id?, credential_ref?, deadline, resume_key, policy_revision }` を制御側に保存し、秘密・ページ本文を入れない。browser の存続期限と worker slot の占有は別に管理する。

| 遷移 | 条件と副作用 |
| --- | --- |
| RUNNING → WAITING_FOR_AUTH | broker の認証・unlock・MFA が必要。observation と agent 操作を止め、期限付き wait を保存 |
| RUNNING → WAITING_FOR_APPROVAL | task policy が要求する操作単位の承認が未取得。対象操作と policy revision を束縛 |
| RUNNING → WAITING_FOR_HUMAN | 質問または takeover が必要。制御 lease を取り上げてから人へ渡す |
| WAITING_* → RUNNING | 正当な回答/承認、期限内、最新 policy と origin の再検証、単一 controller の取得が全て成功 |
| RUNNING → COMPLETED | harness 完了と cleanup 成功を確認 |
| RUNNING / WAITING_* → FAILED | 拒否・待機期限切れ・cancel・回復不能 error。lease 失効、browser cleanup、理由は固定 code |

Phase 1 の question は既存 D3 のとおり browser を閉じる。後続で同一 session を再開できるのは生存・排他・policy を確認した場合だけで、失われていれば新 run/session を作り承認を取り直す。COMPLETED/FAILED を直接 RUNNING に戻さない。副作用を送信済みか不明な crash は idempotency key だけで安全に再送できないため、照合または人間確認を要する。wait の期限到達・再起動時は reconciler が一度だけ終端化し、承認通知の重複は `resume_key` で排除する。

## D4. Policy、events、artifacts

MVP は admin profile grant と task のbrowser要求から upstream action policy を生成する。一般task policyとの細かな制約交差はPhase 2。agent-browser 0.38.1 はdocsのcategory表と異なり**内部action名を完全一致**で評価する。MVP は必要な非空allowlistだけを生成する:

```json
{
  "default": "deny",
  "allow": ["launch", "navigate", "click", "snapshot", "gettext", "screenshot", "download", "scroll", "close"]
}
```

`evaluate`, `waitforfunction`, `cookies_get`, `storage_get`, `state_save`, `auth_login`, `cdp_url` 等は許可しない。CLI shim も引数の固定形式だけを認める。`--allowed-domains` と `--content-boundaries`、出力量上限を毎回渡す。

navigation と subresource は異なる policy 意図を持つが、upstream は一つの domain 集合で両方を制限する。MVP は共通集合を使い、CDN等は人が明示追加する。ページの指示で権限を拡張しない。Phase 2 の policy案は `navigation_origins` と `network_domains` を別欄にし、Phase 4 の egress と整合させる。unionだけを渡して navigation が分離できたとは主張しない。

操作監査は既存 Progress/ToolResult を再利用し、`browser.navigate/click/extract/download/screenshot/scroll/close/policy_block` と `success/failure/blocked` のみを記録する。URL、query、入力値、stdout/stderr、page content、raw errorはeventへ保存しない。browser専用sinkで汎用harnessのprogress/comment/delegate/任意artifactを遮断し、ACP/Claudeのraw stream記録と反射されたRPC/error本文の保存も抑止する。通常の最終summaryと公開page artifactsの任意内容のsecret検出は保証しない。`BrowserUpdated` はtask/run/session/stateとtrusted dashboard baseだけ。将来の追加event案:

```text
BrowserEvent {
  task_id, run_id, session_id, sequence, at,
  kind: navigate|click|extract|download|policy_block|auth_request|
        credential_use|approval|human_wait|resume|stop,
  status: requested|success|failure|blocked,
  policy_id?, credential_id?, lease_id?, approval_id?, artifact_id?
}
```

将来も引数・secret・生のpage errorをこのeventへ入れない。phase2のauditはactorとpolicy decisionを追加し、live viewではなくevent + artifactsをsource of truthとする。

screenshots、抽出JSON、downloadはrunごとのtask artifacts下へgenerated nameで保存し、既存 `artifact::resolve` と `EventSink::artifact` を通す。任意パスとsymlinkを受け付けない。抽出結果にはuntrusted表示を付ける。

**秘密が存在しないという一般保証ではない。**公開ページでも文字列・画像・downloadに秘密やPIIが含まれ得る。Phase 1 の保証範囲はcredential入力経路を設けないこと、秘密参照だけのcontract、危険操作の既定拒否、操作eventへ任意内容を転記しないこと。DOM/screenshotの任意秘密除去、認証後の画面への秘密再表示防止は未実装である。

### D4 補足: task policy の生成と監査契約（Proposed）

```text
BrowserTaskPolicy {
  policy_id, revision,
  navigation_origins: [HTTPS origin],
  network_domains: [host pattern],
  allowed_actions, approval_actions,
  credential_policy_ids, artifact_policy_id
}
EffectiveBrowserPolicy = admin grant ∩ task policy ∩ backend supported actions
```

ページ・モデルが提案する task policy は grant を狭める要求に限り、広げる変更は認証済み管理者の別経路で行う。生成器は不明 action/壊れた schema/空の実効許可集合を起動前に拒否し、版ごとの内部 action 名へ写像する。deny は allow/承認より優先し、承認は deny を解除しない。生成 policy の revision/hash を run と承認へ結び付け、変更後の古い承認を再利用しない。cleanup に必要な close は agent の業務操作とは分けて supervisor が保持する。

navigation は top-level・iframe・popup・redirect ごとに origin を検証し、subresource/network は fetch・画像・script・WS 等の通信先を別に制限する。CDN を通信先に追加しても navigation は許可しない。現行 upstream の単一 allowlist の union ではこの意味を実現できないため、P2-A は既存の enforcement 接点で分離できることを先に検証する。できなければ共通集合の MVP を維持し、分離を必要とする task は拒否して P4-A に依存させる。DNS/IP やポートの制約を origin/host パターンで代用しない。

将来 event は D4 の参照フィールドに加え `schema_version, actor_id, decision_code, policy_revision` を持つ。broker は grant/use/deny/expire/revoke を永続監査し、秘密取得前に記録不能なら拒否する。`sequence` は task の event 順序で、browser の stream sequence とは分ける。live URL/query、console、frame、credential 値は監査 event へコピーしない。認証後 artifact は policy に従って capture を止めるか保留領域へ置き、閲覧 ACL・保管期限・削除を適用する。

## D5. CredentialBroker（Proposed / Phase 2）

agent-browser はout-of-process `agent-browser.plugin.v1` の `credential.read` / `credential.resolve` を持つ。Celeris broker adapter はこの接点を使い、provider SDKをbrowser coreへ追加しない。

```text
CredentialRef { credential_id, provider, policy_id }
CredentialPolicy {
  policy_id, allowed_origins: [exact HTTPS origin],
  allowed_task_scopes, require_human_approval, max_lease_seconds,
  allow_persistence: false
}
CredentialLease {
  lease_id, credential_id, provider, policy_id,
  task_id, run_id, session_id, origin,
  approved_by, approval_id, issued_at, expires_at,
  max_uses: 1, status: granted|used|expired|revoked,
  idempotency_key
}
CredentialUseResult { success: boolean, failure_code?: enum }
```

Celeris DB/context/eventは上記参照だけを持つ。brokerはpeer認証済みIPCとtask/session/originの照合後にproviderへ問い合わせる。秘密はbroker→agent-browser credential plugin protocolだけを通し、LLMへsuccess/failureのみ返す。raw password/TOTP secretをconfig/args/DBに置かない。providerは1Password/Bitwarden/専用手動登録等を差替え可能にする。Google Password ManagerのJIT APIは前提にしない。

originはscheme/host/effective portの完全一致、redirect時再確認、expiry/cancel/revokeで失効する。credential lookup名とページ内容だけで新しいgrantを発行しない。承認主体は認証済み人または既存の明示task policyで、Webページではない。

plugin/coreのredaction機構を再利用するが、入力後の秘密はページやCDPから再取得され得る。認証区間のobservation停止・再開、screenshot漏洩試験をPhase 2に含め、より強い要件ではPhase 4のbroker→extension/injectorによるE2E injectionを評価する。

### D5 補足: celeris-credentiald と CredentialProvider 抽象（Proposed）

broker の仮称を **celeris-credentiald** とし、harness とは別 process/権限で配置する。1Password、Bitwarden、手動登録は次の同じ内部 interface を実装する。特定 vendor の SDK や API が利用可能という確認は未了で、Google Password Manager は必須にしない。

```text
CredentialProvider {
  capabilities() -> { interactive_unlock, totp, revoke, persistent_session }
  resolve(CredentialRef, AuthorizedLeaseContext) -> SecretEnvelope | NeedsHuman | ErrorCode
  revoke(lease_id) -> ResultCode
}
AuthorizedLeaseContext { lease_id, task_id, run_id, session_id, exact_origin, expires_at }
SecretEnvelope { username?, password?, otp? } // broker/plugin 専用、保存・LLM 出力不可
```

lease 発行/承認/TTL/uses/origin/audit は broker が担当し、provider は vendor 認証と取得だけを担当する。provider の revoke が未対応でも broker は即時に以後の使用を拒否する（既に発行されたサイト側 cookie の失効とは別）。手動登録は認証済み人の専用 UI/IPC で受け付け、task 入力や LLM 会話へ秘密を渡さない。MFA の人間待ちは WAITING_FOR_AUTH とし、TOTP seed は provider 側に留める。

upstream は `credential.read` capability と `credential.resolve` request を持つが、task/run/lease の Celeris 用認証を提供するわけではない。固定 plugin bridge が peer 認証済み broker IPC を呼び、制御側発行の session binding を参照する。モデルが渡した item 名・URL・lease_id 単体を権限証明にしない。取得直前と注入直前の現在 origin、および redirect/iframe の対象 origin を trusted browser 側で照合できることを P2-B の受け入れ条件とする。upstream plugin に必要な情報/原子性が無ければ broker-only で解決したとせず、認証利用を保留して P4-B に依存させる。

secret は broker→plugin→browser の短命メモリ経路だけに限定し、プロセス引数・環境・一時ファイル・trace・core dump に出さない。通常の LLM-facing 結果は `CredentialUseResult` のみとする。ただし同一 UID からのメモリ/IPC/CDP 取得への強い隔離は Phase 4 が前提であり、この interface だけでは成立しない。

## D6. GUI と運用

Phase 1 は `BrowserRunsPanel` をtask/run画面へ追加し、現在のstate/sessionと **Open Browser Live View** を示す。live_urlはoperatorが設定したHTTPS dashboard base、現在activeなRUNNINGだけクリック可能。外部リンクに `noopener noreferrer` を付ける。未設定/完了時は理由を表示する。自前frame転送は作らない。

agent-browser dashboardは独立processでloopbackにbindし、同一namespaceの全sessionを表示する**管理者用画面**。sessionごとのauthorizationではない。MVPでは人が表示されたsession IDを選ぶ。upstreamの `?port=` deeplinkは将来使えるが、portを権限とみなさない。

operatorはCeleris用namespaceでdashboardを起動し、認証済みHTTPS reverse proxyとupstream exact allowed-originsを設定する。upstream初回fragment token/cookieはDB・event・LLMへ保存せず、人へ別経路でbootstrapする。proxyはcookiesをログから除外し、Celeris API tokenを別originへ転送しない。多人数/task別ACL環境へ共有dashboardをそのまま開放しない。

Phase 3 は既存WS `frame/status/tabs/url/console/event` を認証済みCeleris proxy経由で統合する。pause/takeover/resume/stopはharness実行状態とbrowser制御leaseを同期し、human/agentの二重操作を防ぐ。観測UIは復元可能なviewとし、監査記録を置き換えない。

### D6 補足: stream と制御 API の役割（Proposed / Phase 3）

同梱 `streaming.md` で確認できる server message は `frame/status/tabs/url/console`、client message は mouse/keyboard/touch/config/ack である。一般化された `event` message の wire schema はこの資料では**未確認**。D6 の将来 UI にある event は Celeris の永続 event feed で供給し、upstream activity feed の転送を監査の代用にしない。

proxy は接続時と再接続時に task/run ACL を確認し、ブラウザへ raw CDP/port/token を公開しない。frame・URL・console は機密となり得るため認証区間で停止し、通常監査ログに保存しない。pause は新しい agent 操作を停止して実行中操作の収束を確認、takeover は期限付き controller lease を人へ移譲、resume は lease を回収して fresh snapshot と policy/origin を再確認、stop は task cancel と lease 失効/cleanup を同期する。制御要求には expected state/version と idempotency key を付け、切断だけで agent を自動再開しない。これらは upstream の同名コマンドの存在を仮定した API ではなく Celeris の制御契約である。

## D7. Threat model と限界

| 脅威 | Phase 1 / 残る課題 |
| --- | --- |
| Web prompt injectionでgrant拡張 | content boundariesとscope命令、admin grant、既定deny。LLMが従うだけではhard boundaryにならない |
| raw CDP/eval/cookie/storage経由の秘密 | wrapperの固定grammar、empty explicit config、env清掃、非空exact policy。任意shellの別経路は同一UIDでは防げない |
| 外部domainへの流出 | upstream allowlistを利用。navigation/subresource共通集合、JS系制限はbest-effort、OS firewallではない |
| credential のDB/log露出 | MVPでcredential入力なし、eventは固定metadataのみ。page/artifact自体の秘密は保証外 |
| config/policy改ざん | run-private fileと予測困難なsession、既定mode制限。同一UIDの悪意あるworkerは改ざん可能、Phase 4でread-only mounts/別UID/IPC分離 |
| upstream policyの欠陥 | 0.38.1 は空allow配列がpermissive、invalid policy loadがfail-open。非空valid fileを生成しnegative test、version pin。権限拡張前にupstream修正確認 |
| dashboard経由の他task制御 | operator専用namespace/proxy、全session画面であることを明示。Phase 3/4でtask別authorization |
| stale/orphan browser | normal/drop cleanupとGUI active照合。host crash・悪意あるprocessの完全reapingはPhase 4 |
| download / screenshot | generated path、登録時検証、サイズ制限。内容はuntrusted、downloadの実行を自動化しない |

### D7 補足: prompt injection と container/egress の境界

ページ本文に加え aria/ref label、download、console、error、画像中の文字も untrusted と扱う。content boundary は由来の表示であり検出器や権限制御ではない。注入された「policy を変更」「別 task の session を使う」「秘密を送る」という指示は拒否し、固定 code の policy_block のみ監査する。ページ由来の要求で credential 承認 UI や shell tool の権限を拡張しない。許可サイト自体への情報送信や destructive click は domain 制限だけでは防げず、後続の操作単位承認を要する。

P4-A は worker と browser を task 単位の隔離 runtime に置き、broker/control-plane を別 UID/namespace に分離する。host network、host home、個人 browser profile、container socket、他 task artifacts を mount しない。policy/config は worker から read-only、書込みは run scratch と検査対象 artifacts だけに限定する。UID、mount、process/IPC、CPU/メモリ/存続期限も制約する。

egress は既定拒否とし、許可 proxy/DNS 経由だけに固定する。直接 IP、loopback/private/link-local/metadata endpoint、IPv6、DNS rebinding、redirect、WebSocket、QUIC/WebRTC 等の bypass を負例に含める。worker の shell 通信にも同じ制約を適用し、container 化だけで network 制約済みとしない。broker socket は認可 bridge だけへ、CDP/stream は制御側だけへ露出する。private service を対象にする例外は origin と接続先を管理者が別途指定する。隔離の実装選択は後続だが、この境界を満たさない runtime は機密 task に routing しない。

## D8. 小さな後続タスクと依存関係

| ID / Phase | 前提 | 成果と単独検証 |
| --- | --- | --- |
| P1-A grant/routing | 現状調査 | profile capability、acp既定/Claude明示、未許可profile/adapterの負例 |
| P1-B lifecycle/CLI | P1-A | isolated session、exact policy、env/config隔離、events/artifacts、fake+real substrate smoke |
| P1-C dashboard導線 | P1-B | task/run panel、安全なURL、running-only、browser表示確認 |
| P1-D release | P1-A〜C | workspace fmt/test/clippy、GUI typecheck/test、ADR-0040 release/verify、検証済みSHA。昇格は人 |
| P2-A policy契約 | P1-D | task policyとadmin grantの交差、navigation/network別意図、prompt注入による拡張拒否試験 |
| P2-B broker 1 provider | P2-A + backend選択 | CredentialRef/lease/provider API、origin/TTL/revoke、raw secretをmodel/eventへ出さないsentinel試験 |
| P2-C durable approval/auth wait | P2-B | wait永続化、worker解放、restart/retry/idempotency、認証区間のobservation制限 |
| P3-A Browser Identity | P2-C + persistence方針 | project/origin境界、暗号化/期限/失効、他identity混入試験。allowlist/state非互換の代替境界を先に確認 |
| P3-B live view proxy | P1-C + task別ACL設計 | upstream WSの認可relay、frame/status/tabs/url/console/event、切断/再接続/他task拒否 |
| P3-C takeover | P2-C + P3-B | pause/takeover/resume/stop、制御lease排他、double-action防止 |
| P4-A isolated runtime | P2-A | container/別UID/read-only policy、egress proxy/firewall、直接CDP/IPC/任意shell bypassの負例 |
| P4-B stronger injection | P2-B + P4-A | broker→extension/injector E2E、LLM/worker側のsecret取得不能を攻撃試験 |
| P4-C backend routing | P1-D + capability conformance suite | Codex/Browser Use/browser-specialist候補、同一fixture比較、能力を失わないfallback |

認証とpersistent identityを機密taskで使う場合はP4-Aの前倒しが必要になり得る。全Phaseを一括で同じworkerへ渡さず、表の成果単位で起票する。

## 人の決定事項

2026-09-28 の人の決定により、Phase 2 の credential provider は**手動登録から開始する**。以前の「既存vault優先／専用1Password vaultを初期候補」は、この開始方針で置き換える。CredentialProvider 抽象を残し、外部vaultは後から追加できるようにする。

LLM が credential を必要としたら、**どのサイトで何のために必要かを示して人へ登録依頼を出す**。人は Celeris Web GUI の専用画面から登録する。秘密値を会話や task の回答へ渡さない。credential 使用と高リスク操作は人の承認を挟む。

**Live View の導線は本人の認証済み session だけに限定する**。他の利用者や外部から全session dashboardへ届かないことを、表示制御だけでなく接続経路でも確かめる。

上記の人の決定を具体化する [ADR-0080](0080-browser-phase2-policy-broker-approval.md) を Phase 2 の実装契約とする。同 ADR は D3/D4 補足・D5 と補足・D6・D8 P2-A〜C の対応部分を更新し、policy生成、暗号化保存、leaseと承認頻度、Blockedへの写像、owner sessionと保護導線を定める。これは実装・実機検証の完了宣言ではない。

Phase 3でproject/origin限定persistent identityの保存期間を選び、個人Chrome profileの共用は避ける。多人数/task別ACLには後続の認証・proxyを必須とする。機密taskの前にcontainer/egress境界を評価する。各候補の比較材料はtaskの調査成果物に保持し、決定後の契約はADRへ記録する。

## 検証とリリースの扱い

本ADRは検証成功を宣言しない。capability grant拒否、adapter選択、session分離、sentinel付きerrorの非露出、危険grammarとupstream action拒否、artifact登録、cleanup、GUI URL/lifecycleフィルタを検証する。実機public fixtureとfake substrateの成功は区別して報告する。リリースはADR-0040 D5に従い検証済みSHAを報告し、本番昇格は人がGUIで実施する。

## 一次情報

- [agent-browser v0.38.1](https://github.com/vercel-labs/agent-browser/releases/tag/v0.38.1)、[exact action policy](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/cli/src/native/policy.rs)、[action dispatch](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/cli/src/native/actions.rs)
- [Security](https://agent-browser.dev/security)、[Sessions](https://agent-browser.dev/sessions)、[Dashboard](https://agent-browser.dev/dashboard)、[Streaming](https://agent-browser.dev/streaming)
- [credential plugin protocol](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/docs/src/app/plugins/page.mdx)
- [OpenCode skills](https://opencode.ai/docs/skills)、[Claude Code skills](https://code.claude.com/docs/en/skills)
- [Browser Use browser parameters](https://docs.browser-use.com/open-source/customize/browser/all-parameters)、[sensitive data](https://docs.browser-use.com/open-source/examples/templates/sensitive-data)、[Cloud live preview](https://docs.browser-use.com/cloud/browser/live-preview)
