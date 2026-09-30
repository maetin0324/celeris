# ADR-0091: 本番 H3 経路 — controller 所有 Chromium/CDP の共有と trusted selector の出所・検証

---
tasks: [01M3RVA7P40BFPNCKY362WC243]
---

- 日付: 2026-09-30
- 状態: Accepted（設計。実装・実 browser での e2e は後続 unit（selector / shared-cdp / h3-wire / e2e）が出す。この ADR 自体は受け入れの証拠にならない）
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md) H3・監査、[ADR-0084](0084-browser-phase4-isolation-injection-routing.md) D1/D3/D6、
  [ADR-0085](0085-browser-phase4-runtime-selection.md) 3〜5、[ADR-0088](0088-browser-p4a-relay-supervisor-launch-restore.md) D2、
  [ADR-0089](0089-browser-p4b-injection-ipc-cdp-sink.md) D1〜D6

## 文脈

前回の wire unit（71640d79、wire-harness で取り込み済み）で、injection-only IPC（`injection.sock`）・control の
`RegisterLiveSession`/`OpenAuthSection`/`CloseAuthSection`・`CdpController::inject`・`same-uid-harness` admission は
試験 harness 上で結線された。本番の H3 経路（`task-worker/src/browser.rs` の credential segment）は次の 2 点で止まっている。

1. **同じ page に届かない**: runtime は `cdp_pipe: false` で `sandboxd python3 /session/browser_action.py` を起こし、
   `browser_action.py` が毎回 `agent-browser` を呼ぶ。agent-browser は自前の Chromium を起こすので、controller が
   fd 3/4 で持つべき CDP pipe はそもそも作られず、`CdpController::inject` の宛先が無い。現行の segment は
   `browser_credential::use_credential`（`auth login --credential-provider` + 旧 plugin `bridge`）のままで、
   これは ADR-0085 4 により固定失敗する。
2. **selector の出所が無い**: ADR-0080（L96）は「ログイン URL と top-level selector は管理者の site policy の値で、
   モデルは指定できない」とするが、`CredentialPolicy`（celeris-credentiald）にも `ConsumedBrowserApproval`（task-core）にも
   URL・selector の欄が無い。`InjectionRequest.selector` は今は呼出し側が自由に入れられる。

加えて、固定版 agent-browser 0.38.1 の `--help` は `--allowed-domains` が「CDP・auto-connect … を拒否する」と明記している
（`--allowed-domains <list> Restrict network domains; rejects CDP, auto-connect, …`）。agent-browser を外部 CDP に繋ぐなら
`--allowed-domains` は使えず、ドメイン制限を別の層に移す必要がある。

## 決定

### D1. controller 所有の 1 つの Chromium を共有する（a）

**構成**

- supervisor は runtime を `cdp_pipe: true` で起こす。sandbox の init（`sandboxd`）が 2 つの子を起こす:
  1. `chrome-headless-shell --remote-debugging-pipe --proxy-server=http://127.0.0.1:3128 --proxy-bypass-list=<-loopback> …`
     — fd 3/4 はこの process にだけ継承させる（他の子では CLOEXEC）。controller（daemon 内 supervisor thread 側）が pipe の
     反対側を持つ唯一の CDP 当事者で、`CdpController` はこの pipe の上で動く。
  2. `python3 /session/browser_action.py` — 従来どおり agent-browser の CLI を呼ぶが、agent-browser に
     **自前の Chromium を起こさせない**。常に `--cdp ws://127.0.0.1:<relay_port>/<token>` を付けて controller の中継へ繋ぐ。
     `--executable-path`・`--auto-connect`・`--profile`・`--state`・`--restore` は使わない（ADR-0084 D2 のまま）。
