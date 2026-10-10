# ADR 2026-10-10: browser launcher の本人向け Live View frame 経路

---
tasks: [01M4HS8VQDB3JQHJRJ0ZDBY57H]
---

- 日付: 2026-10-10
- 状態: 採用（frame の揮発配送・分離契約）。credential session の既定公開範囲は人の決定 `live-credential-default`（D7、2026-10-10）で確定済み
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md) D3・D6、[ADR-0100](0100-browser-phase3-live-proxy-acl.md) D2、[ADR-0115](0115-browser-ptrace-owner-ns-launcher.md)、[ADR-0116](0116-browser-launcher-implementation.md)、[2026-10-05 browser department / web Live View](2026-10-05-browser-department-web-live-view.md)、[2026-10-09 credential username / post-login read](2026-10-09-browser-credential-username-and-post-login-read.md)、[2026-10-09 launcher CredentialUse release](2026-10-09-browser-launcher-credential-release.md)

## 文脈

launcher runtime の Chrome は uid 995 の sandbox 内で `--remote-debugging-pipe` を使い、CDP pipe と controller は launcher process が所有する（`crates/task-worker/src/browser_runtime.rs`）。daemon worker は pipe FD を持たない。現在 `browser_launcher_run.rs` と `browser.rs` は `live_view_url: None` を返し、`browser_live.rs` の `LiveSink` は status/tabs/url/console の永続イベント用で、実 sink は未配線である。既存の永続 event 型は frame と title を表現できない。

既存 web live proxy は owner session、Origin、run/session、grant を確認し、frame ではなく browser dashboard の許可 path を中継する。frame はその proxy と task-api の永続 event `/read` を通さない。Chrome の `Page.startScreencast` を launcher 内の CDP controller が扱い、daemon を経て同一の web owner session に結び付いた viewer へ一方向に転送する。

## 決定

### D1. frame 経路、認可、背圧

`Page.startScreencast` / `Page.screencastFrame` を扱うのは launcher が所有する CDP controller だけとする。launcher protocol v9 は session binding に結び付いた `live_frame` notification を追加する。daemon controller は notification を opaque frame として専用 live-frame channel に送り、gateway が既存 owner session grant を再確認した WebSocket へ中継する。ブラウザから得た CDP pipe を daemon に渡さず、frame を event、task API、harness/tool result、LLM input、agent-visible message に変換しない。認証失敗・owner disconnect・run/session 不一致・期限切れ・Origin 不一致では即時破棄し、fail closed とする。

各段は容量 1 の最新 frame slot とし、下流が詰まっていれば未送信 frame を置換して古いものを捨てる。非同期 queue、再送、disk spill は設けない。viewer 接続の開始・終了、frame の開始・停止は既存 Live View grant の lifecycle に従う。D2 の owner session 以外、別 viewer、agent、LLM には同じ frame 型・stream handle・購読経路を与えない。通常の browser event 型と frame 型を分け、汎用 event relay から frame channel へ到達できない境界を設ける。

理由: launcher は CDP の唯一の所有者であり、独自の短命 channel にすれば pipe や画像を永続 event / agent 経路へ混ぜずに済む。最新 frame のみを流すことで遅い viewer が画像を蓄積させず、操作画面の遅延も抑える。

試験: `browser_launcher_live_frame_` は CDP frame のみが v6 notification に符号化され、非 frame 応答・別 session の frame が拒否されることを確認する。`browser_live_frame_` は owner grant 成功時のみ stream が開き、他 owner・別 run/session・不正 Origin・期限切れ・未認証では frame が届かないこと、agent/tool/event 経路に frame 型が存在しないことを確認する。`browser_live_frame_backpressure_keeps_latest_only` は遅い sink に対して queue が増えず最新 frame だけが残ることを確認する。

## 永続層に残さない

### D2. frame の非永続化

frame bytes と CDP frame payload は DB、SQLite WAL、events、progress、journal/log、artifacts、core dump、crash report、一時 file、通常の browser result のいずれにも保存しない。frame を保持する型は Debug/Serialize/汎用 event 化を実装せず、診断値は固定 code、byte length、時刻、session id の非機密 metadata に限る。既存 scrub は文字列化された URL 等を扱う防御層であり、画像 payload を保存してよい根拠にはしない。永続化可能な `PersistedLiveEvent` と揮発 `LiveFrame` の型・channel を分離し、通常 serializer に frame を渡せない構成にする。

理由: scrub は既に記録された自由形式の値を後処理するものなので、画素中の秘密を発見・除去できない。型と経路で永続 sink への到達を防ぐ方が確実である。

