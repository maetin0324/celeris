# ADR-0109: P4-B injection-only IPC・CDP sink・実攻撃試験と機密能力の解放条件

---
tasks: [01M3RK6XG30KD3QC0Z67XBB5G5, 01M4D6TWCKEH25YVZK1YHMBFJQ]
---

- 日付: 2026-09-30
- 状態: Accepted（設計。実装と実攻撃試験の結果は後続の unit が出す。この ADR 自体は受け入れの証拠にならない）
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md) H3、[ADR-0102](0102-browser-phase4-isolation-injection-routing.md) D3/D4/D6、
  [ADR-0103](0103-browser-phase4-runtime-selection.md) 3〜5、[ADR-0105](0105-browser-p4a-same-uid-bwrap-runtime.md)、
  [ADR-0106 P4-C](0106-browser-phase4-conformance-dispatch.md)、[ADR-0108](0108-browser-p4a-relay-supervisor-launch-restore.md) D2/D5、人の決定 p4a-uid

## 文脈

- ADR-0103 3/4 は旧 `resolve.sock` と plugin `bridge` を固定拒否（`trusted_injection_required`）にし、将来の controller は
  別の injection-only IPC を使い、SO_PEERCRED の役割・稼働中の隔離 session・CDP 対象・auth_section を照合して
  receipt だけを返すとした。
- ADR-0102 D3 は `TrustedInjector::prepare/commit`（frame 鎖・redirect 鎖・要素型・TOCTOU）と `RedisplayGuard` を純関数で
  用意したが、peer の役割割当て・実 CDP sink・実攻撃試験は無い。
- P4-A（ADR-0105/0108）で、実 bwrap runtime・supervisor（daemon 内の専用 thread）・CDP pipe（fd 3/4 を controller が保持）・
  `LiveSessionRegistry` が入った。この host は同一 host UID（決定 p4a-uid）なので `verify_isolation` は `SameUid` を返し、
  attestation は出ない。broker（`celeris-credentiald serve`）は daemon とは別 process で、daemon の PID を control の
  許可 PID として起動引数に受ける。
- 必要なのは「秘密は broker から CDP sink へ一方向に流れ、要求者（controller の injection client）への応答には receipt しか
  出ない」配線と、それが攻撃の下で壊れないことの実証である。

## 決定

### D1. socket の置き場と wire 形式（a）

- 新しい socket は `<XDG_RUNTIME_DIR>/celeris-credentiald/injection.sock`。既存の `control.sock` と同じ dir（0700、broker の
  UID 所有、symlink 不可）に 0600 で作り、既存 `socket()` と同じ所有者・mode 検査を通す。**`resolve.sock` と plugin `bridge` は
  固定拒否のまま触らない**（再開しない。設定で旧経路を戻す抜け道も作らない）。
- sandbox には bind しない（ADR-0105 D1: `/run/user` は見えない）。worker・harness は同一 UID なので path に connect は
  できるが、D2 の役割判定で拒否される。
- 1 接続 = 1 要求 = 1 応答。frame は `u32`（big endian）の長さ + JSON 本文。要求の上限 16 KiB、読み取り期限 5 秒
  （超過は本文を読み捨てて `invalid_request`）。JSON は `deny_unknown_fields`。
- 要求に 1 つだけ `SCM_RIGHTS` で **sink FD** を付ける（D4。controller の CDP mux へ繋がる `SOCK_SEQPACKET` の一端）。
  FD が無い・2 本以上・種別が違う要求は `invalid_request`。

要求（controller の injection client → broker）:

| field | 型 | 意味・制約 |
|---|---|---|
| `v` | u32 | `1` 固定。他は `unsupported_version` |
| `request_id` | string | ULID。応答と audit に echo |
| `session_id` | string | 稼働中の隔離 browser session（`LiveSessionRegistry` の key） |
| `cdp_target_id` | string | 注入先 page の CDP `TargetID` |
| `frame_id` | string | 注入先 frame の CDP `FrameId` |
| `loader_id` | string | 注入先 document の CDP `LoaderId`（= `InjectionTarget.document_id`） |
| `frame_chain` | string[] | top → 注入先 frame の origin（canonical） |
| `redirect_chain` | string[] | 現 document に至る redirect の origin（最後が現在） |
| `selector` | string | 要素を解決した CSS selector（≤ 512 byte。audit 用に記録。秘密ではない） |
| `object_id` | string | controller が注入先 frame の isolated world で解決した要素の `RemoteObjectId` |
| `field` | `"username"` \| `"password"` | 注入する項目 |
| `input_type` | string | DOM の `type`（小文字） |
| `auth_section_id` | string | 開いている H3 認証区間の id |
| `lease_id` | string | P2-B の単回 lease |
| `cdp_command_id` | u64 | mux が broker 用に予約した CDP command id（D4） |

