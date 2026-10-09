# ADR 2026-10-09: credential 注入での username 入力と、ログイン後の頁の読み取り

---
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
---

- 日付: 2026-10-09
- 状態: **承認 2026-10-09**（人の確認事項 Q1〜Q9 の回答を下に記録。D2 の action 集合は Q3/Q4 の回答で screenshot・download・click を足した）
- 決定者: 人（方針「credential 注入の login で manaba の課題監視を自動化できる設計に変える」は 2026-10-09 に決定済み。本書の案は 2026-10-09 に Q1〜Q9 の回答付きで承認）
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md) D2/D3（H3）、[ADR-0109](0109-browser-p4b-injection-ipc-cdp-sink.md) D4、
  [ADR-0110](0110-browser-p4b-h3-shared-cdp-trusted-selector.md) D1〜D3・未解決、[ADR-0111](0111-browser-p4b-redisplay-guard-wiring.md)、
  [ADR-0116](0116-browser-launcher-implementation.md)、[ADR-0138](0138-browser-prod-admission-confidential-release.md)、
  [launcher CredentialUse 解放](2026-10-09-browser-launcher-credential-release.md)（付記「launcher の Authenticate 経路」）、
  [allowed origins](2026-10-05-browser-allowed-origins.md)、[click/download 承認](2026-10-08-browser-click-download-approval-policy.md)、
  [egress wildcard](2026-10-09-egress-wildcard-origin.md)
- 上書き（承認された場合）: ADR-0080 D3「credential を注入した session の終わりまで観測を再開しない」と同 e2e 補足の「認証後の区間」、
  ADR-0110「未解決: username の注入」、launcher 解放 ADR 付記 5「注入後も session の終わりまで観測停止を保つ」。
  いずれも **site policy で明示的に opt-in した site に限って**変える。opt-in していない site の挙動は変えない。

## 背景

task 01M4GYJ3XGJNWZQDF35F1MDE0H（manaba 課題監視）は、筑波大学統一認証 IdP（`https://idp.account.tsukuba.ac.jp`）で
manaba（`https://manaba.tsukuba.ac.jp`）にログインし、課題一覧を読み、レポートの下書きを作る。main `2df8feca` の時点で
次の 2 点が止めている。

1. **username が入らない。** site policy（`browser_site_policies`、`CredentialPolicy`、`TrustedLogin`、launcher の
   `AuthenticateArgs`）は `login_url`・`password_selector`・`submit_selector` だけを持つ。1 auth section = 1 lease = 1 欄で、
   注入は password 1 欄だけ（ADR-0110 未解決）。IdP の form は `j_username`（type=text）と `j_password` が同じ頁にあるので、
   password だけ入って送信され、ログインは失敗する。
   - 既にあるもの: 手動登録は username と password を受け（`task-api/src/browser.rs`）、vault の暗号化 envelope に両方入る
     （ADR-0080 D3「username も秘密 payload の一部として暗号化する」）。credentiald の `InjectionRequest.field` は
     `Username | Password` を持ち、固定 `INJECT_FUNCTION` は username を text/email 欄に入れられる。ただし username 欄の
     trusted selector 照合は無く（`injection_ipc.rs` 4b は password だけ）、lease は `max_uses=1` なので 2 欄目は `lease_invalid`。
2. **ログイン後に読めない。** ADR-0080 D3 H3 により、注入した session では session の終わりまで snapshot / extract（gettext）/
   screenshot / download が harness の allow から外れる（`browser.rs::credential_harness_policy`）。launcher 経路は controller の
   auth section を閉じず、store の `browser_auth_section` も session 停止まで立てたまま、worker event も捨てる。

### H3 が何を防いでいるか（読んだ ADR からの整理）

| # | 脅威 | 今の対策 | 根拠 |
|---|---|---|---|
| T1 | 入力中の秘密を agent / LLM が見る（DOM の value、input event、console、画面） | 認証区間中は relay 全面遮断・event 破棄・新規接続拒否 | ADR-0110 D1、ADR-0109 D4-6 |
| T2 | 区間後に頁が秘密を再表示する（エラー頁が password を反射、頁 script が value を DOM に写す） | 区間終わりに注入値を消去、`RedisplayGuard` が password の再表示を含む観測を破棄。ただし画素（screenshot）は検査できない | ADR-0110 D1、ADR-0111 5 |
| T3 | 認証後の頁にある**別の**秘密・個人情報（session token、CSRF token、成績・個人情報）が LLM 入力・provider・event・artifact に流れる | 「認証後ページの任意秘密を除去できるとは仮定しない」→ 観測を一切再開しない | ADR-0080 D3・D7 |
| T4 | 認証済み頁の内容による prompt injection で、認証済み session を悪用・外部へ持ち出す | navigation / egress は許可 origin だけ、eval・cookie・storage・CDP raw・fill/type は grammar で不可 | ADR-0080 D1、ADR-0110 D1、allowed origins |
| T5 | 区間中の request 本文（POST の password）を区間後に取り出す | postData 削除、区間中 requestId の body 取得拒否 | ADR-0110 D1 |