試験: `browser_live_frame_no_persist_` は代表的な秘密 marker を含む frame を流し、DB/WAL/events/log capture/artifact/core-dump hook/tempdir の各検査対象に marker と frame bytes が無いことを確認する。`browser_live_frame_not_serializable` は frame 型に Serialize/Debug の汎用実装がなく、persistable event へ変換できないことを compile-time または型境界の検査で固定する。

## credential session と auth section

### D3. credential session の本人表示

人の決定（2026-10-10）により、credential session はログイン後を含め本人の Live View に表示する。これは D1 の本人向け揮発表示だけを許すもので、frame の永続化、agent/LLM への観測、他 viewer への共有を許可しない。launcher credential release の proof / isolation gate、短期 lease、credentiald の secret 経路は変えない。

auth section 中も本人の frame は表示できる。既存 H3 の agent event / action 観測停止は維持し、永続 event emitter の auth guard を解除しない。auth section 専用の Live View stream は grant、owner session、run/session binding を継続検査し、session stop または grant 失効時に破棄する。入力欄の値は画像に写る可能性があるため、本人表示に限ることを画面上で明示する。

理由: credential session でも本人がログイン状態を確認できるという人の決定を実現しつつ、H3 の agent 観測遮断と secret の非露出を別々の境界として維持できる。

試験: `browser_live_frame_auth_section_owner_only` は auth section 中に本人 viewer へ frame が届き、agent event emitter・LLM/tool result・別 viewer には届かないことを確認する。`launcher_credential_input_not_in_events_cdp_response_or_logs` は注入した一意な入力 marker が event、CDP response、daemon/launcher log に含まれず、status が固定 code のみであることを確認する。

## input 転送

### D4. launcher 経路は読み取り専用

この変更で Live View からの mouse/keyboard/input 転送を追加しない。launcher runtime の Live View は画像表示のみとし、takeover/control の input は launcher 経路で拒否する。ADR-0080 D6 の読み取り専用方針と ADR-0100 D2 の input gate を維持する。2026-10-05 の web Live View の操作契約が他 runtime に適用される範囲を、launcher へ暗黙に拡張しない。

理由: frame の購読権限は browser 操作権限ではない。別 UID launcher / credential session に対する操作は独立した認可・監査・confused-deputy 検討を要する。

site policy の欄は設けない（D7）。credential session の本人向け表示も含め、input 転送は本節の拒否を維持する。

試験: `browser_launcher_live_view_rejects_input` は launcher live channel 上の mouse/keyboard/control input を拒否し、CDP command と runtime state が変化しないことを確認する。`browser_live_view_existing_takeover_policy` は ADR-0080 D6 / ADR-0100 D2 の既存拒否・grant 条件を回帰固定する。

## protocol v6 と互換

### D5. launcher protocol の更新

`PROTOCOL_VERSION` は 9 とする。v9 は D1 の `live_frame` notification と、session-bound live stream の start/stop 制御を追加する。frame payload は有界長の binary body とし、JSON metadata は session id、sequence、dimensions、encoding など非機密 framing 情報だけを持つ。unknown verb/field、過大 frame、session binding 不一致、認可されない開始要求は拒否する。hello の版確認は session 起動・Live View 有効化前に行う。v8 は browser artifact transfer（[credential username / post-login read](2026-10-09-browser-credential-username-and-post-login-read.md) 付記 2026-10-10e、実装済み）に割り当てた。

v9 未満 launcher は既存 session を継続できるが、Live View frame は提供しない。daemon は protocol v9 専用の frame 操作を送らず、Live View を `launcher_protocol_no_live_frames` として無効化する。screenshot / download は v8 を要し、v8 未満では固定理由 `browser_launcher_protocol_artifacts_required` で action を失敗させる。credential login に必要な契約の確認は別途維持し、Live View 不在を理由に daemon runtime へ fallback しない。

理由: optional capability として版で識別すれば、既存 launcher の起動・認証契約を壊さず、daemon が未対応 launcher に誤って frame 要求を送るのを防げる。

試験: `browser_launcher_protocol_v6_frames` は v6 encode/decode、上限、未知 field/verb 拒否を確認する。`browser_launcher_v5_continues_without_live_view` は v5 hello の既存 session 継続と frame capability 無効化を確認する。`browser_launcher_daemon_checks_live_protocol_before_enable` は version check 前に start/subscribe を行わないことを確認する。

## 実装 task の分け方

### D6. 実装を分割する

