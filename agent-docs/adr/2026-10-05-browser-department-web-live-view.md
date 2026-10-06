# ブラウザ実行課と web の Live View・操作・承認画面（2026-10-05）

---
tasks: [01M46W97H391DSFW1XJ745W0G9]
---

- 状態: 採用（設計）。実装は同じ task の葉 org-node・gateway・web-ui・real-check が行う
- 関連: [ADR-0078](0078-browser-execution-capability.md) D2（browser-specialist profile・grant）、
  [ADR-0080](0080-browser-phase2-policy-broker-approval.md) D5・D6（人の経路・本人 session）、
  [ADR-0081](0081-web-spa-frontend.md)（web SPA）、[ADR-0099](0099-browser-phase3-control-lease.md)（control lease）、
  [ADR-0100](0100-browser-phase3-live-proxy-acl.md)（live proxy ACL）、[ADR-0101](0101-browser-phase3-identity-contract.md)（identity）、
  [ADR-0113](0113-browser-p3c-control-gate-action-server.md)（control gate）、[ADR-0138](0138-browser-prod-admission-confidential-release.md) H3・H4

## 背景

ブラウザ実行の能力（ADR-0078〜0116・0138）は入っており、本番 config は `[browser] runtime = "launcher"` で動く。
ただし次の 2 点が欠けていて、人もエージェントも使えない。

- org（DB）に `browser` grant を持つ node が無い。`matching::decide`（`crates/task-ops/src/matching.rs:118-125`）は
  `skills: ["browser-enabled"]` の task について、grant が無いか `validate()` に落ちる node を候補から外す。
  そのため browser task は誰にも割り当たらない。
- 人の画面は旧 GUI（`gui/app/routes/browser.*`、`components/BrowserRunsPanel.tsx`・`BrowserControl.tsx`・
  `BrowserWaitsPanel.tsx`、`celeris/browser-live*.server.ts`）にしか無い。web/ には画面が無く、
  gateway（`web/server`）には次のどれも無い。
  - live relay、WebSocket、owner（本人）session、attestation 鍵

本 ADR は、部署の定義、web gateway の live proxy と browser の人向け経路、web 画面、試験方針を決める。
browser の安全規則（ADR-0080 D6・ADR-0100 D2・ADR-0099・ADR-0138 H3/H4）は変えない。
web は旧 GUI と同じ規則を同じ強さで実装する。D2 の入力転送だけは、ADR-0080 D6 が Phase 3 に送っていた人の操作を
web 側で開く拡張であり、人の決定 `browser-live-input` を条件とする。

## D1. 部署: ブラウザ実行課

### D1.1 id・名前・置き場所

| 項目 | 値 |
|---|---|
| id | `browser-execution` |
| name | `ブラウザ実行課`（`Browser Execution`） |
| kind | `section` |
| parent_id | `engineering` |
| genre | `coding` |
| brief | エージェントのブラウザ操作で Web 上の作業を行う。人の監視・承認・引き継ぎを前提に動く。 |

**Engineering の下に置く**。CoS 直下にはしない。理由は次のとおり。

1. 実行の形が coding と同じ。browser task は coding harness（adapter `acp` / `claude-code`）に agent-browser の
   shim を足して走る（ADR-0078 D1）。Engineering から `harnesses.allowed = ["coding"]` と memory 知識を
   継げば、新しい harness や genre を足さずに済む。
2. 組織の段の形を崩さない。CoS 直下は部（department）の段であり、課（section）を置くと
   「cos > 部 > 課」の形が崩れる。browser を部に格上げするほどの node 群も無い。
3. grant を広げない。`browser` grant は祖先から子へ継がれ、子の grant は親の grant を丸ごと置き換える
   （ADR-0078 D2）。課（葉）に置けば grant はその課だけに留まる。Engineering や CoS に置くと、
   software-engineering・ui-ux 等の配下が全て browser の候補になる。
   - ブラウザ実行課には子 node を作らない。子を作るときは、子ごとに grant を書く。

Research（web-research）の下には置かない。web-research は Local Deep Research（adapter `local-deep-research`）で動き、
browser の adapter 制約（`acp | claude-code | browser-specialist`、`task_core::browser::browser_adapter`）と
harness が合わないため。

### D1.2 profile

```json
{
  "skills": ["browser-enabled", "browser", "web-automation"],
  "tools": [],
  "harnesses": { "allowed": ["coding"], "default": "coding" },
  "budget": { "max_lane": "standard", "max_attempts": 2 },
  "browser": {
    "allowed_domains": ["localhost", "127.0.0.1"],
    "allowed_actions": null,
    "credential_policy_ids": []
  },
  "policy": [
    "ブラウザ操作は grant と task の browser policy の範囲だけで行う。範囲外の origin・操作が要るときは自分で広げず、waits で人に上げる。",
    "人が control lease を持つ間は操作しない。返却されたら現在の画面を読み直してから続ける。",
    "credential・cookie・token をメモ・成果物・会話に書かない。"
  ]
}
```

- **grant**: `skills` に `browser-enabled` を入れる。matching の採点で一致するように「要求」と同じ語を持たせる。
  実際に効く grant は `browser` 欄である。
  - matching は実効 profile に `browser-enabled` を持つ node を、browser を要求しない task の候補から外す。
    専用課が空 skill の task を id 順の同点判定で奪わないため。grant だけを持つ兼任 node は対象外とし、
    既存の通常 task の振り分けを保つ。
  - `allowed_actions` は省く（`null`）。Phase 1 の集合になり、`credential_use` は含まれない（ADR-0080 D1）。
  - `credential_policy_ids` は空にする。credential を使う task は、人が policy を登録してから PATCH で足す。
- **許可 origin**: 初期の `allowed_domains` は loopback（`localhost`・`127.0.0.1`）だけにする。
  - real-check の試験ページを開ければ十分であり、外向きの権限を先に配らない。
  - 実際の業務の host は人が決め、`PATCH /api/v1/org/browser-execution` で `browser` 欄ごと置き換える。
    profile の PATCH は丸ごと置き換えになる（`OrgPatchBody.profile`）。
  - task 側の browser policy（ADR-0080 D1）は grant を狭めることしかできない。
  - 初期集合は人の決定 `browser-initial-domains` で確定する。推奨は loopback のみ。
- **live_view_url**: grant には入れない。web は D2 のとおり、gateway の起動設定の loopback upstream だけを宛先にする。
  grant の URL を宛先にも可否の判定にも使わない。旧 GUI は「設定済み」の判定に使っていたが、web では使わない。
- **harness と adapter**: node が受ける harness は `coding`。adapter は task の作成時に
  `browser_adapter`（`crates/task-ops/src/add.rs:649-670`）が決め、既定は `acp`、明示すれば `claude-code`。
  本番の `acp` は cheap lane の qwen（tunnel 経由）しか持たない。browser task は長く、判断も多いので
  `claude-code` を推奨する。
  - 既定を変えるか（`browser_adapter(None)` を `claude-code` にする）は人の決定
    `browser-default-adapter` とする。推奨は `claude-code` 既定。
  - 決定までは、browser task を作る経路（CoS・planner の objective、投入手順）で `adapter: "claude-code"` を明示する。
- **長時間実行の予算**: 新しい harness は足さず、本番の `coding` harness の予算（`max_turns = 120`、
  `max_wall_secs = 7200`）を使う。node の `budget` は次の 2 つを持つ。
  - `max_lane = "standard"`: frontier に上げない。
  - `max_attempts = 2`: 失敗した browser 操作を何度も繰り返さない。

  人の介入に関わる時間は既存の上限のままにする。
  - control lease: 既定 60 秒・上限 300 秒（`crates/task-core/src/browser_control.rs:13-15`）
  - live grant: 60 秒（ADR-0100 D2-3）

  人の待ち（waits）は run の壁時計を消費しない既存の待ち状態で扱い、予算を足さない。

### D1.3 適用と試験

