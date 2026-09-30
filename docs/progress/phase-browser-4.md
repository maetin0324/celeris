# Phase browser-4: isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG, 01M3QGRC542ZC23996DNCTHZF5, 01M3QGRC6AQ81PWM1XP4C7BH45]
---

- 状態: **P4-A/B の受け入れは未完、P4-C の公開能力は scripted LLM による実 backend 適合済み**。runtime 方式・H7 は人の回答を採用済み（ADR-0085）。旧 broker IPC の秘密返却を廃止。機密能力は実 runtime・CDP sink・injection-only IPC 適合まで拒否。本番 H3 経路（controller 所有 Chromium/CDP 共有・trusted selector・`browser.rs` 結線・実 e2e 注入検証）は ADR-0091 の unit 群（selector/shared-cdp/h3-wire/e2e）で実装済みだが、本番 broker の admission は `Attested` のみで、この host は同一 UID（`SameUid`）のため機密起動（実注入）は本番経路で拒否のまま。本番未昇格。
- 更新: 2026-09-30（run 01M3S43TR5TQJ4PQ40VVTR6VY0、WorkUnit closeout: ADR-0091 の後続 unit（selector・shared-cdp・h3-wire・e2e）を workspace に統合済みであることを確認し、workspace 検査を実行。`cargo test --workspace` exit 0（3041 passed / 0 failed / 11 ignored）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all --check` は本 unit 外の既存フォーマット崩れ 2 箇所を `cargo fmt --all` で解消し再検査 exit 0）
- ADR-0091: [H3 shared CDP and trusted selector](../adr/0091-browser-p4b-h3-shared-cdp-trusted-selector.md)（controller 所有 1 Chromium/CDP の共有・認証区間中の全面遮断、管理者 site policy 由来の trusted selector の固定・照合、`browser.rs` H3 の D3 照合順、e2e の 6 面 sentinel 非露出検証と負の対照）
- ADR-0087: [same-uid bwrap runtime](../adr/0087-browser-p4a-same-uid-bwrap-runtime.md)、[conformance dispatch](../adr/0087-browser-phase4-conformance-dispatch.md)（P4-A と P4-C が同番号で別 file を追加。ADR-0088 も同様）
- ADR: [ADR-0084](../adr/0084-browser-phase4-isolation-injection-routing.md) D6、[ADR-0085](../adr/0085-browser-phase4-runtime-selection.md)

## 行ごとの判定

| 行 | 判定 | 証拠・限界 |
|---|---|---|
| P4-A isolated runtime | 一部達成（決定 p4a-uid: 同一 host UID）。実 bwrap runtime・事実採取・分離・daemon 起動時 orphan 回収・production worker の egress 接続は実装済み。復元結合は別 WorkUnit が担当 | `cargo test -p task-worker --test browser_runtime_isolated` → 4 passed / 1 ignored（helper）。実 chrome-headless-shell（agent-browser の browser、playwright 1243）を bwrap で起動し CDP pipe で `Browser.getVersion` 応答、host 側 `/proc` で 6 namespace 別・root ro・書ける mount は `/session` だけ・NoNewPrivs=1・CapEff/CapPrm=0・uid_map `1000 1001 1`・netns TCP LISTEN 0 件。同 spec の probe で broker/control socket・`/run/user`・host tmp 不可視、`/usr`・`/etc` 書込不可、host loopback fixture・10.0.0.1・::1・192.0.2.53:53 へ接続不可。controller SIGKILL 後に bwrap・sandbox 内 process が消える（実 process）。記録からの再起動回収は starttime 一致だけを殺す。`verify_isolation` は弱めず、違反は `SameUid` だけ → attestation 無し → 復元拒否。D3 の実 daemon 起動試験 1 passed、D4 の run_with_executable 実 chrome/egress/fixture 試験 1 passed（下記）。 |
| P3-A identity 復元（隔離下のみ） | 結合済み・この host では拒否（決定 p4a-uid: 同一 host UID → `SameUid`）。成功経路は別 UID の実 runtime が無いので実証できていない | ADR-0088 D5。`POST /api/v1/browser/identities/{id}/restore` に `session_id` を追加し、supervisor が登録する registry 経由で稼働中 session に結合。`cargo test -p task-api --test browser_restore_live_session` → 1 passed（実 bwrap + 実 chrome-headless-shell の session に対して拒否 7 経路・`open_attempts()==0`）。`cargo test -p task-api --lib restore_` → 4 passed。`--restore` / `--state` / `--profile` は利用しない。 |
| P4-B stronger injection | 本番 H3 経路まで実装済み（2026-09-30、ADR-0091）。broker IPC・controller CDP sink・trusted selector・`browser.rs` の H3 結線・実 e2e の 6 面 sentinel 非露出検証を実 bwrap + 実 browser + loopback fixture で確認。本番 admission（`Attested`）では同一 UID のため機密起動は拒否のまま。 | 【unit ipc】ADR-0089 D1〜D3 の broker injection-only IPC 証拠・制約は上記のとおり。【unit wire-harness】commit `35ab5564`（cherry-pick `71640d79`）で `tests/browser_injection_wire.rs` を追加し、実 broker の `injection.sock` と実隔離 browser の CDP sink を結線。`cargo test -p task-worker --test browser_injection_wire` → **2 passed / 0 failed / 0 ignored**（`real_broker_browser_injection_receipt_and_origin_guards` は skip 無し、receipt-only 注入・origin ガードを確認）。`cargo test -p celeris-credentiald` → injection_ipc 12、broker 12、lib 14 passed。`cargo clippy --workspace -- -D warnings` → exit 0。【unit selector】ADR-0091 D2: `task_core::browser_wait::{validate_trusted_selector, validate_trusted_login_url, validate_trusted_login}` と `ConsumedBrowserApproval.trusted_login`、broker の `selector_mismatch`。`cargo test -p task-core browser_wait::` に `trusted_login_url_must_stay_on_the_exact_origin`・`trusted_login_is_pinned_only_on_valid_credential_use_approvals` を含む。【unit shared-cdp】ADR-0091 D1: `task_worker::browser_shared_cdp`（relay の token/Origin 検査・CDP 多重化・認証区間中の全面遮断・postData 削除）。`cargo test -p task-worker --test browser_shared_cdp` → **2 passed**（`real_shared_cdp_and_auth_section`・`inner_shared_cdp`、実 bwrap + 実 chrome-headless-shell、skip 無し）。【unit h3-wire】ADR-0091 D3: `browser.rs` の H3 開始/終了を `RegisterLiveSession → DescribePolicy 照合 → 承認消費 → lease grant → relay 遮断/OpenAuthSection → inject（selector_mismatch 照合は lease 消費より前）→ submit → close_auth_section/CloseAuthSection → relay 解除` の順に結線し、旧 `use_credential`（`auth login` + 旧 bridge）を H3 経路から除いた。`cargo test -p task-worker --test browser_h3_wire` → **2 passed**（`real_broker_browser_injection_receipt_and_origin_guards`・`inner_injection_wire`、実 broker + 実 CDP pipe、skip 無し）。【unit e2e】ADR-0091 D4: `crates/task-api/tests/browser_h3_injection.rs` の `production_h3_injects_once_without_exposure`（実 daemon 経路・実 bwrap・実 chrome-headless-shell・実 broker process・実 CDP pipe・relay、loopback DNS/HTTPS fixture、scripted LLM で 1 回注入し、fixture の受理と event 列・live event・LLM 入力・SQLite DB・WAL・run log・run/artifact ファイルの 6 面を sentinel（password・SENTINEL_USER の raw/base64/percent/UTF-16LE/JSON escape 表現）で 0 件検索）と負の対照 `injected_leak_is_caught_by_the_same_scanner`（検索器自体が植え込んだ sentinel を検出することを確認）→ 両方 **passed**。`cargo test -p task-api --test browser_h3_injection` は `production_h3_injects_once_without_exposure` が内部で `unshare --user --map-root-user --net` の子 process を起こし `inner_h3_injection` を再実行するため sandbox 内では動かず、sandbox 外の workspace 検査で通す。【unit attacks】`tests/browser_injection_attacks.rs` で実 chrome・実 broker・loopback fixture 上の A1〜A17 を実行。`cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → **2 passed / 0 failed**（skip 無し、全 `ATTACK-Ax-OK` 出力）。A8（page script が値を可視 div に写す）は **未達の所見**: 区間後に agent の `Runtime.evaluate`/`DOM.getDocument` で sentinel が読める（`RedisplayGuard` が `CdpController::agent_command` に未配線）。詳細は下の「P4-B 実攻撃試験」節。ADR-0091 D4 の N1/N2（`same-uid-harness` feature 下での区間遮断無効化・値消去無効化を使った検出器の実地変異試験）はこの e2e 単体では未実装で、次段の攻撃試験課題として残る。機密能力は本番では未解放・本番未昇格。 |
| H3 観測停止の維持 | 実装維持かつ本番 H3 経路まで結線済み。本番の機密起動は同一 UID のため拒否 | `cargo test -p task-worker browser --lib` → 31 passed。`browser_auth_section_forward_events_drops_progress_artifact_and_live` と LiveEmitter の抑止試験を含む。API 結合テストの store auth_section / takeover 拒否も成功。ADR-0091 の e2e `production_h3_injects_once_without_exposure`（上記）が実 daemon 経路での end-to-end 検証（注入成功＋6 面 sentinel 非露出）を通す。本番 broker admission は `Attested` のみで、この host は `SameUid` のため本番の機密起動（実注入）は拒否のまま。e2e の正例は `same-uid-harness` feature 限定の試験 admission に依る。 |
| P4-C backend routing | 公開能力を実 backend protocol で適合。機密要求は拒否 | `--protocol-scripted --fallback-scenario` runner が実 agent-browser 0.38.1 + loopback fixture で ACP RPC・Claude CLI・specialist wrapper を各7/7 実行。runner の ledger を worker routing と実 browser fallback に渡して成功。主 ACP harness を SIGKILL し、別 session の Claude が click/download を完了。無候補と `CredentialUse` は明示拒否。`CredentialInjection`・`IdentityRestore` は P4-A/B の実適合まで拒否。実 LLM 比較は未。 |

