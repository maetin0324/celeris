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

## 付記 2026-10-09: 実装時の決定（branch `ops/credential-login-v5`）

本文の範囲内で、実装で決めたこと。

1. **store の区間と credential session の印。** 区間が条件どおり閉じたら store の `browser_auth_section(false)` を書き（agent の操作
   gate が再び通る）、Live View・takeover の拒否は control 状態の `credential_used`（`enter_auth_section` で立ち、session の終わり
   まで下りない。旧い state の JSON は欄なし = 偽）で続ける（D2-3）。worker event の破棄（`LiveEmitter` の guard）は区間を閉じた
   時点で外す（extract 等の artifact 登録に要る）。Live View の URL は credential session では元から出さない。
2. **観測ごとの検査の単位は CDP command。** 区間後、agent の page session への command は、固定の「頁を読まず操作もしない」一覧
   （domain の enable、navigate、emulation、Target・Browser、`Page.createIsolatedWorld` 等。`post_login_control_method`）以外の
   すべてが、実行の**前と後**に検査を通る。検査は controller だけの CDP session（agent に event を流さない）で、top document の
   origin が `read_origins` にあること（空の `about:blank` の tab は可）と、controller の isolated world で数えた password 入力欄
   （open shadow root・同 origin の frame を含む）が 0 個であること。どちらかを確かめられなければ拒否。区間を閉じる条件の
   password 欄も同じ数え方。
3. **download。** 区間後、`read_origins` 外の URL の download は `downloadWillBegin` を受けた時点で取消を送る。取消より先に完了した
   場合は、その session の観測を以後すべて止める（fail closed）。download 元の origin は egress（task の許可 domain）でも絞られる。
4. **CDP の受信上限。** screenshot・大きな DOM/AX tree の応答は 1 message で sink frame の上限（64 KiB）を超えるので、browser から
   読む 1 message の上限を 64 MiB にした（sink frame の上限は 64 KiB のまま）。
5. **launcher の版ずれ。** v5 launcher は username 欄も post_login も持たない（v4 の形の）要求には `observation` の無い v4 の形で
   答える（launcher を先に差し替えても旧 daemon が decode できる）。v5 の daemon は `observation` の無い答えを「観測停止のまま」
   と読む。launcher は session 開始時に post-login の verb を含めて起動し、login の後は `AfterLogin`（再開した・その verb が
   post_login.actions にある）でしか snapshot / extract / screenshot / download / click を通さない。
6. **実効集合。** `post_login.actions` は task の harness policy（task ∩ grant、承認が要る action は除く）に入っている action
   だけ、`read_origins` は task の許可 domain に入る origin だけが効く。どちらかが空なら opt-in 無しと同じ。
7. **登録し直し。** broker の vault は登録時の site policy（login URL・selector・post_login）を写して持ち、承認で固定するログインは
   その写しなので、site policy を変えた後は credential を登録し直す（運用手順 §4）。
8. **残る制限（付記 2026-10-10e で解消）。** launcher runtime の screenshot / download の artifact は daemon に渡らない（`LauncherExecutor` の既存の制限）。protocol v8 の artifact transfer で run の `browser/output` に届くようにした。
   A3/A4 で許したが、launcher runtime では成果物にならなかった。artifact の受け渡し（protocol の拡張）は付記 2026-10-10e。

## 付記 2026-10-10: 本番の `post_login_unconfirmed` と、待ち方・理由の固定 code（launcher protocol 6）

本番 run 01M4HR6PY6NJXSPWTW9X5AZ81Y（release 93cb3a36、筑波大 IdP → manaba）は `browser.credential_use: success` の直後に
`post_login_unconfirmed` になり、どの条件が外れたかの記録が無かった。session 開始（01:52:53.8Z）から login 完了の記録
（01:53:09.75Z）まで 16 秒で、IdP の中継（localStorage の interstitial の自動 POST、ログイン後の localStorage 書き込みの
interstitial、属性送信の同意頁、SAML の自動 POST）を含む login と 15 秒の待ちの両方には足りないので、待ちが期限前に終わった
（Page.getFrameTree などの一時的な CDP 失敗を `Err` として即座に打ち切っていた）か、IdP の中継・同意頁で期限を迎えたかの
どちらかと見る。Shibboleth IdP の既定は同意の記録を browser 側（localStorage）に持つので、毎回新しい profile の
session では同意頁が毎回出ると見込む。

決定（D2-2 の補い。安全側の条件は変えない）:

1. **待ち方。** 区間は従来どおり「ログイン頁を離れ・top が read_origins・password 欄 0」のときだけ閉じる。待ちは最長 60 秒
   （15 秒から延長）で、IdP の頁・SAML の自動 POST・about:blank 等の中間状態と一時的な CDP 失敗は打ち切らずに待ち続ける。
   観測はその間ずっと止まったまま。
2. **早く終える状態。** IdP の属性送信の同意頁（Shibboleth の固定の欄名 `_shib_idp_consentIds` 等で判定）と、IdP の
   login form の再表示（ログイン失敗）は、3 秒続いたら待ちを終える。同意は人の判断なので、agent にも controller にも
   押させない（承認画面の後に人がどうするかを決める）。
3. **理由の固定 code。** 観測を再開しなかった理由を progress に出す: `post_login_consent_required`・`post_login_idp_login_form`・
   `post_login_idp_timeout`・`post_login_password_field`・`post_login_other_origin`・`post_login_login_document`・
   `post_login_no_document`・`post_login_check_failed`・`post_login_resume_failed` と、top の origin の分類
   （`idp` / `read_origin` / `other` / `none`）。URL・頁の内容は出さない。