T1・T2・T5 は区間後の観測を再開しても既存の仕組み（遮断・消去・`RedisplayGuard`・postData 削除）で守れる設計が既にある
（ADR-0110 D3 順 11 は daemon 経路で区間後に relay を解除し、`RedisplayGuard` を通す前提で書かれている）。H3 を
「session の終わりまで」に延ばした決め手は **T3（汎用の除去を保証できない）** と、それを確かめる契約が無かったことである。
本書は T3 を「読んでよい site と範囲を人が明示的に選ぶ」ことで受け入れ、T1・T2・T5 の機械的な防御は弱めない。

## 決定（案）

### D1. username の入力: 1 承認 = 1 lease = 1 回の同期注入で 2 欄を入れる

1. **site policy に `username_selector`（任意）を足す。** ADR-0110 D2 と同じ文法・長さ制限・管理者だけが設定・revision 付き。
   持つ層: `browser_site_policies`（migration で列追加、NULL 可）、config.toml の seed、`CredentialPolicy`、`DescribePolicy` の応答、
   `TrustedLogin`（wait に固定）、承認画面の表示、policy hash。`username_selector` を持つ policy の承認は「この URL・この 2 欄」に対する承認になる。
2. **username の扱いは今の保存のまま（秘密と同じ）。** vault の暗号化 envelope に入れ、DB・event・log・API 応答・audit に書かない
   （ADR-0080 D3 のまま）。ただし D2 の `RedisplayGuard` は username には**掛けない**（manaba は頁に学籍番号等を表示するので、
   掛けると全観測が捨てられる）。つまり「Celeris 自身は書かないが、ログイン後の頁に表示された分は読める」扱い
   （機密だが秘密ではない）。この線引きは人の確認事項 Q1。
3. **lease の単回性（`max_uses=1`）は変えない。** 2 本の lease や `max_uses=2` は採らない（代替案 C）。代わりに
   `InjectionRequest` に任意の `username: {selector, object_id, input_type}` を足し、broker は 1 回の lease 消費・1 回の
   provider resolve で、**1 個の CDP frame** に 2 欄の代入を入れる。
   - 新しい固定関数 `INJECT_PAIR_FUNCTION`（`this` = password 要素、引数 `[origin, depth, {objectId: username 要素}, username, password]`）。
     1 回の同期実行で、両要素が同じ document・`isConnected`・origin/depth 一致・型（password / text|email）であることを確かめてから
     両方に代入する。どれか外れれば何も入れず `target_changed`。1 欄 policy は従来の `INJECT_FUNCTION` のまま（挙動不変）。
   - broker の照合（ADR-0109 D3 順 4b）: lease の policy 断面の `password_selector` と `username_selector` を両方 byte 一致で比べる。
     policy が `username_selector` を持つのに要求に username が無い／その逆は `selector_mismatch`（lease 未消費）。
   - `RedisplayGuard` は password の値だけで作る（D1-2）。
4. **controller の解決と sink frame 照合（両 runtime）。** controller は注入用 session で top document から `username_selector` も
   `pierce:false` で解決し、ちょうど 1 要素・`INPUT`・type text/email・password 要素と同じ frame id + loader id であることを確かめる。
   `CdpController::inject` の sink frame 照合（2df8feca）は、`functionDeclaration` が `INJECT_FUNCTION` か `INJECT_PAIR_FUNCTION` の
   どちらかと byte 一致、pair のときは `arguments[2].objectId` が controller 自身の解決した username 要素の objectId と一致、
   引数の個数・型が固定形、を足す。外れれば Chrome に書かず `sink_failed`。