- **中継（CDP relay）**: controller は session dir の controller 専用 dir に unix socket を作り、sandbox へ bind する。
  sandbox 内では `sandboxd` が sandbox の netns の `127.0.0.1:<relay_port>` で listen し、接続ごとの byte 列を解釈せず
  その unix socket へ転送するだけにする。WebSocket の upgrade・CDP message の解析・許否はすべて controller 側で行う。
  - 受け付けるのは path が `/<token>`（session ごとに 256 bit の乱数、agent-browser の argv だけで渡す）で、
    `Origin` header の**無い** upgrade だけ。page 由来の WebSocket（必ず `Origin` が付く）と token 不一致は upgrade 前に切る。
  - Chromium は `--proxy-bypass-list=<-loopback>` で loopback も filtering proxy に送るので、page からは relay port に
    そもそも届かない（proxy は loopback を拒否する。ADR-0084 D1 の egress 検査）。token・Origin 検査はその上の二重化。
  - ADR-0084 D1 の「CDP は pipe か controller 専用 dir の unix socket、TCP は loopback でも拒否」は **Chromium の debugging
    endpoint** についての要件で、これは pipe のまま満たす（`RuntimeFacts` の CDP 事実は Pipe）。relay port は Chromium の
    endpoint ではなく controller の検査を必ず通る agent 用の口であり、`verify_isolation` の検査を緩めない。
    facts 採取が relay port を「TCP CDP」と判定する場合は、検査ではなく facts の記述（Chromium の listen socket だけを見る）を
    直し、その変更を shared-cdp unit の試験で示す。
- **CDP の多重化**: relay は agent 接続の command id を controller 側の id 空間へ写像し、agent が
  `Target.attachToTarget`（flatten）で得た sessionId だけを agent に使わせる。controller 自身の session（注入用）の
  sessionId を agent が指定した message は拒否する。`CdpController` の予約 id（ADR-0089 D4-1）は agent から見えない。
- **ドメイン制限の移し替え**: `--cdp` を使うので agent-browser の `--allowed-domains` は渡さない。代わりに
  (i) filtering proxy の egress allowlist（既存。正本）、(ii) relay が `Page.navigate`・`Target.createTarget`・
  `Page.navigateToHistoryEntry` の URL を policy の許可 domain（`https://` のみ）で検査して拒否、
  (iii) controller が `Fetch.enable` で許可外 host への document/subresource 要求を `Fetch.failRequest` する — の三層にする。
  `--action-policy`・`--content-boundaries`・`--max-output` は従来どおり渡す。固定版で `--cdp` と `--action-policy` の
  併用が拒否される場合、shared-cdp unit は止まって PROGRESS に記録する（`--allowed-domains` を戻す・版を上げるで回避しない）。

**認証区間中の遮断（H3）**

- `OpenAuthSection` の前に relay を **遮断状態** にし、`CloseAuthSection` の後で解除する（D3 の順序）。遮断中は:
  - agent 接続からの command は method を問わず転送しない（CDP error `auth_section_active` を返す）。
    ADR-0089 D4-6 の列挙（screenshot・screencast・DOM 読み出し・`Runtime.evaluate`/`callFunctionOn`・`Accessibility.*`）を
    包含する全面遮断で、`Input.*` も通さない（区間中に agent が入力する正当な理由は無い）。
  - browser → agent の event は **転送せず、溜めずに捨てる**（console・`Log.entryAdded`・`Runtime.exceptionThrown`・
    `Network.*`・`DOM.*`・`Page.screencastFrame` を含む全部）。遮断解除後に過去 event を再送しない。
  - 新しい agent 接続の upgrade は区間が閉じるまで受け付けない（503 で切る）。
- 区間を跨ぐ持ち越しの遮断（区間外でも常に）:
  - agent へ転送する `Network.requestWillBeSent` 等から `request.postData`・`postDataEntries`・`hasPostData` を削る。
  - 区間中に controller が観測した `requestId` を記録し、区間後に agent が `Network.getRequestPostData`・
    `Network.getResponseBody`・`Fetch.getResponseBody`・`Network.replayXHR` でそれらを指したら拒否する。
  - `Tracing.*`・`Page.startScreencast` の区間を跨いだ継続は区間を開く時点で controller が停止させる。