4. **launcher protocol 6。** `authenticate_result` に `held_reason`（固定 code）を足した。daemon は post_login を使う login に
   v6 を要求する（username 欄だけなら v5 のまま）。launcher は要求の `report_held_reason` が真のときだけ `held_reason` を返すので、
   v5 の daemon と v6 の launcher も併存できる。
5. **task policy。** 本番の task policy（`https://*.tsukuba.ac.jp`、全 action）で post_login の実効集合は空でなく、区間が閉じれば
   snapshot / extract などは戻る。その run の「not permitted by the task browser policy」は観測停止のままの状態の表示で、
   policy の欠けではない（試験 `post_login_read_with_the_production_task_policy_shape_enables_reading`）。

## 付記 2026-10-10b: IdP の属性送信の同意頁を controller が固定ボタンで押す（人の決定）

人の決定（2026-10-10）: IdP の属性送信の同意頁では、controller が**固定の**同意ボタンを自動で押してよい。

設計:

1. **site policy に `consent`（任意）を足す。** `consent: {selector, choice_selector?}`。`selector` は同意を送るボタン
   （`button` か `input[type=submit]`）、`choice_selector` は任意の選択肢（radio）。どちらも他の selector と同じ文法・長さ
   制限で、管理者だけが設定する（API・web の site policy 編集。モデル・worker・task policy からは入らない）。押す回数は
   1 login につき 1 回に固定（設定値ではない）。`consent` は `post_login` と組でだけ置ける（区間後の待ちの中でだけ使うため）。
   選択肢の既定: `choice_selector` を置かなければ選択肢には触らず頁の既定のまま送る。置くなら最も狭い同意
   （Shibboleth の「次回も確認する」= `_shib_idp_doNotRememberConsent`、その login だけの同意）を推奨し、運用手順にも
   そう書く（同意の記録は browser 側で session の終わりに消えるので、どちらでも次回また出るが、IdP 側に長期の同意を
   残さない方を選ぶ）。
2. **押してよい条件（全部）。** 人がこの回の credential_use を承認した run の中で、login の submit の後の区間後の待ちの間、
   controller 自身が（agent・LLM は決して押さない。区間中は agent の CDP は全面遮断のまま）、login tab の top document が
   site policy の `exact_origin`（IdP）にあり、その頁が同意頁と分類され（既存の分類器: Shibboleth の固定の欄名）、
   `selector` がその document でちょうど 1 要素（ボタン、form の中）に一致し、`choice_selector` があればそれも同じ form の
   radio ちょうど 1 要素に一致するとき。押すのは 1 login につき最大 1 回。押した後は既存の待ちを続け、同意頁が残る・
   別の同意頁が出るなら今どおり `post_login_consent_required` で止める。
3. **承認画面と固定。** `consent` は承認で固定する `TrustedLogin` に入り（承認画面に表示し、承認後の変更は
   `policy_changed`、broker の grant も登録時の写しと照合）、broker の vault の写しにも入る（site policy を変えたら
   credential を登録し直す点は同じ）。
4. **同意頁の欄名の診断。** 同意頁で止まったとき（`consent` 未設定・selector 不一致・押しても残った）は、progress に同意 form の
   `button` と `input[type=submit|radio]` の `name`・`type`・`value` だけを出す（ASCII の印字可能文字だけ、各 64 文字まで、
   最大 12 個、全体 512 文字まで。label・本文・利用者の値・URL は出さない）。運用者が実頁の selector を決めるために使う。
5. **launcher protocol 7。** `authenticate` に `consent` と `report_consent_controls`、応答に `consent_controls` を足す。
   consent を使う login は v7 を要求する（v6 の launcher は daemon が承認消費前に「protocol 7 required」で拒否）。
   応答の新しい欄は要求が求めたときだけ返す（v6 の daemon と v7 の launcher も併存できる）。

## 付記 2026-10-10c: 本番の click / snapshot の失敗、隠れた password 欄、ログイン後の navigate

本番 run 01M4J3S5706DNVEZA5EASGV10C（ログインと区間の終わりは成功）で、manaba の `/ct/home`・`/ct/home_course` だけ
snapshot・screenshot・click が全部失敗し、要約頁では成功した。実 agent-browser 0.38.1 を shim と同じ argv で relay 越しに
動かす試験（`real_agent_browser_reads_clicks_and_is_refused_after_login`）で、agent-browser の click の CDP
（`DOM.scrollIntoViewIfNeeded`・`DOM.getBoxModel`・`DOM.resolveNode`・`Runtime.callFunctionOn`・`Input.dispatchMouseEvent`）は
区間後の検査を通り、click 自体は成功することを確かめた。失敗は頁ごとの検査で、manaba の home 系の頁が持つ（と見られる）
折り畳まれた login 用の部品の空の password 欄を「password 欄あり」と数えて、その頁の観測を全部拒否していたことによる
（click が全部失敗したのは、課題への click をそれらの頁で試みたため）。

決定:

1. **password 欄は「生きている」ものだけ数える。** 描画されている（layout の箱があり `visibility: hidden` でない）か、値を
   持っている `input[type=password]` だけを数える（区間の終わりの条件と区間後の観測ごとの検査の両方）。隠れていて空の
   password 欄は画面に何も出さず値も持たないので、T1・T2（入力中・再表示の秘密）に関わらない。頁の script が値を入れれば
   次の検査で数え、注入値の再表示は `RedisplayGuard` が捨てる（変えない）。