5. **launcher protocol v5。** `AuthenticateArgs` に `username_selector?` と D2 の `post_login`（下記）を足す。
   `AuthenticationStatus` / `AuthenticateResult` に固定 enum `observation: held | resumed` を足す（自由文・頁内容は返さない）。
   `PROTOCOL_VERSION = 5`。daemon は承認消費の前の `hello` で、policy が `username_selector` か `post_login` を使うなら v5 以上を要求し、
   足りなければ従来どおり承認未消費で「protocol 5 required」の固定文で拒否する。1 欄・opt-in なしの policy は v4 launcher でも動く。
   旧 launcher は新しい欄を `deny_unknown_fields` で `bad_request` にするので、版確認を飛ばしても fail closed。
6. **範囲外**: username と password が別頁の 2 段 login（username → 次頁で password）、iframe 内 form、MFA。いずれも policy の
   形式検証か注入時の検査で拒否する。

### D2. ログイン後の読み取り: site 単位の opt-in と、条件付きで認証区間を閉じる

1. **site policy に `post_login` を足す（既定は無し = 今の H3 のまま）。**
   ```text
   post_login: {
     read_origins: [exact HTTPS origin],   // 例 ["https://manaba.tsukuba.ac.jp"]。credential の exact_origin（IdP）は入れられない
     actions: [snapshot, extract, screenshot, download, click]   // 承認時の Q3/Q4/Q6 回答で 5 つ（下の D2-6）
   } | null
   ```
   管理者経路（API・web の site policy 編集）だけが書け、モデル・worker・task policy からは広げられない。revision・policy hash・
   `TrustedLogin` に入り、承認画面に「ログイン後、次の origin の頁を読み取る（snapshot / extract）」と出す。
   人の credential_use 承認（毎回）がそのまま「login して読む」ことへの承認になる。実効集合は
   `post_login.actions ∩ task allowed_actions ∩ grant`、`read_origins ⊆ task network_domains ∩ grant`（空なら opt-in 無しと同じ）。
2. **認証区間の終わり方（opt-in の site だけ）。** 注入・submit の後、controller（daemon 経路は `CdpController`、launcher 経路は
   launcher の controller）が観測を止めたまま次を**全部**確かめてから区間を閉じる。期限（15 秒）内に揃わなければ区間は閉じず、
   今の H3 と同じく session の終わりまで観測停止のまま、固定 code `post_login_unconfirmed` を返す（頁内容は返さない）。
   - top document が login document から別の document へ移った（loader id が変わった）。
   - top document の origin が `read_origins` のどれか（IdP に留まる＝エラー・同意画面は不成立）。
   - top document と同 origin の全 frame に `input[type=password]` が無い（controller の trusted 側で `DOM.getDocument`
     `pierce:true` を数える。数だけを使い、DOM の内容は agent に渡さない）。
   - 注入値の消去（ADR-0110 D1）が済んでいる、または注入した document が既に無い。
   成立したら `close_auth_section` → store の `browser_auth_section(false)` → relay 解除の順（ADR-0110 D3 順 10〜11 と同じ）。
3. **区間後の観測の常時条件（区間後のすべての agent 観測に毎回掛ける）。**
   - 観測時点の top document の origin が `read_origins` に入る。IdP（credential の exact_origin）と、それ以外の許可 domain の頁は
     読めない（navigate は今の許可 domain のまま可）。外れれば `observation_origin_denied`。
   - その document（と同 origin の frame）に `input[type=password]` があれば拒否（`password_field_present`）。session 切れで IdP の
     login form に戻った場合もここで止まる。再 login は新しい承認から（今の毎回承認のまま）。
   - `RedisplayGuard`（ADR-0111）を password について全応答・全 event に掛け続ける。
   - relay の拒否に cookie・storage 読取り（`Network.getCookies`・`Network.getAllCookies`・`Storage.getCookies`・`DOMStorage.*`・
     `IndexedDB.*`・`CacheStorage.*`）と、全 request の `Network.getResponseBody`・`Fetch.getResponseBody` を足し、agent へ送る
     `Network.*` event から `Cookie`・`Set-Cookie`・`Authorization` header を削る（今の postData 削除と同じ場所）。shim の grammar
     （eval・cookies・storage・CDP raw・fill/type 不可）は変えない。
   - Live View・takeover（ADR-0099/0100 の H4）は credential session の終わりまで今どおり出さない（agent の読み取りだけ再開する）。
     store には `browser_auth_section` と別に「credential を使った session」の印を残し、takeover / renew の拒否はこの印で続ける。
   - egress・navigation は今の allowed domains（manaba・IdP）のまま。読み取りの許可で domain は広がらない。
