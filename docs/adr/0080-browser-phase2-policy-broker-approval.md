# ADR-0080: Browser Phase 2 の task policy・手動登録 credential broker・人間承認

---
tasks: [01M3MZKB3DFYJNBH015MJGQ0BT]
---

- 日付: 2026-09-28
- 状態: **Accepted（実装契約）。実装・実機検証・リリースは後続 WU**
- 関連: [ADR-0078](0078-browser-execution-capability.md) D3/D4 補足、D5 と補足、D6、D8 P2-A〜C、[ADR-0040](0040-self-improvement-deploy.md)
- 基準: repository `93d7f2990a9c371a9cee96f29462a3f66315e551`、agent-browser **0.38.1**（source `aff6125c023b810ea3f2e5deec5379e9a4270bdc`）。

## 文脈と適用範囲

人は「credential は手動登録から開始する」「必要になった LLM は人へサイトと目的を示して登録依頼を出す」「Live View は本人専用」と決めた。本 ADR はこの決定を実装契約へ落とす。ADR-0078 の Phase 2 Proposed のうち本書と重なる部分は本書を優先する。Phase 1 の検証済みリリースは本変更の検証を意味しない。

現行 `task-core/src/browser.rs` の grant は domains と dashboard URL のみで、task policy はない。`task-worker/src/browser.rs` は固定 allow を生成し、`browser_cli.py` が grammar と生成ファイルを検査する。`Status` は `Blocked` を持ち、`BrowserRunState` は既に二種類の wait を持つ。一般の `Approval` は拒否でも回答経路で再開するため、credential 承認へそのまま流用しない。

現行 `BrowserRunsPanel` は全 session の dashboard への外部リンクを表示する。GUI の `auth.server.ts` は単一パスワード方式で、loopback の既定は認証なし、署名 cookie の `id` はログイン session の識別子でありユーザー ID ではない。この現状を複数ユーザーの ACL と解釈しない。

## D1. task policy と生成器（a）

### 入力と権限の交差

`Task.browser_policy: Option<BrowserTaskPolicy>` を新設し、作成・編集 API でも同じ検証を行う。browser-enabled の Phase 2 起動には明示 policy を必須とする。省略した旧 browser task は `browser_policy_required` で起動を拒否し、管理者が設定する。browser を使わない task は従来どおり。組織 grant やページ内容から task policy を暗黙に広げない。

```text
BrowserTaskPolicy {
  policy_id, revision,
  domain_mode: common_hosts | separate_origins,
  navigation_origins: [exact HTTPS origin],
  network_domains: [host pattern],
  allowed_actions: [BrowserAction], approval_actions: [BrowserAction],
  credential_policy_ids: [id], artifact_policy_id
}
EffectiveBrowserPolicy = admin grant ∩ task policy ∩ supported actions
```

admin `BrowserCapability` に `allowed_actions` と `credential_policy_ids` を追加する。旧 grant の actions 省略は Phase 1 の業務操作だけ、credential policy 省略は空（credential 不可）とする。task の未知 field/action、未知 credential policy、壊れた origin を拒否する。既知 action の grant 外要求は交差で落とす。approval_actions は task allowed_actions の部分集合とし、実効集合に対して適用する。deny、credential policy の禁止、認証後の出力禁止は承認より優先する。

モデルからの編集は既存実効 policy を狭める変更だけ認め、grant・credential 紐付け・権限拡張は管理者経路に限定する。policy の削除を既定権限への復帰に使わせない。実効 policy の正規化 JSON、admin revision、credential policy revision を hash 化して run/wait/approval/lease に束縛し、変更時は古い承認と lease を失効させる。

### agent-browser 0.38.1 への写像

| BrowserAction（task 側） | upstream 内部 action 名 | 条件 |
| --- | --- | --- |
| navigate | `navigate` | shim の `open`、domain 検査 |
| click | `click` | D5 の一回承認が必須 |
| snapshot | `snapshot` | 公開・未認証区間だけ |
| extract | `gettext` | 同上。`extract` を upstream allow に書かない |
| screenshot | `screenshot` | 同上、artifact policy に従う |
| download | `download` | 同上、明示許可と一回承認 |
| scroll | `scroll` | 量を制限する |
| credential_use | `auth_login`, `plugin:celeris-credential:credential.read` | supervisor 専用、一回承認済み lease が必要 |