2. **snapshot に link の URL を出す。** shim の snapshot は `snapshot --urls`（2026-10-10d: 当初の `-i` は本文を落としたので外した）。agent は課題の URL を snapshot から読み、
   click の代わりに `open` できる（prompt にも書く）。read_origins の頁の URL は T3 で受け入れた範囲。
3. **ログイン後の navigate は read_origins だけ。** 試験で、agent の `Page.navigate`（agent-browser の `open`）が他 origin の
   添付ファイルに向くと、Chromium は download の event（`downloadWillBegin`）を出さずに保存することが分かった。download の
   即時取消（付記 2026-10-09 3）はこの形を止められないので、区間後の agent の `Page.navigate`・`Target.createTarget` の URL は
   read_origins（と about:blank）に限る。他の許可 domain の頁は元から読めない（観測の検査で拒否）ので、読める範囲は
   変わらない。read_origins の頁から server の redirect で他 origin に移ることは今どおり起こりうるが、その頁は読めない。
   残る隙: read_origins の URL が他 origin の添付へ redirect するときの `open` は、download の event が出ず取消できない
   （egress の許可 domain の範囲に限られる）。

## 付記 2026-10-10d: snapshot は本文と link URL の両方

付記 2026-10-10c の `snapshot -i --urls` は対話要素（見出し・link）だけを返し、manaba の課題説明・教材頁（`/ct/page_*`）の
本文が agent に届かなかった（本番 1904b3c2 の報告）。shim の snapshot を `snapshot --urls`（agent-browser 0.38.1 で全体の
accessibility tree と link の URL を返す。`-c` は本文の段落を落とすので使わない）にし、出力の上限（16000 文字）を超える長い
本文は extract で読むよう prompt に書く。区間後の観測の検査（read_origins・生きている password 欄・RedisplayGuard）は
command 単位なので変わらない。

## 付記 2026-10-10e: launcher runtime の screenshot・download を run の browser/output に渡す（launcher protocol 8）

manaba の課題（task 01M4GYJ3XGJNWZQDF35F1MDE0H）で授業資料が PDF だけのとき、agent は資料を読めなかった。launcher runtime では
screenshot / download の file が launcher の session dir（uid 995 の sandbox）に残り、`LauncherExecutor` が両 verb を失敗として
返していたため（付記 2026-10-09 8 の残る制限）。Q3/Q4 で許したログイン後の read_origins 内の screenshot・download を、agent が
読める file として run に届ける。

決定:

1. **daemon が同じ session の接続で pull する（protocol v8）。** 新しい要求 `fetch_artifact {session_id, lease_id, name, offset}` と
   応答 `artifact {name, kind, size, offset, data}` を足す。`data` は最大 32 KiB の base64（64 KiB の frame に収まる）、`kind` は
   固定の型。launcher から daemon へ何かを自発的に送る notification は使わない（同期の request/response を壊さない）。
   - launcher は**その session が作った名前だけ**を返す（`screenshot-<32 hex>.png` / `download-<32 hex>.bin`。session ごとの一覧に
     無い名前は `unauthorized`）。path は受け取らない・返さない。`O_NOFOLLOW` で開き、通常 file でなければ拒否。
   - session の policy に生んだ verb（screenshot / download）が無ければ `fetch_artifact` 自体を拒否する。lease・接続の持ち主・
     `isolation_ok` の検査は他の要求と同じ。
2. **上限。** 1 file 10 MiB（shim の既存上限と同じ）、1 session（daemon 側は 1 run）32 件。超えたら launcher は `limit`、daemon は
   固定理由 `browser_artifact_too_large` / `browser_artifact_count_limit`。
3. **型。** 先頭の byte で判定する（拡張子・page の申告 MIME は信じない）。許すのは PNG・JPEG・GIF・WebP・PDF・OOXML（zip 容器の
   docx/xlsx/pptx）・OLE2（doc/xls/ppt）。screenshot は PNG だけ。それ以外（HTML・実行形式・不明）は launcher が
   `artifact_rejected`、daemon も受け取った byte を再判定し `browser_artifact_type_rejected`。
4. **置き場所と agent の読み方。** daemon は shim が名付けた `runs/<run>/browser/output/<name>` に新規（`create_new`・
   `O_NOFOLLOW`・0600）で書き、失敗した途中の file は消す。既存の events → `ArtifactRef` 登録（`forward_events`）がそのまま
   task の artifact にする。shim は download の先頭 byte が PDF 等なら同じ dir に `download-<hex>.pdf` の hard link を作り、応答の
   `file` に返す（agent の既存の文書読み取りは拡張子で PDF を本文化する）。daemon 経路の download にも同じく働く。
5. **秘密。** 応答の型は name・kind・長さ・offset・data だけで、URL・header・cookie・path の欄は無い（`deny_unknown_fields`）。
   file の byte は event・progress・log・protocol error に入れない（client の診断用の生応答の抜粋も artifact 応答では伏せる）。
   download は read_origins 外の取消（付記 2026-10-09 3）を変えない。取消された download は launcher の action 失敗になり、
   daemon は file を作らず `browser_artifact_action_failed` を返す。
