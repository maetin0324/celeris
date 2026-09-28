# ADR-0078: 既存 harness に agent-browser execution capability を付与する

---
tasks: [01M3MBV3AKXZGEG5RXR60XC62J]
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

## D6. GUI と運用

Phase 1 は `BrowserRunsPanel` をtask/run画面へ追加し、現在のstate/sessionと **Open Browser Live View** を示す。live_urlはoperatorが設定したHTTPS dashboard base、現在activeなRUNNINGだけクリック可能。外部リンクに `noopener noreferrer` を付ける。未設定/完了時は理由を表示する。自前frame転送は作らない。

agent-browser dashboardは独立processでloopbackにbindし、同一namespaceの全sessionを表示する**管理者用画面**。sessionごとのauthorizationではない。MVPでは人が表示されたsession IDを選ぶ。upstreamの `?port=` deeplinkは将来使えるが、portを権限とみなさない。

operatorはCeleris用namespaceでdashboardを起動し、認証済みHTTPS reverse proxyとupstream exact allowed-originsを設定する。upstream初回fragment token/cookieはDB・event・LLMへ保存せず、人へ別経路でbootstrapする。proxyはcookiesをログから除外し、Celeris API tokenを別originへ転送しない。多人数/task別ACL環境へ共有dashboardをそのまま開放しない。

Phase 3 は既存WS `frame/status/tabs/url/console/event` を認証済みCeleris proxy経由で統合する。pause/takeover/resume/stopはharness実行状態とbrowser制御leaseを同期し、human/agentの二重操作を防ぐ。観測UIは復元可能なviewとし、監査記録を置き換えない。

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

Phase 2でcredential backend、leaseの承認頻度、認証区間の観測制限を選ぶ。既存vaultがあれば優先し、なければ専用1Password vaultを初期候補とする。Phase 3でproject/origin限定persistent identityの保存期間を選び、個人Chrome profileの共用は避ける。MVP運用はoperator専用dashboardを推奨し、多人数/task別ACLには後続proxyを必須とする。機密taskの前にcontainer/egress境界を評価する。各候補の比較材料はtaskの調査成果物に保持し、決定後の契約は後続ADRへ記録する。

## 検証とリリースの扱い

本ADRは検証成功を宣言しない。capability grant拒否、adapter選択、session分離、sentinel付きerrorの非露出、危険grammarとupstream action拒否、artifact登録、cleanup、GUI URL/lifecycleフィルタを検証する。実機public fixtureとfake substrateの成功は区別して報告する。リリースはADR-0040 D5に従い検証済みSHAを報告し、本番昇格は人がGUIで実施する。

## 一次情報

- [agent-browser v0.38.1](https://github.com/vercel-labs/agent-browser/releases/tag/v0.38.1)、[exact action policy](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/cli/src/native/policy.rs)、[action dispatch](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/cli/src/native/actions.rs)
- [Security](https://agent-browser.dev/security)、[Sessions](https://agent-browser.dev/sessions)、[Dashboard](https://agent-browser.dev/dashboard)、[Streaming](https://agent-browser.dev/streaming)
- [credential plugin protocol](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/docs/src/app/plugins/page.mdx)
- [OpenCode skills](https://opencode.ai/docs/skills)、[Claude Code skills](https://code.claude.com/docs/en/skills)
- [Browser Use browser parameters](https://docs.browser-use.com/open-source/customize/browser/all-parameters)、[sensitive data](https://docs.browser-use.com/open-source/examples/templates/sensitive-data)、[Cloud live preview](https://docs.browser-use.com/cloud/browser/live-preview)