業務 action の交差が空なら起動しない。成立時だけ lifecycle 用 `launch`/`close` を加え、整列・重複排除した**非空** `allow` と `"default":"deny"` を生成する。たとえば snapshot/extract だけなら `{"default":"deny","allow":["close","gettext","launch","snapshot"]}`。`close` は supervisor の cleanup 権限であり task の権限に数えない。

一般 harness の policy に credential の二 action を常時追加しない。承認済みの専用実行区間だけ supervisor が当該 action を含む policy を生成し、終了時に戻す。モデル向け grammar は credential_id による「利用要求」だけで、raw `auth`/`plugin run`/任意 flags は引き続き禁止する。`evaluate`/`waitforfunction`/`cookies_get`/`storage_get`/`state_save`/`auth_save`/`cdp_url`/任意 plugin は対応しない。`fill`/`type`/`press`/upload も Phase 2 の業務 action に追加しない。category 名、alias の透過転送、未知 action の黙殺は禁止する。

upstream の `confirm` は allow より先に評価されるため、durable 承認に使わない。短命な pending CLI を人の回答まで保持せず、Celeris 側の action gate が承認後に新たに操作を送る。upstream policy は毎回有効なファイルを指定する。欠落・空 allow・JSON 不正・hash 不一致・版違いなら shim と supervisor が実行前に拒否する。cleanup だけは supervisor が独立した close-only policy で実行できる。

### allowed-domains の規則と origin の限界

host は小文字 ASCII DNS 名へ正規化し、曖昧な Unicode 表記、userinfo、URL/path/query、port、空 label、単独 `*` を拒否する。wildcard は先頭 `*.` のみ、apex を含まない。交差は文字列一致でなく包含関係で計算する。たとえば `*.example.org ∩ app.example.org = app.example.org`、`*.example.org ∩ *.sub.example.org = *.sub.example.org`、`*.example.org ∩ example.org = ∅`。wildcard の実機挙動が契約に合わなければその入力を拒否し、拡大解釈しない。

Phase 2 の標準は **`domain_mode=common_hosts`**、`navigation_origins=[]`、非空 `network_domains` とする。admin.allowed_domains と task.network_domains の交差を、整列した `--allowed-domains` として全操作に渡す。空なら起動拒否。navigation、redirect、iframe、popup、subresource に同じ host 集合が適用されることを GUI に明記する。CDN を追加するとその host への遷移も許される。scheme/port/IP/接続先ネットワークの制限はこの集合では保証しない。

`separate_origins`（非空 navigation_origins、CDN との分離、HTTPS/port の厳密な navigation 要件）は Phase 2 の生成器で `unsupported_origin_separation` として拒否する。両集合の union や最初の `open` だけの検査では代用しない。ADR-0078 D4 補足の fallback を選択した決定であり、P4 の enforcement が成立してから有効化する。**credential の exact origin 照合は別の必須境界**であり、この fallback で弱めない。

## D2. broker・provider・lease・監査（b）

`celeris-credentiald` を独立 executable/process とし、daemon と同じローカル利用者の user service として起動する。harness の子 process やライブラリ内 vault にしない。Phase 2 は trusted local harness の範囲であり、別 process は別 UID の強い隔離を意味しない。同一 UID の悪意ある shell、root、CDP を奪う相手からの隔離は P4 の責務である。

socket は `$XDG_RUNTIME_DIR/celeris-credentiald/{control,resolve}.sock`、親 0700、socket 0600、所有者を検査する。XDG runtime directory が適切でなければ起動拒否し、共有 `/tmp` へ fallback しない。Linux `SO_PEERCRED` で peer UID を必ず確認する。TCP listener は設けない。control と resolve は別 protocol/権限で、resolve peer は登録・承認・lease 発行を呼べない。daemon が作る予測不能な短命 binding と、信頼する supervisor の task/run/session 登録も照合する。UID、PID、モデルが指定した itemRef/lease_id のいずれか一つだけを認可にしない。

```text
CredentialProvider {
  capabilities() -> { manual_registration, interactive_unlock, totp, revoke }
  resolve(CredentialRef, AuthorizedLeaseContext) -> SecretEnvelope | NeedsHuman | ErrorCode
  revoke(lease_id) -> ResultCode
}
CredentialRef { credential_id, provider, policy_id }
AuthorizedLeaseContext { lease_id, task_id, run_id, session_id, exact_origin, expires_at }
SecretEnvelope { username, password } // manual v1。Debug/汎用 event Serialize は禁止
```