- 本番の org は DB が正（ADR-0122 系の運用）なので、worker は本番に触れない。葉 org-node は次を成果物にする。
  - `docs/ops/browser-execution-section.md`: POST の body、確認の GET、grant 変更の PATCH 例、戻し方（PATCH で `browser` を外す）
  - 投入 JSON: `docs/ops/browser-execution-section.org.json`（`OrgCreateBody`）

  適用は配送後に Fable が `POST /api/v1/org` で行う。
- seed: `config/org.example.toml` に同じ node を `[[org]]` で足す。`[org.profile.browser]` の例を含める。
- 試験（`crates/task-ops` の matching 試験）は次の 4 本。
  1. `skills: ["browser-enabled"]` の task が `browser-execution` に割り当たる。
  2. grant の無い software-engineering・ui-ux は候補から外れる。
  3. 子 node を足すと grant が継承され、子で置き換えられることを確かめる。
  4. 不正な grant（空の `allowed_domains`）の node は候補から外れる。

  あわせて、seed の org.toml が読めて `validate()` を通ることを確かめる。

## D2. web gateway の live proxy と browser の人向け経路

### D2.0 人が web から変える browser 設定（2026-10-06 追記）

本番 org は DB を正とし、管理画面で browser-execution node の profile を読み書きする。対象は
`browser.allowed_domains`（許可 origin）、`harnesses.allowed/default`、`budget`、
`browser.credential_policy_ids`、credential と identity の対応を含む browser 関連設定である。
管理画面は `PATCH /api/v1/org/browser-execution`（または同等の browser 専用管理 API）を使い、
変更前後の値、actor、時刻を events に残す。機密値そのものは event に保存しない。
管理権限と CSRF を確認し、profile の置換前に scheme（外向きは HTTPS、loopback の開発用 HTTP のみ例外）、
host、port、wildcard の範囲を検証する。`*` 全体、public suffix を覆う wildcard、URL の userinfo・path・query・fragment、
不正 scheme は拒否する。検証失敗時は DB と event を変更しない。
初期値は loopback だけとし、業務 host は人が web から追加する。既存の `allowed_domains` が host 表現なら、
origin の scheme/port 制限を表せる形式へ移行してから画面を公開する。host だけへの丸めで許可を広げない。

task の browser requirements に task ごとの `allowed_domains` を必須で加える。browser-enabled task の作成時に
これが欠落・空なら **拒否する**。org grant 全体への暗黙の拡張はしない。CoS の起票、planner の execution plan の
子 task、`create_task` のいずれも、必要な最小 origin だけを spec に渡す。子 task の集合は親の集合の部分集合に限る。
実効許可は task の集合と browser-execution grant の集合の交差であり、grant 外は policy broker と egress の
両方で拒否する。wildcard の包含関係は origin の scheme・host・port を含めて判定し、交差を単なる文字列一致に
しない。task 作成後の grant 縮小は既存 task にも即時適用する。

CoS と planner の prompt・skill に次を明記する: 「browser task の `requirements.browser.allowed_domains` には、
作業に必要な最小の origin のみを書き、親の範囲を超えない。例: 社内請求画面だけを使うなら
`["https://billing.example.com"]` とし、`["*.example.com"]` や grant 全体をコピーしない」。
試験は交差、grant 外拒否（broker と egress）、親子包含、web での編集と不正 scheme/wildcard 拒否、
actor 付き event、欠落・空の task 作成拒否を決定的に確かめる。

この追記は既存の host 形式の grant と task policy の実装変更を要する。旧形式のままで広い許可を与える
運用は開始しない。org-node・web-ui・close で実装と試験を突き合わせる。

旧 GUI の `browser-owner.server.ts`・`browser-attestation.server.ts`・`browser-live*.server.ts`・
`browser-control.server.ts`・`browser-waits.server.ts` を `web/server/browser/` に移す。
置き場所は express の app に、`/api` relay より前に mount する。言語は既存の `web/server` に合わせて JS（ESM、`.d.ts` 付き）。

### D2.1 本人（owner）session

web の cookie（`__celeris_web_session`）には利用者の識別が無いので、ADR-0080 D6 の owner grant を web にも設ける。

- `POST /browser/owner-session` は 12 hex・一回限り・5 分期限の challenge を出す。
  - 本人は `celerisctl browser owner-session approve <challenge>` で確定する。
  - 受けるのは web gateway 専用の Unix socket（`CELERIS_WEB_OWNER_SOCKET`。所有者の runtime dir 0700・socket 0600）。
    Node の socket API は peer UID を取得できないため、同一 UID の process は区別できない。GUI と同じ制約として記録する。
  - grant は gateway のメモリだけに置く。cookie の期限を超えない。logout・再登録・再起動で失効する。
- `celerisctl` は socket の path を引数で受ける。web と GUI の並行運用中は、どちらの gateway の challenge かを
  path で区別する。CLI の変更が要る場合は葉 gateway の範囲に含める。
- password 認証が無効（`CELERIS_WEB_PASSWORD_FILE` 未設定）なら、本人を識別できない。
  browser の全経路を `403 owner_unavailable` にし、画面には理由を出す。
- attestation 鍵は `CELERIS_WEB_ATTESTATION_KEY_FILE`（Ed25519 PKCS#8、0600 file・0700 dir）。
  - daemon の公開鍵は変えずに、GUI と同じ鍵 file を指す。daemon 側の nonce 一回性はそのまま効く。
  - 鍵が無いときは `503 attestation_unavailable`。
  - 署名は 2 種類で、TTL はどちらも 20 秒（daemon の上限は 30 秒）。
    - waits 用: `AttestationClaims`
    - live/control 用: `RelayClaims`

### D2.2 認可の判定（全経路共通の guard）

`/browser/*` の全入口は、HTML・asset・JSON・WS upgrade の別なく、毎回この順で判定する。判定の結果は cache しない
（同時に走る判定の共有だけ許す）。

| 順 | 判定 | 拒否 |
|---|---|---|
| 1 | Host allowlist（既存） | 400 |
| 2 | password 認証が有効 | 403 `owner_unavailable` |
| 3 | 有効な cookie | 401 `unauthenticated` |
| 4 | cookie の session が owner grant を持つ | 403 `not_owner` |
| 5 | 変更系・WS は Origin が自 origin と完全一致（欠落も拒否）。変更系は CSRF token（session hash の HMAC）も要る | 403 `csrf_failed` / `origin_mismatch` |
| 6 | 経路の `task_id`/`run_id` が実在し、browser session がその task/run に束縛されている | 404 `not_found` / 403 `other_task` |
| 7 | run が RUNNING、session が active | 409 `not_running` / 410 `run_ended` |
| 8 | 認証区間外（ADR-0080 H3）。namespace 内のどの未完了 task にも認証待ち・credential 注入の履歴が無い（旧 relay の namespace guard） | 409 `auth_interval` / 403 `observation_stopped` |
| 9 | task-api の live grant（60 秒）が有効。`POST …/live/{run}/{session}/check` が `connected: true` | 410 `grant_expired`、task-api のコードをそのまま返す |

- 応答は固定コードの JSON（`{"code": "..."}`）だけにし、ページ内容・URL・upstream の本文を echo しない。
  全応答に `Cache-Control: no-store` と `Referrer-Policy: no-referrer` を付ける。
- 照会の失敗・取得上限の超過は拒否に倒す（fail closed）。

### D2.3 live proxy（HTTP と WebSocket）

- 宛先は起動設定 `CELERIS_WEB_LIVE_VIEW_UPSTREAM`（loopback の `host:port` だけ）。
  - 不正値なら gateway は起動しない（exit 2）。
  - 未設定なら本人にも `503 live_view_relay_unavailable` を返す。画面は「監視（イベント）」だけになる（D3.2）。
  - task event・grant・query の URL を宛先に使わない。上流へは固定の loopback Host/Origin で繋ぐ。
    Location・Set-Cookie 等の上流の header は Content-Type 以外転送しない。