- 要求に**秘密・value・長さ・hash は無い**。broker は要求に `value` 等の未知 field があれば `invalid_request`（`deny_unknown_fields`）。

応答（broker → client。成功・拒否とも同じ形）:

| field | 成功 | 拒否 |
|---|---|---|
| `v`, `request_id` | 有 | 有 |
| `ok` | `true` | `false` |
| `receipt` | `{lease_id, auth_section_id, session_id, cdp_target_id, frame_id, loader_id, field, injected_at}` | 無 |
| `code` | 無 | D3 の拒否コード |

- receipt には値・長さ・hash・selector・入力後の DOM 観測を入れない。`InjectionReceipt`（ADR-0102 D3）はこの形に拡張する
  （`element_id` は `object_id` を返さず `field` で置き換える。object id は page 内の参照で再利用価値があるため）。
- audit（broker の既存 audit dir）には要求の非秘密 field・`code`・peer pid を書く。秘密・sink FD 上の frame は書かない。

### D2. SO_PEERCRED による `PeerRole` 判定（b）

- broker は daemon から control 経由で**稼働中 session の登録**を受ける（新しい `ControlRequest`）:
  - `RegisterLiveSession { session_id, controller_pid, controller_start, runtime_pid, runtime_start }` — supervisor（ADR-0108 D2）
    が `LiveSessions::insert` と同じ時点で送る。`UnregisterLiveSession { session_id }` は `remove` と同じ時点。
  - `OpenAuthSection { session_id, auth_section_id, lease_id, exact_origin, cdp_target_id }` /
    `CloseAuthSection { session_id, auth_section_id }` — worker の `browser_auth_section(…, true/false)`（H3）と同じ時点。
  - control は既存どおり daemon の PID + starttime でだけ受ける。登録は broker の memory だけに持ち、永続化しない
    （broker 再起動で全部消え、以後の注入は `session_not_live` になる = 閉じる側）。
- 役割は peer から決める。自己申告（JSON の role 等）は読まない:
  1. `SO_PEERCRED` の UID ≠ broker の UID → `peer_uid_mismatch`。
  2. peer pid と `/proc/<pid>/stat` の starttime が、要求の `session_id` に登録された `(controller_pid, controller_start)` と
     一致すれば `PeerRole::Injector`。
  3. それ以外は、peer pid が登録済みのどれかの runtime の process group（bwrap の pgid）内なら `PeerRole::Agent`
     （browser・sandbox 内 process）、そうでなければ `PeerRole::Worker`。どちらも `injection_worker_not_allowed`。
- 照合は「peer = その session の controller」であり、「peer = どれかの controller」ではない（別 session の controller が
  他 session に注入できない）。
- **稼働中隔離 session の照合は broker が自分で行う**。登録を信じた boolean は使わない: broker は `runtime_pid` の
  starttime 一致を確かめ、`/proc/<runtime child>/…` から `RuntimeFacts` を採り直して `task_core::browser_isolation::verify_isolation`
  に通す。事実採取の関数（今は `task_worker::browser_runtime`）は task-core に移して broker と worker で共有する
  （credentiald は task-core に依存済み）。attestation が出なければ `isolation_required`。
- この host（同一 host UID、決定 p4a-uid）では `SameUid` により production の broker は**必ず** `isolation_required` を返す。
  検査を緩める分岐は production build に入れない。

### D3. 照合順と拒否コード（c）

照合は下表の順に行い、最初の失敗で止まる。**lease 消費より前の拒否は lease・一回承認を消費しない**。provider（秘密の開封）は
最後で、それより前の失敗では秘密を memory に読み出さない。