1. **launcher** — 範囲 `crates/task-worker/src/browser_launcher/`, `crates/task-worker/src/browser_runtime.rs`。CDP screencast start/frame/ack/stop、v6 bounded framing、protocol tests `browser_launcher_live_frame_`。credential value と CDP response は frame notification に含めない。
2. **controller・daemon** — 範囲 `crates/task-worker/src/browser_launcher_run.rs`, `crates/task-worker/src/browser_live.rs`, `crates/task-worker/src/browser.rs`。version negotiation、session-bound opaque forwarding、容量 1 の latest-only handoff、run/session cleanup、persistable event との型分離。tests `browser_live_frame_` と `launcher_credential_input_not_in_events_cdp_response_or_logs`。
3. **gateway・web** — 範囲 `web/server/browser-live.js`, `web/server/relay.js`, `web/server/` の browser live 関連試験、および browser run viewer UI の関連ファイル。既存 owner session/Origin/grant guard の再利用、専用 no-store WebSocket、credential session（認証 section を含む）の本人向け既定表示、非 owner 拒否、input 不許可。site policy の欄は作らない。tests `browser_live_frame_` / `browser_live_view_`（既定表示は `browser_live_credential_default_owner_only`）。
4. **試験・統合** — 範囲 `crates/task-worker/` と `web/server/` の該当試験、必要な browser runtime 統合試験。各層の永続 sink 走査、v5/v6 組合せ、auth section owner-only、backpressure、input refusal を追加し、prefix `browser_launcher_live_frame_`, `browser_live_frame_`, `browser_live_view_`, `launcher_credential_` を用いる。実 browser が必要な検証は opt-in とし、CPU 負荷試験を使わない。

理由: launcher は CDP を唯一保持し、controller は型と背圧、gateway は人の認証、試験は層間の非永続性をそれぞれ所有する。範囲を切れば各境界を独立に review できる。

試験: `browser_live_frame_task_scope_checks` は各 task の変更 path が上記範囲内であることを確認し、各層の指定 prefix の試験を実行する。

## 決定（人の決定、2026-10-10）

### D7. `live-credential-default`（決定済み）

- 決定: credential session（ログイン後を含む）の Live View は、既定で本人の owner session に表示する。site policy ごとの opt-in は設けない。
- 回答者: 人（運用セッション経由、2026-10-10 の発言「資格情報を使った場合でも live view が見れるほうがいい、どちらにせよ資格情報を入力した人間しか見ないので」）
- 日付: 2026-10-10
- task: 01M4HS8VQDB3JQHJRJ0ZDBY57H（決定 record-decision、進捗 [2026-10-10 launcher Live View frame](../progress/2026-10-10-browser-launcher-live-view-frames.md)）
- 注記: 選択肢は「既定で出す」を選ばれた。本 ADR の初稿の推奨は site policy opt-in で、人の選択はそれと異なる。決定の選択肢の表記に「推奨どおり」と付いていたが、初稿の推奨とは一致しない。この食い違いは進捗に残す。
- 維持する境界: frame を DB/WAL/events/logs/artifacts に残さない（D2）。本人以外（agent・LLM・他の viewer）へ渡さない（D1・D3）。認証区間中の入力欄の値（password は Chrome が伏字にする）を frame 以外の経路で出さない（D3）。input 転送（takeover）は緩めない（D4）。

理由: 資格情報を入力する本人だけが見るため、本人への表示を既定にしても露出は増えない。既定表示なら本人が login の状態を確認でき、opt-in の設定漏れによる表示不能を避けられる。

試験: `browser_live_credential_default_owner_only` は未設定時（既定）に credential session の本人 owner viewer へ frame が届き、非 owner・agent・LLM・別 viewer へは届かないことを確認する。site policy の試験は置かない。

## 付記 2026-10-10b（実装: protocol v8）

この付記は D1〜D6 の実装契約を確定する。本文中の protocol v6/v5、v6 notification、v5/v6 組合せおよびそれらを名指す試験は、Live View frame に関する記述ではそれぞれ v8/v7、v8 notification、v7/v8 組合せおよび v8/v7 を名指す試験として読み替える。本番 launcher は既に protocol v7（post_login が v6、consent が v7）である。既存の session、credential login、consent の意味と動作は、Live View の版不一致によって変えない。

### v8 wire contract

v8 は launcher から daemon への専用 frame 接続で `live_start` / `live_stop` verb と `live_frame` notification を追加する。各 JSON metadata は session binding、連番、寸法、encoding、body byte length など framing に必要な非機密値だけを持つ。`live_frame` の画像本体は metadata に埋め込まず、宣言長を前置した bounded binary body とする。1 frame の body 上限は 2 MiB（2,097,152 bytes）。上限超過、宣言長と実長の不一致、未知 verb/field、誤った session binding は拒否する。credential value、CDP response、URL は frame metadata/body の外側の制御情報に混ぜない。