- 入口は `GET /browser/live/{task_id}/{run_id}`（HTML）と `/browser/live/{task_id}/{run_id}/*`。
  - HTML の入口で task-api の `POST /api/v1/tasks/{id}/browser/live/{run}/{session}/grant` を呼ぶ。
  - 返った grant を cookie session × task/run の binding として gateway のメモリに持つ。
  - grant の失効前（残り 15 秒）に、guard を通したうえで grant を取り直す。旧 GUI は更新せず、60 秒で切れていた。
- dashboard（agent-browser 0.38.1）は絶対 path を使う。そのため次の固定 path だけを、`/api` relay より前で live relay が受け持つ。
  - `/_next/static/*`
  - `GET /api/sessions`
  - `GET /api/chat/status`
  - `GET /api/session/{port}/{tabs|status}`
  - WS `/api/session/{port}/stream`

  上の path は binding を持つ owner session からの要求だけを通す。それ以外は 404 にし、daemon へは流さない。
  daemon（task-api）に同じ名前の route が無いことは、gateway 試験で route 表と突き合わせて固定する。
  - 他の dashboard API（`/api/exec`・`/api/kill`・`POST /api/sessions`・`/api/chat`・`/api/models` 等）は
    `403 live_view_action_denied`。
- WebSocket は express server の `upgrade` で受ける。
  - 接続時は D2.2 の全判定を行う。
  - その後は、client→upstream の message ごと、upstream→client の data chunk ごと、無通信時 5 秒ごとに
    4・6・7・8・9 を再判定する。
  - 失敗したら close code 1008（logout・owner 失効は 1001）で切る。
- **人の入力の転送**（ADR-0080 D6 の読み取り専用を拡張する。人の決定 `browser-live-input`）。
  - client→upstream で常に通すのは `ack`・`config` だけ。
  - `input_mouse`・`input_keyboard`・`input_touch` は、次を全て満たすときに限り転送する。
    - この owner session が ADR-0099 の lease holder である
    - phase が `human_control` である
    - lease が期限内である
    - 認証区間外である

    判定は message ごとに `GET …/browser/control/{run}/{session}` で行う。
  - 満たさないときは捨て、client に `{"type":"input_denied","code":…}` を返す。binary frame は常に捨てる。
  - 決定が「読み取り専用のまま」なら、入力は常に捨てる。その場合の人の操作は lease を取って agent を止めるところまでとし、
    画面にも「操作は監視のみ」と出す。
- 埋め込み: `/browser/live/*` の応答だけ、CSP を `frame-ancestors 'self'` に、`X-Frame-Options` を `SAMEORIGIN` にする。
  他の経路は今のまま（`DENY`）にする。
  - SPA は `<iframe sandbox="allow-scripts allow-same-origin">` で埋め込む。
  - 「別タブで開く」も残す。
  - raw の `live_view_url` は、どの応答にも href にも出さない。

### D2.4 control・waits・identity の中継

SPA から daemon へは generic `/api` relay（daemon token を差す）で届く。しかし browser の人の操作は
本人の attestation が要るので、generic relay では通さない。

| gateway の経路 | 使う task-api | 備考 |
|---|---|---|
| `GET /browser/runs?task_id=` | `GET /api/v1/tasks/{id}/events?types=browser_updated`（最大 20 page） | 旧 `celeris/browser.ts` の導出を server 側で行い、`live_view_url` を落とす。各 run の live の可否（D3.2 の理由）を添える |
| `GET /browser/runs`（全体） | tasks の一覧（非終了・最近 200 件）から `browser-enabled` の task を選び、各 task の上の導出を行う（最大 20 task） | API が無いので gateway で合成する。後続の提案参照 |
| `GET /browser/control/{task}/{run}/{session}` | `GET …/browser/control/{run}/{session}` | owner 必須 |
| `POST /browser/control/{task}/{run}/{session}` | `POST …/browser/control/{run}/{session}` に live/control assertion を付ける | body は 4 KiB 以下。`command`: pause / takeover{ttl} / renew{ttl} / resume{fresh_snapshot, policy_origin_ok} / stop。`expected_version`・`idempotency_key` 必須。応答は POST の結果の直後に GET で取り直した状態（旧 GUI の型ずれを直す） |
| `POST /browser/control/{task}/{run}/{session}/release` | `POST …/browser/control/{run}/{session}/disconnect` | 画面を閉じる・離れるときの返却（D3.3）。`navigator.sendBeacon` でも送れるよう、CSRF は form 値でも受ける |
| `POST /browser/waits/{wait}/decision` | `POST …/browser/waits/{wait}/decision` | `approve_once` / `deny`。旧 GUI と同じ判定順（method→Origin→owner→size 8 KiB→csrf→wait の状態） |
| `POST /browser/waits/{wait}/credential` | `POST …/browser/waits/{wait}/credential` | username 1–256 B・password 1–1024 B。応答に入力値を含めない |
| `GET /browser/identities?project_id=` | `GET /api/v1/browser/identities?project_id=` | owner 必須 |
| `POST /browser/identities` | `POST /api/v1/browser/identities` | 256 KiB 以下 |
| `POST /browser/identities/{id}/revoke` | `POST /api/v1/browser/identities/{id}/revoke` | 対象の identity がその project のものかを確かめる |
| `POST /browser/identities/{id}/restore` | `POST /api/v1/browser/identities/{id}/restore` | 旧 GUI に無かった。追加する |
| `DELETE /browser/identities/{id}` | `DELETE /api/v1/browser/identities/{id}` | |

generic relay（`web/server/relay.js`）には次の拒否と redact を足す。

- 次の要求は `403 browser_route_required` で拒否する。
  - `/api/tasks/{id}/browser/**` の GET 以外
  - `/api/tasks/{id}/browser/live/**` の全て
  - `/api/browser/identities**` の全て
- `GET /api/tasks/{id}/browser/waits` と `GET /api/browser/waits` は通す（badge と受信箱に使う。秘密を含まない）。
- JSON 応答の `live_view_url` は値ごと除く。対象は generic relay と SSE（`/events`）の両方。
  task 詳細・events・org の profile のどこから出ても同じに扱う。

## D3. web 画面

### D3.1 構成（`web/features/browser/`）

| file | 役割 |
|---|---|
| `browser-query.ts` | `/browser/runs`・control・waits・identities の query と mutation。realtime invalidation は既存の `browser_updated`・`browser_wait_*`（`web/api/realtime/invalidation-map.ts`）に乗せる |
| `browser-runs-screen.tsx` | `/browser`: 実行中・最近の browser run の一覧（task・run・状態・待ちの数・lease の状態）と未決の waits |
| `browser-run-screen.tsx` | `/browser/runs/$taskId/$runId`: Live View（iframe）、control bar、イベントの流れ、待ち |
| `live-view-frame.tsx` | iframe・別タブ link・利用不可の理由 |
| `control-bar.tsx` | 監視のみ／操作中の表示、lease 操作、残り時間、返却 |
| `live-events.tsx` | status・tabs・url・console の scrub 済みイベント（task-api `/read` 由来。`last_seen` から再開） |
| `browser-waits-panel.tsx` | waits の decision・credential の form |
| `owner-session-notice.tsx` | 本人登録の challenge 発行と CLI の手順 |
| `identities-screen.tsx` | `/projects/$id/browser-identities`: 一覧・登録・失効・復元・削除 |
| `task-browser-section.tsx` | task 詳細 overview に差す run の要約と待ち |
| `*.test.ts(x)` | model の単体試験（状態の表示、理由の文言、lease 残り、href の検査） |

route file は次の 3 つ。

- `web/routes/browser.index.tsx`
- `web/routes/browser.runs.$taskId.$runId.tsx`
- `web/routes/projects.$id.browser-identities.tsx`

あわせて `web/server/spa-routes.js` に 3 path を足す。gateway の `/browser/live/*` などは SPA の route にしない。
`/browser/runs/...` と衝突しない。

### D3.2 Live View と表示状態