## attempt 3 の挙動と検査

- 公開操作だけの policy は ACP / 明示 Claude の既存 loop で実行する。`CredentialUse` を含む effective policy は、承認状態を問わず `browser backend lacks required conformance` で拒否する。task が公開操作を行うには管理者がその policy を明示する。要求能力を黙って落とさない。
- API 結合テストは Phase 2 から残った durable wait を store に作り、実API・brokerによる手動登録と一回承認後も worker が起動しないことを確認。承認は未消費、broker の grant/use は0件、run directoryも作らない。DB/WAL・vault・event・worker出力の sentinel 非露出も確認する。
- 旧 trusted-local の認証成功・redirect 中の注入・認証区間の注入成功を期待するテストは、現在拒否する挙動に合わせて更新した。新しい拒否テストを P4-B の TOCTOU/redirect/iframe/DOM 実攻撃試験や隔離実行の証拠として数えない。H3 の production `auth_section` と `forward_events` の配線は変更していない。
- `cargo test -p task-worker browser --lib` → exit 0、31 passed。
- `cargo test -p task-api --test browser_e2e` → exit 0、4 passed。
- `cargo test --workspace` → exit 0（2957 passed、0 failed、既存のignored 7件）。`cargo clippy --workspace -- -D warnings` → exit 0。workspace 検査に isolation 14件、backend 7件、injection 6件、auth_section 名の7件を含む。
- 初回の sandbox 内実行は sccache 接続が EPERM。Celeris の環境変数は変更せず、sandbox 外で同じ検査を実行する。結合テストの `owner_id` 型エラーを修正し再実行済み。