| 順 | 照合 | 拒否コード |
|---|---|---|
| 0 | frame 長・期限・JSON 形・sink FD 1 本・`v` | `invalid_request` / `unsupported_version` |
| 1 | 役割: UID 一致、peer = 当該 session の controller（pid+starttime） | `peer_uid_mismatch` / `injection_worker_not_allowed` |
| 2 | 稼働中隔離 session: 登録あり・runtime pid の starttime 一致・broker 自身の `verify_isolation` が attestation を返す | `session_not_live` / `isolation_required` |
| 3 | CDP 対象と origin: `cdp_target_id` が開いている auth_section の target と一致、`frame_chain` 非空・全 exact origin、`redirect_chain` 全 exact origin、要素型（password ↔ `type=password`、username ↔ text/email） | `target_mismatch` / `empty_frame_chain` / `cross_origin_frame` / `redirected` / `redisplay_field` |
| 4 | auth_section: 当該 session で `auth_section_id` が開いており、その lease・exact origin が要求と一致 | `auth_section_required` / `auth_section_mismatch` |
| 5 | lease: 存在・未使用・期限内・`session_id` と exact origin 一致（`Broker::resolve` の既存検査）→ **ここで消費** | `lease_expired` / `lease_used` / `other_session` / `lease_invalid` |
| 6 | provider 呼出し（`CredentialProvider` 契約は不変）→ D4 の sink | `provider_failed` / `target_changed` / `sink_failed` |

- 3 は ADR-0102 D3 の `prepare`（`check_target`）そのもの。6 の中で D4 の in-page 再確認が `commit` の TOCTOU 検査を実 page 上で担う。
- 拒否応答に理由の詳細（どの origin だったか等）は入れない。audit には非秘密の詳細を書いてよい。
- 5 の後の失敗（6）は lease を返さない。再試行は新しい承認・lease から（P2-B の単回 lease を変えない）。

### D4. CDP sink の注入手順（d）

秘密を運ぶのは broker → sink FD → controller の CDP mux → CDP pipe（fd 3）→ browser の一方向だけとする。

1. controller（supervisor thread 側の CDP mux）は注入の直前に:
   - 注入先 frame 用の isolated world を `Page.createIsolatedWorld` で作り、`DOM.resolveNode`（`executionContextId` 指定）で
     `object_id` を得る。page の main world の script はこの world の prototype を改変できない。
   - `cdp_command_id` を予約し、CDP pipe への他の書込みをその command の応答まで止める（per-session の write lock）。
   - 要求を送り、`SOCK_SEQPACKET` の socketpair の一端を sink FD として渡す。
2. broker は D3 の 0〜5 を通った後、provider から値を得て **1 本の CDP frame** を作る:
   `Runtime.callFunctionOn { objectId, functionDeclaration: <固定の関数>, arguments: [expected_origin, frame_depth, field, value],
   returnByValue: true, silent: true }`。関数は同期実行の中で、
   - `this.isConnected`、`this.ownerDocument.defaultView` の `location.origin` === expected、
   - 祖先 frame を 1 段ずつ `parent.location.origin` で辿り（cross-origin の祖先は例外 → 拒否）全部 expected、段数一致、
   - `this.type`（password は `password`、username は `text`/`email`）
   を確かめ、1 つでも違えば値を触らずに `"target_changed"` を返す。通れば native の value setter（isolated world 側）で値を入れ、
   `input`/`change` を dispatch して `"ok"` を返す。戻り値は固定語彙の文字列だけで、値・長さを返さない。
   - navigation 後は `object_id` の実行 context が消えるので CDP がエラーを返す → `target_changed`。検査と代入が
     1 つの同期 JS 実行に入るので、照合後の origin 変更（TOCTOU）が入り込む隙間が無い。
3. broker は frame を sink FD に 1 回書いて自分の buffer を zeroize する。mux は frame を**解釈せず** pipe に書き、自分の
   buffer を zeroize し、`cdp_command_id` の応答だけを sink FD に返す。mux は broker の frame をログ・event・観測に出さない。
4. broker は応答の `result.value` が `"ok"` なら receipt、`"target_changed"` または CDP エラーなら `target_changed`、
   FD の切断・期限（5 秒）なら `sink_failed` を IPC の応答にする。
5. cross-origin iframe: 3 の `cross_origin_frame` と 2 の祖先 origin 検査の二重。OOPIF（別 target）の要素を main target の
   `cdp_target_id` で指定したものは 3 の `target_mismatch`。