- live の可否と理由は gateway が返す。理由は旧 GUI の 6 つ（`owner_unavailable`・`not_owner`・`not_running`・
  `not_configured`・`auth_interval`・`relay_unavailable`）に `grant_expired` を足し、各理由の文言を出す。
- href は `^/browser/live/[0-9A-Za-z_-]{1,64}/[0-9A-Za-z_-]{1,64}$` に合うものだけを使う（旧 `safeLivePath`）。
- upstream が無い（launcher runtime で dashboard が無い等）ときも、イベントの流れ（`/read`）と control bar は使える。
  画面は「映像なし・イベントで監視中」と明示する。

### D3.3 監視のみ／操作中と lease

- control bar の先頭に常に大きく状態を出す。phase の表示は次のとおり。

  | phase | 表示 |
  |---|---|
  | `agent_running` | **監視のみ — エージェントが操作中** |
  | `pausing` | 一時停止を待っています（実行中 n 件） |
  | `paused` | 一時停止中 — 引き継げます |
  | `human_control` かつ holder が自分 | **あなたが操作中 — 残り N 秒** |
  | `human_control` かつ他 | 他のセッションが操作中 |
  | `stopped` | 停止 |

  「操作中」の間は枠の色を warning token に変える。色だけに頼らず、文言と `aria-live="polite"` の status で伝える。
- 操作のボタン:
  - 一時停止（pause）
  - 引き継ぐ（takeover、60 秒）
  - 延長（renew、60 秒。残り 15 秒で目立たせる）
  - エージェントに返す（resume。fresh snapshot と origin の確認 checkbox が要る）
  - 停止（stop。ConfirmDialog で確かめる）

  要求は 1 度に 1 つだけ送り、idempotency key は UUID とする。状態は 2 秒ごとに poll し、SSE が来たら取り直す。
- **返却を確実にする**: 返却漏れには 3 層で備える。
  1. 明示の「エージェントに返す」（resume）
  2. 画面の離脱（route 遷移・`pagehide`）で `/release`（disconnect）を beacon で送る。
     task-api の disconnect は人の lease を外して `paused` にする（`human_disconnected`）。agent は再開しない。
     agent への再開は、人が状態を確かめる resume だけが行う（ADR-0099 の検証済み resume）。
  3. beacon が届かなくても、lease は期限で `paused` に落ちる（ADR-0099。自動で agent に戻らない）

  `paused` のまま残った run は、一覧と task 詳細に「一時停止中（人の返却待ち）」の badge で出す。
- 認証区間中は全ボタンを無効にし、理由を出す。lease も取れない（ADR-0099 D8）。

### D3.4 waits と identity

- waits は reason ごとに form を出し分ける。
  - decision（`approve_once` / `deny`）: operation action、args digest、origin、policy revision
  - credential: username・password。`autocomplete="off"`、送信後に消す
- identity: project ごとに origin・id・generation・期限・状態を出す。
  - active のものに失効と復元、全件に削除（ConfirmDialog）を付ける。
  - 登録は state JSON を password 型の textarea 相当で受ける（7 日 TTL）。
- 本人でない session には、操作 form の代わりに `owner-session-notice` を出す。

### D3.5 導線と待ち badge

- task 詳細（`web/features/tasks/overview-view.tsx`）: browser run があれば `task-browser-section` を出す。
  run ごとに「Live View を開く」（`/browser/runs/$taskId/$runId`）と待ちの数を示す。
  待ちは `#browser-waits` に置き、その場で decision・credential できる。
- 受信箱（`web/features/inbox`）: 既存の `InboxKind` `browser_wait` の項目に「ブラウザの承認待ち」
  「credential 待ち」の badge を出し、対象の run 画面（無ければ task 詳細の `#browser-waits`）へ link する。
  approvals 画面にも同じ link を置く。
- project 画面: `/projects/$id/browser-identities` への link を 1 つ置く。
- nav: `web/components/shell/nav-items.ts` の work group の末尾に「ブラウザ」（`/browser`）を 1 行だけ足す。
  他の行は動かさない。並行 web task（01M46VAZ0G・01M46VEZYD・01M46VVAD0）と同じ file に触れる場合も
  1 行の追加に留め、統合の衝突は両方の行を残して解く。shell・page header・ScreenFrame の構造は変えない。

### D3.6 狭幅と a11y

- 360px 幅では次の順に縦に積む。control bar は上端に sticky にする。
  1. 状態
  2. 操作ボタン
  3. iframe（16:10、幅いっぱい）
  4. 待ち
  5. イベント
- タップ領域は 44×44 以上（`min-h-11`。`pnpm mobile-audit`）。
- iframe に `title`（「ブラウザの Live View: <task>」）を付け、キーボードで iframe を抜けられるよう前後に focus 可能な見出しを置く。
- 状態変化は `role="status"` で読み上げる。lease の残り秒は 15 秒ごとにだけ読み上げる。
- 色・token は既存の design system（`web/DESIGN.md`・`FRONTEND_CONTRACT.md`）に従う。生の色は使わない。

### D3.7 旧 GUI との対応表

| 旧 GUI | web | 差 |
|---|---|---|
| `components/BrowserRunsPanel.tsx`（task overview の「ブラウザ」card） | `task-browser-section.tsx`＋`/browser` 一覧 | 全体の一覧を新設。待ちの数・lease 状態を一覧に出す |
| `BrowserRunsPanel` の `safeLivePath`・`DISABLED_TEXT` | `live-view-frame.tsx` | 理由に `grant_expired` を追加。新タブだけでなく同一 origin の iframe でも出す |
| `routes/browser.live.ts`＋`celeris/browser-live.server.ts`＋`browser-live-relay.server.ts`（HTTP・WS relay） | `web/server/browser/live*.js` | 同じ allowlist と再判定。grant を更新する（旧は 60 秒で切れた）。lease holder にだけ入力を転送する（決定次第） |
| `browser-live.server.ts` の injected 「Live events」aside | `live-events.tsx`（SPA 側） | dashboard の HTML を書き換えず、SPA で `/read` を表示する。upstream が無くても監視できる |
| `routes/browser.owner-session.ts`＋`browser-owner.server.ts` | `POST /browser/owner-session`＋`owner-session-notice.tsx` | 同等。socket は web 専用 |
| `browser-attestation.server.ts` | `web/server/browser/attestation.js` | 同じ鍵 file を使う |
| `routes/browser.control.ts`＋`components/BrowserControl.tsx` | `/browser/control/*`＋`control-bar.tsx` | 「監視のみ／操作中」の明示、返却の 3 層、POST 後に状態を取り直す |
| `routes/browser.waits.$waitId.decision.ts`／`credential.ts`＋`BrowserWaitsPanel.tsx` | `/browser/waits/*`＋`browser-waits-panel.tsx` | 同等。受信箱の badge から直接辿れる |
| `routes/approvals.tsx`・`routes/inbox.tsx` の読み取り一覧 | approvals・inbox の badge と link | link 先を run 画面にする |
| `routes/browser.identities.$projectId.tsx` | `identities-screen.tsx` | restore を追加 |
| `celeris/task-detail.server.ts` の `redactLiveViewUrls`、`routes/events.ts` の SSE redact | relay.js・events.js の redact | org profile 等の全 JSON に広げる |

## D4. 試験方針

- **gateway 単体**（`web/server/browser/*.test.mjs`、`node:test`）
  - 既存の relay 試験と同じく、loopback の偽 daemon（`http.createServer`）を使う。
  - 偽 browser upstream（偽 dashboard。HTML・`/_next/static`・tabs/status・WS stream）も loopback に立てる。
  - 確かめること:
    - D2.2 の各拒否コード（未認証・他 session・Origin 欠落・他 task・終了 run・認証区間・grant 失効）
    - 許可外 dashboard API の 403
    - WS の再判定による切断（偽 daemon の check を途中で `connected:false` にする）
    - logout での 1001 close
    - 入力 message の lease 条件での転送と破棄
    - generic relay の `browser_route_required`
    - `live_view_url` の redact
    - attestation の署名の claims
  - 時間は注入した時計で進める（grant 更新・lease 残り）。sleep で待たない。