## 人の回答反映と旧 IPC 拒否（run 01M3QCTV524JJ41X9MSFS0756V）

- runtime は bubblewrap + subuid/subgid、host trusted controller/proxy の分離を採用。H7 は固定 agent-browser 0.38.1 + 既存 harness の browser-specialist を採用。方式・候補の回答待ちは解消し、再決定要求は出していない。
- 旧 `resolve.sock` は `trusted_injection_required` を返す。control 許可 PID、有効 binding/lease、JSON の `role: injector` 自己申告、同じ UID の別 worker process からも秘密を返さない。grant 2件に対し use 0件で lease 未消費を確認する。旧 bridge は socket に接続せず固定失敗を返す。
- API の実手動登録・承認・grant 後の旧秘密取得成功テストも拒否へ更新した。`CredentialProvider` と broker 内部の単回 lease の契約・試験、H3/auth_section 実装は変更していない。これらの拒否は実 trusted injection 成功や実CDP攻撃試験を意味しない。
- 初回の限定テストは sandbox 内 sccache が EPERM。環境を変更せず sandbox 外で実行。control 拒否前に request を読まない変更では Unix socket の未読データによる切断が `Io` となったため、既存の bounded read を維持して固定応答を返すよう修正した。
- 最終検査: `cargo test --workspace` → exit 0、2957 passed / 0 failed / 既存 ignored 7件。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all --check` → exit 0。
- workspace 実行に `retired_plugin_bridge_never_connects_or_echoes_input`、`daemon_rejects_worker_secret_retrieval_even_with_valid_lease`、`invalid_control_process_id_is_rejected`、API の `real_broker_registration_keeps_sentinel_out_of_db_events_artifacts_and_worker_output` を含み、すべて成功。P4-A の実 runtime と P4-B の実 CDP 攻撃、P4-C の実 backend 適合は未実施なので、workspace 成功をそれらの達成証拠とはしない。

## 未解決

- P4-A: egress relay と production 起動経路は D1〜D4 で接続し、ローカル fixture 正例を検証済み。実 agent-browser 0.38.1 本体は host に無いため、その browser（chrome-headless-shell）を起動 script から使って検証した。
- P4-A 別 host UID: 同一 UID のため host 側の同 UID process からの ptrace・/proc 参照は防げない。機密解放は拒否のまま。手順書 [browser-isolated-runtime-subuid](../ops/browser-isolated-runtime-subuid.md)。
- 注意: `PR_SET_PDEATHSIG` は起動した thread の終了でも発火する。tokio の blocking thread から起動すると thread 終了で runtime が殺される。production 配線では専用の長寿命 thread から起動すること。

- P4-A: 同一 host UID の残存リスク、D4 action の channel message と実装の差、実 agent-browser 0.38.1 本体での検証、稼働中隔離 session への identity 復元の成功経路（D5 で結合したが、同一 UID のため `SameUid` で拒否され、開封から controller への受け渡しまでを実 runtime で確かめられていない）。subuid mapping はこの run の親 user namespace の範囲外で EPERM。
- P4-B: 実 CDP sink（受け取り・receipt-only・origin/iframe 拒否）は取り込み済み（run 01M3RPPYTPT43N8ZHXFDDWESX0）。残るのは injection-only IPC の SO_PEERCRED role/session/lease 照合と controller への実結線（並行 WorkUnit ipc・後続 wire）、ローカル fixture での TOCTOU・redirect・DOM 再表示を含む実攻撃負例（WorkUnit attacks）、H3 の端から端の実注入検証（WorkUnit h3e2e）。旧 endpoint は再開しない。
- P4-C: 実 LLM の同一 task 比較は ACP CLI/認証を利用できる環境で行う。scripted LLM による実 backend protocol 適合・実 agent-browser での能力を失わない worker fallback・無候補拒否は実施済み。機密機能は P4-A/B の実適合まで拒否する。
- 3件の継続小タスクを run の `delegate.json` に提案した。P4-B は P4-A に依存し、公開能力の P4-C は独立。採用・実行・完了はこの run では確認できていない。提案を実装済みとして数えず、親の受け入れ条件0/1/2は未達のままとする。
- 内部 origin の追加、本番昇格、リモート実行は行っていない。


## 実 egress transport（run 01M3QDM7H5RYRZF2RNCARHQWX6）

- ADR-0086 を先に追加し、`task-worker::browser_egress` と独立実行 `celeris-browser-egress` を実装。既存の機密 routing 拒否・旧 resolve IPC 拒否・CredentialProvider・H3/auth_section は変更していない。
- proxy は controller から継承する接続済み AF_UNIX stream（FD 3）と stdin の JSON policy だけを受ける。host の listener は開かない。policy 入力 64 KiB / 5秒、HTTP header 8 KiB、DNS応答 16 KiB、setup 10秒、tunnel 300秒 / 各方向64 MiBで制限。stderr は固定拒否コード、stdout は空。proxy は no_new_privs / dumpable=0、親死亡 SIGKILL を設定する。
- CONNECT authority を DNS 前に拒否できるようにした。指定 resolver の TCP/53 へ直接照会し、A/AAAA の両方・ID/question・CNAME/owner・圧縮pointer・応答サイズを検査。全候補に public 判定を行い、検査済みIPへ直接connectする。proxy環境変数・OS再解決・上位proxyは利用しない。IPv6無効時に公開AAAAを含む回答も拒否するため、dual-stackサイトへの互換性は限定的。この初期の拒否境界は緩めていない。
- IANA registry を確認し、IPv4 の廃止済み6to4 relay、IPv6のdiscard-only・benchmark・documentation・SRv6等を拒否へ追加。IPv6有効時も通常のglobal unicast以外とspecial-purpose範囲を保守的に拒否する。出典と方針はADR-0086。
- 実Unix/TCP fixture 9件: DNS rebinding・private/v6・proxy auth/転送/本文header・DNS/DoT port・IP literal・非許可host・不正DNS/循環CNAME・TLS初期byte/half-close。実子プロセス6件: inherited socket必須・worker指定のprivate/proxy拒否・不正/過大policy・policy入力期限・peer切断終了・親死亡時SIGKILLとsubreaperのwaitpid回収。すべて外部ネットワークを使わない。
- `cargo test -p task-worker browser_egress --lib` → exit 0、9 passed。`cargo test -p task-worker --test browser_egress_process` → exit 0、6 passed。
- 初回検査でDNS未知recordの扱い、clippyのtest module配置と不要borrow、ハイフン付きbinary名のcompile-time env参照を修正。最後のものは既存credentiald試験と同じruntimeの環境参照へ変更した。親死亡試験はNoNewPrivsが継承されるケースを考慮し、dumpable=0による/proc/fd読取り拒否も待ってから親を終了する。reap後のPIDへsignalを送らない。
- sandbox外の `bwrap --unshare-user --unshare-net --ro-bind / / true` は exit 0。`unshare --user --map-auto --map-user=1 --map-group=1 id` は exit 1（newuidmap EPERM）。親uid_mapは `0:100000:1001, 1001:1001:1, 1002:101002:64534`、subuid/subgidの rmaeda 割当ては `165536:65536` で親の範囲外。管理者設定は変更していない。同UIDのbwrap成功を別UIDの実証として扱わない。
- P4-A全体、P3-A復元、P4-B実CDP/IPC、P4-C実fixture/specialistは未達。production workerは新proxyをまだ起動しない。既存browserのネットワークがこのproxyで制限されるとは主張しない。namespace接続・trusted controllerの運用配線と実適合が必要。

- このrunの最終検査: `cargo test --workspace` → exit 0（2972 passed / 0 failed / 既存 ignored 7件）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。`cargo fmt --all --check` / `git diff --check` → exit 0。機密機能の実適合・production接続の証拠ではない。

## P4-A 実 runtime（run 01M3QGRCAHDK1AB4R9WBHJ9XHZ, task 01M3QGRC542ZC23996DNCTHZF5）

- 前回 branch `celeris/01M3Q49ZTST3XQ9DGF6AGNR0XG`（331c6dd）を fast-forward で採用。ADR-0087 を追加してから実装。
- `task_worker::browser_runtime`: bwrap argv（user/pid/net/ipc/uts/cgroup unshare、`--die-with-parent`、`--new-session`、`--cap-drop ALL`、`--clearenv`、tmpfs root + `/usr`・`/etc` の必要 file・browser dir の ro bind、`/session` だけ rw、`--remount-ro /`）、`--info-fd` による child pid、CDP pipe（fd 3/4）、`/proc` からの `RuntimeFacts` 採取、`attest()`、`LiveSession`、`record`/`reap_recorded`。
- `task_core::browser_isolation::LiveIsolation` と `BrowserIdentityService::restore_for_session`: 呼ぶたびに稼働中 session から attestation を採り直し、session 停止・違反・session id 不一致は `isolation_required`、別 identity は `identity_not_found`/`other_project`、期限切れは拒否。`cargo test -p task-api restore_for_session` → 1 passed（attestation は fake facts 由来。実 runtime では `SameUid` で出ないことを task-worker の実試験で確認）。
- `/etc` を丸ごと bind した初回は `verify_isolation` が `BrokerVisible{/etc}`（`/etc/celeris` の親）で拒否した。検査を緩めず bind を file 単位に絞って解消。
- 最終検査: `cargo test --workspace` → exit 0（2977 passed / 0 failed / ignored 8）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0（workspace の `--all-targets` は既存 `task-api/tests/browser_e2e.rs` の type_complexity で失敗、本変更外）。
- 本番昇格・本番設定変更・内部 origin 追加はしていない。

## P4-A D3/D4 production 配線（run 01M3RCSFK5ZTC4JV0YJF17DV7C）

- D3: `celeris::run` の `Started::Running` 直後、`set_orphan_takeover` と同じ起動経路で `start_instance` を呼び、`browser-runtime/<instance_id>/` の記録を starttime と照合して回収する。`Duplicate` と `--mode verify` は回収しない。`cargo test -p celeris --test browser_startup_reap -- --nocapture` → 1 passed。実 daemon 起動関数を経て死んだ instance の記録を消し、生きた instance の runtime を維持した。
- D4: `browser::run` / `run_with_executable` は resolver・bwrap・sandboxd・egress が揃わなければ起動前に `isolated_runtime_unavailable`。host 直接起動への fallback はない。`cargo test -p task-worker --lib browser::tests::missing_egress_resolver_refuses_before_browser_start -- --nocapture` → 1 passed（run dir を作る前に固定コード拒否）。shim の `action.sock` は sandbox の外に置き、worker 側で policy を再検査する。現在の sandboxd への action 配送は `/session/actions` のファイルキューであり、ADR-0088 D4 に書かれた channel の別 message 種別とは実装が異なる。この差は解消が必要。
- 実結合: `cargo test -p task-worker --lib browser::tests::production_action_path_reaches_fixture_through_real_browser_and_egress -- --nocapture` → 1 passed。`unshare --user --map-root-user --net` の試験用 netns 内で公開扱いの 93.184.216.34 を loopback に設定し、ローカル DNS/HTTPS fixture を動かした。`run_with_executable` から shim → action.sock → sandboxd → 実 chrome-headless-shell → sandboxd relay → 実 celeris-browser-egress → fixture の本文を取得し、禁止 flag は exit 2 で拒否した。外部ネットワークには接続していない。試験環境に unshare・bwrap・実 chrome 等が無ければ失敗し、skip で成功扱いにしない。実 agent-browser 0.38.1 本体はこの host に無く、起動 script が同梱 chrome-headless-shell を呼んだ。
- 検査: `cargo test --workspace` → exit 0（新試験を含む）。`cargo clippy --workspace -- -D warnings`、`cargo fmt --all --check`、`git diff --check` → 各 exit 0。
- D1/D2 は変更していない。H3/auth_section・機密要求の起動前拒否・旧 IPC 拒否も維持。本番昇格・本番設定変更・内部 origin 追加はしていない。同一 host UID は決定 p4a-uid の制約で、`SameUid` の identity 復元拒否は続く。D5 の復元結合は別 WorkUnit が担当する。

## P4-A D5 restore 結合（run 01M3REKJA5PF78ZTD0XT42VA0N, WorkUnit restore-evidence）

- registry: `task_core::browser_isolation::LiveSessionRegistry`（`get(session_id) -> Option<Arc<dyn LiveSessionEntry>>`、`RuntimeKind::{Isolated, NotIsolated}`）と実装 `LiveSessions` を追加。`celeris::run` が 1 つ作り、`IsolatedBrowserConfig.live_sessions` 経由で supervisor（起動成功後に登録、停止の最初に削除）と `ApiState::with_live_sessions` に同じ `Arc` を配る。supervisor の entry の `current_attestation()` は runtime thread に毎回採り直させる。
- HTTP: `POST /api/v1/browser/identities/{id}/restore` の body に `session_id` を追加。判定順は (1) `sweep_expired` → not_found / other_project / 期限切れ（410）→ (2) registry 未登録・`NotIsolated` → `isolation_required` → (3) `current_attestation()` の採り直しと session id 一致 → (4) controller の投入口があるときだけ `restore_isolated` で開封し `deliver_state` で controller にだけ渡し、応答は 204・本文なし。`session_id` 無しは従来どおり `isolation_required`。現在の supervisor entry は投入口を持たない（`accepts_state()==false`）ので、別 UID でも開封前に止まる。
- 実 session 試験: `cargo test -p task-api --test browser_restore_live_session -- --nocapture` → 1 passed（2.28s）。実 bwrap + 実 chrome-headless-shell を `Supervisor::start` で起動し、CDP pipe の `Browser.getVersion` 応答後、supervisor が登録した entry に `restore_for_session` を呼んで `isolation_required`（attestation は `SameUid`）。同じ registry を結線した `task_api::router`（daemon と同じ router）に HTTP で (a) 無い identity → 404 `identity_not_found`、別 project → 422 `other_project`、(b) 期限切れ → 410、(c) 同一 UID の実隔離 session → 403 `isolation_required`、(d) 未登録 → 403、停止後（registry から削除済み・握った entry も `NoProcessGroup`）→ 403、(e) `NotIsolated` → 403、`session_id` 無し → 403。応答に秘密値は出ず、全体で `IdentitySealer::open_attempts()==0`。
- 単体: `cargo test -p task-api --lib restore_` → 4 passed。`restore_in_session_delivers_only_to_controller_after_all_checks` は fake entry で、全条件を満たしたときだけ開封 1 回・controller にだけ state が届くことと、各拒否で開封 0 回であることを確かめる（fake なので実 runtime の証拠には数えない）。
- 検査: `cargo test --workspace` → exit 0（2992 passed / 0 failed / 8 ignored）。`cargo clippy --workspace -- -D warnings` → exit 0。
- 未解決: 別 host UID の実証は引き続き未（決定 p4a-uid。subuid が親 uid_map の範囲外で newuidmap が EPERM）。このため復元の成功経路（開封 → controller の CDP への投入）は実 runtime で未実証で、supervisor entry の投入口（`deliver_state`）も未実装。H3・auth_section・Phase 3 配線・ADR-0084 D6 の起動前拒否は変更していない。本番昇格・本番設定変更・内部 origin 追加はしていない。

## P4-C 継続（run 01M3QGRCA745JCZTKDBDY9R83B）

- ADR-0087 を先に追加。worker 内の固定 `ConformanceResult` を削除し、`CELERIS_BROWSER_CONFORMANCE_FILE` の version 一致記録だけで route する。記録無し・破損・旧 version は起動前に拒否。`browser-specialist` を既存 ACP harness の別 ID として設定可能にし、browser 要求専用にした。dispatch は設定済みの非 account-pool adapter を候補として worker に渡し、公開操作で失敗した backend から別 session の候補へ実行時 fallback する。機密能力は宣言せず、CredentialUse の経路を再試行しない。H3/auth_section の配線は変更していない。
- `scripts/browser-conformance.py` は agent-browser **0.38.1 の実 binary**を起動し、127.0.0.1 の HTML fixture を ACP・Claude・specialist の各 ID で同じ手順で実行した。scripted driver は open 後に SIGKILL され、別 process が同じ browser session を再開する。denied origin は shim で拒否し、snapshot refs、click 後の fixture POST、screenshot、download を event と server 観測から判定する。外部ネットワークへの fixture 要求は無い。
- 実行: `python3 scripts/browser-conformance.py --scripted --agent-browser <agent-browser-0.38.1 の絶対パス> --output-dir <この run の成果物ディレクトリ>/p4c-conformance-v4` → exit 0。各 ID で 7/7 case、policy violation 0、harness process crash からの recovery 1。機械記録はこの run の `p4c-conformance-v4/conformance.json` / `same-task.json`。初回は snapshot の `ref=e1` 構文を runner が `@e1` と誤読して失敗し、正規表現を修正して再実行した。
- **この scripted 実行は backend 適合の証拠にしない。** 三つの ID は同じ scripted driver を起動しており、ACP RPC・Claude CLI・specialist wrapper の実 harness protocol はまだ実行していない。runner は scripted 出力の `source` を別値にし、worker はそれを適合記録として読まない。`--backend-command` に実 backend の command を三件渡し、その実行結果で生成した ledger だけが routing に使える。したがって P4-C の受入 0/1 の実 backend 比較は残る。fake substrate を使う worker 試験は fallback の実経路・session 分離・拒否だけの証拠。
- 実 LLM 比較: この環境は `claude auth status` が loggedIn=true だが `opencode` CLI が PATH に無く、三 backend の同一 task 比較を実施できない。ACP/OpenCode と specialist 用 harness の認証が整った環境で、三つの `--backend-command ID=<JSON argv>` を指定して同じ runner を再実行する。各 command は環境変数 `CELERIS_BROWSER_CLI`・`CELERIS_BROWSER_ORIGIN`・`CELERIS_BROWSER_PHASE` を読み、`open` phase で local origin を開いて denied origin を試した後に SIGKILL、`resume` phase で snapshot refs・click・screenshot・download を同じ session で実行する。`same-task.json` の accepted・違反・復旧・費用・時間を記録し、worker に渡す適合 file には実 harness 実行の `source` を要求する。実 LLM が使える時は費用を各 harness の usage から記録する。これは ADR-0009 P-34 の残課題。
- 本番昇格・本番設定変更・内部 origin 追加なし。`CELERIS_BROWSER_CONFORMANCE_FILE` は本番に設定していないため、現在の本番 browser 起動は記録不足で拒否される。適合記録の実 harness 生成とその独立試験が残る。
- 検査: `cargo test --workspace` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all --check` と `git diff --check` → exit 0。workspace gate は実 agent-browser の scripted fixture を自動実行しないため、上記 runner の独立した exit 0 を併記する。`browser_specialist_provider_uses_configured_acp_harness`、`specialist_wraps_existing_harness_and_runs_same_browser_task`、`execution_fallback_uses_fresh_session_and_refuses_without_conformance`、機密承認非消費の API 試験は workspace gate で成功した。