frame は daemon が `live_start` を送った Live View 専用の別接続でのみ流す。通常の launcher control 接続には frame を流さない。v7 daemon は frame 接続および `live_start` を実装しないため、v8 launcher に対しても frame を要求・受信しない。

### protocol 互換

| daemon | launcher | Live View | その他 |
|---|---|---|---|
| v8 | v7 | hello の protocol version で frame capability を無効化し、reason `launcher_protocol_no_live_frames` を返す。`live_start` を送らない | session・credential login・consent は従来どおり |
| v7 | v8 | frame を要求しないため従来どおり Live View frame なし | session・credential login・consent は従来どおり |

版確認は Live View の開始前に行う。Live View 非対応を理由に session を失敗させたり、別 runtime へ fallback したりしない。

### daemon、API、gateway の契約

- `task-core` に揮発専用 `LiveFrame` を置く。この型は `Serialize` / `Debug` を実装せず、persistable event へ変換できない。`LatestFrameSlot` は容量 1 で最新 frame のみを保持する。`LiveSessionEntry` は frame 購読口を持ち、既定値は `None`。
- `task-api` は grant を再確認する専用 frame stream route を設ける。既存 relay assertion と grant を接続ごとおよび frame ごとに再確認し、`Cache-Control: no-store` を付け、長さ前置 binary frame を流す。grant 失効または owner 切断で stream を閉じる。run 一覧には frame 経路の可否を返す。
- web gateway は既存 owner session / Origin / grant guard を通した WebSocket で、認可済み本人だけに frame を送る。frame 経路が利用可能なら `liveAvailability` は link を返し `enabled` とする。credential session と auth section にも D3/D7 の本人向け既定表示を適用する。input 転送は引き続き拒否する。

### 実装 unit の範囲と試験契約（D6 更新）

| unit | path 範囲 | 試験 prefix / 固定名 |
|---|---|---|
| frame-core | `crates/task-core/src/` の LiveFrame・LatestFrameSlot・LiveSessionEntry 関連実装と試験 | `browser_live_frame_`、`browser_live_frame_not_serializable`、`browser_live_frame_backpressure_keeps_latest_only` |
| launcher | `crates/task-worker/src/browser_launcher/`, `crates/task-worker/src/browser_runtime.rs` | `browser_launcher_live_frame_`、`browser_launcher_protocol_v8_frames`、`browser_launcher_v7_continues_without_live_view` |
| api-stream | `crates/task-api/src/` の frame stream route・run view 実装と関連試験 | `browser_live_frame_`、`browser_live_frame_no_persist_` |
| daemon | `crates/task-worker/src/browser_launcher_run.rs`, `crates/task-worker/src/browser_live.rs`, `crates/task-worker/src/browser.rs` | `browser_live_frame_`、`browser_launcher_daemon_checks_live_protocol_before_enable`、`launcher_credential_input_not_in_events_cdp_response_or_logs` |
| gateway | `web/server/browser-live.js`, `web/server/relay.js`, browser live 関連 gateway 試験 | `browser_live_frame_`、`browser_live_view_` |
| spa | browser run viewer の関連 `web/` UI と UI 試験 | `browser_live_view_`、`browser_live_credential_default_owner_only` |
| cross-tests | `crates/task-worker/`, `crates/task-api/`, `crates/task-core/`, `web/server/` の該当試験 | `browser_live_frame_no_persist_`、`browser_live_frame_auth_section_owner_only`、`browser_live_view_`、`launcher_credential_` |
| ops-doc | `docs/ops/` の launcher v8 再 build・差し替え・Live View 確認手順 | 文書 check。コード試験 prefix は追加しない |

各 unit は表の範囲だけを変更する。層をまたぐ追加試験は cross-tests に置き、各段の容量 1、非永続性、v7/v8 互換、本人限定、input 拒否を固定する。D6 の従来の v6/v5 試験名・組合せはこの表の v8/v7 契約に置き換える。

### 実装記録

protocol v8 の実装は完了し、全体検査を通過した。主要な実装 commit は launcher `07aceb77`、task-core/daemon `516ea993`、task-api `13bc444d`、gateway `702c40ad`、SPA `d17ff1bf`、cross-tests `047a3e40`、運用手順 `82af5b94`。close-out HEAD は `127e53e3d0a02576d160465fc566b64ed55323ea`。全体検査と残る実 browser/本番確認は [進捗](../progress/2026-10-10-browser-launcher-live-view-v8.md) を参照。