- **偽 browser backend と fake-daemon fixture**（`web/e2e/support/fake-daemon.mjs`）
  - rich fixture に次を足す。fixture は schema の `$defs`（`BrowserRun`・`BrowserWait*` 等）から作り、`validateFixture` を通す。
    - browser task 1 件
    - `browser_updated` events（RUNNING 1 run・終了 1 run）
    - waits（decision・credential）
    - control 状態の遷移
    - identities
    - live `/read` の events
  - Playwright の gateway は偽 upstream も起動する。
- **Playwright**（functional）
  - 一覧 → run 画面 → 監視のみ表示 → takeover で「あなたが操作中」 → resume で返却
  - 離脱で release が送られる
  - waits の承認と credential（入力値が応答・DOM に残らない）
  - identity の失効・復元
  - 受信箱の badge → run 画面
  - 本人でない session の notice と直打ち URL の拒否
  - 360/390/412px と desktop の screenshot、axe、mobile-audit
- **daemon 側**: org-node の matching 試験（D1.3）。task-api の browser 系は既存試験のまま変えない。
- **実機確認（opt-in）**: 葉 real-check が台本を書く。
  - 構成: 試験用 DB の daemon（ADR-0126、本番 port を拒否）、launcher runtime、web gateway、loopback の試験ページ
  - 確かめる流れ: grant 付き node への matching、Live View の表示、takeover と返却
  - 台本は既定で走らない。未実行は exit 2 とし、check は `[ $? -eq 2 ]` で skip を合格にしない。
  - LLM を使う実行は認証のある環境でだけ行い、証跡を進捗に残す。
  - launcher runtime に dashboard の upstream があるかは、ここで初めて確かめる。

## D5. 設定の web 編集と task 単位 allowed_domains（2026-10-06 人の追加要望）

人から次の 2 点が追加で求められた。いずれもこの task の範囲に含め、決定だけここに記す。
具体的な実装は org-node の外（follow-up task）に送る。理由は、変更が task-api（org PATCH の event 記録）・
task-core（task 作成時の検証・`BrowserTaskPolicy` のスキーマ）・CoS/planner の prompt という複数 crate に
またがり、org-node 葉の受け入れ条件（org node 定義・matching 試験・投入手順）を超えるため。

### D5.1 web からの設定編集

既に足りているもの:

- grant（`allowed_domains`・harness・budget・`credential_policy_ids`）の編集は、D1.3 で使う
  `PATCH /api/v1/org/{id}`（`profile` 丸ごと置換、`crates/task-api/src/handlers/org.rs:134-179`）で
  既にできる。browser 専用の管理 API は**新設しない**（経路を二重にしない）。
- 不正な値の拒否は `BrowserCapability::validate()`（`crates/task-core/src/browser.rs:31-51`）が既に行う。
  `allowed_domains` の各 host は `valid_host()`（同 L54-66、英数・ハイフン・ドット区切りの DNS ラベルのみ）
  を通るので、裸の `"*"` や `scheme://` 付きの値、ポート付きの値は構文として弾かれる
  （既存試験 `rejects_credential_urls_and_domain_policy_bypasses`、同ファイル L645-693 に host 側の
  `"*"` 拒否を含む）。`live_view_url` も同様に `valid_live_view_url()`（L70-89）が scheme・userinfo・
  query・fragment を拒否する。追加の検証コードは不要。

欠けているもの（決定）:

- **D5.1-a**: `patch_org_node` は `tracing::info!(who = "admin", ...)` の固定文字列ログだけで、
  `Event` には org 系 variant が無い（`crates/task-core/src/model.rs` の `Event` enum）。
  **決定**: `Event::OrgNodeUpdated { node_id, actor, fields_changed }`
  （値そのものは入れず、変更した欄名の要約だけを持つ）を足し、org の POST/PATCH 時に実 actor
  （web の owner session 識別子、または CLI 呼び出し元）付きで記録する。
- **D5.1-b**: 管理画面（org の browser 欄を編集する web 画面）は web-ui WU の範囲に含める。
  バックエンドは D5.1-a の event 記録込みの既存 `PATCH /api/v1/org/{id}` をそのまま使う。

### D5.2 task 単位の allowed_domains と交わり判定

既に足りているもの:

- `BrowserTaskPolicy.network_domains`（ADR-0080 D1、`crates/task-core/src/browser.rs:231-248`）と
  `EffectiveBrowserPolicy::derive`（同 L423-503）が、まさに要望どおり
  「grant の `allowed_domains` ∩ task の `network_domains`」を実装済み（L465-486）。
  - `task` 引数が `None`（`BrowserTaskPolicy` 未設定）なら `BrowserPolicyRequired` で拒否。
  - 交わりが空集合なら `EmptyDomains` で拒否。
  - つまり「grant の外は拒否」も「task 側が空集合なら動けない」も、既に fail-closed で成立している。
  - この結果（`policy.json`）は `task-worker/src/browser_policy.rs` 経由で worker 内の
    `browser_isolation.rs::EgressPolicy`（namespace の filtering proxy）にそのまま渡る。
    「policy broker と egress の両方で判定する」という要望は、**単一の `derive()` の出力を両方が読む**
    ことで満たす（判定を 2 箇所に別々に実装しない。実装が割れると片方だけ直し忘れる事故が起きる）。

欠けているもの（決定）:

- **D5.2-a（作成時に拒否するか、空集合として扱うか）**: **作成時に拒否する**。
  `skills` に `browser-enabled` を含む task は、`TaskSpec` に新欄 `browser_network_domains`
  （非空の `Vec<String>`）を必須にする。欠落・空なら `POST /tasks` を 422 で拒否する
  （`crates/task-ops/src/add.rs` の `browser_requested` 分岐、現状 L645-663 を拡張）。
  理由: 黙って空集合のまま作れると、後から気づかれない「何もできない browser task」が残る。
  `derive()` の fail-closed 挙動（L465-486）はそのまま実行時の二重の網として残す。
  作成時の検証は `normalize_host_pattern`（grant 側と同じ正規化）で構文も確かめ、grant との交わりは
  まだ求めない（grant は assignee 決定後に変わり得るため、matching 前の作成時点では grant を前提にしない。
  交わりの判定は実行時の `derive()` に任せる）。
- **D5.2-b（子 task の上限）**: 子 task の `browser_network_domains` は、親 task のそれの**部分集合**
  でなければ作成を拒否する。親が `browser_network_domains` を持たない（browser-enabled でない）場合、
  子も持てない。これにより「子 task が親を超えて広い host を要求する」ことを作成時に防ぐ。
  grant 自体の拡大（人が PATCH で grant を広げる）とは独立の軸であり、この部分集合チェックは
  task 系譜の中だけで完結する。
- **D5.2-c（CoS・planner の最小 domain 規則）**: task を作る側（CoS の起票、planner の子 task 生成、
  `create_task` の説明文）の prompt・skill に次を足す。
  - 規則: 「browser-enabled task には、その task が実際に必要とする最小の host 集合だけを
    `browser_network_domains` に書く。部署の grant 全体を書き写さない。」
  - 書き方の例: 社内 wiki の 1 ページだけを読む task なら
    `browser_network_domains: ["wiki.internal.example"]`（grant が
    `["*.internal.example", "github.com"]` であっても、使わない host は書かない）。
  - 置き場所: CoS の prompt（`crates/task-core/src/console_action.rs` 近辺の説明文）と
    `create_task` のツール説明（`crates/task-api/src/docs.rs` 等）。

### D5.3 follow-up task の範囲（この WU の外）

次を 1 本の follow-up task として起票する（`followups.json`）。

1. `Event::OrgNodeUpdated`（actor 付き）を足し、org の POST/PATCH で記録する。
2. `TaskSpec.browser_network_domains` を足し、browser-enabled task の作成時検証（D5.2-a）と
   親子の部分集合チェック（D5.2-b）を実装する。