## P4-C backend protocol 適合（run 01M3QJ5CY8366206MQ20A3RBPB）

- ADR-0087 の決定に従い、`scripts/browser-conformance.py --protocol-scripted` を追加した。Rust の `AcpAdapter`、`ClaudeCodeAdapter`、ACP を包む `BrowserSpecialistAdapter` をそれぞれ起動し、`scripts/browser-conformance-harness.py` の scripted LLM が ACP JSON-RPC または Claude stream-json を話す。実 harness protocol を通過した後に同じ browser CLI を操作する。従来の `--scripted` は `source=celeris-browser-conformance-scripted` のままで、routing は引き続き拒否する。
- 実 agent-browser 0.38.1 と 127.0.0.1 fixture で三 backend 各7/7 case。open 後の harness crash、別 process での session 再開、denied origin、snapshot refs、click、screenshot、download を browser event と server 観測で判定した。`p4c-protocol-v2/same-task.json` は accepted=true、policy_violations=0、recoveries=1、scripted LLM の cost_usd=0 を三件とも記録。出力は run artifacts の機械記録であり、本番設定には投入していない。
- runner は全件成功後、生成した `conformance.json.next` を **同じ file のまま** `p4c_runner_record_routes_and_falls_back` に渡す。試験は三 backend の production route、同 ledger に対する機密能力拒否、ACP 失敗後に Claude へ新 session で移る実 worker 経路を検査し、成功した場合だけ `conformance.json` に昇格する。`p4c-protocol-v2/routing-test.json` は exit 0。機密拒否 assertion 追加後も同 ledger を使う試験が exit 0。単体テスト内で作った mock ledger をこの結合の証拠には使っていない。fallback 実行には fake substrate を用いるため、実 agent-browser での失敗後再試行は別証拠が必要。
- 実行コマンド: `python3 scripts/browser-conformance.py --protocol-scripted --agent-browser <0.38.1 binary の絶対パス> --output-dir <run artifacts>/p4c-protocol-v2` → exit 0。外部 network への fixture 要求は無し。loopback socket のため sandbox 外実行が必要だった。
- 実 LLM 同一 task 比較は未実施。Claude は認証済みだが ACP/OpenCode CLI はこの環境の PATH に無く、三 backend を同じ条件で実行できない。認証が揃った環境で `--backend-command acp=<JSON argv>`、`--backend-command claude-code=<JSON argv>`、`--backend-command browser-specialist=<JSON argv>` を指定し、各 command が runner の `CELERIS_BROWSER_CLI` / `CELERIS_BROWSER_ORIGIN` / `CELERIS_BROWSER_PHASE` を受けて同じ二段階 fixture を実行する。`same-task.json` の成否・違反・復旧・時間・実 usage 由来の費用を比較して PROGRESS に追記する（ADR-0009 P-34）。
- 機密能力の適合は記録していない。`CredentialInjection`・`IdentityRestore` は P4-A/B の実適合まで拒否し、H3/auth_section の配線は変更していない。本番昇格・本番設定変更・内部 origin 追加なし。
- 最終検査: `cargo test --workspace` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0。最初の workspace 実行は前 run の specialist 登録に対応する config エラー期待文言が古いため1件失敗し、期待文言を更新して再実行した。`cargo fmt --all --check` と `git diff --check` も exit 0。