6. **版と fail closed。** `PROTOCOL_VERSION` を 7 → 8 にする（`ARTIFACT_PROTOCOL = 8`）。daemon は session 開始後に `hello` で版を
   確かめ、8 未満なら screenshot / download を launcher に頼まず、固定理由 `browser_launcher_protocol_artifacts_required` で失敗に
   する（shim は理由をそのまま agent に返し、run には progress `browser.artifact: <理由>` を残す）。他の verb・credential login は
   従来どおり動く。v7 daemon と v8 launcher の組合せは、v7 daemon が `fetch_artifact` を送らないので従来どおり。
7. **Live View の版。** Live View の frame（ADR 2026-10-10-browser-launcher-live-view-frames、task 01M4JAK3MY が v8 を予定）は
   v9 に送る。
8. **launcher 側の権限。** sandbox 内の action child（subuid）は成功した screenshot / download の file を 0644 にし、launcher
   （別 UID）が読めるようにする（session dir は launcher だけが辿れる 0700 の root の下）。
9. **運用。** launcher を daemon と同じ commit から再 build して差し替える（`docs/ops/browser-launcher-host-setup.md` の
   「launcher protocol v8」）。

試験: `browser_launcher_run_tests.rs` の `launcher_artifacts_*`・`launcher_serves_only_names_its_session_produced`・
`launcher_shim_download_lands_in_the_run_output_as_a_readable_pdf`・`artifact_responses_carry_no_secrets_*`、
`backend_tests.rs` の `launcher_artifact_reader_is_bounded_typed_and_refuses_symlinks`、`client.rs` の
`artifact_chunks_are_withheld_from_diagnostics`。

残る制限: 実 launcher（uid 995・subuid の sandbox）での往復は host で人が確かめる（運用手順の確認）。launcher 側の 32 件の上限は
`RuntimeSession` の中で数え、偽 backend の試験では daemon 側の上限だけを確かめている。

## 付記 2026-10-10f: launcher の Chrome が PDF を viewer で開く問題と、artifact 失敗の固定診断（protocol 8 のまま）

release fe20a5a7（protocol 8）の後、task 01M4GYJ3XGJNWZQDF35F1MDE0H の run 01M4JWEYZAG6B7H0WTD451042T で manaba の PDF の
download と screenshot が全部 `browser_artifact_action_failed` になった。daemon は launcher の失敗を全部この 1 つの理由に
畳み、launcher は action の失敗を何も記録しないため、本番の log からは原因が分からなかった。

調べたこと:

- 実 launcher（本番の socket・uid 995・subuid）で screenshot と `fetch_artifact` は通る（about:blank、ログイン前）。session dir の
  権限・UMask=0077・output の 0644 化は原因ではない。
- daemon 経路の試験は chrome-headless-shell を使う。本番の launcher は `launcher.toml` の `chrome` = Chrome for Testing 153
  （`/opt/celeris-browser/chrome/chrome`）で、**PDF viewer を持つ**。Content-Disposition の無い PDF（inline 配信）の link を
  click すると、tab が viewer に移り download が始まらない。agent-browser の `download` は 30 秒待って timeout する。
  実 sandbox（bwrap + sandboxd + action runner + 実 agent-browser 0.38.1）・ログイン後 mode で再現した
  （`browser_sandbox_artifacts.rs`。修正を外すと `download Inline PDF: … error_class=timeout` で落ちる）。
- 本番の 5 件のうち 1 件目（12:27:09）は 30 秒待ちに合う。残り（0.16 秒差の連続失敗、screenshot）は速い失敗で、fixture の同一
  origin の viewer では再現しなかった。tab が viewer、または read_origins 外（PDF の配信が別 host へ redirect する場合）に
  残り、ログイン後の gate が拒否した可能性が高いが、本番の記録が無いので断定しない。

決定:

1. **PDF は常に download にする。** sandboxd が Chrome 起動前に、新しい profile に限り `Default/Preferences` を
   `{"plugins":{"always_open_pdf_externally":true}}` で作る（既存の profile は触らない）。viewer を持たない
   chrome-headless-shell では何も変わらない。read_origins 外への redirect で始まる download は従来どおり relay が取り消す。
2. **失敗の固定診断（秘密を出さない）。**
   - action runner（`browser_action.py`）は応答に `runner_reason`（`request_invalid` / `config_invalid` / `exec_failed` /
     `exec_timeout` / `agent_browser_exit` / `output_missing` / `output_chmod_failed` / `runner_error`）と、agent-browser の
     error 文を固定語彙に写した `error_class`（relay の code 6 種・`policy_denied`・`unknown_ref`・`timeout`・
     `download_error`・`screenshot_error`・`target_closed`・`no_page`・`other`・`unparsed`・`no_error_text`）を足す。
     error 文そのものは返さない。成功と言いながら生成名の file が無ければ `output_missing` で失敗にする。
   - controller は relay が拒否した agent の command を（CDP method を `Domain.method` の形に縮めて、固定 code と）最後の 8 件
     まで持つ。read_origins 外の download の取消（`download_origin_denied`）と取消後の完了（`download_breach`）も記録する。
   - launcher は action が失敗すると 1 行だけ journal に書く:
     `action <verb> failed: session=<id> code=<ErrorCode> launcher_reason=… | status=… runner_reason=… error_class=… gate=<method!code,…>`。
     runner の値は `[a-z_]{1,40}` 以外を `invalid` にする（sandbox 内から任意の文を journal に入れさせない）。session の policy に
     よる拒否と action の期限切れも同じ形で書く。URL・selector・page の文・引数は書かない。
   - daemon は launcher の error code ごとに理由を分ける: `unauthorized` → `browser_artifact_action_refused`、`timeout` →
     `browser_artifact_action_timeout`、`limit` → `browser_artifact_count_limit`、`isolation_failed` →
     `browser_artifact_isolation_failed`、応答なし → `browser_artifact_launcher_unavailable`、それ以外（runner・gate の失敗）は
     従来の `browser_artifact_action_failed`。どれも shim の `browser_[a-z0-9_]` の語彙。