最初は provider=`manual`。登録・更新は broker の専用 control operation とし、通常の resolve trait に秘密の書込みを混ぜない。1Password/Bitwarden は後で同じ trait を実装できる。manual v1 は TOTP seed、OTP 自動生成、vault unlock、persistent session を提供しない。MFA が必要なら秘密を会話へ入力させず、未対応として停止する。

lease は `credential_id/provider/policy_id/policy_revision`、`task_id/run_id/session_id`、canonical exact HTTPS origin（scheme、host、effective port）、`approval_id/approved_by`、`issued_at/expires_at`、`max_uses=1`、状態、idempotency key を持つ。wildcard origin、userinfo、path/query/fragment を credential の origin として受け付けない。`:443` は既定 port と同一視する。credential policy は task scope・origin・TTL 上限・毎回の人間承認・`allow_persistence=false` を保持する。

TTL は **既定60秒、上限300秒**、policy/wait/承認の残期限との最小値。承認直後に長時間 lease を先払いせず、実際の操作直前に発行する。承認、policy、credential revision、origin、session の生存、controller 所有者を再検証する。broker が durable な consume 記録を確定してから一度だけ復号・返却する。並行 resolve の二件目、期限切れ、cancel、revoke、他 run/session、policy 更新は拒否する。応答喪失でも再送で秘密を返さず、再承認を要する。再起動後は未使用 lease も全失効させる。

監査は `request/grant/use/deny/expire/revoke`、時刻、sequence、actor_id、task/run/session、各参照 ID、policy hash、固定 decision_code だけを broker の耐久 journal と Celeris の型付き event に残す。broker journal は vault と別の 0600 ファイル、親0700で fsync する。監査不能なら lease 発行・秘密取得を拒否する。二つの記録先への反映は audit_id で重複排除し、consume journal を再送許可の根拠にしない。URL query、selector、ユーザー名、秘密 fingerprint、raw error、ページ本文は記録しない。

固定 bridge 名は `celeris-credential`。`agent-browser.plugin.v1` / `credential.read` / `credential.resolve` だけを受け、stdin 一要求・stdout 一応答の private pipe を使う。plugin request 自体には task/session 認証がないので、supervisor の binding に結び付ける。stderr は捨て、任意 plugin の追加は不可。secret を返すのはこの pipe だけで、harness への結果は `success` と固定 failure_code に変換する。

credential 利用は `auth login --credential-provider celeris-credential --no-navigate --url <trusted login URL>` を supervisor が構成する。ログイン URL と top-level selector は管理者の site policy の値で、モデルが指定できない。固定版 source は provider resolve 前、返却後、各 fill/submit 前に active top-level page と origin を検査する。iframe ログイン、SSO の origin またぎ、任意 frame 選択は Phase 2 で拒否する。popup/redirect/page 差替えと注入の競合試験を必須にし、この source 確認だけで原子的 injection と主張しない。防げない経路は認証利用を拒否し、P4-B へ送る。

## D3. 手動登録秘密の暗号化・鍵管理・非露出（c）

保存暗号は **XChaCha20-Poly1305（256-bit key、192-bit nonce、認証 tag）** とする。`Cargo.lock` には `ring 0.17.14`、`zeroize 1.9.0`、`chacha20 0.10.2` があるが `chacha20poly1305` はない。ring の既存 AEAD は96-bit nonceであり、chacha20 単体は認証付き暗号ではない。broker に RustCrypto `chacha20poly1305 0.10.1` を直接追加し、broker WU が依存解決・lock 更新・build を検証する。既存 chacha20 と別版が必要でも独自 crypto で代用しない。

| 保存物 | 場所（broker 所有） | 権限・扱い |
| --- | --- | --- |
| master key | `~/.config/celeris/credentiald/keys/<key_id>.key` | raw 32 bytes、0600、key directory と credentiald directory は0700 |
| encrypted envelope | `~/.local/celeris/credentiald/vault/<credential_id>.json` | 0600、vault と credentiald directory は0700、DB外 |
| 監査・使用済み lease | `~/.local/celeris/credentiald/audit/` | directory0700、各ファイル0600、秘密なし |