## P4-C 実 browser fallback（run 01M3R8T40E9CFNGZG9WHEKMZXH、attempt 2）

- 固定版 `agent-browser 0.38.1` はローカル npm cache の tarball（SHA-512 integrity を検証）から `/tmp/p4c-agent-browser/package/bin/agent-browser-linux-x64` へ展開した。外部通信、本番設定の変更、内部 origin の追加はない。
- 実行: `python3 scripts/browser-conformance.py --protocol-scripted --fallback-scenario --agent-browser /tmp/p4c-agent-browser/package/bin/agent-browser-linux-x64 --output-dir /var/lib/celeris/workspaces/01M3QGRC6AQ81PWM1XP4C7BH45/wu/real-fallback/artifacts/p4c-real-fallback-final-v3` → **exit 0**。ACP、明示 Claude、browser-specialist は同一 `p4c-local-v1` fixture で各 **7/7**、policy violation 0、復旧 1。runner が生成した ledger を worker route が受理した（`routing-test.json` exit 0）。
- 同 runner の `p4c_fallback_real_harness_scenario` は worker の公開入口 `run_with_candidates` と実 ACP/Claude adapter を使い、主 ACP harness process を browser の最初の navigation 後に SIGKILL した。代替 Claude は新しい session で同じ fixture を開き、snapshot refs、click、screenshot、download を完了。fixture server は fallback 区間で navigation 3件（主、代替、無候補試験の主）、`POST /clicked` 1件、download 1件を観測。`fallback-test.json` exit 0、`fallback-fixture.json` に要求列、`fallback/fallback-outcome.json` に session 分離と拒否を記録した。
- 無候補では runner ledger から Claude/specialist の適合結果を除いた記録を使い、主 harness 失敗後に `all capable backends failed` を返す。候補の browser session は起動しない。`CredentialUse` は ledger に機密能力の適合がないため起動前に `lacks required conformance` を返した。`p4c_fallback_ledger_parsing_and_refusal` は runner 形式の欠落・scripted source を決定的に拒否する（限定試験 exit 0）。`CredentialInjection`・`IdentityRestore` の拒否および H3/auth_section を維持した。
- 最初の実 runner は ACP の初回 navigation が失敗し exit 1（ACP 1/7、残り二 backend は7/7）。同一固定版を再実行すると三 backend 7/7・fallback exit 0、最終実行も exit 0。初回失敗の原因は特定できていないため、冷間起動の安定性は残課題。適合 ledger は成功した最終実行のものだけを採用した。
- 実 LLM 比較: `claude auth status` exit 0 だが ACP/OpenCode CLI は PATH にないため三 backend 比較は未実施。ADR-0009 P-34 の手順は上の「P4-C backend protocol 適合」節に記載した三つの `--backend-command`、同一二段階 fixture、`same-task.json` の accepted・違反・復旧・費用・時間の比較を使う。P4-A/B の機密実適合と本番昇格は未実施。
- gate: `cargo test --workspace` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0。`cargo test -p task-worker --lib p4c_fallback_ -- --nocapture` → exit 0（決定的試験1件成功、実 browser 試験1件は通常 gate では ignored）。