3. CoS・planner の prompt/skill に最小 domain の規則と例（D5.2-c）を足す。
4. 決定的な試験: 交わり判定（既存 `derive()` の試験で十分か確認し、無ければ足す）、grant 外の拒否、
   作成時拒否（domains 無し）、子が親を超える拒否、org PATCH の event 記録と actor、
   不正値（wildcard・scheme）の拒否（既存試験の確認）。

## 帰結

- browser task は `browser-execution` にだけ割り当たり、grant は loopback から始まる。
- 人は web だけで browser run を見つけ、監視し、本人として lease を取って止め、返し、承認と credential 入力ができる。
- web gateway の守る範囲が増える。owner socket・attestation 鍵・live upstream の 3 つの起動設定が要る。
  docs/ops に書き、`celeris-web@` unit の環境に足すのは人の操作とする。
- 旧 GUI の browser 画面は web と同等以上になる。旧 GUI の撤去は本 ADR の範囲外とする。
- task 単位の `allowed_domains`（D5.2）は作成時に必須化し、黙って部署 grant 全体に広がらない。
  org の browser 設定は既存の `PATCH /api/v1/org` で編集し、actor 付き event を残す（D5.1）。

## 付記: 実装との突き合わせ（2026-10-06、close-out）

統合後の HEAD（`a0927c8b`）で各節を実装と突き合わせた。証拠は
[agent-docs/progress/2026-10-05-browser-web-live-view.md](../progress/2026-10-05-browser-web-live-view.md)。

### D1 部署

| 決めたこと | 実装 |
|---|---|
| `browser-execution`（section、親 `engineering`、genre `coding`）を seed に足す | `config/org.example.toml`（`[[org]] id = "browser-execution"`、D1.2 の profile と同じ） |
| 投入 JSON と手順 | `docs/ops/browser-department-org.json`（`OrgCreateBody`）と `docs/ops/browser-department.md`（POST・GET・PATCH・DELETE の curl 例） |
| matching 試験 | `browser_specialist_node_receives_browser_enabled_tasks_and_ungranted_nodes_are_excluded`（`crates/task-ops/src/matching/tests.rs`）。grant なしで `Unroutable`、grant 付きで `browser-execution` 宛、grant 無し node（software-engineering 等）は通常 task のまま、grant を外すと再び `Unroutable` |
| browser を要求しない task を専用課から外す | `crates/task-ops/src/matching.rs`（実効 profile に `browser-enabled` を持つ node を除外）＋ org-tests 葉で `crates/celeris` の org 試験 3 件を更新（seed 15 件・id 一覧・`route(&[])` が `browser-execution` にならない） |

D1.3 の 4 本目中「子 node で grant が継承・置き換え」の個別試験は未追加（`Profile` 継承規則の汎用試験で
カバー、org-node の申し送りと同じ判断）。本番への POST は配送後に Fable が行う（この run は未実施）。

### D2 gateway

| 決めたこと | 実装 |
|---|---|
| live proxy（HTTP・WS） | `web/server/browser-live.js`（928 行。`/browser/live/{task}/{run}` の grant 取得・15 秒前更新、`/_next`・`/api/sessions`・`/api/chat/status`・`/api/session/{port}/*` の固定 path、WS `upgrade` で message ごとの再判定） |
| 全経路共通の guard（D2.2） | 同上。拒否コード `owner_unavailable`・`unauthenticated`・`not_owner`・`csrf_failed`・`origin_mismatch`・`other_task`・`not_running`・`run_ended`・`auth_interval`・`grant_expired` |
| owner session（D2.1） | 同上（`GET/POST /browser/owner-session`、`CELERIS_WEB_OWNER_SOCKET`、12 hex challenge）。`celerisctl browser owner-session approve <challenge> --socket <path>` |
| 入力転送 | 同上。`ack`・`config` は常に、`input_mouse`・`input_keyboard`・`input_touch` は lease holder かつ `human_control` かつ期限内かつ認証区間外のみ。binary frame は常に破棄 |
| control・waits・identity 中継（D2.4） | 同上（`/browser/control/*`・`/browser/control/.../release`・`/browser/waits/{wait}/decision`・`/credential`・`/browser/identities*`。restore は追加） |
| generic relay の拒否と redact | `web/server/relay.js`（`browserRouteRequired` で `browser_route_required`）・`web/server/events.js`（SSE の `live_view_url` 除去） |
| 試験 | `web/server/browser-live.test.mjs`（`node:test`。D4 各項目）・`web/server/relay.test.mjs`・`web/server/events.test.mjs` |

### D3 web 画面

`web/features/browser/` には `browser-query.ts`・`browser-model.ts`・`browser-runs-screen.tsx`・
`browser-run-screen.tsx`・`live-view-frame.tsx`・`control-bar.tsx`・`live-events.tsx`・
`browser-waits-panel.tsx`・`owner-session-notice.tsx`・`task-browser-section.tsx`・
`browser-settings-screen.tsx`（D2.0/D5.1-b の設定画面、ADR 表の 1 行追加分）と単体試験がある。
route は `web/routes/browser.index.tsx`・`browser.runs.$taskId.$runId.tsx`・
`projects.$id.browser-identities.tsx`・`browser.settings.tsx`（追加）の 4 つ。
`web/server/spa-routes.js` にも 4 path。

- 導線: `web/components/shell/nav-items.ts` の「ブラウザ」1 行、task 詳細の
  `web/features/tasks/overview-view.tsx`（`TaskBrowserSection`・`#browser-waits` への link）、
  受信箱 `web/features/inbox/inbox-screen.tsx`・承認 `web/features/approvals/approvals-screen.tsx` の
  browser_wait badge と run 画面 link。
- Live View は同一 origin の `/browser/live/...` だけ（`live-view-frame.tsx`）。raw `live_view_url` は
  SPA の DOM・JSON に出さない（`safeLivePath`、relay・events の redact）。
- 試験: `web/e2e/browser/`（runs-list・lease・proxy-authz・waits・identities・links・narrow-a11y・settings・
  fixture-smoke の 9 spec）、`web/e2e/support/fake-daemon.mjs` の browser backend。

### D2.0 / D5（2026-10-06 追記）

| 決めたこと | 実装 |
|---|---|
| origin 形式の検証・包含・交差 | `crates/task-core/src/browser/origin.rs`（`AllowedOrigin::parse`・`origin_covers`）。`browser.rs` の `task_run_policy`・`EffectiveBrowserPolicy::derive` が origin で交差。形式の決定は [2026-10-05-browser-allowed-origins.md](2026-10-05-browser-allowed-origins.md) |
| browser-enabled 新規 task の欠落・空は作成拒否（全経路） | `Task.requirements.browser.allowed_domains` を正本に。`NewTaskSpec`・`PlanUnitSpec`・`ExecutionChildSpec`・`ConsoleAction::CreateTask`・`SqliteStore::insert_tx`・`PATCH /tasks/{id}` で同じ決定的検証 |
| 子は親の部分集合 | 同上（`origin_covers` で scheme・host・port。親が browser-enabled でない場合子は持てない） |
| broker と egress の両方で grant 外を拒否 | `task_worker::browser_policy::{prepare_for_task, admit, egress_allow}`。単一の `derive()` 出力を broker・egress・credential 判定・shim（`browser_cli.py`）が共有（D5.2 の「2 箇所に別実装しない」どおり） |
| grant 縮小は次の run から | `task-dispatch` の `browser_run_policy`（run ごとに org の現 grant を読み `RunContext.browser_policy` に入れる） |
| 管理 API | `PATCH /api/v1/org/{id}/browser-settings`（`crates/task-api/src/handlers/org.rs::patch_browser_settings`）。D5.1 の「browser 専用管理 API は新設しない」とは異なり下位経路を新設した（`{id}` が node id 文字列の `PATCH /api/v1/org/{id}` と URL 衝突するため）。`allowed_domains`・`credential_policy_ids`・`credential_identity_ids`・`harnesses`・`budget` の任意組合せ。失敗時は DB と event を変えない。成功時は `org_browser_events`（migration `0051`、`crates/task-core/src/store/org.rs`）に actor=`admin`・変更前後を同 transaction で書く |
| actor 付き event（D5.1-a） | `Event::OrgNodeUpdated` 型の新 variant は**追加していない**。browser 設定変更は専用の監査 table `org_browser_events` に残す形にした（`Event` enum の変更を browser 設定に限定せず済ませるため）。機密値は credential policy ID と identity ID のみ |
| 最小 origin 規則の prompt | `crates/task-worker/src/claude_code/prompt.rs`（`BROWSER_ALLOWED_DOMAINS_GUIDANCE`、execution-plan/2・/3 両方）と `crates/task-worker/src/preamble.rs`（CoS の `create_task` 例、例 `https://billing.example.com`）。CoS 専用の外部 skill file は無く（`config/skills/` に無し）、preamble の Rust 文字列 1 箇所に入った |
| web の設定画面 | `web/features/browser/browser-settings-screen.tsx`（`/browser/settings`。`web/routes/browser.settings.tsx`）。422 の field 表示・確認 dialog・scheme/wildcard の入力補助 |