key は初回明示初期化時に OS CSPRNG で作り、環境変数、GUI password、API token から導出しない。暗号文があるのに鍵が欠落・破損した場合は新しい鍵で黙って上書きせず `vault_locked` で止める。毎回の暗号化で CSPRNG の24-byte nonceを新規生成し、乱数失敗時は保存しない。envelope は `format_version/key_id/nonce/ciphertext/tag` を持ち、AAD に format_version、credential_id、provider、credential revision、exact origin、policy_id を canonical encoding で含める。入替え・改ざん・異なる origin/policy による復号を拒否する。

broker だけが読み書きし、親 directory の所有者と mode、regular file、symlink/hardlink を検査する。新規作成は O_EXCL/O_NOFOLLOW と0600、更新は同じ directory の**暗号文だけ**の temp→fsync→atomic rename→directory fsync とする。起動時に緩い既存権限を見つけたら拒否する。登録後の metadata DB commit に失敗したら orphan ciphertext を reconcile し、参照がない値を利用可能にしない。

DB の credential 台帳には **credential_id・provider・policy（revision含む）・origin だけ**を保存する。lease/wait/監査はそれらの参照と状態 metadata を別に保持する。username も秘密 payload の一部として暗号化する。DB の既存 `/secrets` 平文ファイル機能、upstream `auth save`、auth state export は使わない。

鍵 rotation は broker を新規 resolve 停止にし、全 lease を失効、新 key_id で各 envelope を再暗号化する。旧鍵は全件検証・切替完了まで保持し、途中失敗は key_id により再開する。DB/通常成果物バックアップには鍵と vault を含めない。鍵の保管・復旧は本人による別の保護済みバックアップで行い、鍵紛失時は復号できず再登録する。削除は credential revision を失効させ、暗号文と不要鍵を消すが、SSD/バックアップの完全消去までは保証しない。鍵と暗号文を同一 UID で読める環境の盗難に対する完全防御ではない。

登録秘密は GUI の password input→認証済み HTTPS POST（または本人の loopback）→GUI server の専用処理→broker control socket の短命メモリだけを通す。Celeris の汎用 API client/error logger、DB、event、task answer、artifacts、URL、args/env へ通さない。フォーム再表示・validation error に値を反射しない。request body logging、trace、HAR/video/screenshot 記録、core dump をこの経路で無効にし、Rust 側 buffer は zeroize する。GUI runtime のメモリ消去は完全保証できないため保持・複製を最小化する。

認証要求を受けた時点で snapshot/extract/screenshot/download、console/URL/raw stdout 転送、任意 artifact 登録、Live View を停止する。**Phase 2 は credential を注入した session の終わりまでこれらを再開しない**。認証後ページの任意秘密を除去できるとは仮定しない。秘密を反射する DOM/エラー/画像の sentinel 試験で、モデル入力・モデル結果・DB/WAL・events・logs・artifacts に平文がないことを検証する。ページ本文を含めない固定コードの結果だけを返す。認証済みページを読んで処理する一般業務は、観測を安全に再開する追加契約が成立するまで不可とする。

## D4. task Status・durable wait・再開（d）

**Status の variant は増やさない。** task は `Blocked`、永続 `BrowserWait.reason` は `waiting_for_auth` / `waiting_for_approval` とする。対応する `BrowserRunState` を `WAITING_FOR_AUTH` / `WAITING_FOR_APPROVAL` に写す。自由文の question を唯一の正本にしない。

`BrowserWait` は wait_id、task/run/session、reason、credential_ref または operation_intent_id、approval_id、policy_hash、owner_id、deadline、resume_key、version、状態を持つ。専用 store と transaction で task 遷移・wait 作成・event を一緒に確定する。browser の wait が未解決なら一般の answers、承認、コメント割込み、retry/continue/phase-resume を使って Ready に戻せない。解除は専用 browser 操作だけを経由する。