3. **版は 8 のまま。** wire の型は変えない（runner の欄は sandbox 内の file、daemon の理由は既存の action 応答の失敗の写し方）。
   protocol 9 は Live View（task 01M4JAK3MY）に予約。
4. **運用。** launcher と **sandboxd の両方**を再 build して差し替える（Preferences は sandboxd、runner と診断は launcher に入る）。

残る制限: `target="_blank"` の link からの download（popup で始まる download）は agent-browser が拾えず 30 秒で timeout する
（headless-shell でも同じ）。PDF の URL の `open` も navigation の download になり file は渡らない。本番の速い失敗の原因は、差し替え後の
journal の `gate=` と `error_class=` で確定させる。

## 付記 2026-10-10g: credential 登録 wait を site policy のログイン origin に結び付ける・`policy_changed` の固定 sub-reason

release b865201c の後、task 01M4GYJ3XGJNWZQDF35F1MDE0H の run が 3 回続けて 2 秒以内に
`infra_requeue: … policy_changed` で終わった。原因は release ではない。agent が
`request-credential manaba-tsukuba https://manaba.tsukuba.ac.jp …` と **LMS の origin** を名指しした（12:13 の成功例では IdP
`https://idp.account.tsukuba.ac.jp`）。wait 01M4K0SATAY54M4NM96E6R5WS8 はその origin のまま開き、人はそこに登録した。登録時の
site policy の照合（`policy_id` と `exact_origin` の一致）に当たらず、vault には login URL・selector の無い credential が
入った（vault の非秘密の欄で確認: `exact_origin=https://manaba.tsukuba.ac.jp`、`login_url` なし）。以後の run は毎回
`describe_policy` が credentiald の `denied` で落ち、`policy_changed` になっていた。登録済みの wait は期限で閉じないため、task は
そこから進めなかった。

決定:

1. **登録 wait の origin は site policy のログイン origin。** store が `waiting_for_auth` の wait を開くとき、`credential_policy_id`
   に site policy があれば、agent の名指した origin が `exact_origin` か `post_login.read_origins` のどれかなら `exact_origin` に
   結び付ける。それ以外の origin は `browser_wait_invalid:credential_origin` で開かない。site policy の無い policy は従来どおり。
2. **使えない登録は run を止めない。** credentiald が `denied` / `invalid_request` / `not_found` で答える（登録そのものが
   ログインに使えない）とき、承認 wait を開かずに run を続ける（credential は使わない。progress
   `browser.credential: registered credential unusable (<sub-reason>)`）。agent は credential を頼み直せる。credentiald に
   届かない・vault が鍵掛かり等の一時的な失敗は従来どおり run を失敗にする。
3. **`policy_changed (<sub-reason>)`。** 固定語彙: `supervisor_missing`・`runtime_dir_missing`・`request_encode`・
   `credentiald_unreachable`・`describe_<credentiald の code>`（`describe_denied` 等）・`trusted_login_missing`・
   `policy_id_mismatch`・`pinned_login_missing`・`pinned_login_invalid`・`credential_reference_missing`・
   `pinned_differs_<欄>`（login_url・selector・post_login・consent など、どの欄が変わったかだけ）。lease の付与は
   `policy_changed_revision` / `policy_changed_policy_id`。値は入れない。daemon log にも同じ sub-reason を warn で出す。
   wait を開けないときも store の固定 code を `browser wait could not be opened (<code>)` に載せる。

運用: daemon を更新した後に task を再開すれば、使えない登録は飛ばされ、agent が頼み直した wait（IdP の origin）に人が登録
し直す。launcher・sandboxd・credentiald の再 build は要らない。

## 付記 2026-10-10h: agent の頁の JavaScript dialog は controller が固定の既定で答える

release 342ec107 の本番（task 01M4GYJ3XGJNWZQDF35F1MDE0H、14:40:47Z）で、manaba の資料頁の `download @e28`（link
「Slides1 Slides1.pdf」、URL は `.../Slides1.pdf?view=full`）が付記 2026-10-10f の診断で
`code=bad_request status=1 runner_reason=agent_browser_exit error_class=cdp_command_failed gate=Input.dispatchMouseEvent!sink_failed`
になった。`sink_failed` は controller が CDP の返事を 5 秒待って諦めたもの（`read_response` の期限）。

原因（fixture で再現）: click で JavaScript dialog（alert・confirm・beforeunload）が開くと、Chrome は dialog が閉じるまで
その `Input.dispatchMouseEvent` に答えない。relay は agent の command を 1 つずつ通す（controller の lock を返事まで持つ）ので、
agent-browser は dialog を閉じる `Page.handleJavaScriptDialog` を送れず（agent-browser は alert と beforeunload を既定で自動で
受け入れる）、click は 5 秒で `sink_failed`、tab は dialog で止まったままになる。直接接続の agent-browser では起きない。
manaba の link のどの dialog かは本番の頁を見ていないので確定していない（alert / beforeunload なら今回の修正で通る。confirm
なら下の 1 のとおり通らず、`gate=Page.javascriptDialogOpening!dialog_dismissed_confirm` が出る）。

決定:

1. controller は agent の頁（controller 自身の private session 以外）で `Page.javascriptDialogOpening` を受けたら、その場で
   `Page.handleJavaScriptDialog` を送る。`alert`・`beforeunload` は受け入れ（agent-browser の既定と同じ）、`confirm`・`prompt`
   は退ける（site の確認を agent の代わりに与えない。提出・削除の確認を通さない）。
2. 記録は dialog の種類だけ（`Page.javascriptDialogOpening!dialog_accepted_alert` / `_beforeunload` /
   `dialog_dismissed_confirm` / `_prompt` / `_other`）。dialog の文言・URL は記録しない。開いた event は agent に渡さない
   （答え済み）。閉じた event はそのまま渡る。
3. protocol は 9 のまま。controller は launcher の process と daemon の両方にあるので、**launcher の再 build** と daemon の
   更新が要る。sandboxd は変わらない。

残る制限: confirm で守られた download は通らない（agent は click 後に何も起きず、download は 30 秒で timeout）。通すかは人の
決定（提案）。

## 付記 2026-10-10i: agent の command は応答の遅い navigation の間も待つ（controller の 5 秒をやめる）

付記 2026-10-10h の後も（release f42a181b、15:48:02Z）同じ link の download が
`gate=Input.dispatchMouseEvent!sink_failed` で失敗した。dialog の記録は無く、dialog ではなかった。Live View は流れていた。

調べたこと（fixture・Chrome for Testing・実 agent-browser・Live View の screencast を launcher と同じく controller 経由で流す）:

- click で始まった navigation の応答（PDF）が遅いと、Chrome はその頁への command（`Page.getFrameTree`・`Runtime.evaluate`・
  入力）に、応答が届くまで答えない（7 秒遅れの応答で、別の CDP session からの `Page.getFrameTree` も 5 秒を超えて待った）。
- controller は CDP の返事を 5 秒しか待たない（`TIMEOUT`）。click 本体か、ログイン後の gate の検査（click の直前・直後の
  `Page.getFrameTree`）がこの間に当たると失敗する。fixture では検査側で `observation_origin_denied`、本番では click 本体で
  `sink_failed` になった（どちらに当たるかは応答の時機による）。ログイン後の gate の無い daemon の素の経路では起きない。
- Live View の screencast（ack は controller の lock を取る）は、この失敗の原因ではなかった（Live View を流しても止めても同じ。
  修正後は Live View を流したまま通る）。
- manaba の応答が実際に何秒かかったかは本番では測っていない。

決定:

1. agent の command（とその gate の検査）は Chrome の返事を最大 25 秒待つ（`AGENT_TIMEOUT`。agent-browser 自身の command
   timeout より短い）。controller 自身の操作は従来の 5 秒のまま。待つ間は controller の lock を持つので、Live View の frame は
   その間止まる。
2. 待ちが期限を越えたら固定 token を記録する: `Controller.reply!cdp_reply_timeout`、agent の頁の document の応答が来ていなければ
   `Controller.seenWhileWaiting!document_response_pending`、待つ間に Chrome が告げたもの（`navigation_requested`・
   `frame_started_loading`・`document_request`・`document_response`・`download_will_begin`・`download_progress`・
   `target_created`・`dialog_opening`・`frame_navigated`）。URL・request id は記録しない。
3. protocol は 9 のまま。controller は launcher の process にあるので **launcher の再 build** が要る（daemon も同じ commit）。

残る制限: 応答が 25 秒を越える download は失敗する（journal の token で分かる）。controller が href を自分で取りに行く経路
（coordinator の案）は作っていない（提案）。

## 付記 2026-10-10j: 新しい tab で開く file link の download・download 失敗の固定診断（agent にも返す）

付記 2026-10-10i の後（release edccce6b、18:05:31Z）、同じ link の download は relay・gate の失敗なしに
`runner_reason=exec_timeout error_class=none gate=none` で終わった。agent-browser は download の始まりを見ないまま待ち続けた。
人の決定: click で download する方式のまま原因を探す（controller が href を取りに行く代替は作らない）。

調べたこと（fixture・Chrome for Testing・実 agent-browser・ログイン後 mode・Live View）:

- `target="_blank"` の link を click すると、Chrome は新しい tab を開き、その tab の navigation が download になる。agent-browser
  0.38.1 は自分の tab の download しか待たず、30〜45 秒待って失敗する。Chrome は file を `/session/output/<guid>` に保存するが、
  その tab には誰も attach していないので `Browser.downloadWillBegin` / `downloadProgress` は controller にも届かない（read_origins
  の取消も効かない）。controller に届くのは opener の `Page.windowOpen`（URL つき）と `Target.targetCreated` だけ。
- 遅い応答（7 秒）でも、中継頁（script で file へ移る頁）でも、同じ tab の download は通る。
- manaba の link が実際に `target="_blank"` かは本番の頁を見ていない（下の `link=` token で次の失敗から分かる）。

決定:

1. **新しい tab の download は opener で行う。** launcher が `download` の間だけ controller に「download の見張り」を付ける。
   その間に agent の頁が新しい tab を開いたら、controller は新しい tab を閉じ（`Target.closeTarget`）、`Page.windowOpen` の URL が
   http(s) でログイン後なら read_origins の origin のときだけ、opener の頁をその URL へ script で移す（`window.location.assign`。
   頁が始める navigation なので download の event が agent の頁に出て、agent-browser が保存し、read_origins 外の download は従来
   どおり取り消される）。read_origins 外の URL は追わない（`window_open_origin_denied`）。`click` の間は何もしない。