試験名の完全な対応表は
[domain-policy.md](../progress/2026-10-05-browser-web-live-view/domain-policy.md)（`browser_allowed_domains_` 23 件・
`browser_settings_` 9 件〈task-api/tests/organization.rs。`crates/celeris` の
`browser_settings_default_to_unconfigured_and_site_policy_validates` は 2026-10-01 以前からの
 config 検証試験で本節の管理 API 試験ではない〉の一覧）。

### 残っているもの（この run で直さない）

- **本番の org 投入と実機確認は未実施**: 配送後に Fable が `docs/ops/browser-department.md` で POST し、
  `scripts/dev/browser-web-live-check.sh`（opt-in・未実行時 exit 2）で実行し、証跡を進捗に転記する。
- **egress は `host:port` の完全一致**: wildcard origin（`*.example.com`）は egress で一致せず
  fail-closed に拒否される（以前からの挙動）。実運用では wildcard ではなく単一 origin で指定する。
- **launcher 経路（`browser_launcher`）は prepared policy を受け取るだけ**: 本 ADR の範囲では追加の
  origin 検証を足していない。
- **`org_browser_events` の読み取り API は無い**: web 設定画面は直前の保存の actor・`updated_at` だけを出し、
  過去の履歴は出せない（読み取り API の追加は別 task）。
- **mobile-audit の browser fixture**: owner 状態の control bar・待ち form の 44 px は
  `scripts/mobile-audit.mjs`（password 無・owner socket 無の fixture）では見ていない。owner 状態は
  `web/e2e/browser/narrow-a11y.spec.ts`（360px）が確かめる。
- **`v3: true` を browser 画面に付けていない**: S1 latency・S2 realtime の browser backend 対応が
  別途要るため（web-ui の申し送り）。
- **`check:parity` が exit 1（`/browser/settings: missing V3 screen`）**: web-settings 葉が
  `web/server/spa-routes.js` に route を足したが、V3 画面台帳 `web/e2e/support/screens.ts` の
  対応行を足していない（close-out での突き合わせで発見）。修正は `screens.ts` 1 行だが、
  `mobile-gate`（nfr project）が fixture gateway（browser backend 無し）で h1 を期待するため、
  fixture 側の browser grant 付き org と `mobile-audit` の再実行が要る。web-settings 葉の
  再計画で直す（close-out は文書のみを編集する範囲）。
- **migration 0048〜0050 はこの branch では空番**: 本 branch では 0051 のみが入る。統合時に他の
  branch 側の 0048〜0050 実体が入ったら、番号の重複検査（`scripts/dev/check-migration-numbers.sh` 相当）で
  確認する。

## 付記: 実機再確認（2026-10-06、人の決定 rerun-evidence への回答 fable-rerun）

launcher-fix 統合後の tree（`0e167295`、`d8f3e5e6` を祖先に持つ。証跡の `versions.txt` と
`git merge-base --is-ancestor` で確認済み）で Fable が `scripts/dev/browser-web-live-check.sh` を
再実行し、証跡を commit（`3680b70e`）した。D4・D6 修正は実機で効いていた（run 起動・shim
config の `policy_sha256` あり・`policy.json` が `launch` を許可）が、再実行は**不合格**で、
loopback の試験ページが launcher egress の設計（`check_egress` が IP literal と private/loopback を
拒否）で許可 origin でも開けず、範囲外 origin の egress 拒否の証跡は未取得（egress は拒否を記録しない）。
再実行で台本・gateway・egress の確認 4 点（settings URL、拒否 upgrade の未処理 ECONNRESET、
試験専用 egress 許可と拒否記録の欠落、cleanup の socket 残留）が見つかり、修正後に Fable が
3 回目の再実行を行う。経緯（attempts 1〜8 → launcher-fix → 再実行）と各段の結果は
[agent-docs/progress/2026-10-05-browser-web-live-view/real-check.md](../progress/2026-10-05-browser-web-live-view/real-check.md)
に記録する。

## 付記: 試験専用 egress 許可と拒否理由の記録（2026-10-06）

final review の指摘 R3 への回答。実機確認台本 `scripts/dev/browser-web-live-check.sh` の試験ページは
`http://127.0.0.1:<port>` に立つが、launcher 経路の egress（`crates/task-worker/src/browser_egress.rs`）と
`task_core::browser_isolation::check_egress` は IP literal（`IpLiteral`）と DNS で loopback に解ける名前
（`PrivateAddress`）を拒否するので、許可 origin への navigation も egress 拒否の証跡も取れなかった。
egress は拒否を記録しなかった。ADR-0102/0104 の egress 判定は変えず、次の 2 つを足す。

### E1. 試験専用 loopback 許可の形（既定 off・完全一致のみ）

- `EgressPolicy` に `test_loopback_allow: BTreeSet<String>` を足す。serde は `default`（無ければ空）で、
  空なら JSON に出さない（`skip_serializing_if`）。既存の policy・既存の JSON はそのまま空集合になる。
- `check_egress` の `Connect` は、**host が文字列として `127.0.0.1` そのもの**で、port が 0・53・853 以外、
  かつ `127.0.0.1:<port>` が `test_loopback_allow` に**完全一致**で入っているときだけ `Ok` を返す
  （この経路では DNS 結果を見ない）。それ以外は従来どおり判定し、`IpLiteral`・`PrivateAddress`・
  `NotAllowed`・`InvalidHost`・`Unresolved`・`Ipv6Disabled`・`DnsBypass`・`ProxyChain` の結果は 1 つも
  変わらない。特に次は許可集合にあっても拒否のまま: 別 port、`localhost` 等の名前（DNS を経た loopback は
  `PrivateAddress`）、`10.x` 等の private literal、`[::1]`、`2130706433`・`0x7f.1`・`0177.0.0.1` などの
  10/8/16 進の変種（`IpLiteral`）、`127.0.0.2` など `127.0.0.1` 以外の loopback。
  集合に `127.0.0.1:<port>` 以外の形の項目があっても、どの要求とも一致しないので何も許さない。
- egress の CONNECT 経路（`browser_egress::destination`）は、許可された loopback literal なら resolver に
  問い合わせず `127.0.0.1:<port>` へ直接つなぐ（名前解決を経ないので DNS rebinding の余地も無い）。
- 集合を埋めるのは launcher だけ。launcher の root 所有の固定 config（`/etc/celeris-browser/launcher.toml`
  と、試験用に別 path で起こす launcher の config）に `test_loopback_allow = ["127.0.0.1:<port>", …]` を
  明示したときに限る。daemon の設定・task policy・web の設定画面からは入れられない（ADR-0116 D5 と同じ
  理由: daemon が乗っ取られても egress を広げられない）。