6. 認証区間中の観測停止（H3 を強める方向だけ）: auth_section が開いている間、controller の CDP mux は当該 session で
   `Page.captureScreenshot`・`Page.startScreencast`・`DOM.getOuterHTML`・`DOM.getAttributes`・`Runtime.evaluate` /
   `Runtime.callFunctionOn`（broker の予約 id 以外）・`Accessibility.*` の agent 起点の呼出しを拒否し、`Runtime.consoleAPICalled`・
   `Log.entryAdded`・`Runtime.exceptionThrown` の event を worker へ転送しない。worker 側の既存 `forward_events` 抑止
   （progress・artifact・live view）はそのまま。
7. 区間の終わり: auth_section を閉じる前に、mux は注入した要素が同じ document に残っていれば isolated world から値を空にする
   （navigation 済みなら不要）。以後の観測は `RedisplayGuard`（salt 付き hash だけを持つ）で秘密の再表示を検出して観測ごと捨てる。
   guard は broker が区間ごとに作り controller には salt と hash だけを渡す。

- 却下: (i) 秘密を IPC 応答で controller に返し controller が `Input.insertText` する — 応答が receipt だけという契約に反し、
  daemon の memory に平文が広く渡る。(ii) `DOM.focus` → `Input.insertText` の 2 command — 間に page script が focus を text input へ
  移せる（再表示）・navigation が入る（TOCTOU）。(iii) CDP pipe の FD 自体を broker に渡す — pipe の読み側を mux と奪い合い、
  他の command と frame が混ざる。(iv) main world での `Runtime.evaluate` — page script が prototype を差し替えて値を横取りできる。

### D5. 攻撃試験行列と H3 sentinel 検査面（e）

すべてローカルの fixture（ADR-0108 D1 の試験用 netns・自己署名 HTTPS・試験用 DNS）と実 chrome-headless-shell、実 broker
process、実 CDP pipe で行う。外部ネットワークに出ない。秘密は試験ごとの sentinel（ランダムな 32 byte の hex）。
fake の sink・fake の attestation は証拠に数えない。

| # | 攻撃 | 手順（fixture） | 期待 | 確かめる所 |
|---|---|---|---|---|
| A1 | TOCTOU: 照合後の origin 変更 | login page が要求送信を検知して（`cdp_command_id` の予約後）別 origin へ `location.replace` | 拒否 `target_changed` | 別 origin の fixture に sentinel が届かない（fixture の受信 log）、lease は消費済み・再利用不可 |
| A2 | TOCTOU: 同 origin 内の document 差し替え | 照合後に `document.open()`／同 origin navigation | 拒否 `target_changed` | 新 document の input に値が無い |
| A3 | redirect 経由 | 別 origin → 302 → 正しい origin の login page | 拒否 `redirected` | lease 未消費 |
| A4 | cross-origin iframe | 正しい origin の top に別 origin の iframe で password 欄 | 拒否 `cross_origin_frame`（OOPIF の target 指定は `target_mismatch`） | iframe の fixture に sentinel 無し |
| A5 | 逆向き iframe | 別 origin の top に正しい origin の login iframe | 拒否 `cross_origin_frame` | 同上 |
| A6 | 再表示用の欄 | password を `type=text` に（直前に `type` を書換えも） | 拒否 `redisplay_field`／書換えが照合後なら `target_changed` | 画面・DOM に sentinel 無し |
| A7 | value 読み戻し（agent） | 注入後、区間中と区間後に agent が `get value`・`eval`・snapshot で欄を読む | 区間中は action 拒否、区間後は値が空または `RedisplayGuard` で観測破棄 | worker 出力・event に sentinel 無し |
| A8 | DOM 再表示（page script） | page script が値を visible な `<div>` に写す | 観測を `RedisplayGuard` が破棄 | snapshot・artifact に sentinel 無し |
| A9 | screenshot / screencast | 区間中に agent が screenshot を要求、page は値を text 表示 | 区間中は拒否、区間後の画像は区間後の観測として guard を通る（OCR はしない。区間中の要求が 0 件で通らないことを主に確かめる） | artifact dir に区間中の画像が無い |
| A10 | console | page script が `console.log(input.value)`・例外 message に値 | event を転送しない | worker 出力・event・run log に sentinel 無し |
| A11 | worker から取得 | 同一 UID の worker process が `injection.sock` に正しい形の要求（有効な lease・auth_section id つき）、`resolve.sock`、`bridge` | `injection_worker_not_allowed`／`trusted_injection_required`／固定失敗 | lease 未消費、audit に use 0 |
| A12 | browser から取得 | sandbox 内 probe と page JS が broker socket の path・`/run/user`・抽象 socket に到達を試みる | 到達不能（ENOENT 等） | broker の audit に接続無し |
| A13 | 別 UID から | 実 socket 上で役割判定を行う関数に、別 UID の `ucred` を与える試験と、別 host UID が使える host（ops 手順書）での実 connect。`unshare -r` の子は host では同じ UID に見えるので別 UID の証拠にしない | `peer_uid_mismatch` または接続不能（dir 0700） | 同上。この host で実 process による別 UID 試験ができない場合は PROGRESS の未解決に残す |
| A14 | 別 session の controller | session B の controller pid から session A の lease で要求 | `injection_worker_not_allowed` | lease 未消費 |
| A15 | 区間外 | auth_section を閉じた後・開く前の要求 | `auth_section_required` | lease 未消費 |
| A16 | 二重消費 | 成功した要求と同じ lease で再要求 | `lease_used` | sink FD に 2 本目の frame 無し |
| A17 | 要求に値を混ぜる | `value`/`length` field を足す | `invalid_request` | provider 呼出し 0 |