| 入力 | Task / Browser | 処理 |
| --- | --- | --- |
| credential 未登録 | Blocked / WAITING_FOR_AUTH | 登録要求を保存。既定24時間、管理者上限24時間。browser は閉じる |
| 登録完了 | Ready（未 dispatch） | metadata のみを結び付け、wait を一度だけ解決。次 run で credential 使用承認を別途要求する |
| credential 使用・高リスク操作に承認なし | Blocked / WAITING_FOR_APPROVAL | intent と session を凍結、待機期限は既定・上限5分。操作はまだ送らない |
| 承認 | Ready → Running | 同じ run/session が生存・一致している場合だけ再開し、一回の実行権を消費 |
| 拒否 / wait期限切れ | Failed / FAILED | 固定 `approval_denied` / `browser_wait_expired`、lease失効、cleanup、自動retryなし |
| cancel | Cancelled / FAILED | waitをcancelled、全lease失効、cleanup |
| session喪失・policy変更 | Blocked | 元承認をinvalidated。新run/session/intentの新承認を要求し、旧承認で実行しない |

wait 保存後、supervisor は harness を停止して終了を確認し worker slot・worker実行 lease を解放する。人待ちを worker wall-time/再試行回数へ算入しない。認証待ちは新 run で再投入する。承認待ちは最大5分だけ制御側の軽量 browser owner が session を保持し、worker とは別に寿命と排他を管理する。upstream idle timeout もこの上限に合わせ、延長で無期限に保持しない。

同一 session の再開は新 worker slot を取得し、**同じ論理 run_id/session_id の continuation**として扱う。待機時点では run を completed/failed にしない。harness 会話の継続 ID と browser session ID を混同しない。現行 `SessionGuard`/終了時 close をこの明示 suspend の場合だけ owner へ移管する。cancel/error/default drop は従来どおり close する。再接続の生存確認・単一 controller lock に失敗したら新 run/session を作り承認を取り直す。

resume は wait version と resume_key の CAS で重複排除する。最終承認と deadline 競合は同一 transaction で決め、実行直前にも期限を確認する。reconciler は daemon 再起動時・期限到達時に wait を回収し、broker 再起動で失効した lease を再利用しない。副作用送信後の crash は `execution_uncertain` として Blocked + WAITING_FOR_HUMAN にし、照合できるまで自動再送しない。max_uses と idempotency key は外部サイトの exactly-once を保証しない。

compound task は待機した WorkUnit と run に同じ理由を保持し、その WU の slot を解放する。並行 WU の結果を消さない。親 task が他 WU の実行で Running の場合も GUI は当該 wait を表示する。全体停止に移るときは親を Blocked に集約し、一般の WU retry が browser wait を解決しないようにする。

## D5. 登録依頼・承認 API と GUI（e）

LLM は秘密ではなく「登録が必要」という typed request と credential policy 参照を出す。表示するサイトは trusted exact origin、目的は task 由来の短い説明（最大500字、plain text、untrusted 表示）とする。ページの指示から送信先や grant を採用しない。秘密を自由文の質問・回答・通知へ記入させない。GUI には「サイト」「task/run」「用途」「期限」「一回だけの利用」「拒否」を示す。

以下は新設契約。`/api/v1` は Celeris API、GUI の専用 resource route は cookie 認証を受ける BFF とする。

| 接点 | request / response と責務 |
| --- | --- |
| `POST /api/v1/tasks/{id}/browser/requests` | trusted supervisor が run/session/policy/intent を指定。登録待ちまたは承認待ちを作成し wait_id を返す。秘密 field を拒否 |
| `GET /api/v1/tasks/{id}/browser/waits` | wait、origin、目的、期限、状態、version のみ。秘密・鍵・socket・dashboard token は返さない |
| `POST /browser/waits/{wait_id}/credential`（GUI） | 本人が username/password を登録。専用処理が broker control socket へ渡し、credential_id のみを得る。GET/echo/readback API は設けない |
| `POST /api/v1/tasks/{id}/browser/waits/{wait_id}/registered` | GUI が登録済み ID と broker receipt を通知。broker の存在・origin/policy一致を照合し CAS で登録待ちを解決 |
| `POST /browser/waits/{wait_id}/decision`（GUI） | `approve_once` / `deny`、expected_version、idempotency key。owner session から認証された人の decision を作る |
| `POST /api/v1/tasks/{id}/browser/waits/{wait_id}/decision` | GUI service の署名付き human attestation を検証して専用遷移。一般 API bearer token だけでは承認不可 |
| `POST /api/v1/tasks/{id}/browser/waits/{wait_id}/revoke` | 本人が未消費承認・leaseを失効。既に送信した外部操作の取消とは別 |