2. **取りこぼしの採用は検証つきの予備。** agent-browser が download を待って諦めた（timeout）ときだけ、その action の間に Chrome
   が完了させた `<guid>` の file が 1 つなら runner がそれを生成名に移す（`adopted_guid`）。launcher は controller がその guid の
   完了を見ていて、取消・breach が無いときだけ受け取り、違えば file を消して失敗にする。新しい tab の download は controller に
   見えないので、この経路では通らない（1 が主経路）。
3. **固定診断。** runner は download の前に link の静的な形を agent-browser で読み、固定 token にする（`target_blank` /
   `target_named` / `target_none`、`download_attr`、`onclick`、`href_same_origin` / `href_other_origin` / `href_javascript` /
   `href_none`、`path_pdf`・`path_ct_page`・`path_file`・`has_query`・`has_fragment`、読めなければ `link_unreadable`）。URL・文言は
   出さない。そのため launcher の agent-browser policy に `download` があるときだけ `getattribute`・`url` を足す（agent の verb は
   固定のまま。agent-browser を呼ぶのは runner だけ）。controller は action の間に Chrome がしたことを token で記録する
   （`navigation_requested`・`same_document_navigation`・`frame_navigated`・`viewer_page`・`document_request`・
   `document_response`・`download_will_begin`・`download_completed`・`download_canceled`・`new_tab_from_page`・`new_tab`・
   `target_destroyed`・`dialog_opening`・`window_open_followed_in_opener`・`window_open_origin_denied`、何も無ければ `none`）。
   launcher の失敗行は `code= status= runner_reason= error_class= link= after= gate=`。
4. **agent への返し方（protocol 9 のまま）。** launcher は失敗した screenshot / download を、artifact の無い observation の text に
   `celeris_artifact_failure <同じ token 列>` として返す。daemon はこの形（英数字と `_ = , . ! -` と空白、600 byte 以内）だけを
   `detail` として shim に渡し、shim は `{"success":false,"error":"browser_artifact_action_failed","detail":"…"}` を出す。形が
   崩れていれば `detail` は出さない。古い daemon は artifact の無い応答を従来どおり失敗（`browser_artifact_transfer_failed`）にする。
5. 同じ dialog が 1 つの頁の複数の agent session に告げられたら、controller は一度だけ答える（付記 2026-10-10h の補足）。

運用: **launcher の再 build**（controller・runner・診断は launcher の process の中）と daemon の更新（shim・daemon の detail）。
sandboxd は不要。
## 付記 2026-10-10k: 保存済み credential の次 run 再利用

人の決定（2026-10-10）: 一度入力した credential は次 run 以降で再入力を求めず、使用の承認だけで使う。

1. 保存済み credential は同一 owner・同一 `policy_id` に限り候補にできる。候補の登録時に写した完全な `TrustedLogin`（login URL、username/password selector、submit selector、post-login、consent を含む）が現在の site policy と一致するときだけ有効とする。比較不能・credentiald の describe 失敗・期限切れは候補から除外し、既存の `WaitingForAuth` を開く。
2. 有効候補がある credential request は `WaitingForAuth` を作らず、credential reference と現在の `TrustedLogin` を固定した `WaitingForApproval`（operation `credential_use`）を直接作る。承認 UI は従来どおり毎回 login URL・2 欄・read_origins・consent を表示し、承認後は既存 Authenticate 経路だけを使う。保存済みであることは承認を省略する理由にならない（Q7）。
3. task をまたぐ再利用も owner と site policy の両方が一致する場合に限る。秘密は credentiald vault 内だけに置き、API は reference/metadata のみを返す。credentiald の登録・検索・削除は daemon control peer の admission を維持する。
4. policy 不一致、credentiald が describe できない場合、または post-login の `post_login_idp_login_form` 等の認証失敗では当該登録を再利用候補から無効化し、次回は手動登録を求める。失敗した認証を同じ run で自動再試行しない。
5. credentiald は保存期限を設定可能とし、既定 90 日を上限に登録時刻から期限を計算する。期限切れは検索・使用から除外する。一覧は秘密を含めず、所有者本人が削除できる。

検証では同 owner/policy の次 run が承認待ちだけを開くこと、承認が毎回必要なこと、policy 変更・describe/login failure・期限・削除・別 owner/policy で候補にならないこと、秘密が daemon/agent/event/log に出ないことを固定する。

### 付記 2026-10-10j 実装の確定

- owner は人の回答どおり、この instance の人 `owner`。daemon の StoreSink が固定する。worker request や agent の JSON からは受け取らない。他の actor の登録は自動再利用しない。複数 owner を導入するときは task と人の信頼できる対応を追加するまで、この制限を維持する。
- 現在の site policy は StoreSink が DB から毎回読む。登録時と同じ revision 1 の TrustedLogin に直して完全一致を照合する。承認後も DB と vault を照合し直す。
- 認証失敗・post-login の保留は登録を vault から削除して無効化し、認証 session の終了後に再入力 wait を開く。削除に失敗した場合は再利用へ進めずエラーとする。
- 一覧・削除は owner session の署名を必要とする専用 API。保存期限は credentiald の `CELERIS_CREDENTIAL_MAX_AGE_DAYS`（1〜90 日、既定90日）で設定する。
- task policy の revision と site policy（TrustedLogin）の revision は別の版。wait は両方を固定し、同じ番号であることは要求しない。使用直前に task policy binding と DB/vault の TrustedLogin をそれぞれ照合する。

## 付記 2026-10-10l: release gate の 2 件の不定（898929d9）