H3 sentinel 検査面（実注入を含む認証区間を 1 回通した後、全面で sentinel と、その base64・URL encode・UTF-16LE を検索し 0 件）:

| 面 | 対象 |
|---|---|
| event | store の events 表・API の SSE・`forward_events` を通った worker event |
| artifact | run の artifact dir（screenshot・snapshot・download）と live view 出力 |
| LLM 観測 | harness に渡る tool 結果・プロンプト（scripted LLM が受け取った全 message の記録） |
| DB / WAL | SQLite の本体・`-wal`・`-shm` の byte 列（checkpoint 前後の両方） |
| worker 出力 | worker の stdout/stderr・run log・tracing 出力（daemon の log を含む） |
| broker | audit dir・IPC 応答・receipt |
| CDP mux | mux の trace（有効時）。broker frame は記録されないこと |

- 正例（D6 の harness）: sentinel が fixture の login form の受信側にだけ届く（fixture が POST を受けて保存する）ことを確かめる。

### D6. この host での正例と、機密能力の解放条件（f）

- production の broker は D2 の attestation を要求し、この host では `SameUid` で必ず拒否する（決定 p4a-uid・ADR-0105 のまま）。
- 実 CDP sink と照合順の**正例**（A1〜A17 の前提として一度は注入が成功する経路）を得るため、試験専用の admission を置く:
  `InjectionAdmission::SameUidHarness`。これは `verify_isolation` の違反が `SameUid` **だけ**のときに限り session を通すもので、
  - crate feature `same-uid-harness` の下でだけ compile し、既定 feature・release build には入らない
    （crate の `[dev-dependencies]` で自分自身に feature を付けて試験 binary にだけ入れる）。
  - `IsolationAttestation` を作らない。identity restore（ADR-0108 D5）や他の attestation 要求箇所は通れない
    （ADR-0108 D5 の「試験用 attestation 生成口を作らない」とは矛盾しない）。
  - この admission で通した注入の結果・適合記録には `admission = "same_uid_harness"` を必ず記録する。
- 解放条件（P4-C `certify` への追加）: `CredentialInjection`・`IdentityRestore` を適合と記録するのは、同じ backend・browser version の
  実測記録（ADR-0106 P4-C D1）に次が**全部**そろうときだけ:
  1. `IsolationSuite`・`EgressNegativeSuite` が違反 0（`SameUid` を含まない。= 別 host UID の実 runtime）。
  2. `InjectionAttackSuite` が A1〜A17 全部期待どおり、かつ `admission = "attested"`。
  3. `AuthSectionObservationStop` が D5 の全検査面で sentinel 0 件、かつ `admission = "attested"`。
  4. 記録が runner の実行出力から作られ、壊れていない・古くない。
- 未達時: 記録に `same_uid_harness` が 1 件でも混ざる、どれかが欠ける・失敗・古い場合は、機密能力を適合なしとし、ADR-0102 D6 の
  起動前拒否（承認・lease を消費しない）を維持する。この host では 1 が満たせないので、P4-B の実装・試験がすべて通っても
  **機密能力は解放されない**。これは失敗ではなく決定 p4a-uid の帰結であり、PROGRESS の未解決に「別 UID host での
  attested 実行」を残す。手順は `docs/ops/browser-isolated-runtime-subuid.md` に追記する。