4. **opt-in していない site・条件不成立の session** は今と同じ（H3 を session の終わりまで、観測 action を harness から外す）。
5. **harness の policy。** 区間が条件どおり閉じた session だけ、`credential_harness_policy` が外している observation action のうち
   `post_login.actions` の実効集合を戻す。prompt の固定文も「ログイン後、read_origins の頁は snapshot / extract できる。
   IdP と password 欄のある頁は読めない」に変える（今は「無効」と書いている）。

6. **承認時の追加（Q3/Q4/Q6 の回答）。** `post_login.actions` に選べるのは `snapshot`・`extract`・`screenshot`・`download`・`click`。
   - screenshot: 区間が閉じた後、top document の origin が `read_origins` 内で、頁（top と同 origin の frame）に `input[type=password]`
     が 1 個も無いときだけ撮る。区間中・password 欄のある頁では `password_field_present` で拒否。画素は `RedisplayGuard` で検査できない
     ことを受け入れる（人の判断）。
   - download: 区間後、`read_origins` の頁から始まり、取得 URL の origin も `read_origins` に入るものだけ。大きさ・型の制限は通常の
     download と同じ。
   - click: 区間後、top document の origin が `read_origins` 内の頁だけ。login / IdP の頁での click（注入そのものの submit を除く）は拒否。
     課題の提出をさせないことは task の指示で扱い、ここでは強制しない（残るリスクに記録）。
   - いずれも実効集合は `post_login.actions ∩ task allowed_actions ∩ grant`。

## 代替案

| 案 | 内容 | 長所 | 短所・却下理由 |
|---|---|---|---|
| A | 人が Live View で login（takeover）し、agent が読む | Celeris が password を持たない | 毎回人が要る（監視の自動化にならない）。prod の Live View・takeover は未開放。注入しないので `RedisplayGuard` も無く、人が打った password の再表示を検出できない（T2 がむしろ弱い） |
| B | H3 を保ち自動化を諦める | 境界が今のまま | 目的を満たさない |
| B' | manaba の通知メールを受信箱（CoS inbox）で読む | login 不要、H3 不変 | 通知が届く設定・範囲に依存し、課題の本文や一覧の網羅は保証されない。人の判断次第で併用可（Q8） |
| C | 1 承認から username・password の 2 lease、または `max_uses=2` | broker の変更が小さい | 単回 lease の不変条件（ADR-0110 D5）を崩す。2 回の注入の間に頁が差し替わる TOCTOU が増える |
| D | username を site policy に平文で置く | guard から外す理由が明確 | username は credential（人）に属し site に属さない。登録経路・vault が二重になる |
| E | agent が `fill` で username を打つ | 実装が小さい | 区間中は agent 全面遮断（T1）。fill/type は grammar で不可 |
| F | 区間後の観測を opt-in 無しで全 site に戻す | 汎用 | T3 を人の選択無しに受け入れることになる |
| G | site 専用の固定抽出器（controller が課題一覧だけを構造化して返す） | 最も狭い。LLM に頁全体が渡らない | site ごとの専用コードが Celeris に入り、頁の変更で壊れる。将来の狭め方として残す |
| H | screenshot も区間後に許す | 視覚的な確認ができる | `RedisplayGuard` は画素を検査できない（ADR-0111 5）。Q3 |

## 影響

- **コード**:
  - `celeris-credentiald`: `CredentialPolicy.username_selector`・`post_login`、`validate`、`DescribePolicy`、`InjectionRequest.username`、
    `INJECT_PAIR_FUNCTION`・`cdp_frame`、照合 4b の 2 欄化、guard は password だけ。
  - `task-core`: `TrustedLogin`・`validate_trusted_login`（username selector、post_login の origin 検査）、store の site policy・auth 区間の印、
    policy hash、`browser_wait` の固定。
  - `task-worker`: `browser_cdp_sink.rs`（username 要素の解決、pair の sink frame 照合、区間終わりの条件判定、区間後の origin / password 欄検査）、
    `browser_shared_cdp.rs`（cookie・storage・response body の拒否と header 削除）、`browser.rs`（daemon 経路の区間終わり・harness policy・prompt 文）、
    `browser_launcher_run.rs`・`browser_launcher/{protocol,backend}.rs`（v5、区間終わり、store の解除時点）。
  - `task-api`: site policy API の欄、migration（`username_selector`、`post_login` の JSON 列）。