main 898929d9 の release gate（他の task の負荷が高い時）で `browser_sandbox_artifacts` の 2 試験が落ちた。

1. **他 origin の download の取消と完了の競争。** 試験の他 origin の PDF は小さく即座に返るので、負荷が高いと relay の取消より先に
   Chrome が書き終え、設計どおり breach で観測が止まり、次の screenshot が `cdp_command_failed`（agent-browser の
   `Target.setDiscoverTargets`）になった。製品の振る舞いは正しい（fail closed）。fixture の他 origin の file を「header を先に返し、
   本文は 10 秒後」にして、取消が必ず先に効くようにした。
2. **`Unknown ref`。** 中継頁ではない beforeunload の頁で、snapshot の直後の download が `unknown_ref` になった（runner の link の
   読み取りは成功していた）。agent-browser は自分で snapshot を取り直すと ref を振り直す。fixture の負荷（SIGSTOP stutter を含む）では
   再現しなかった。controller が答えた dialog の `Page.javascriptDialogClosed` は agent に渡していたので（開いた event は渡さない）、
   これも渡さないことにした（agent-browser から見て dialog は一切起きていない）。試験は `unknown_ref` のときだけ、その段を頁を開く
   ところからもう一度だけ行う（agent が snapshot を取り直すのと同じ。2 回目の結果をそのまま確かめる）。製品の agent には
   `detail` の `error_class=unknown_ref` が届く。

## 付記 2026-10-10m: ログイン後の `download @ref` は controller が href を頁の文脈で取りに行く（人の決定）

人の決定（2026-10-10 22:00Z 頃）: click による download が manaba で通らないため、controller が link の href を取りに行く経路を作る
（付記 2026-10-10j で見送った代替）。

本番（release e65571fe、task 01M4GYJ3XGJNWZQDF35F1MDE0H、21:48–21:52Z）: manaba の資料頁の `download @ref` 3 回が全部
`code=timeout launcher_reason=runner_timeout`（約 50 秒）。`after=target_destroyed` が 1 回、`after=none` が 2 回。timeout には
`link=`・`after=` が agent に届かず、timeout の後は同じ tab の snapshot も頁を開き直すまで timeout した。古い ref の `unknown_ref` では
`link=target_none,no_download_attr,no_onclick,href_none`（その要素に href が無い）。

決定:

1. **経路の選択。** ログイン後（post-login の read が開いている session）の `download @ref` では、launcher はまず runner に ref の要素を
   読ませる（`get attr @ref href` ほか。値は runner から出ない）。controller はこの読み取りで agent-browser が解いた要素
   （`DOM.resolveNode` の `backendNodeId`）を控える。controller はその要素か、いちばん近い祖先の `<a href>` の絶対 URL を自分の
   isolated world で読む。
   - href が無い（JS や form で動く要素）・要素を控えられない → 従来の click による download（`path=click`）。
   - href の origin が read_origins に無い → 失敗（`origin_denied`。click もしない）。
2. **取りに行き方（`path=fetch_href`）。** 頁の上端の文書が read_origins の origin で password 欄が無いこと（付記 D2-3 の検査）を
   確かめたうえで、controller の isolated world で `fetch(href, {credentials: 'same-origin', redirect: 'follow'})` を行う。cookie・
   header は controller から出ない（agent にも launcher の journal にも渡らない）。
   - 最終 URL（redirect の後）の origin が read_origins に無ければ捨てる（`redirect_denied`）。他 origin への redirect には
     `credentials: 'same-origin'` なので cookie は送られない。
   - HTTP status が 2xx でなければ `fetch_failed_<status の百の位>xx`、通信の失敗は `fetch_failed_network`。
   - 本文は読みながら数え、artifact の上限（10 MiB）を越えたら打ち切る（`size_exceeded`）。
   - 型は既存の artifact の判定（先頭 byte、PDF・画像・OOXML・OLE2・zip）。合わなければ `type_denied`（HTML のログイン頁など）。
     Content-Type の申告は信じない。text は既存の artifact の型に無いので通さない。
   - 1 回の呼び出しで 1 file。file 名は従来どおり launcher の生成名（`download-<hex>.bin`）で、shim が先頭 byte で
     `.pdf` などの名前を足す。Content-Disposition・URL の名前は protocol に欄が無いので渡さない。
   - 取った file は launcher が session の output に書き、既存の protocol 8 の artifact の受け渡しで run の `browser/output` に届く。
     agent には通常の download と同じ成功が返る。
3. **timeout の詳細と tab の回復。** launcher の runner 待ちの期限切れ（`runner_timeout`）も、screenshot / download なら
   `link=`・`after=`・`path=` の token を agent に返す（付記 2026-10-10j 4 と同じ形）。runner は click の download が期限切れになったら
   agent-browser の daemon を止める（次の command で daemon は作り直され、同じ tab の snapshot が通る）。runner の各段の期限は
   launcher の待ち（50 秒）より短くする。
4. **固定 token。** 成功・失敗の行に `path=fetch_href` / `path=click`、失敗の理由 `href_missing`・`origin_denied`・`redirect_denied`・
   `type_denied`・`size_exceeded`・`fetch_failed_<n>xx`・`fetch_failed_network`・`page_denied`（頁の検査に落ちた）。URL・file 名は
   記録しない。
5. **版。** protocol は 9 のまま（launcher と controller の中の変更。daemon・shim は変わらない）。

ログイン前（read の開いていない session）の download は従来どおり click。
