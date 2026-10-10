# ADR 2026-10-10: browser launcher の本人向け Live View frame 経路

---
tasks: [01M4HS8VQDB3JQHJRJ0ZDBY57H]
---

- 日付: 2026-10-10
- 状態: 採用（frame の揮発配送・分離契約）。既定公開範囲は `live-credential-default` の人の決定待ち
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md) D3・D6、[ADR-0100](0100-browser-phase3-live-proxy-acl.md) D2、[ADR-0115](0115-browser-ptrace-owner-ns-launcher.md)、[ADR-0116](0116-browser-launcher-implementation.md)、[2026-10-05 browser department / web Live View](2026-10-05-browser-department-web-live-view.md)、[2026-10-09 credential username / post-login read](2026-10-09-browser-credential-username-and-post-login-read.md)、[2026-10-09 launcher CredentialUse release](2026-10-09-browser-launcher-credential-release.md)

## 文脈

launcher runtime の Chrome は uid 995 の sandbox 内で `--remote-debugging-pipe` を使い、CDP pipe と controller は launcher process が所有する（`crates/task-worker/src/browser_runtime.rs`）。daemon worker は pipe FD を持たない。現在 `browser_launcher_run.rs` と `browser.rs` は `live_view_url: None` を返し、`browser_live.rs` の `LiveSink` は status/tabs/url/console の永続イベント用で、実 sink は未配線である。既存の永続 event 型は frame と title を表現できない。

既存 web live proxy は owner session、Origin、run/session、grant を確認し、frame ではなく browser dashboard の許可 path を中継する。frame はその proxy と task-api の永続 event `/read` を通さない。Chrome の `Page.startScreencast` を launcher 内の CDP controller が扱い、daemon を経て同一の web owner session に結び付いた viewer へ一方向に転送する。

## 決定

### D1. frame 経路、認可、背圧

`Page.startScreencast` / `Page.screencastFrame` を扱うのは launcher が所有する CDP controller だけとする。launcher protocol v6 は session binding に結び付いた `live_frame` notification を追加する。daemon controller は notification を opaque frame として専用 live-frame channel に送り、gateway が既存 owner session grant を再確認した WebSocket へ中継する。ブラウザから得た CDP pipe を daemon に渡さず、frame を event、task API、harness/tool result、LLM input、agent-visible message に変換しない。認証失敗・owner disconnect・run/session 不一致・期限切れ・Origin 不一致では即時破棄し、fail closed とする。

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

試験: `browser_launcher_live_view_rejects_input` は launcher live channel 上の mouse/keyboard/control input を拒否し、CDP command と runtime state が変化しないことを確認する。`browser_live_view_existing_takeover_policy` は ADR-0080 D6 / ADR-0100 D2 の既存拒否・grant 条件を回帰固定する。

## protocol v6 と互換

### D5. launcher protocol の更新

`PROTOCOL_VERSION` を 5 から 6 にする。v6 は D1 の `live_frame` notification と、session-bound live stream の start/stop 制御を追加する。frame payload は有界長の binary body とし、JSON metadata は session id、sequence、dimensions、encoding など非機密 framing 情報だけを持つ。unknown verb/field、過大 frame、session binding 不一致、認可されない開始要求は拒否する。hello の版確認は session 起動・Live View 有効化前に行う。

v5 launcher は既存の browser session と credential 動作を従来どおり継続できるが、Live View frame は提供しない。daemon は v5 を見た場合 Live View を `launcher_protocol_no_live_frames` として無効化し、v6 専用 verb を送らない。credential login に必要な v5 契約の確認は別途維持し、Live View 不在を理由に daemon runtime へ fallback しない。

理由: optional capability として版で識別すれば、既存 launcher の起動・認証契約を壊さず、daemon が未対応 launcher に誤って frame 要求を送るのを防げる。

試験: `browser_launcher_protocol_v6_frames` は v6 encode/decode、上限、未知 field/verb 拒否を確認する。`browser_launcher_v5_continues_without_live_view` は v5 hello の既存 session 継続と frame capability 無効化を確認する。`browser_launcher_daemon_checks_live_protocol_before_enable` は version check 前に start/subscribe を行わないことを確認する。

## 実装 task の分け方

### D6. 実装を分割する

1. **launcher** — 範囲 `crates/task-worker/src/browser_launcher/`, `crates/task-worker/src/browser_runtime.rs`。CDP screencast start/frame/ack/stop、v6 bounded framing、protocol tests `browser_launcher_live_frame_`。credential value と CDP response は frame notification に含めない。
2. **controller・daemon** — 範囲 `crates/task-worker/src/browser_launcher_run.rs`, `crates/task-worker/src/browser_live.rs`, `crates/task-worker/src/browser.rs`。version negotiation、session-bound opaque forwarding、容量 1 の latest-only handoff、run/session cleanup、persistable event との型分離。tests `browser_live_frame_` と `launcher_credential_input_not_in_events_cdp_response_or_logs`。
3. **gateway・web** — 範囲 `web/server/browser-live.js`, `web/server/relay.js`, `web/server/` の browser live 関連試験、および browser run viewer UI の関連ファイル。既存 owner session/Origin/grant guard の再利用、専用 no-store WebSocket、認証 section の本人向け表示、input 不許可。tests `browser_live_frame_` / `browser_live_view_`。
4. **試験・統合** — 範囲 `crates/task-worker/` と `web/server/` の該当試験、必要な browser runtime 統合試験。各層の永続 sink 走査、v5/v6 組合せ、auth section owner-only、backpressure、input refusal を追加し、prefix `browser_launcher_live_frame_`, `browser_live_frame_`, `browser_live_view_`, `launcher_credential_` を用いる。実 browser が必要な検証は opt-in とし、CPU 負荷試験を使わない。

理由: launcher は CDP を唯一保持し、controller は型と背圧、gateway は人の認証、試験は層間の非永続性をそれぞれ所有する。範囲を切れば各境界を独立に review できる。

試験: `browser_live_frame_task_scope_checks` は各 task の変更 path が上記範囲内であることを確認し、各層の指定 prefix の試験を実行する。

## 未決（人の決定待ち）

### D7. `live-credential-default`

credential session の Live View を既定で出すか、site policy の明示的 opt-in にするかは未決である。推奨は **site policy opt-in**。auth section 中は入力値が画面に表示され得て、本人限定でも肩越し閲覧・共有画面などの露出は起きる。既定を opt-in にすれば、site ごとに本人表示の必要性を選び、既存の最小公開を維持できる。opt-in の site でも D1 の owner session 以外には配信しない。

理由: auth section の frame は入力値を含む可能性があり、site ごとに本人表示を許す必要性を選べる方が露出を抑えられる。

試験: `browser_live_credential_default_policy` は未設定時の選択結果と opt-in site の本人限定動作を確認する（決定後に規則を確定する）。