- **protocol**: launcher v4 → v5（D1-5）。injection IPC の `InjectionRequest` に任意欄を足すので `v` を上げ、旧 controller・旧 broker の
  組合せは `invalid_request` で fail closed。
- **設定**: config.toml の `[[api.browser_site_policies]]` に 2 欄。DB が正（prod-enablement D3）なので、manaba は API / web で更新する。
- **site policy UI**（web `/browser/settings`）: `username_selector` 入力、`post_login` の opt-in（read_origins と actions）。opt-in は
  「ログイン後の頁の内容（個人情報を含みうる）が LLM に渡る」ことを明記した確認を挟む。承認画面に両方を表示する。
- **運用手順**: `docs/ops/browser-launcher-credential-release.md` に manaba の policy 値（login_url、2 selector、post_login）、
  launcher の差し替え（v5、root）、host 実証の手順を足す。`celerisctl browser doctor` に site policy の新欄の検査。
- **試験**（外部ネットワーク無し、ローカル fixture、CPU 負荷無し）:
  - 既存の Shibboleth 風 fixture（`browser_sso_fixture.py`）に `j_username` を足し、SP が username・password の両方を受けたことを
    fixture の受信記録で確かめる（daemon・launcher 両経路）。
  - pair 注入の負例: username 欄が別 document・別 frame・型違い・0/2 個、selector 不一致（lease 未消費）、偽 sink frame（pair 関数の
    objectId 差替え・関数差替え）が Chrome に届かない。
  - 区間終わりの負例: IdP に留まる（エラー頁・同意画面）、password 欄が残る、期限切れ → `post_login_unconfirmed` で観測停止のまま。
  - 区間後の負例: IdP 頁・read_origins 外の頁の snapshot、session 切れで login form に戻った頁、cookie / storage / response body の CDP、
    Network event の Cookie header、頁 script による password の再表示（`redisplay_detected`）。
  - sentinel（ADR-0110 D4 の面すべて）で password が 0 件。username は「Celeris が書く面（event・audit・DB・log）」で 0 件、
    agent 観測には出てよい（D1-2）ことを試験名と期待で明記する。
  - opt-in 無しの site で挙動が変わらない回帰試験。

## 残るリスク

- **T3 を受け入れる**: read_origins の頁にある個人情報（氏名・学籍番号・成績・課題内容）や頁に埋め込まれた token が、LLM provider・
  artifact（extract は artifact に自動登録される）・会話に渡る。どの model（外部 provider か local Qwen か）に渡すかは Q5。
- **prompt injection**: manaba 上の他者が書いた文面（掲示・課題本文）が agent を誘導しうる。click が承認不要（2026-10-08 決定）なので、
  認証済み session で提出・削除などの操作を押せる。読み取り専用の task には click を task policy で外すことを推奨（Q6）。
- **guard の限界**: `RedisplayGuard` は変換（大小・逆順・部分・圧縮）や画素を検出しない（ADR-0111 5）。screenshot は A3 で許したので、
  画素に password が出る頁は「password 入力欄がある頁では撮らない」以外の防御が無い（人が受け入れた）。
- **提出操作**: click を read_origins 内で許すので、prompt injection や誤操作で課題を提出・削除しうる。task の指示で禁じ、ここでは強制しない（A6）。
- **non-httpOnly cookie・頁内 token**: snapshot の本文に token が書かれていれば読める。cookie は grammar と relay の拒否で直接は読めないが、
  頁が表示した値までは除けない。
- **username の露出**: guard を掛けないので、頁に表示された username は agent・LLM・artifact に入る。
- **同意画面**: IdP が属性送信の同意を毎回求めると、区間が閉じず読めない（fail closed なので安全側）。
- **毎回の承認**: 監視 run のたびに credential_use の人間承認が要る（今の決定のまま）。定期監視の運用負荷は残る（Q7）。
- 新しい区間終わり判定と relay の拒否表は保守対象が増える。判定の誤りは「読めない」側に倒す設計だが、実装の誤りで開く可能性は試験で詰める。

## 人の確認事項

- **Q1** username を「機密だが秘密ではない」とし、保存・ログは秘密と同じ扱いのまま、ログイン後の頁に表示された username は agent が読めてよいか。
  （否なら username にも guard を掛け、manaba の多くの頁が読めなくなる見込み）