- launcher config の欄は `BackendConfig.test_loopback_allow`（省略時は空 = off、
  `crates/task-worker/src/browser_launcher/backend.rs`）。各項目は `127.0.0.1:<port>`（port は 10 進・先頭
  0 なし・1〜65535・53/853 以外）の形だけを受け、それ以外が 1 つでもあれば config 読込みで起動を拒否する
  （`validate_test_loopback`）。session の `EgressPolicy.test_loopback_allow` に入るのは、session の
  `allowed_domains`（`origin_host_port` の `host:port`）と config の集合の**交わり**だけ（`egress_policy`）。
  有効なとき launcher は起動時に stderr（journal）へ
  `test-only loopback egress enabled: 127.0.0.1:<port>, …` を出す。

### E2. 本番では有効にできない（fail-closed の判定方法）

試験許可が空でない設定は、次のどれか 1 つでも本番と一致したら**起動を拒否**する（判定できないときも拒否）。

- **launcher 側**（config を読んだ直後、socket を listen する前）: 自分の config path が
  `/etc/celeris-browser/launcher.toml`、`socket` が `/run/celeris-browser/launcher.sock` と一致するか、
  `state_dir` が `/var/lib/celeris-browser` と一致・その配下・その祖先なら exit 非 0
  （`refuse_test_loopback_in_production`、`crates/task-worker/src/bin/celeris-browser-launcher.rs`）。
  比較は path を正規化（存在する祖先までの `canonicalize`、symlink を解き、残りを足す）してから行い、
  正規化できない path（相対 path、未作成部分の `..`）は一致とみなす。systemd の socket activation で
  受け取った socket は、その path（`getsockname`。採れなければ拒否）で同じく比べる。
- **daemon 側**: launcher protocol に `hello`（要求 `{"type":"hello"}`、応答
  `{"type":"hello","protocol_version":3,"test_loopback_allow":[…]}`。空なら欄を出さない）を足し、launcher は
  config の試験許可をそのまま申告する。daemon は `[browser] runtime = "launcher"` のとき起動時に本番判定を
  し（`crates/celeris/src/daemon/bootstrap.rs` の `browser_launcher_refuses_test_loopback` /
  `launcher_test_loopback_refusal`）、本番なら各 session の `start_session` の前に `hello` を送る。申告が
  空でない、または `hello` に答えない（旧版の launcher・error）なら session を作らず
  `isolated_runtime_unavailable` にする（`LauncherRuntime::start_guarded`）。本番判定は次のどれか:
  ADR-0126 A1 の `worker_db_guard_protected_set`（getpwuid の home の `~/.config/celeris/config.toml`）で
  P を決められない、daemon が読んだ config がその本番 config そのもの（正規化して比べる。verify の staging
  のように DB だけ差し替えた daemon も含む）、`judge_worker_db_guard`（印は `Absent` で呼ぶ）が
  `RefuseProduction`（DB・DB のディレクトリ・token の path か中身が本番に当たる）。本番 daemon が試験用
  launcher に誤ってつながっても有効にならない。本番の daemon は `hello` に答える launcher を要るので、
  この版の daemon を入れるときは launcher の binary も同じ release のものに入れ替える。
- 試験許可が空の launcher（既定）にはこの検査は掛からず、従来どおり動く。

### E3. 拒否理由の記録

- 1 件の形: `{"kind": <EgressDenied の snake_case>, "host": <要求の host>, "port": <u16>,
  "at": <RFC 3339 UTC>, "session_id": <session id>}`。`host` は CONNECT の authority から取った値で、
  構文で落ちた要求（`EgressError::Denied` で `EgressDenied` が付かないもの）は `kind: "malformed"` とし
  host・port を空にする。**要求本文・header・DNS 応答・上流の error 文は残さない**。
- 置き場所: launcher の session dir（`<session_root>/<session_id>/egress-denied.jsonl`、session と同じ
  所有者・0600）に 1 行 1 件で追記する。親 relay は proxy の stderr から固定 JSON の `kind`・`host`・`port`
  のみを最大 512 byte 読み、時刻と session id を親で付ける。1 行は最大 512 byte、拒否は最大 128 件。
  超えたら `{"kind":"truncated","at":...,"session_id":...}` を 1 行だけ追加し、その後は捨てる。
  記録ファイルは browser 起動前に排他的に作成し、既存なら起動を拒否する。親は開いた file descriptor
  を保持して追記し、browser が書ける session dir 内での差し替えを追わない。
  session の終わりに launcher が controller 経由で daemon に返し、daemon は run の artifact（browser の
  session 証跡と同じ所）に置く。台本 `browser-web-live-check.sh` はそれを `egress-denied.json` として集める。
- 実装の葉: E1 は policy（本付記と `check_egress`・`browser_egress`）、E2 は launcher-cfg、E3 は denial-record。

### E4. close 実装との突き合わせ（2026-10-06）

- host 設定: `docs/ops/browser-launcher-host-setup.md` §4 は `BackendConfig.test_loopback_allow`（`crates/task-worker/src/browser_launcher/backend.rs::BackendConfig`）を記す。省略は off、launcher 起動時の拒否と有効ログは `crates/task-worker/src/bin/celeris-browser-launcher.rs::run` / `refuse_test_loopback_in_production`。本番 daemon の fail-closed handshake は `crates/celeris/src/daemon/bootstrap.rs::browser_launcher_refuses_test_loopback` と `crates/task-worker/src/browser_launcher/runtime.rs::LauncherRuntime::start_guarded`。
- 許可ページは `scripts/dev/browser-web-live-check.sh` が launcher TOML の `test_loopback_allow` に `127.0.0.1:<PAGE_PORT>` があることを起動前に検査する。拒否証跡段は `<state_dir>/sessions/*/egress-denied.jsonl` を読み、範囲外 `127.0.0.1:<DENIED_PORT>` の `kind=private_address`・host・port・時刻・session id を検査して `egress-denied.json` に集める。不許可ページに GET が無いことも検査する。本文/header/DNS 応答は証跡に含めない。
- launcher 配置時は daemon と同じ release の launcher binary へ更新する。host の root 操作や本番設定変更は人がこの運用手順で実施し、実機再実行は `CELERIS_USERNS_TESTS=1` を含め [実機確認手順](../../docs/ops/browser-web-live-check.md) に従う。

### E5. 実装・試験対応

| 契約 | 実装 | 試験 |
| --- | --- | --- |
| loopback allow 欄の検証・既定 off・session allowlist との交差 | `crates/task-worker/src/browser_launcher/backend.rs::BackendConfig::{validate_test_loopback, egress_policy}` | `egress_test_loopback_default_off_when_config_omits_it`, `egress_test_loopback_policy_is_intersection_with_session_domains`, `egress_test_loopback_config_rejects_anything_but_127_0_0_1_port` |
| 本番 launcher path/socket/state_dir 起動拒否と enabled log | `crates/task-worker/src/bin/celeris-browser-launcher.rs::run`; `backend.rs::refuse_test_loopback_in_production` | `egress_test_loopback_refuses_production_config_socket_or_state_dir` |
| 本番 daemon と launcher hello の fail-closed | `crates/celeris/src/daemon/bootstrap.rs::browser_launcher_refuses_test_loopback`; `crates/task-worker/src/browser_launcher/runtime.rs::LauncherRuntime::start_guarded` | `egress_test_loopback_production_db_or_config_daemon_refuses_test_launcher`, `egress_test_loopback_production_daemon_refuses_test_launcher`, `egress_test_loopback_production_daemon_refuses_launcher_without_hello` |
| session 単位の拒否記録と秘密非記録 | `crates/task-worker/src/browser_egress.rs`; `crates/task-worker/src/browser_launcher/backend.rs` relay | `egress_denial_record_reasons_and_no_request_secrets`, `egress_denial_record_allowed_connection_has_no_entry`, `egress_denial_record_malformed_and_bounded`, `egress_denial_record_rejects_preexisting_file` |