GUI cookie を daemon API token で代用しない。GUI→API の human attestation は GUI 専用鍵で署名し、actor/owner-session hash、task/wait/version、decision、policy hash、nonce、30秒以内の expiry を束縛する。daemon は公開鍵と一回 nonce を検証し、worker に署名鍵を渡さない。専用鍵は owner が初期設定する GUI 用0700 directory の0600ファイルとし、公開鍵の登録・交換を一般task/API経路から許可しない。API token を持つ LLM が `approved_by` を自称しても拒否する。登録 receipt は broker が発行し、秘密を含まない。登録先 origin/policy は保存済み wait から取得し、フォームからの差替えを拒否する。秘密登録と DB 更新は二段階なので、同じ wait/credential revision に対する再送は冪等にし、片方失敗時に未承認利用へ進まない。

すべての人向け経路は明示 auth enabled + 有効 cookie + D6 の owner session を要求し、変更系は exact Origin と CSRF token も必須とする。既存の「Origin header がない curl は通す」規則だけに依存しない。認証なし401、他session403、version競合409、失効410、schema不正422とし、エラーは固定コードだけ。入力サイズを制限し、全応答 `Cache-Control: no-store`、Referrer-Policy/no content echo を適用する。

**高リスク**は credential 使用、フォーム送信、購入/決済、削除、公開/外部送信、権限変更、upload/download と定義する。DOM label やモデルの自己申告では安全性を判定できないので、Phase 2 では全 `click` と `download` を承認対象とする。未対応の fill/press/upload を承認で有効化しない。credential_use の一回承認は当該 top-level ログインの fill と submit だけを含み、以降の click は別承認である。

操作 intent は内部 action、exact origin、trusted target/page identity、非秘密の引数 digest、policy revision に束縛する。古い `@eN` を別ページへ再生しない。承認後に target/page/origin/引数が変われば失効し、再要求する。対象の同一性を確認できない操作も送信しない。GUI の「登録」は使用承認を兼ねず、standing approval は Phase 2 では設けない。

## D6. Live View は本人の session だけ（f）

Phase 2 は**単一所有者の専用 Celeris instance**に限定する。共有 password を複数人が使う構成では本人を識別できないため、credential UI と Live View を無効にし、別 instance または将来のユーザー認証を要求する。`authenticated=true` だけ、loopback だけ、URL の HTTPS 判定だけでは許可しない。

本人の GUI ログイン session の cookie `id` に対し、制御側に期限付き owner grant を保持する。GUI の `POST /browser/owner-session/request` が発行した非秘密 challenge を、本人がローカルの `celerisctl browser owner-session approve <challenge>` で確定して一つの session に束縛する。この CLI は GUI の専用 Unix control socket（runtime directory0700/socket0600、peer UID検証）を使い、一般 HTTP bearer API では owner grant を発行できない。challenge は一回限り・5分期限、grant は cookie の期限を超えない。先着アクセスを自動で owner にせず、他 cookie session は同じパスワードでログイン済みでも403とする。cookie 自体や cookie ID を CLI 引数/ログへ出さない。再登録時は旧 grant を失効させる。logout、cookie期限、GUI再起動で grant と既存接続を失効させる。本人を区別できない未設定状態は表示・接続とも拒否する。

`BrowserRunsPanel` は raw `live_view_url` を href にせず、サーバが認可した同一 origin の `/browser/live/{task_id}/{run_id}` を表示する。route は毎回 owner grant、task/run対応、active controller、RUNNING、認証区間外を照合する。query、cookie、session_id を知るだけでは認可されない。過去 event に残った dashboard URL も直接リンクとして使わない。

agent-browser dashboard/stream/CDP は専用 namespace の **loopback のみ**に bind し、外部 listener、公開 reverse proxy、公開 port forwarding は設けない。GUI server が既存 dashboard を同一 origin の保護された経路で relay する。HTML/assets/API/WebSocket upgrade を含め全入口を同じ owner guard に通し、既存 WS も logout/失効時に切断する。upstream の bootstrap token は server メモリで処理し、URL fragment、event、ブラウザの Location、ログへ渡さない。上流への arbitrary URL/port 指定は禁止する。