- fallback: 機密能力を要求する run は ADR-0107（fallback）D3 のまま fallback しない。

## 変えないもの

- `CredentialProvider` 契約、broker 内部の lease/use/revoke と単回 lease、旧 `resolve.sock`・`bridge` の固定拒否。
- H3・Phase 3 の auth_section 配線と観測停止（D4-6 は強める方向の追加だけ）、ADR-0102 D6 の未適合機密要求の起動前拒否。
- ADR-0105/0108 の runtime・supervisor・registry・restore の拒否判定。本番設定・本番昇格・内部 origin（追加しない）。
- dispatcher・store に LLM 呼出しを入れない。

## 後続 unit への割当て

- ipc（celeris-credentiald）: D1〜D3。`injection.sock`、control の登録 4 種、役割判定、照合順、拒否コード、receipt、audit、
  `same-uid-harness` feature。事実採取の task-core への移設。
- sink（task-worker）: D4。CDP mux の予約 id・write lock・isolated world・sink FD、injection client、区間中の CDP 拒否と
  console event 抑止、区間終わりの値消去、supervisor からの登録送信。
- attacks: D5 の A1〜A17。h3e2e: D5 の検査面。unlock: D6 の `certify` 追加と PROGRESS。

## 結果

- 良い: 秘密は broker → sink の一方向だけを流れ、要求者への応答・audit・worker 側の面に出ない。検査と代入が 1 回の同期 JS に
  入るので、照合後の navigation で別 origin に値が入らない。worker・browser・別 UID・別 session の controller は peer から
  決まる役割で拒否される。
- 悪い: controller の mux（daemon process）は broker の frame を一瞬 memory に持つ（解釈・記録はしない）。page の main world の
  script は注入後に同じ origin の欄の値を読める（正当な site が password を受け取るのと同じで、egress allowlist の範囲に限られる）。
  この host では正例が試験 feature に依存し、機密能力の解放は別 UID の host での実行まで得られない。

## 付記: Shibboleth 型 trusted login（2026-10-08）

タスク: `01M4D6TWCKEH25YVZK1YHMBFJQ`。D3 検査 3 の全 exact origin という条件を worker にも適用する。

- H3 を開き、対象 page session に Network を有効化してから静的 login URL に移動する。
  対象 session の top-level Document request のみを記録する。redirectResponse の旧 URL と
  request の新 URL の origin を保持する（query・SAML 本文は保持しない）。JS 自動 POST 等の
  文書遷移の origin も記録し、別 origin の履歴は戻ってきても拒否する。subresource と他 session は対象外。
  redirect は最大 32 hop、broker wire の鎖と文書 request の記録も最大 32 件、超過・origin 不明は `redirected` で閉じる。
- 移動開始から 15 秒以内に exact origin 上で password selector が一意の HTMLInputElement
  （type=password）に解決できる文書を待つ。isolated world の判定は真偽値だけで、DOM・値を観測しない。
  前後の frame_id/loader_id が異なる場合は再度待つ。期限まで現れなければ `navigation_failed`。
  注入に渡す redirect_chain は実際に記録した origin の鎖。注入直前にも履歴を照合し、既存の
  origin・要素型・broker の同期 TOCTOU 検査、単回 lease、H3 観測遮断、RedisplayGuard は維持する。
- 注入成功後の submit は既存の loaderId 変更待機を維持する。その後の IdP→SP 自動 POST は
  通常遷移であり、この redirect 判定には含めない。selector 文法（ADR-0110 D2）と毎回承認は不変。

実装: `crates/task-worker/src/browser_cdp_sink.rs` の `begin_login_navigation` は移動前の
main frame ID を取得し、`queue_event` は session/frame/type を照合して origin だけを保持する。
`login_password_document` が同一文書の前後検査と type=password の一意性を確認し、
`browser.rs` の `complete_trusted_login` が期限内の文書を `inject` に渡す。`inject` は broker
呼出し直前に最新履歴を再検査する。成功後にこの履歴を閉じ、submit 後の SP 遷移を許可する。
試験は `browser_trusted_login_sso_`（実 Chromium/HTTPS の SSO 3 件、履歴の scope・拒否・上限 3 件）。
SSO 試験の broker transport は検証用であり、隔離・admission の適合証拠は既存 H3 wire/攻撃行列で確認する。