## P4-B 実 CDP sink 取り込み（run 01M3RPPYTPT43N8ZHXFDDWESX0、WorkUnit sink-retry）

- 前回 unit（WorkUnit sink、commit `968110ae`）は `task_worker::browser_cdp_sink`（`CdpController`・`BrokerClient`・`InjectionRequest`・`PendingInjection`・`InjectionError`）と `tests/browser_cdp_sink.rs` を実装・試験とも成功していたが、`cargo clippy -p task-worker --all-targets` が既存 `browser_tests.rs` の `await_holding_lock` で落ちて unit 全体が失敗扱いになった。この unit は `git cherry-pick 968110ae` でその成果をこの WU ブランチ（base `c8cd45f3`）に取り込んだ。衝突は無かった。
- `cargo test -p task-worker --test browser_cdp_sink` → **exit 0、2 passed**（`inner_cdp_sink`・`real_browser_injection_receipt_and_origin_guards`、skip 無し）。実 bwrap + 実 chrome-headless-shell（playwright 1243）+ loopback DNS/HTTPS fixture で receipt のみ返る注入、origin 不一致拒否、cross-origin iframe 拒否を確認。外部ネットワークへは接続していない。
- `cargo clippy --workspace -- -D warnings`（`--all-targets` は付けない、Objective の指示どおり）→ **exit 0**。既存 `browser_tests.rs` の `--all-targets` clippy 違反はこの unit の範囲外として触っていない。
- `cargo test --workspace` → **exit 0、3006 passed / 0 failed / 11 ignored**（既存の ignored 合計、この unit で新規追加なし）。
- injection-only IPC（SO_PEERCRED role/session/lease 照合）と controller への実結線は並行 WorkUnit（ipc・後続 wire）の担当で、この unit では変更していない。攻撃試験行列（TOCTOU・redirect・cross-origin iframe・DOM 再表示・worker/browser からの取得）と H3 端から端の実注入検証は後続 WorkUnit（attacks・h3e2e）の担当。
- 本番昇格・本番設定変更・内部 origin 追加はしていない。新しい設計は足していない（ADR-0089 の範囲内）。unwrap 不使用。