これは upstream dashboard をそのまま開く**限定的な保護導線**であり、GUI 内で frame を描画・集約する live stream 統合ではない。dashboard は全 session が見えるため、namespace に本人の session だけを置く。認証要求開始時は owner grant による dashboard relay 全体と既存接続を閉じ、認証済み session が存在する間は再開しない。dashboard の操作経路は Phase 2 では禁止し、読み取り専用 relay で upstream の HTTP/WS action を明示的に拒否する。これを列挙・遮断できなければボタンを無効のままとし、takeover や承認を迂回する抜け道にしない。

GUI WU は固定版 dashboard の実際の HTTP/WS/assets を調べ、guard を通らない絶対 URL、token bootstrap、直接 WS 参照が残らないことを確認する。満たせない場合の安全な動作は「Live View 利用不可」である。完成の受け入れは本人の正常アクセスと負例の両方を必要とし、リンク非表示だけで達成扱いにしない。

検証は owner のブラウザ context A と別ログイン B、未認証 context、期限切れ cookie を使う。A のみ対象 route 成功、B/未認証による直打ち・API・asset・WS upgrade・他run差替えは拒否、logout 後の既存 WS は切断する。loopback 認証無効時はリンクも route も拒否する。`ss` 等の listener 確認に加え、別 network namespace/外部 host から dashboard/stream/CDP の IPv4/IPv6 port に到達不能を確かめる。GUI の正規 HTTPS 経路だけが本人に届く。外部 probe ができなければ未検証と記録し、完全な到達制限を宣言しない。

## D7. 非目標（g）と限界

- persistent auth、cookie/state/profile の保存・復元、Browser Identity は Phase 3。本書の暗号化保存は credential 自体であり browser auth state ではない。
- GUI の frame/live stream 統合、pause/takeover/resume の人間操作は Phase 3。D6 の認証 relay で controller を人へ渡さない。
- container/別UID/egress firewall、任意 shell の network 制御、強い injection 境界は Phase 4。peer UID と file mode だけで同一UID workerを隔離したと主張しない。
- 複数利用者 ACL、外部 vault provider、MFA/TOTP、認証後ページの任意秘密除去は本実装に含めない。

## D8. 後続 WU の範囲・ファイル所有・受け入れ

新設名は実装時の推奨配置。共有ファイルの変更箇所は以下の owner に集中させる。後続 WU は必要な interface を先に共有し、他 WU の本文を編集しない。

| WU | 実装範囲と所有ファイル | 必須検証 |
| --- | --- | --- |
| policy | `task-core/src/browser.rs` の BrowserTaskPolicy/grant/生成関数、`task-worker/src/browser_policy.rs` 新設、`browser_cli.py` の grammar/生成policy検査、対応 policy unit tests。worker起動への接続は e2e に渡す | 空集合/未知 action/壊れた policy/交差/wildcard、許可外 action と redirect/iframe/popup/subresource の実機否定試験、origin分離要求の拒否 |
| broker | `crates/celeris-credentiald/` 新設（library+daemon+plugin bridge、provider/crypto/lease/audit）、root `Cargo.toml`/`Cargo.lock`、broker unit/integration tests。TaskStoreやGUIには触れない | 保存暗号の改ざん/AAD/鍵欠落/権限、peer UID/偽binding、並行一回consume、TTL/cancel/restart/audit障害、secret sentinel |
| waits | `task-core/src/browser_wait.rs` 新設、`model.rs` の task policy参照/Event、`transition.rs`、`store.rs` と migration、core/ops/API の `lib.rs` module登録、`task-ops/src/browser.rs`・`task-api/src/browser.rs` 新設、API routing/schema、一般gate/answer/approval/continue迂回防止 | Blockedとreasonのtransaction、CAS、登録≠承認、拒否/期限/二重回答、API tokenだけの承認拒否、再起動reconcile |
| gui | `gui/app/auth.server.ts`、server middleware、`BrowserRunsPanel.tsx`、`gui/app/celeris/{types,client.server,browser}.ts`、`lib/browser.ts`、関連 routes と `routes.ts`、owner/credential/live専用server modules、GUI tests。owner grant のローカル CLI は `celerisctl` の専用 subcommand と登録箇所も担当 | 登録フォーム非反射、owner/別session/未認証、CSRF、承認/拒否、全dashboard入口/WS/失効、外部到達不能 |
| e2e | `task-worker/src/browser.rs`/`browser_tests.rs` の lifecycle/接続、`task-dispatch/src/dispatcher.rs` と専用 browser runtime、`crates/celeris` の起動設定/service結線、`tests/e2e` の統合 fixture。policy/broker/waitsの公開interfaceを利用 | worker slot解放と再取得、未登録→登録→別承認→成功、拒否/期限/再起動/多重resolve/target変更、secret非露出を全経路で検証 |