- **Q2** ログイン後の読み取りを site policy の opt-in（`post_login`）にし、毎回の credential_use 承認画面にその旨を出す形でよいか。
  それとも opt-in に加え task ごとの別承認も要るか。
- **Q3** screenshot は v1 で区間後も不可でよいか（課題一覧は snapshot / extract で足りる見込み）。
- **Q4** download（課題の添付 PDF 等）は v1 で不可でよいか。要るなら read_origins 内・承認付きなどの条件を別に決める。
- **Q5** ログイン後の頁を読む run の LLM を限定するか（例: local Qwen だけ、外部 provider 可）。model routing の task に条件を渡すか。
- **Q6** manaba の監視 task は click を外した読み取り専用（navigate・snapshot・extract・scroll）にしてよいか。課題一覧への移動に click が要るなら、
  read_origins 内の click だけ許すか。
- **Q7** 監視 run ごとの毎回承認を受け入れるか（persistent identity・standing approval は今回入れない前提）。
- **Q8** 通知メールの受信箱取り込み（代替 B'）を併用・先行する価値はあるか。
- **Q9** manaba の site policy 値: `login_url`（IdP の login 頁へ入る URL。IdP の root には form が無い）、`username_selector`
  （`#username` / `input[name="j_username"]` 等）、`read_origins = ["https://manaba.tsukuba.ac.jp"]` を人が実頁で確認して入れるか、
  それとも host 実証の段で運用セッションが調べて提案するか。

### 回答（人、2026-10-09）

- **A1** 推奨どおり。username は「機密だが秘密ではない」。保存・log は password と同じ（vault の envelope、DB・event・log・API 応答・audit
  に書かない）。ログイン後の頁に表示された username は agent が読めてよい（`RedisplayGuard` は password だけ）。
- **A2** site policy の opt-in（`post_login.read_origins`）と、毎回の credential_use 承認画面への表示。task ごとの別承認は追加しない。
- **A3/A4** screenshot と download の両方を v1 で許す（「両方許す」）。ただし `read_origins` 内だけ、認証区間が閉じた後だけ。
  screenshot は頁に password 入力欄がある間（と区間中）は拒否。download は `read_origins` 内だけで、通常の大きさ・型の制限を掛ける（D2-6）。
- **A5** 外部 provider 可（通常の routing）。LLM の限定はしない。
- **A6** click は区間後の `read_origins` 内だけ許す（login / IdP の頁では注入そのもの以外の click をしない）。agent に課題を提出させない
  ことは task の指示の事項とし、ここでは強制しない。
- **A7** run ごとの credential_use 承認を受け入れる。
- **A8** 通知メールの取り込み（B'）は今はしない。
- **A9** 運用セッションが host 実証の段で manaba / IdP の実頁を調べ、site policy の値（login_url、username_selector、read_origins）を
  提案する。manaba の値はコードに書かない。

## 実装の作業単位（承認後、概算）

| WU | 範囲 | 主な確認 |
|---|---|---|
| policy | migration・store・API・config seed・`CredentialPolicy`・`TrustedLogin`・`validate_trusted_login`・policy hash・doctor | 形式検証の正負、revision、承認画面の固定値 |
| broker | `InjectionRequest.username`、`INJECT_PAIR_FUNCTION`、照合 4b、guard、IPC 版 | 単回 lease、selector_mismatch で未消費、sentinel |
| controller | `browser_cdp_sink.rs` の username 解決・pair sink frame 照合・区間終わり判定・区間後の検査、`browser_shared_cdp.rs` の拒否表と header 削除 | 実 Chromium の fixture 負例一式 |
| daemon-wire | `browser.rs` の区間終わり・harness policy・prompt・store 印 | 回帰（opt-in 無し）、daemon 経路 e2e |
| launcher-v5 | protocol v5、backend の login・区間終わり、`browser_launcher_run.rs` の版確認と store 解除 | v4/v5 の版ずれ拒否、launcher_credential_ 試験 |
| web | site policy 編集 UI と opt-in 確認、承認画面の表示 | e2e（`web/e2e/browser/settings.spec.ts`） |
| ops | 手順書、host 実証（root、運用セッション）、manaba の実 login 1 回（人の資格情報、人の承認） | 証跡を progress に記録 |

順は policy → broker → controller → daemon-wire / launcher-v5 → web → ops。D1（username）だけを先に出し、D2 を後に出す分割も可
（D1 だけでは manaba の課題監視は成立しない）。