## P4-B 実攻撃試験（2026-09-30、WorkUnit attacks）

- 試験: `crates/task-worker/tests/browser_injection_attacks.rs`（外側 1 本が `unshare --user --map-root-user --net` で内側を起動し、実 chrome-headless-shell を bwrap 隔離 runtime で 1 回起動、実 broker `injection.sock`（`Admission::SameUidHarness`）と自己署名 HTTPS fixture・試験用 DNS で A1〜A17 を順に実行）。外部ネットワークに出ない。
- 検査面: 各攻撃で sentinel とその percent・hex・UTF-16LE・base64（3 offset）を、agent への CDP 応答・receipt・broker IPC 応答・`journal.jsonl`・子 worker process の stdout/stderr・screenshot の PNG bytes と base64・内側 process の出力で検索。
- 証拠コマンド: `cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → exit 0、2 passed / 0 failed。`cargo test -p task-worker --test browser_injection_wire` → 2 passed。`cargo clippy --workspace -- -D warnings` → exit 0。

| # | 観測 |
|---|---|
| A0 正例 | 注入成功、fixture input が sentinel を受け取り（長さ・文字コード和で照合）、区間終わりで空 |
| A1 | 照合用 id 取得後に別 origin へ遷移 → `target_mismatch`、別 origin の input は空、lease 未消費 |
| A2 | 同 origin の新 document → `target_changed`、新 document の input は空。`document.open()` は loader が変わらず同 origin・同 document への注入として成功（契約違反ではない） |
| A3 | 実 meta refresh redirect（other → fixture）→ controller・broker とも `redirected`、同じ lease で後に成功（未消費） |
| A4 | iframe 指定 → `cross_origin_frame`（controller・broker raw 要求とも）、別 target → `target_mismatch`、空 frame_chain → `empty_frame_chain` |
| A5 | 逆向き iframe → `target_mismatch` / `cross_origin_frame` |
| A6 | `type=text`・script で type 書換え → `redisplay_field`、DOM 値は空 |
| A7 | 区間中の `Runtime.evaluate`・`DOM.getOuterHTML`・`DOM.getAttributes`・`DOM.getDocument`・`Accessibility.getFullAXTree`・`Runtime.callFunctionOn` → `auth_section_required`、区間後の値は空 |
| A8 | **未達（所見）**: page が input 時に値を可視 div に複製すると、区間後に agent の `Runtime.evaluate`/`DOM.getDocument` 応答に sentinel が現れる。試験は `FINDING-A8` を出して記録している（合格扱いにしない） |
| A9 | 区間中の `Page.captureScreenshot`・`Page.startScreencast` は拒否、区間後の screenshot（7103 bytes）に sentinel 文字列無し（OCR はしない） |
| A10 | page の `console.log(value)`・値入り例外は実行済み、`CdpController` は event を外へ出さず応答にも sentinel 無し |
| A11 | 実 python3 子 process が有効 lease・section つき要求 → `injection_worker_not_allowed`（SO_PEERCRED pid 不一致）、sink FD に frame 無し。`resolve.sock` → `trusted_injection_required`、bridge は固定拒否 |
| A12 | page から `fetch('file://…injection.sock'/'…resolve.sock')` は blocked、`/proc/<runtime pid>/root` に broker run dir・socket が無い（対照 `/session` は有る） |
| A13 | `InjectionService::handle` に別 UID の `PeerCred` → `peer_uid_mismatch`（関数単位。`unshare -r` の子は host で同 UID なので実 process 試験は未） |
| A14 | 別 session として登録した controller 子 process が session A の lease で要求 → `injection_worker_not_allowed`、未登録 session → `session_not_live` |
| A15 | 区間を開く前・閉じた後 → `auth_section_required`（controller・broker とも）、lease 未消費 |
| A16 | 成功後の再要求 → `lease_used`、sink FD に 2 本目の frame 無し |
| A17 | `value`/`length` field を含む要求 → `invalid_request` |

- 未解決:
  - A8: `RedisplayGuard`（`celeris-credentiald/src/injection.rs`）が controller の観測経路に配線されていない。ADR-0089 D4-7 の「broker が区間ごとに guard を作り controller に salt と hash を渡す」の IPC・controller 側実装が必要。これが入るまで D5 行列は全部期待どおりではない。
  - A1: 「controller の照合後・`Runtime.callFunctionOn` 前」の競合を決定的に作れず、broker の `target_changed`（lease 消費後の拒否）経路は実 browser で未再現（stale id は controller 側で止まる）。
  - A4: fixture の 2 host が同一 site のため OOPIF にならず、OOPIF の `target_mismatch` は未再現。
  - A13: 別 host UID の実 process による試験は、別 UID が使える host が必要。
  - `cargo clippy --workspace --all-targets -- -D warnings` は既存の `task-worker/src/browser_tests.rs`（`await_holding_lock`）と `task-dispatch`（`type_complexity`）で exit 101（この unit の変更と無関係）。