- 区間の終わり: `CdpController::close_auth_section` が、注入した要素が同じ document（frame id + loader id 一致）に残っていれば
  isolated world から値を空にしてから、`CloseAuthSection` → relay 解除の順に進む（D3）。消去に失敗したら区間を閉じずに
  session を破棄する（fail closed、観測は再開しない）。区間後の観測は ADR-0089 D4-7 の `RedisplayGuard` を通す。

**却下**

- agent-browser に自前の Chromium を起こさせ、controller が別経路で同じ page に触る — 同じ browser に届く口が無い（現状の障害）。
- Chromium の debugging endpoint を TCP（`--remote-debugging-port`）で開き agent-browser と controller が両方繋ぐ —
  ADR-0084 D1 違反で、区間中の遮断を controller が強制できない。
- 区間中の event を溜めて区間後に流す — 溜めた event に秘密が入りうる（console・入力 event）。
- agent-browser を sandbox の外で動かす — agent 側の process を隔離の外に出すことになる。

### D2. trusted selector の出所・固定・照合・形式検証（b）

**出所**

- ログイン URL と selector は管理者が broker に登録する `CredentialPolicy`（site policy）の欄だけに持つ:
  `login_url`（必須）、`password_selector`（必須）、`submit_selector`（任意）。`revision` を上げずに値を変えられない。
  `username_selector` はこの ADR の範囲では持たない（下の「未解決」）。
- モデル・worker・harness の要求（`browser.credential_use` の引数、action request、`InjectionRequest` を組む入力）には
  URL・selector の欄を置かない。既存の `deny_unknown_fields` で、要求に混ぜれば `invalid_request` で落ちる。
- Celeris 側は broker の control に非秘密の要約を問う（`DescribePolicy { policy_id }` → `{policy_id, revision,
  exact_origin, login_url, password_selector, submit_selector}`。秘密・username は含まない）。control は既存どおり daemon の
  PID + starttime からだけ受ける。

**固定（pin）**

- 承認要求（`WaitingForApproval` の wait）を作る時点で `DescribePolicy` の値を `TrustedLogin`
  `{policy_id, revision, login_url, password_selector, submit_selector}` として wait の operation に固定し、承認画面に
  そのまま出す（人はこの URL・selector を承認する）。
- `ConsumedBrowserApproval` は `trusted_login: TrustedLogin` を持つ。`consume_credential_approval` は wait に固定された値を
  そのまま移すだけで、呼出し側から値を受け取らない。

**照合（どれも lease 消費より前）**

1. approval 消費の前: supervisor が `DescribePolicy` を再度引き、固定値と revision・全欄が一致しなければ `policy_changed`
   で止める（一回承認も lease も消費しない）。
2. lease の grant: 既存の `credential_revision` 照合で、固定 revision と broker の policy が違えば grant しない。
3. 注入時: controller は `InjectionRequest.selector` に `trusted_login.password_selector` をそのまま入れる（他の出所は無い）。
   broker は ADR-0089 D3 の順 4（auth_section 照合）の直後・順 5（lease 消費）の前に、lease が保持する policy 断面の
   `password_selector` と要求の `selector` を byte 一致で比べ、違えば **`selector_mismatch`**（lease 未消費）。
   順 3 の origin・frame 鎖検査は変えない。

**形式検証**（task-core の純関数 `validate_trusted_login` を broker の `CredentialPolicy::validate` と Celeris の pin 時の両方で使う）

- `login_url`: `https` のみ、`canonical_origin(login_url) == exact_origin`、userinfo・fragment 不可、2048 byte 以下。
  query は許すが audit・event には書かない（ADR-0080 監査規則）。