waits が task policy の保存欄と API wiring を担当し、policy はその型と純粋生成器を担当する。broker の wire DTO は新 crate の library から公開し、secret DTO を task-core の event 型へ持ち込まない。GUI と e2e の統合試験はそれぞれ `gui/test` と `tests/e2e` に分ける。migration 番号は waits が実装時の最新 schema を見て確保する（現在最大0030）。`integrate-build` が module登録・依存・型の境界を確認してから e2e/gui を結合する。

| 統合の gate | 成功条件 |
| --- | --- |
| policy/broker/waits | 固定0.38.1上の負例を fake substrate の unit test と区別して記録。未検証境界は既定拒否 |
| e2e/gui | GUIで登録→WAITING_FOR_APPROVAL→承認→再開と、拒否→停止をブラウザで確認。秘密入力区間のtrace/HAR/videoは禁止し、sanitizedな検証記録のみ保存 |
| release | `cargo fmt --check`、`cargo test --workspace`、`cargo clippy --workspace -- -D warnings`、変更GUIの型検査/テスト、ADR-0040 D5 のrelease.sh/verify.sh。検証済みfull SHAを報告し、本番昇格は人 |

本 adr WU は上記を実装・リリースしない。専用 `celeris-wu/.../adr` に文書だけを commit し、最終 release WU が統合済み SHA と self/<task-id> の配送を担当する。Phase 2 全体の完了や元の SSH 途中目標の達成とは区別する。

## 根拠

- 現行コード: [browser schema](../../crates/task-core/src/browser.rs)、[Status](../../crates/task-core/src/model.rs)、[worker](../../crates/task-worker/src/browser.rs)、[shim](../../crates/task-worker/src/browser_cli.py)、[一般承認](../../crates/task-core/src/approval.rs)、[既存secret保存](../../crates/task-api/src/secrets.rs)、[GUI認証](../../gui/app/auth.server.ts)、[GUI panel](../../gui/app/components/BrowserRunsPanel.tsx)。
- 固定版の [policy evaluator](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/cli/src/native/policy.rs) と [auth_login 実装](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/cli/src/native/actions.rs)、[plugin protocol](https://github.com/vercel-labs/agent-browser/blob/aff6125c023b810ea3f2e5deec5379e9a4270bdc/docs/src/app/plugins/page.mdx) を2026-09-28に読み取り確認。source確認は実機負例の代わりにはしない。
- [Cargo.lock](../../Cargo.lock) とローカル ring 0.17.14 `aead/nonce.rs` を確認。[RustCrypto chacha20poly1305 0.10.1](https://github.com/RustCrypto/AEADs/blob/chacha20poly1305-v0.10.1/chacha20poly1305/src/lib.rs) の XChaCha20Poly1305 を保存方式として採用。依存追加と実行検証は broker WU の責務。

## 補足（e2e WU の実装判断、2026-09-28）

- 承認の消費: 承認 wait を開いた run は browser を起動しない。承認後の dispatch は新しい run ID を持つので、store の「同じ run/session」照合には wait に記録した論理 run/session を使う。browser session もその予約 session ID を使い続け、broker の binding と lease も同じ論理 run/session に結び付ける。消費は substrate の版確認の後に行い、起動できない run で承認を失わせない。
- broker ID: task policy hash は `sha256:<hex>` 形式である。broker の ID 制約（`[A-Za-z0-9_-]{1,64}`）に合わせ、binding と lease には `<hex>` だけを渡す。
- 認証後の区間: 同じ session の harness policy から snapshot・gettext・screenshot・download を外し、Live View の URL も出さない（D3）。cleanup は supervisor の credential 区間 policy に含まれる `close` で行う。
- 実機未検証の境界: `auth login` の lease 参照 flag、plugin 設定の形、daemon 経由の FD 3 継承は fake substrate でしか確かめていない。実 agent-browser で確認するまで、fake の成功を実機の成功として扱わない。
- 端から端の試験は `tests/e2e` ではなく `crates/task-api/tests/browser_e2e.rs` に置いた。API の test harness と broker の実 IPC を同じ process で使うためである。