- selector: ASCII 印字可能・256 byte 以下・compound 8 個以下。許す文法は
  `type`（`[a-z][a-z0-9-]*`）・`#ident`・`.ident`・`[attr]`・`[attr=ident]`・`[attr="…"]`（値に `"`・`\`・改行を含まない）と、
  結合子の空白・`>` だけ。`,`（selector list）・`:`/`::`（pseudo 全般）・`*`・`+`・`~`・`>>>`・`/deep/`・`\` escape・
  engine 接頭辞（`css=`・`xpath=`・`text=`・`internal:`・`frame=`・`@e` ref）は拒否。
- iframe 越え不可: 解決は controller の注入用 session で top target の top document に対する `DOM.getDocument`
  （`pierce: false`）→ `DOM.querySelectorAll` で行い、**ちょうど 1 要素**・`INPUT`・`type=password` でなければ拒否
  （0 件・複数は `target_mismatch`、型違いは `redisplay_field`）。shadow tree・子 frame の要素は querySelector の範囲外で、
  加えて `CdpController::inject` の frame 鎖長 1 の検査（既存）で子 frame を拒否する。
- `submit_selector` も同じ検証。submit は注入後に controller が isolated world で要素の `requestSubmit()`/`click()` を呼ぶ
  （agent は関与しない）。

### D3. browser.rs の H3 開始/終了の順序（c）

照合はすべて broker の lease 消費（ADR-0089 D3 順 5）より前に置き、失敗は閉じる側へ倒す。番号順に実行し、最初の失敗で止まる。

| 順 | 処理 | 失敗時 |
|---|---|---|
| 0 | policy 確定 → ADR-0084 D6 の起動前 routing 判定（既存、変えない） | 承認・lease 未消費で拒否 |
| 1 | `Supervisor::start`（D1 の構成、`cdp_pipe: true`）。`LiveSessions::insert` と同時に control `RegisterLiveSession`（ADR-0089 D2） | `isolated_runtime_unavailable`、未消費 |
| 2 | agent-browser 版確認（relay 経由、既存の `__version__`） | 同上 |
| 3 | `DescribePolicy` 再取得と wait の `TrustedLogin` の照合・形式検証（D2 照合 1） | `policy_changed`、未消費 |
| 4 | 一回承認の消費 → `ConsumedBrowserApproval`（`trusted_login` 付き） | 既存どおり |
| 5 | broker lease の grant/bind（発行のみ、未使用） | segment 失敗 |
| 6 | relay 遮断（D1）→ `CdpController::open_auth_section(id)` → store `browser_auth_section(true)` と worker の `live.auth_section()` | 以後の失敗は 11→10→9 の後始末を必ず通す |
| 7 | controller が `Target.createTarget(about:blank)` → control `OpenAuthSection{session_id, auth_section_id, lease_id, exact_origin, cdp_target_id}` | 同上 |
| 8 | controller が `Page.navigate(trusted_login.login_url)`・load 待ち → D2 の selector 解決 → `InjectionRequest{selector = trusted_login.password_selector, …}` → `CdpController::inject`。broker 側で ADR-0089 D3 順 0〜4 → `selector_mismatch` 照合 → 順 5 lease 消費 → 順 6 provider/sink | 順 5 より前の拒否は lease 未消費 |
| 9 | receipt 受領 → `submit_selector` があれば controller が submit、navigation 完了または期限（10 秒）まで待つ | 同上 |
| 10 | `CdpController::close_auth_section`（同 document なら値を空に）→ control `CloseAuthSection` | 消去失敗は session 破棄 |
| 11 | store `browser_auth_section(false)`・`live` の区間解除 → relay 遮断解除 → harness 操作へ | `auth_section_close_failed` |
| 終 | `Supervisor::stop` と同時に control `UnregisterLiveSession` | — |

- 現行の `browser_credential::use_credential`（`auth login` + 旧 `bridge`）は H3 経路から外す。旧 `resolve.sock`・`bridge` の
  endpoint 自体は固定拒否のまま残す（ADR-0085 3/4）。
- 秘密は broker → sink FD → mux → pipe の一方向だけ（ADR-0089 D4）。browser.rs・`browser_action.py`・relay は秘密を持たない。
- 本番 build の broker は admission `Attested` だけを通すので、この host（決定 p4a-uid、`SameUid`）では順 8 の broker 順 2 で
  `isolation_required` になる。これは期待どおりの本番挙動で、e2e の正例は D5 の試験 admission で得る。

### D4. e2e の非露出観測面と負の対照（d）

task-api の e2e として、実 daemon 経路（API → dispatcher → worker → supervisor → 実 chrome-headless-shell・実 broker process・
実 CDP pipe・D1 の relay）で 1 回の H3 を通す。fixture はローカル（ADR-0088 D1 の試験 netns・自己署名 HTTPS・試験 DNS）、
LLM は scripted（外部ネットワークに出ない）。

- sentinel: 試験ごとにランダム 32 byte の hex。検索する表現は raw・base64（標準・URL-safe、3 通りの位置ずれ）・
  percent-encode・UTF-16LE・JSON escape。
- 正例: fixture の login form が POST を受け、受信 body の password が sentinel と一致したことを fixture の受信記録
  （全観測面の外に置く）で確かめる。broker の応答は receipt だけ。
- scripted LLM は区間後に snapshot・`get value <password_selector>`・screenshot・console 取得を要求する（D1 の区間後の
  状態を突く）。区間中は harness が走らないことも event 列で確かめる。
- byte 検索して 0 件を要求する面:

| 面 | 対象 |
|---|---|
| event 列 | store の events 表、API の SSE を読み切った byte 列、`forward_events` を通った worker event |
| artifact | run の artifact dir（screenshot・snapshot・download）と live view 出力 |
| LLM 観測 | scripted LLM が受け取った全 message（tool 結果・プロンプト）の記録 |
| SQLite | DB 本体・`-wal`・`-shm`。checkpoint 前に `-wal` を読み、`wal_checkpoint(TRUNCATE)` 後に本体を再度読む |
| run log | run log・worker stdout/stderr・daemon の tracing 出力 |
| session dir | runtime の `/session`（agent-browser の daemon log・`events.jsonl`・relay の trace） |
| broker | audit dir・IPC 応答・receipt |

- 負の対照（検出器が本当に効くことの証拠。どれも正例と同じ検索関数を使う）:
  - N1 変異: `same-uid-harness` feature 下だけに compile される試験 hook で relay の区間中遮断を無効にし、fixture page が
    `console.log(input.value)` する。同じ e2e を走らせ、event 列・LLM 観測・run log のどれかで sentinel が**検出される**
    （試験が落ちる側の判定になる）ことを確かめる。
  - N2 変異: 区間終わりの値消去を無効にし、scripted LLM の `get value` で LLM 観測に sentinel が出ることを検出する。
  - N3 面ごとの植え込み: 各面の実ファイル（DB・WAL・artifact・log のコピー）に各表現の sentinel を植え、検索が全表現・全面で
    1 件以上を返すこと。
  - 変異 hook は既定 feature・release build に入らないことを `cargo tree -e features` 相当の検査か cfg の試験で示す。

### D5. 変えない不変条件（e）

- `CredentialProvider` の trait 契約、broker 内部の lease/use/revoke、**単回 lease**（1 lease = 1 回の注入、再試行は新しい
  承認・lease から）。
- 本番 broker の admission は `Attested` だけ。`InjectionAdmission::SameUidHarness` は `same-uid-harness` feature の下だけに
  compile され、`task-worker`・`task-api` の `[dev-dependencies]` からだけ有効にする（通常の `[dependencies]` に feature を
  付けない）。この admission の結果には `admission = "same_uid_harness"` を記録し、P4-C `certify` の証拠にしない（ADR-0089 D6）。
  e2e が ADR-0084 D6 の起動前 routing 判定を通る入力も同じ feature の下だけに置き、release build の判定は変えない。
- 旧 `resolve.sock` と plugin `bridge` の固定拒否（`trusted_injection_required`・固定失敗）。設定で戻す抜け道を作らない。
- ADR-0084 D1 の隔離要件（Chromium の CDP は pipe）と `verify_isolation` の検査内容。
- dispatcher・store に LLM 呼出しを入れない。内部 origin を egress に足さない。本番昇格しない。

## 既存 ADR との関係

- **ADR-0080**: H3 の「ログイン URL と top-level selector は管理者の site policy の値でモデルは指定できない」を D2 で実体化する。
  L96 の `auth login --credential-provider` による supervisor 構成は ADR-0085 で既に不能になっており、D3 で controller の
  注入へ置き換える（「source 確認だけで原子的 injection と主張しない」の方針どおり、原子性は ADR-0089 D4 の同期 JS が担う）。
  監査規則「selector・URL query を記録しない」を保つため、audit には selector 文字列でなく `policy_id`・`revision`・`field` と
  照合結果を記録する（ADR-0089 D1 の「selector を audit 用に記録」はこの範囲に狭める。値は revision から一意に引ける）。
- **ADR-0084**: D1 の CDP pipe 要件を Chromium について保ち、relay は controller の検査の内側に置く（D1）。D3 の
  prepare/commit・`redisplay_field`・`RedisplayGuard` を変えない。D6 の起動前拒否は D3 表の順 0 のまま先頭に置く。
- **ADR-0085**: 3/4 の旧 endpoint 固定拒否を変えず、H3 経路から旧経路の呼出しを除く。5 の不変条件を D5 で再掲する。
- **ADR-0089**: D2 の登録時点（`RegisterLiveSession` = `LiveSessions::insert` と同時、`Open/CloseAuthSection` = H3 と同時）に
  従う。D3 の照合順に `selector_mismatch` を順 4 と 5 の間へ足すだけで、lease 消費より前の拒否は未消費という性質を保つ。
  D4-6 の区間中 CDP 拒否を全面遮断に強める（強める方向だけ）。D5 の検査面に session dir を足す。D6 の admission と解放条件は
  変えない（この host では機密能力は解放されない）。

## 後続 unit への割当て

- selector: D2（`CredentialPolicy` の欄と `validate_trusted_login`、`DescribePolicy`、wait への固定、`ConsumedBrowserApproval.trusted_login`、
  broker の `selector_mismatch`）。
- shared-cdp: D1（`cdp_pipe: true` の runtime 構成、`sandboxd` の転送、relay の token/Origin 検査・多重化・区間遮断・
  postData 削除、ドメイン制限の三層、`browser_action.py` の `--cdp`）。
- h3-wire: D3 の順序で `browser.rs` を結線し、`use_credential` を H3 経路から外す。
- e2e: D4。closeout: workspace 検査と `docs/progress/phase-browser-4.md`。

## 未解決

- username の注入: broker の 1 auth section = 1 lease = 1 field（`AuthSectionRegistration.lease_id` が 1 つ、lease は単回）なので、
  1 回の承認で username と password を両方入れるには lease と section の対応を変える必要がある。この ADR の範囲は password
  1 欄とし、`username_selector` を持つ policy は D3 の順 3 で拒否する（承認・lease 未消費）。拡張は別 ADR で決める。
- 別 host UID での `Attested` 実行（ADR-0089 D6 の解放条件）は引き続き未達。

## 結果

- 良い: controller が唯一の CDP 当事者になり、H3 とその後の harness 操作が同じ page・同じ cookie を共有する。区間中は agent が
  browser から何も受け取れず、区間後も区間中の request 本文へ戻れない。URL・selector は管理者の policy と人の承認に固定され、
  broker が lease 消費前に照合する。
- 悪い: agent-browser の `--allowed-domains` を失い、ドメイン制限を proxy・relay・`Fetch` の三層で自前に持つ。relay は
  CDP を理解する検査器として保守対象になる。この ADR の範囲では password 1 欄の site にしか使えない。
