# Phase browser-4: isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG, 01M3QGRC542ZC23996DNCTHZF5, 01M3QGRC6AQ81PWM1XP4C7BH45, 01M3SM0WN346ABGF42QV02RZTP]
---

- 状態（2026-09-30、[追跡表](phase-browser-acceptance.md) と一致）: **P4-A は一部達成**（P4-A-1〜6 合格、P4-A-7「別 host UID での隔離 runtime の実証」は後続 `01M3SPN8H05EJ3DHPVEGEYTMEH`）。**P4-B は一部達成**（P4-B-1〜5 と攻撃試験 A1〜A12・A14〜A17 合格、P4-B-6「本番 admission での機密能力の解放」は後続 `01M3SPN8HPHPWZ32F0AG986TWS`、A13「別 host UID の実 process」は後続 `01M3SPN8HE6A1TZ54GBZGHMZYJ`）。**P4-C は一部達成**（P4-C-1〜5 合格、P4-C-6「本番 routing での機密能力 backend の解放」は後続 `01M3SPN8HPHPWZ32F0AG986TWS`）。後続はすべて決定 sep-uid=a による。本番 admission は `Attested` 必須で、この host（同一 UID）では SameUid を拒否する（P4-A-6）。
- 旧状態（2026-09-30 以前の記述、詳細）: **P4-B は局所 fixture で合格し、機密能力は P4-B 実測証拠つき適合記録からだけ解放可能（ADR-0112、2026-09-30）。A1（target_changed）・A4（OOPIF target_mismatch）は 2026-09-30 に実再現し合格。deliver_state は 2026-09-30 に実装済みで試験 admission の成功経路を実証、本番 admission は同一 UID で拒否のまま。P4-C の公開能力は scripted LLM による実 backend 適合済み**。runtime 方式・H7 は人の回答を採用済み（ADR-0103）。旧 broker IPC の秘密返却を廃止。機密能力は実 runtime・CDP sink・injection-only IPC 適合まで拒否。本番 H3 経路（controller 所有 Chromium/CDP 共有・trusted selector・`browser.rs` 結線・実 e2e 注入検証）は ADR-0110 の unit 群（selector/shared-cdp/h3-wire/e2e）で実装済みだが、本番 broker の admission は `Attested` のみで、この host は同一 UID（`SameUid`）のため機密起動（実注入）は本番経路で拒否のまま。本番未昇格。
- 更新: 2026-09-30（task 01M3SM0WN346ABGF42QV02RZTP, run 01M3SM0WN346ABGF42QV02RZTP: control-gate・deliver-state・attacks-a1a4 の 3 unit を統合したブランチで `cargo test --workspace`・`cargo clippy --workspace -- -D warnings` を実行し exit 0 を確認。下記「2026-09-30 統合検査（record）」参照）。
- 前回更新: 2026-09-30（run 01M3S43TR5TQJ4PQ40VVTR6VY0、WorkUnit closeout: ADR-0110 の後続 unit（selector・shared-cdp・h3-wire・e2e）を workspace に統合済みであることを確認し、workspace 検査を実行。`cargo test --workspace` exit 0（3041 passed / 0 failed / 11 ignored）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all --check` は本 unit 外の既存フォーマット崩れ 2 箇所を `cargo fmt --all` で解消し再検査 exit 0）
- ADR-0110: [H3 shared CDP and trusted selector](../adr/0110-browser-p4b-h3-shared-cdp-trusted-selector.md)（controller 所有 1 Chromium/CDP の共有・認証区間中の全面遮断、管理者 site policy 由来の trusted selector の固定・照合、`browser.rs` H3 の D3 照合順、e2e の 6 面 sentinel 非露出検証と負の対照）
- ADR-0105: [same-uid bwrap runtime](../adr/0105-browser-p4a-same-uid-bwrap-runtime.md)。ADR-0106: [conformance dispatch](../adr/0106-browser-phase4-conformance-dispatch.md)。旧版では両組の番号がそれぞれ別文書に重複していたため、[対応表](phase-browser-main-merge.md)で振り直した。
- ADR: [ADR-0102](../adr/0102-browser-phase4-isolation-injection-routing.md) D6、[ADR-0103](../adr/0103-browser-phase4-runtime-selection.md)

## 行ごとの判定

| 行 | 判定 | 証拠・限界 |
|---|---|---|
| P4-A isolated runtime | 一部達成（[追跡表](phase-browser-acceptance.md) P4-A-1〜6 合格、P4-A-7 別 host UID 実証は後続 `01M3SPN8H05EJ3DHPVEGEYTMEH`。決定 p4a-uid: 同一 host UID）。実 bwrap runtime・事実採取・分離・daemon 起動時 orphan 回収・production worker の egress 接続は実装済み。**復元結合（deliver_state）は 2026-09-30 に実装済み**（下記 P3-A 行）。 | `cargo test -p task-worker --test browser_runtime_isolated` → 4 passed / 1 ignored（helper）。実 chrome-headless-shell（agent-browser の browser、playwright 1243）を bwrap で起動し CDP pipe で `Browser.getVersion` 応答、host 側 `/proc` で 6 namespace 別・root ro・書ける mount は `/session` だけ・NoNewPrivs=1・CapEff/CapPrm=0・uid_map `1000 1001 1`・netns TCP LISTEN 0 件。同 spec の probe で broker/control socket・`/run/user`・host tmp 不可視、`/usr`・`/etc` 書込不可、host loopback fixture・10.0.0.1・::1・192.0.2.53:53 へ接続不可。controller SIGKILL 後に bwrap・sandbox 内 process が消える（実 process）。記録からの再起動回収は starttime 一致だけを殺す。`verify_isolation` は弱めず、違反は `SameUid` だけ → attestation 無し → 復元拒否。D3 の実 daemon 起動試験 1 passed、D4 の run_with_executable 実 chrome/egress/fixture 試験 1 passed（下記）。 |
| P3-A identity 復元（隔離下のみ） | 一部達成（[追跡表](phase-browser-acceptance.md) P3-A-6・P3-A-7 合格、P3-A-8 本番復元成功は後続 `01M3SPN8H05EJ3DHPVEGEYTMEH`）。**deliver_state 実装済み（2026-09-30、ADR-0114、WorkUnit deliver-state）。試験 admission（`same-uid-harness` feature）での成功経路（開封 → controller の CDP への投入）を実 bwrap + 実 chrome-headless-shell で実証。本番 admission（`Attested`）では同一 host UID のため `SameUid` 拒否のまま**（決定 p4a-uid） | `SupervisedEntry::attach_controller` が `CdpController` を共有 slot に入れた後だけ `accepts_state()==true`（本番 `browser.rs` は `SharedCdp` の controller を渡す 1 行）。`LiveSession` は CDP pipe を保持する間だけ `accepts_state()==true`。共通の `deliver_state_via` は `IdentityStatePlain`（cookie と https origin のみ）を `Storage.setCookies` 1 command にまとめ、`controller_command` だけで送る。argv・ファイル・`--restore`/`--state`/`--profile` は使わない。`RestoreAdmission::{Attested(既定), SameUidHarness(feature `same-uid-harness`, test 専用)}` を追加、実 runtime の事実の違反が `SameUid` だけのときに限り harness が試験 attestation を作る。`cargo test -p task-worker --test browser_restore_deliver -- --nocapture` → exit 0、**3 passed**（`supervisor_entry_delivers_restored_state_to_controller_cdp_under_harness_admission`: 試験 admission で開封 → `Storage.setCookies` → controller の `Storage.getCookies` に `sid=secret, domain 127.0.0.1` を確認／`attested_admission_refuses_same_uid_before_opening`: 本番 `Attested` で `SameUid` 拒否・開封 0 回／`live_session_delivers_restored_state_over_its_own_cdp_pipe`）。他 project・別 origin・期限切れ（`identity_revoked`）・削除（`identity_deleted`）・別 session（`isolation_required`）は開封前拒否（同試験内で確認、2026-09-30 再検査出力: `denied: ... other_project` / `other_origin` / `identity_revoked` / `identity_deleted` / `isolation_required`）。ADR-0108 D5 の HTTP 経路（`POST /api/v1/browser/identities/{id}/restore`）・registry は変更なし。`cargo test -p task-api --test browser_restore_live_session` → 1 passed。`cargo test -p task-api --lib restore_` → 4 passed。 |
| P4-B stronger injection | 一部達成（[追跡表](phase-browser-acceptance.md) P4-B-1〜5・A1〜A12・A14〜A17 合格、P4-B-6 本番解放は後続 `01M3SPN8HPHPWZ32F0AG986TWS`、A13 は後続 `01M3SPN8HE6A1TZ54GBZGHMZYJ`）。本番 H3 経路まで実装済み（2026-09-30、ADR-0110）。broker IPC・controller CDP sink・trusted selector・`browser.rs` の H3 結線・実 e2e の 6 面 sentinel 非露出検証を実 bwrap + 実 browser + loopback fixture で確認。本番 admission（`Attested`）では同一 UID のため機密起動は拒否のまま。 | 【unit ipc】ADR-0109 D1〜D3 の broker injection-only IPC 証拠・制約は上記のとおり。【unit wire-harness】commit `35ab5564`（cherry-pick `71640d79`）で `tests/browser_injection_wire.rs` を追加し、実 broker の `injection.sock` と実隔離 browser の CDP sink を結線。`cargo test -p task-worker --test browser_injection_wire` → **2 passed / 0 failed / 0 ignored**（`real_broker_browser_injection_receipt_and_origin_guards` は skip 無し、receipt-only 注入・origin ガードを確認）。`cargo test -p celeris-credentiald` → injection_ipc 12、broker 12、lib 14 passed。`cargo clippy --workspace -- -D warnings` → exit 0。【unit selector】ADR-0110 D2: `task_core::browser_wait::{validate_trusted_selector, validate_trusted_login_url, validate_trusted_login}` と `ConsumedBrowserApproval.trusted_login`、broker の `selector_mismatch`。`cargo test -p task-core browser_wait::` に `trusted_login_url_must_stay_on_the_exact_origin`・`trusted_login_is_pinned_only_on_valid_credential_use_approvals` を含む。【unit shared-cdp】ADR-0110 D1: `task_worker::browser_shared_cdp`（relay の token/Origin 検査・CDP 多重化・認証区間中の全面遮断・postData 削除）。`cargo test -p task-worker --test browser_shared_cdp` → **2 passed**（`real_shared_cdp_and_auth_section`・`inner_shared_cdp`、実 bwrap + 実 chrome-headless-shell、skip 無し）。【unit h3-wire】ADR-0110 D3: `browser.rs` の H3 開始/終了を `RegisterLiveSession → DescribePolicy 照合 → 承認消費 → lease grant → relay 遮断/OpenAuthSection → inject（selector_mismatch 照合は lease 消費より前）→ submit → close_auth_section/CloseAuthSection → relay 解除` の順に結線し、旧 `use_credential`（`auth login` + 旧 bridge）を H3 経路から除いた。`cargo test -p task-worker --test browser_h3_wire` → **2 passed**（`real_broker_browser_injection_receipt_and_origin_guards`・`inner_injection_wire`、実 broker + 実 CDP pipe、skip 無し）。【unit e2e】ADR-0110 D4: `crates/task-api/tests/browser_h3_injection.rs` の `production_h3_injects_once_without_exposure`（実 daemon 経路・実 bwrap・実 chrome-headless-shell・実 broker process・実 CDP pipe・relay、loopback DNS/HTTPS fixture、scripted LLM で 1 回注入し、fixture の受理と event 列・live event・LLM 入力・SQLite DB・WAL・run log・run/artifact ファイルの 6 面を sentinel（password・SENTINEL_USER の raw/base64/percent/UTF-16LE/JSON escape 表現）で 0 件検索）と負の対照 `injected_leak_is_caught_by_the_same_scanner`（検索器自体が植え込んだ sentinel を検出することを確認）→ 両方 **passed**。`cargo test -p task-api --test browser_h3_injection` は `production_h3_injects_once_without_exposure` が内部で `unshare --user --map-root-user --net` の子 process を起こし `inner_h3_injection` を再実行するため sandbox 内では動かず、sandbox 外の workspace 検査で通す。【unit attacks】`tests/browser_injection_attacks.rs` で実 chrome・実 broker・loopback fixture 上の A1〜A17 を実行。`cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → **2 passed / 0 failed**（skip 無し、全 `ATTACK-Ax-OK` 出力）。A8（page script が値を可視 div に写す）は当初 **未達の所見**（`RedisplayGuard` が `CdpController::agent_command` に未配線）だったが、【unit redisplay】ADR-0111 で guard を controller の agent 観測経路（`agent_command` 応答・shared CDP relay 応答・event）に配線し **合格**（`ATTACK-A8-OK`・負の対照 `ATTACK-A8n-OK`）。`cargo test --workspace --no-fail-fast` → exit 0（3044 passed / 0 failed / 11 ignored、2026-09-30）。詳細は下の「P4-B 実攻撃試験」節。ADR-0110 D4 の N1/N2（`same-uid-harness` feature 下での区間遮断無効化・値消去無効化を使った検出器の実地変異試験）はこの e2e 単体では未実装で、次段の攻撃試験課題として残る。機密能力は本番では未解放・本番未昇格。 |
| H3 観測停止の維持 | 実装維持かつ本番 H3 経路まで結線済み。本番の機密起動は同一 UID のため拒否 | `cargo test -p task-worker browser --lib` → 31 passed。`browser_auth_section_forward_events_drops_progress_artifact_and_live` と LiveEmitter の抑止試験を含む。API 結合テストの store auth_section / takeover 拒否も成功。ADR-0110 の e2e `production_h3_injects_once_without_exposure`（上記）が実 daemon 経路での end-to-end 検証（注入成功＋6 面 sentinel 非露出）を通す。本番 broker admission は `Attested` のみで、この host は `SameUid` のため本番の機密起動（実注入）は拒否のまま。e2e の正例は `same-uid-harness` feature 限定の試験 admission に依る。 |
| P4-C backend routing | 一部達成（[追跡表](phase-browser-acceptance.md) P4-C-1〜5 合格、P4-C-6 本番の機密 backend 解放は後続 `01M3SPN8HPHPWZ32F0AG986TWS`）。公開能力を実 backend protocol で適合。機密要求は拒否 | `--protocol-scripted --fallback-scenario` runner が実 agent-browser 0.38.1 + loopback fixture で ACP RPC・Claude CLI・specialist wrapper を各7/7 実行。runner の ledger を worker routing と実 browser fallback に渡して成功。主 ACP harness を SIGKILL し、別 session の Claude が click/download を完了。無候補と `CredentialUse` は明示拒否。`CredentialInjection`・`IdentityRestore` は P4-A/B の実適合まで拒否。実 LLM 比較は未。 |

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

- ADR-0104 を先に追加し、`task-worker::browser_egress` と独立実行 `celeris-browser-egress` を実装。既存の機密 routing 拒否・旧 resolve IPC 拒否・CredentialProvider・H3/auth_section は変更していない。
- proxy は controller から継承する接続済み AF_UNIX stream（FD 3）と stdin の JSON policy だけを受ける。host の listener は開かない。policy 入力 64 KiB / 5秒、HTTP header 8 KiB、DNS応答 16 KiB、setup 10秒、tunnel 300秒 / 各方向64 MiBで制限。stderr は固定拒否コード、stdout は空。proxy は no_new_privs / dumpable=0、親死亡 SIGKILL を設定する。
- CONNECT authority を DNS 前に拒否できるようにした。指定 resolver の TCP/53 へ直接照会し、A/AAAA の両方・ID/question・CNAME/owner・圧縮pointer・応答サイズを検査。全候補に public 判定を行い、検査済みIPへ直接connectする。proxy環境変数・OS再解決・上位proxyは利用しない。IPv6無効時に公開AAAAを含む回答も拒否するため、dual-stackサイトへの互換性は限定的。この初期の拒否境界は緩めていない。
- IANA registry を確認し、IPv4 の廃止済み6to4 relay、IPv6のdiscard-only・benchmark・documentation・SRv6等を拒否へ追加。IPv6有効時も通常のglobal unicast以外とspecial-purpose範囲を保守的に拒否する。出典と方針はADR-0104。
- 実Unix/TCP fixture 9件: DNS rebinding・private/v6・proxy auth/転送/本文header・DNS/DoT port・IP literal・非許可host・不正DNS/循環CNAME・TLS初期byte/half-close。実子プロセス6件: inherited socket必須・worker指定のprivate/proxy拒否・不正/過大policy・policy入力期限・peer切断終了・親死亡時SIGKILLとsubreaperのwaitpid回収。すべて外部ネットワークを使わない。
- `cargo test -p task-worker browser_egress --lib` → exit 0、9 passed。`cargo test -p task-worker --test browser_egress_process` → exit 0、6 passed。
- 初回検査でDNS未知recordの扱い、clippyのtest module配置と不要borrow、ハイフン付きbinary名のcompile-time env参照を修正。最後のものは既存credentiald試験と同じruntimeの環境参照へ変更した。親死亡試験はNoNewPrivsが継承されるケースを考慮し、dumpable=0による/proc/fd読取り拒否も待ってから親を終了する。reap後のPIDへsignalを送らない。
- sandbox外の `bwrap --unshare-user --unshare-net --ro-bind / / true` は exit 0。`unshare --user --map-auto --map-user=1 --map-group=1 id` は exit 1（newuidmap EPERM）。親uid_mapは `0:100000:1001, 1001:1001:1, 1002:101002:64534`、subuid/subgidの rmaeda 割当ては `165536:65536` で親の範囲外。管理者設定は変更していない。同UIDのbwrap成功を別UIDの実証として扱わない。
- P4-A全体、P3-A復元、P4-B実CDP/IPC、P4-C実fixture/specialistは未達。production workerは新proxyをまだ起動しない。既存browserのネットワークがこのproxyで制限されるとは主張しない。namespace接続・trusted controllerの運用配線と実適合が必要。

- このrunの最終検査: `cargo test --workspace` → exit 0（2972 passed / 0 failed / 既存 ignored 7件）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。`cargo fmt --all --check` / `git diff --check` → exit 0。機密機能の実適合・production接続の証拠ではない。

## P4-A 実 runtime（run 01M3QGRCAHDK1AB4R9WBHJ9XHZ, task 01M3QGRC542ZC23996DNCTHZF5）

- 前回 branch `celeris/01M3Q49ZTST3XQ9DGF6AGNR0XG`（331c6dd）を fast-forward で採用。ADR-0105 を追加してから実装。
- `task_worker::browser_runtime`: bwrap argv（user/pid/net/ipc/uts/cgroup unshare、`--die-with-parent`、`--new-session`、`--cap-drop ALL`、`--clearenv`、tmpfs root + `/usr`・`/etc` の必要 file・browser dir の ro bind、`/session` だけ rw、`--remount-ro /`）、`--info-fd` による child pid、CDP pipe（fd 3/4）、`/proc` からの `RuntimeFacts` 採取、`attest()`、`LiveSession`、`record`/`reap_recorded`。
- `task_core::browser_isolation::LiveIsolation` と `BrowserIdentityService::restore_for_session`: 呼ぶたびに稼働中 session から attestation を採り直し、session 停止・違反・session id 不一致は `isolation_required`、別 identity は `identity_not_found`/`other_project`、期限切れは拒否。`cargo test -p task-api restore_for_session` → 1 passed（attestation は fake facts 由来。実 runtime では `SameUid` で出ないことを task-worker の実試験で確認）。
- `/etc` を丸ごと bind した初回は `verify_isolation` が `BrokerVisible{/etc}`（`/etc/celeris` の親）で拒否した。検査を緩めず bind を file 単位に絞って解消。
- 最終検査: `cargo test --workspace` → exit 0（2977 passed / 0 failed / ignored 8）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0（workspace の `--all-targets` は既存 `task-api/tests/browser_e2e.rs` の type_complexity で失敗、本変更外）。
- 本番昇格・本番設定変更・内部 origin 追加はしていない。

## P4-A D3/D4 production 配線（run 01M3RCSFK5ZTC4JV0YJF17DV7C）

- D3: `celeris::run` の `Started::Running` 直後、`set_orphan_takeover` と同じ起動経路で `start_instance` を呼び、`browser-runtime/<instance_id>/` の記録を starttime と照合して回収する。`Duplicate` と `--mode verify` は回収しない。`cargo test -p celeris --test browser_startup_reap -- --nocapture` → 1 passed。実 daemon 起動関数を経て死んだ instance の記録を消し、生きた instance の runtime を維持した。
  - D4: `browser::run` / `run_with_executable` は resolver・bwrap・sandboxd・egress が揃わなければ起動前に `isolated_runtime_unavailable`。host 直接起動への fallback はない。`cargo test -p task-worker --lib browser::tests::missing_egress_resolver_refuses_before_browser_start -- --nocapture` → 1 passed（run dir を作る前に固定コード拒否）。shim の `action.sock` は sandbox の外に置き、worker 側で policy を再検査する。現在の sandboxd への action 配送は `/session/actions` のファイルキューであり、ADR-0108 D4 に書かれた channel の別 message 種別とは実装が異なる。この差は解消が必要。
- 実結合: `cargo test -p task-worker --lib browser::tests::production_action_path_reaches_fixture_through_real_browser_and_egress -- --nocapture` → 1 passed。`unshare --user --map-root-user --net` の試験用 netns 内で公開扱いの 93.184.216.34 を loopback に設定し、ローカル DNS/HTTPS fixture を動かした。`run_with_executable` から shim → action.sock → sandboxd → 実 chrome-headless-shell → sandboxd relay → 実 celeris-browser-egress → fixture の本文を取得し、禁止 flag は exit 2 で拒否した。外部ネットワークには接続していない。試験環境に unshare・bwrap・実 chrome 等が無ければ失敗し、skip で成功扱いにしない。実 agent-browser 0.38.1 本体はこの host に無く、起動 script が同梱 chrome-headless-shell を呼んだ。
- 検査: `cargo test --workspace` → exit 0（新試験を含む）。`cargo clippy --workspace -- -D warnings`、`cargo fmt --all --check`、`git diff --check` → 各 exit 0。
- D1/D2 は変更していない。H3/auth_section・機密要求の起動前拒否・旧 IPC 拒否も維持。本番昇格・本番設定変更・内部 origin 追加はしていない。同一 host UID は決定 p4a-uid の制約で、`SameUid` の identity 復元拒否は続く。D5 の復元結合は別 WorkUnit が担当する。

## P4-A D5 restore 結合（run 01M3REKJA5PF78ZTD0XT42VA0N, WorkUnit restore-evidence）

- registry: `task_core::browser_isolation::LiveSessionRegistry`（`get(session_id) -> Option<Arc<dyn LiveSessionEntry>>`、`RuntimeKind::{Isolated, NotIsolated}`）と実装 `LiveSessions` を追加。`celeris::run` が 1 つ作り、`IsolatedBrowserConfig.live_sessions` 経由で supervisor（起動成功後に登録、停止の最初に削除）と `ApiState::with_live_sessions` に同じ `Arc` を配る。supervisor の entry の `current_attestation()` は runtime thread に毎回採り直させる。
- HTTP: `POST /api/v1/browser/identities/{id}/restore` の body に `session_id` を追加。判定順は (1) `sweep_expired` → not_found / other_project / 期限切れ（410）→ (2) registry 未登録・`NotIsolated` → `isolation_required` → (3) `current_attestation()` の採り直しと session id 一致 → (4) controller の投入口があるときだけ `restore_isolated` で開封し `deliver_state` で controller にだけ渡し、応答は 204・本文なし。`session_id` 無しは従来どおり `isolation_required`。現在の supervisor entry は投入口を持たない（`accepts_state()==false`）ので、別 UID でも開封前に止まる。
- 実 session 試験: `cargo test -p task-api --test browser_restore_live_session -- --nocapture` → 1 passed（2.28s）。実 bwrap + 実 chrome-headless-shell を `Supervisor::start` で起動し、CDP pipe の `Browser.getVersion` 応答後、supervisor が登録した entry に `restore_for_session` を呼んで `isolation_required`（attestation は `SameUid`）。同じ registry を結線した `task_api::router`（daemon と同じ router）に HTTP で (a) 無い identity → 404 `identity_not_found`、別 project → 422 `other_project`、(b) 期限切れ → 410、(c) 同一 UID の実隔離 session → 403 `isolation_required`、(d) 未登録 → 403、停止後（registry から削除済み・握った entry も `NoProcessGroup`）→ 403、(e) `NotIsolated` → 403、`session_id` 無し → 403。応答に秘密値は出ず、全体で `IdentitySealer::open_attempts()==0`。
- 単体: `cargo test -p task-api --lib restore_` → 4 passed。`restore_in_session_delivers_only_to_controller_after_all_checks` は fake entry で、全条件を満たしたときだけ開封 1 回・controller にだけ state が届くことと、各拒否で開封 0 回であることを確かめる（fake なので実 runtime の証拠には数えない）。
- 検査: `cargo test --workspace` → exit 0（2992 passed / 0 failed / 8 ignored）。`cargo clippy --workspace -- -D warnings` → exit 0。
- 未解決: 別 host UID の実証は引き続き未（決定 p4a-uid。subuid が親 uid_map の範囲外で newuidmap が EPERM）。このため復元の成功経路（開封 → controller の CDP への投入）は実 runtime で未実証で、supervisor entry の投入口（`deliver_state`）も未実装。H3・auth_section・Phase 3 配線・ADR-0102 D6 の起動前拒否は変更していない。本番昇格・本番設定変更・内部 origin 追加はしていない。

## P4-C 継続（run 01M3QGRCA745JCZTKDBDY9R83B）

- ADR-0106 を先に追加。worker 内の固定 `ConformanceResult` を削除し、`CELERIS_BROWSER_CONFORMANCE_FILE` の version 一致記録だけで route する。記録無し・破損・旧 version は起動前に拒否。`browser-specialist` を既存 ACP harness の別 ID として設定可能にし、browser 要求専用にした。dispatch は設定済みの非 account-pool adapter を候補として worker に渡し、公開操作で失敗した backend から別 session の候補へ実行時 fallback する。機密能力は宣言せず、CredentialUse の経路を再試行しない。H3/auth_section の配線は変更していない。
- `scripts/browser-conformance.py` は agent-browser **0.38.1 の実 binary**を起動し、127.0.0.1 の HTML fixture を ACP・Claude・specialist の各 ID で同じ手順で実行した。scripted driver は open 後に SIGKILL され、別 process が同じ browser session を再開する。denied origin は shim で拒否し、snapshot refs、click 後の fixture POST、screenshot、download を event と server 観測から判定する。外部ネットワークへの fixture 要求は無い。
- 実行: `python3 scripts/browser-conformance.py --scripted --agent-browser <agent-browser-0.38.1 の絶対パス> --output-dir <この run の成果物ディレクトリ>/p4c-conformance-v4` → exit 0。各 ID で 7/7 case、policy violation 0、harness process crash からの recovery 1。機械記録はこの run の `p4c-conformance-v4/conformance.json` / `same-task.json`。初回は snapshot の `ref=e1` 構文を runner が `@e1` と誤読して失敗し、正規表現を修正して再実行した。
- **この scripted 実行は backend 適合の証拠にしない。** 三つの ID は同じ scripted driver を起動しており、ACP RPC・Claude CLI・specialist wrapper の実 harness protocol はまだ実行していない。runner は scripted 出力の `source` を別値にし、worker はそれを適合記録として読まない。`--backend-command` に実 backend の command を三件渡し、その実行結果で生成した ledger だけが routing に使える。したがって P4-C の受入 0/1 の実 backend 比較は残る。fake substrate を使う worker 試験は fallback の実経路・session 分離・拒否だけの証拠。
- 実 LLM 比較: この環境は `claude auth status` が loggedIn=true だが `opencode` CLI が PATH に無く、三 backend の同一 task 比較を実施できない。ACP/OpenCode と specialist 用 harness の認証が整った環境で、三つの `--backend-command ID=<JSON argv>` を指定して同じ runner を再実行する。各 command は環境変数 `CELERIS_BROWSER_CLI`・`CELERIS_BROWSER_ORIGIN`・`CELERIS_BROWSER_PHASE` を読み、`open` phase で local origin を開いて denied origin を試した後に SIGKILL、`resume` phase で snapshot refs・click・screenshot・download を同じ session で実行する。`same-task.json` の accepted・違反・復旧・費用・時間を記録し、worker に渡す適合 file には実 harness 実行の `source` を要求する。実 LLM が使える時は費用を各 harness の usage から記録する。これは ADR-0009 P-34 の残課題。
- 本番昇格・本番設定変更・内部 origin 追加なし。`CELERIS_BROWSER_CONFORMANCE_FILE` は本番に設定していないため、現在の本番 browser 起動は記録不足で拒否される。適合記録の実 harness 生成とその独立試験が残る。
- 検査: `cargo test --workspace` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all --check` と `git diff --check` → exit 0。workspace gate は実 agent-browser の scripted fixture を自動実行しないため、上記 runner の独立した exit 0 を併記する。`browser_specialist_provider_uses_configured_acp_harness`、`specialist_wraps_existing_harness_and_runs_same_browser_task`、`execution_fallback_uses_fresh_session_and_refuses_without_conformance`、機密承認非消費の API 試験は workspace gate で成功した。

## P4-C backend protocol 適合（run 01M3QJ5CY8366206MQ20A3RBPB）

- ADR-0106 の決定に従い、`scripts/browser-conformance.py --protocol-scripted` を追加した。Rust の `AcpAdapter`、`ClaudeCodeAdapter`、ACP を包む `BrowserSpecialistAdapter` をそれぞれ起動し、`scripts/browser-conformance-harness.py` の scripted LLM が ACP JSON-RPC または Claude stream-json を話す。実 harness protocol を通過した後に同じ browser CLI を操作する。従来の `--scripted` は `source=celeris-browser-conformance-scripted` のままで、routing は引き続き拒否する。
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
- 本番昇格・本番設定変更・内部 origin 追加はしていない。新しい設計は足していない（ADR-0109 の範囲内）。unwrap 不使用。

## P4-B 実攻撃試験（2026-09-30、WorkUnit attacks）

- 試験: `crates/task-worker/tests/browser_injection_attacks.rs`（外側 1 本が `unshare --user --map-root-user --net` で内側を起動し、実 chrome-headless-shell を bwrap 隔離 runtime で 1 回起動、実 broker `injection.sock`（`Admission::SameUidHarness`）と自己署名 HTTPS fixture・試験用 DNS で A1〜A17 を順に実行）。外部ネットワークに出ない。
- 検査面: 各攻撃で sentinel とその percent・hex・UTF-16LE・base64（3 offset）を、agent への CDP 応答・receipt・broker IPC 応答・`journal.jsonl`・子 worker process の stdout/stderr・screenshot の PNG bytes と base64・内側 process の出力で検索。
- 証拠コマンド: `cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → exit 0、2 passed / 0 failed。`cargo test -p task-worker --test browser_injection_wire` → 2 passed。`cargo clippy --workspace -- -D warnings` → exit 0。

| # | 観測 |
|---|---|
| A0 正例 | 注入成功、fixture input が sentinel を受け取り（長さ・文字コード和で照合）、区間終わりで空 |
| A1 | 照合用 id 取得後に別 origin へ遷移 → `target_mismatch`、別 origin の input は空、lease 未消費 |
| A1 target_changed | **合格（2026-09-30、WorkUnit attacks-a1a4）**: `CdpController` に試験専用 hook（feature `attack-test-hooks`、task-worker 自身の dev-dependency 経由でのみ有効化。本番経路には現れない）を追加し、全 controller 照合・broker のコマンド frame 到着の後、`Runtime.callFunctionOn` 書き込みの直前にページを別 origin へ遷移させる。broker は `target_changed` を返し（journal に記録）、lease は消費済み（replay で `lease_used`）、別 origin 側の input は空のまま。controller は sink 応答失敗時に broker の裁定を `pending.finish` で報告するよう変更（既存の drop から変更）。`cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → `ATTACK-A1-TARGET-CHANGED-OK` |
| A2 | 同 origin の新 document → `target_changed`、新 document の input は空。`document.open()` は loader が変わらず同 origin・同 document への注入として成功（契約違反ではない） |
| A3 | 実 meta refresh redirect（other → fixture）→ controller・broker とも `redirected`、同じ lease で後に成功（未消費） |
| A4 | iframe 指定 → `cross_origin_frame`（controller・broker raw 要求とも）、別 target → `target_mismatch`、空 frame_chain → `empty_frame_chain` |
| A4 OOPIF | **合格（2026-09-30、WorkUnit attacks-a1a4）**: fixture に別 site ホスト `login.example.net`（試験用 DNS + egress allow）を追加し、`oopif.html` がそれを埋め込む。runtime は `--site-per-process` で起動し、OOPIF は独立 CDP target（type `iframe`、`Target.setAutoAttach` 経由）になる。その target への注入は controller・broker（実 OOPIF target/session）の両方で `target_mismatch` として拒否され、OOPIF 側 input は空。`cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → `ATTACK-A4-OOPIF-OK`（実 chrome-headless-shell、実ブラウザでの再現） |
| A5 | 逆向き iframe → `target_mismatch` / `cross_origin_frame` |
| A6 | `type=text`・script で type 書換え → `redisplay_field`、DOM 値は空 |
| A7 | 区間中の `Runtime.evaluate`・`DOM.getOuterHTML`・`DOM.getAttributes`・`DOM.getDocument`・`Accessibility.getFullAXTree`・`Runtime.callFunctionOn` → `auth_section_required`、区間後の値は空 |
| A8 | **合格（2026-09-30、ADR-0111）**: broker の `redisplay_guard`（salt・digest・len）を controller が保持し、区間後の agent 観測 8 経路（innerText・outerHTML・base64・percent・二重 JSON escape・UTF-16・`DOM.getDocument`・AX tree）は全て破棄され `redisplay_detected` が返る。screenshot に sentinel 無し。負の対照: guard を通らない `controller_command` では同じ経路の 6 件で sentinel が見える／別値の guard は検出しない（A8n）。`cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → 2 passed、`ATTACK-A8-OK`・`ATTACK-A8n-OK`。未解決: 画素上の再表示（OCR）・大小文字変換/圧縮等の表現は検出しない（ADR-0111 決定 5） |
| A9 | 区間中の `Page.captureScreenshot`・`Page.startScreencast` は拒否、区間後の screenshot（7103 bytes）に sentinel 文字列無し（OCR はしない） |
| A10 | page の `console.log(value)`・値入り例外は実行済み、`CdpController` は event を外へ出さず応答にも sentinel 無し |
| A11 | 実 python3 子 process が有効 lease・section つき要求 → `injection_worker_not_allowed`（SO_PEERCRED pid 不一致）、sink FD に frame 無し。`resolve.sock` → `trusted_injection_required`、bridge は固定拒否 |
| A12 | page から `fetch('file://…injection.sock'/'…resolve.sock')` は blocked、`/proc/<runtime pid>/root` に broker run dir・socket が無い（対照 `/session` は有る） |
| A13 | `InjectionService::handle` に別 UID の `PeerCred` → `peer_uid_mismatch`（関数単位。`unshare -r` の子は host で同 UID なので実 process 試験は未） |
| A14 | 別 session として登録した controller 子 process が session A の lease で要求 → `injection_worker_not_allowed`、未登録 session → `session_not_live` |
| A15 | 区間を開く前・閉じた後 → `auth_section_required`（controller・broker とも）、lease 未消費 |
| A16 | 成功後の再要求 → `lease_used`、sink FD に 2 本目の frame 無し |
| A17 | `value`/`length` field を含む要求 → `invalid_request` |

- 未解決（2026-09-30 時点、解消は下記「P4-B A1/A4 再現の解消」参照）:
  - A8: ADR-0111 で解消（`redisplay` WorkUnit）。guard は `InjectionReply.redisplay_guard` で controller に渡り、`agent_command`・relay・event を検査する。
  - ~~A1: 「controller の照合後・`Runtime.callFunctionOn` 前」の競合を決定的に作れず、broker の `target_changed`（lease 消費後の拒否）経路は実 browser で未再現（stale id は controller 側で止まる）。~~ **解消（2026-09-30、試験 hook で決定的に再現）**
  - ~~A4: fixture の 2 host が同一 site のため OOPIF にならず、OOPIF の `target_mismatch` は未再現。~~ **解消（2026-09-30、実 browser の cross-site OOPIF で再現）**
  - A13: 別 host UID の実 process による試験は、別 UID が使える host が必要。**未解決のまま**（この host は同一 UID のため `unshare -r` の子でも host からは同 UID に見える。`PeerCred` を与える関数単位の試験のみ）。
  - `cargo clippy --workspace --all-targets -- -D warnings` は既存の `task-worker/src/browser_tests.rs`（`await_holding_lock`）と `task-dispatch`（`type_complexity`）で exit 101（この unit の変更と無関係）。

## P4-B attacks/h3-prod merge の compile 不整合修正（2026-09-30、WorkUnit attacks-merge）

- 経緯: h3-prod（`ADR-0110` D2, 管理者 site policy の trusted login）を merge commit `717c7733` で取り込んだ結果、`CredentialPolicy` に `login_url`/`password_selector`/`submit_selector` が追加され、攻撃試験 fixture 側の初期化（`crates/task-worker/tests/browser_injection_attacks.rs:247` の `grant()`）がこれらを持たず `cargo test --workspace` が E0063（exit 101）で失敗していた。
- 修正: `grant()` の `CredentialPolicy` に fixture の login URL と selector（`login_url: Some(format!("{ORIGIN}/login.html"))`・`password_selector: Some("#pass".into())`・`submit_selector: None`）を追加。攻撃試験の期待値・A8 所見・A0〜A17 の判定条件は変更していない。新しい `selector_mismatch` 照合で既存攻撃が意図と違う理由で拒否される事象は無かった（`real_browser_injection_attack_matrix` は元の行列どおり通過）。
- 併せて `crates/task-worker/src/browser_credential.rs` の未使用コードを削除: 旧 `use_credential`（`auth login` + 旧 bridge、H3 経路から外れ `#[cfg(test)]` 専用のまま残っていた）と、それが使っていた `top_level_origin`・`celeris_credentiald::canonical_origin` の再 import・`Segment::origin` フィールドを削除（`Segment` は現在 CDP sink 経路のみで使い `origin` を読まない）。本番コード（H3 経路・ADR-0110 の挙動）は変更していない。
- 証拠コマンド:
  - `cargo test -p task-worker --test browser_injection_attacks --test browser_injection_wire --test browser_cdp_sink --test browser_h3_wire --test browser_shared_cdp` → 全 5 バイナリ exit 0（各 **2 passed / 0 failed**、`browser_injection_attacks` は `inner_injection_attacks`・`real_browser_injection_attack_matrix` とも skip 無し）。
  - `cargo test --workspace` → exit 0（212 + 618 他、全クレート `0 failed`、doctest 含む）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `cargo fmt --all --check` → exit 0。
- 未解決: A8（RedisplayGuard 未配線）・A1 の stale-id 競合再現・A4 の OOPIF・A13 の別 UID 実証は上の節のまま未達。次段は `redisplay` WorkUnit（RedisplayGuard を controller の agent 観測経路へ配線）。

## P4-B 判定と適合記録による機密能力の条件付き解放（2026-09-30、WorkUnit unlock、ADR-0112）

- 判定: **P4-B trusted injection は局所 fixture 上で合格**。attacks（A0〜A17、全 22 印 OK、A8 は ADR-0111 の RedisplayGuard 配線で合格・`A8-GAP` 無し）、h3-prod（`production_h3_injects_once_without_exposure`・負の対照 `injected_leak_is_caught_by_the_same_scanner`）、redisplay（A8）が同一 workspace で全て通ったため、P4-C 適合記録に P4-B の実測証拠を載せる経路を作った。
- 実装: `ConformanceResult.evidence`（試験名・`passed|failed|not_run`）。`injection_attack_suite`・`auth_section_observation_stop` は要る試験名が全て `passed` の証拠があるときだけ通る。`scripts/browser-conformance.py --p4b-evidence` が実試験を走らせて証拠を書き、1 件でも不合格なら件を外す。静的な適合登録は無し。`isolated_runtime_ready` を routing と別に判定。
- 本番: ADR-0110 のとおり本番 broker の admission は `Attested` のみで、この host は同一 UID（`SameUid`）のため機密起動は本番経路で拒否のまま（変更なし）。本番未昇格。この run では本番 ledger を生成・配置していない。
- 試験:
  - `browser_backend::tests::p4b_cases_count_only_with_measured_evidence`（件名だけ・印 1 つ欠け・failed/not_run 混入・別件の証拠では `CredentialInjection` が Missing）
  - `browser::tests::credential_use_is_released_only_by_p4b_evidence_in_the_ledger`（証拠なし ledger で CredentialUse の routing 拒否、証拠つきで acp のみ解放、1 印 failed で再び拒否）
  - `browser::tests::released_ledger_still_refuses_unconformant_backend_and_unisolated_runtime`（解放後も記録の無い claude-code・browser-specialist・未知 backend は拒否、隔離 runtime 未設定・bwrap 不在・resolver 無しは `isolated_runtime_unavailable`）
  - `browser_h3_injection` の ledger fixture を証拠つきに更新（証拠なしなら routing で落ちる）
- 証拠コマンドと結果:
  - `cargo test --workspace --no-fail-fast` → exit 0、3047 passed / 0 failed / 11 ignored。`real_browser_injection_attack_matrix ... ok`、`production_h3_injects_once_without_exposure ... ok`、`injected_leak_is_caught_by_the_same_scanner ... ok`、上記新規 3 試験 ok。
    - 直前の 1 回目（fail-fast）は `celeris --test instance_handoff` の 3 件（release handoff のタイミング試験、本変更と無関係）が高負荷で落ちて exit 101。再実行で通過。
  - `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all --check` → exit 0。
- 未解決（2026-09-30、ADR-0112 unlock 時点。A1/A4 は下記「P4-B A1/A4 再現の解消」で解消済み）:
  - ~~A1: controller 照合後・`Runtime.callFunctionOn` 前の競合（broker の `target_changed`）は実 browser で未再現。~~ **解消**
  - ~~A4: fixture の 2 host が同一 site のため OOPIF にならず、OOPIF の `target_mismatch` は未再現。~~ **解消**
  - A13: 別 host UID の実 process からの呼び出しは未試験（`PeerCred` を与えた関数単位のみ）。**未解決のまま**。
  - 本番 admission は `Attested` 必須、同一 UID host では拒否のまま（ADR-0110）。別 UID/attested runtime が要る。
  - P4-A の `isolation_suite`・`egress_negative_suite` は件名のみで証拠化していない。runner の `--p4b-evidence` を実 ledger に対して走らせた記録はまだ無い（運用者が本番 ledger を作るときに実行する）。
  - `IdentityRestore` は `certify` 上は同じ証拠要件だが、worker の routing が要求する起動前能力は `CredentialUse`→`CredentialInjection` のみ。

## P4-B integrate-gate 失敗の再検査（2026-09-30、WorkUnit gate-recheck）

- 対象 tree: `21604dce`。integrate-gate が落ちた `67ef0515`（integrate wu/unlock）と `git diff --stat 21604dce 67ef0515` は差分ゼロ（同一 tree）。
- 結論: **コード起因の失敗・タイミング依存の失敗は再現しなかった。原因は環境（sandbox 内の sccache）と判断する。** コード・試験の期待値・攻撃の合否基準・本番の拒否の意味・ADR-0112 の解放条件は変更していない（この unit の変更はこの節のみ）。
- 環境要因の根拠: 工程の artifacts に残る gate の log（`cargo-test-workspace.log`・`cargo-clippy-workspace.log`）は、どちらもコンパイル前の `sccache rustc -vV` で `sccache: error: Operation not permitted (os error 1)`（exit status 2 → cargo exit 101）で終わっている。これは試験の結果ではなく、sandbox 内から sccache server（socket・`/var/lib/celeris/scratch` 配下）に触れられないために起きる。同じ場所の `*-unsandboxed.log` にある E0063（`browser_injection_attacks.rs:247` の `CredentialPolicy`）は 13:08 時点の古い tree のもので、`1810384a`（attacks-merge）で解消済み。
- 再現手順（環境要因）:
  1. Celeris が渡す `RUSTC_WRAPPER=/var/lib/celeris/scratch/bin/sccache` と `CARGO_TARGET_DIR` のまま、agent の Bash sandbox の中で `cargo test --workspace` または `cargo clippy --workspace -- -D warnings` を実行する。
  2. `error: process didn't exit successfully: .../sccache .../rustc -vV (exit status: 2)` と `sccache: error: Operation not permitted (os error 1)` で exit 101 になる。
  3. 同じコマンドを sandbox 外（同じ環境変数、`RUSTC_WRAPPER` を unset しない）で実行すると下記のとおり exit 0。
  - 対処: gate 検査は sandbox 外で実行する（`RUSTC_WRAPPER`・`CARGO_TARGET_DIR` は上書きしない）。
- 証拠コマンドと結果（すべて sandbox 外、Celeris が渡した `CARGO_TARGET_DIR`・`RUSTC_WRAPPER`）:
  - `cargo test -p task-worker --test browser_injection_attacks -- --nocapture` → exit 0、2 passed。`ATTACK-A8-OK page-copied value discarded with redisplay_detected on 8 agent paths`、`ATTACK-A8n-OK`、`FINDING-A8` 無し。
  - `cargo test -p celeris-credentiald` → exit 0（15 + 12 + 14 + 5 passed、0 failed）。
  - `cargo test -p task-worker --test browser_cdp_sink --test browser_injection_wire --test browser_h3_wire --test browser_shared_cdp` → exit 0（各 2 passed）。
  - `cargo test -p task-api --test browser_h3_injection` → exit 0（3 passed）。
  - 負荷下の反復: `cargo test --workspace --no-fail-fast` を並行実行しながら、attacks（`--nocapture`）・`task-api browser_h3_injection`・`browser_shared_cdp`+`browser_cdp_sink` を各 **5 回**直列に実行 → **15/15 回 exit 0**、attacks は 5 回とも `ATTACK-A8-OK` 1 件・`FINDING-A8` 0 件。並行の workspace test も exit 0（3047 passed / 0 failed / 11 ignored）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
- flaky: 実 browser 試験では検出されず、修正は入れていない。前節（unlock）に記録した `celeris --test instance_handoff` の高負荷時の失敗は今回の負荷下 workspace test でも再発しなかった（P4-B 範囲外、未修正のまま）。
- 未解決: integrate-gate の失敗そのものの log（14:35 以降）は工程 artifacts に残っておらず、sccache 以外の原因を完全には排除できない。gate を sandbox 外で再実行すれば通る見込み。A1・A4・A13 と本番 admission（`Attested` 必須）は前節のまま。

## P4-B gate-recheck 再試行（2026-09-30、run 01M3SEAJ）: `browser_runtime_supervisor` の 2 つの競合を修正

- 前節の結論（環境要因のみ）は不十分だった。前回 run 後の check `cargo test --workspace && cargo clippy --workspace -- -D warnings` が exit 101 で落ち、原因は `task-worker --test browser_runtime_supervisor` の `runtime_processes_do_not_survive_controller_kill_restart_or_stop`（`not alive: RecordedProcess { role: "egress" }`）だった。sccache ではない。
- 原因 1（試験側）: 記録には接続ごとの egress が載り、supervisor は 100ms ごとに採り直す。browser が fixture 取得に使った egress が `CTRL READY` の直後に終わると、書き直される前の記録に死んだ egress が一時的に残る。親はその記録を 1 回だけ読み、「全員生存」を即座に assert していた。修正: `read_full_runtime` で「4 役が揃い、載っている全 process が生存」を条件に（上限 30 秒）記録を読み直す。上限を過ぎた場合は従来どおり `assert_full_runtime` で判定する。期待値（4 役が記録されていて全員生存）は変えていない。
- 原因 2（本番の競合、ADR-0108 追記）: 負荷下で反復すると 8 回中 1 回、(c) 正常停止が `CTRL STOPPED left=[bwrap-init]` で落ちた。`stop_runtime` は外側の bwrap を回収したところで戻っていた。ところが pid namespace の init（`bwrap-init`）とその下は、PDEATHSIG と namespace の後始末で非同期に消える。そのため `stop()` の「戻った時点で残存 0」が破れることがあった。修正: bwrap と egress を回収した後、記録にある本人（pid+starttime）が全て消えるまで `STOP_GRACE` を上限に待つ。signal は追加で送らない。停止の意味・signal の順序・起動時回収・記録の形式は変えていない。
- 変えていないもの: 攻撃試験の期待値と合否基準、ADR-0112 の解放条件、本番の拒否の意味。
- 証拠（すべて Celeris が渡した `CARGO_TARGET_DIR`・`RUSTC_WRAPPER`）:
  - 修正前の負荷下反復（`cargo test --workspace` を並行実行）: supervisor 試験は 8 回中 7 回 exit 0、1 回が上記 (c) で失敗。attacks・`browser_shared_cdp`・`browser_cdp_sink`・`task-api browser_h3_injection` は各 5 回で 20/20 回 exit 0。attacks は毎回 `ATTACK-A8-OK` 1 件、`FINDING-A8` は 0 件。
  - 本番修正後の負荷下反復: `cargo test -p task-worker --test browser_runtime_supervisor` を 10 回 → 10/10 回 exit 0。並行した `cargo test --workspace` も exit 0。
  - gate: `cargo test --workspace && cargo clippy --workspace -- -D warnings` → exit 0（112 個の test binary、3047 passed、0 failed。clippy は警告 0）。
  - `browser_cdp_sink`・`browser_injection_wire`・`browser_h3_wire`・`browser_shared_cdp` → 各 exit 0、2 passed。`browser_injection_attacks -- --nocapture` → exit 0、`ATTACK-A8-OK` あり、`FINDING-A8` 無し。`celeris-credentiald` → exit 0。`task-api --test browser_h3_injection` → exit 0、3 passed。
- 未解決: 無し（前節の `instance_handoff` の高負荷時失敗は今回も再発しなかった）。

## P4-B A1/A4 再現の解消・P4-A deliver_state・P3-C control gate 配線の統合検査（2026-09-30、task 01M3SM0WN346ABGF42QV02RZTP、WorkUnit record）

final review が挙げた 3 件の未達（P3-C の run loop 配線、P3-A/P4-A の deliver_state、P4-B の A1/A4 実再現）を実装した 3 つの WorkUnit（control-gate・deliver-state・attacks-a1a4）を統合したブランチで、ワークスペース全体の検査と各 unit の証跡突き合わせを行った。コードは変更していない（record は検査と文書更新のみ）。

- 統合 commit: `4250befb`（integrate wu/attacks-a1a4）→ `beca61e5`（integrate wu/control-gate）→ `e5c1db5d`（integrate wu/deliver-state、HEAD）。
- ワークスペース全体検査（すべて Celeris が渡した `CARGO_TARGET_DIR`・`RUSTC_WRAPPER`、sandbox 外）:
  - `cargo test --workspace` → **exit 0、3055 passed / 0 failed / 11 ignored**（114 test binary、doctest 含む）。各 unit の result.json が記録した試験数（deliver-state: task-worker lib 620・browser_restore_deliver 3／attacks-a1a4: 3047 passed）と整合し、統合後は単体試験の重複無く合算されている。
  - `cargo clippy --workspace -- -D warnings` → **exit 0**、警告 0。
- 各未解決行の個別再検査（統合ブランチ上、抜粋の再実行で確認）:
  - P3-C control gate 配線: `cargo test -p task-worker --test browser_control_gate_wire -- --nocapture`（実 SQLite store + 実 shim）→ exit 0、**5 passed**（`gate_wire_human_control_blocks_agent_until_resume`・`gate_wire_auth_section_blocks_agent_until_left`・`gate_wire_pause_converges_after_in_flight_then_blocks`・`gate_wire_stopped_closes_session_once_and_never_runs_actions`・`gate_wire_running_actions_reach_the_browser`）。本番経路の `ActionServer::serve` を通す（テスト専用の未配線経路ではない、ADR-0113）。
  - P3-A/P4-A deliver_state: `cargo test -p task-worker --test browser_restore_deliver -- --nocapture`（実 bwrap + 実 chrome-headless-shell）→ exit 0、**3 passed**。試験 admission（`same-uid-harness` feature）での成功経路（開封 → controller CDP への `Storage.setCookies` → `Storage.getCookies` で sid=secret を確認）と、本番 `Attested` admission での `SameUid` 拒否（開封 0 回）、他 project/別 origin/期限切れ/削除/別 session の開封前拒否を確認。
  - P4-B A1/A4: `cargo test -p task-worker --test browser_injection_attacks -- --nocapture`（実 chrome-headless-shell、cross-site OOPIF fixture）→ exit 0、2 passed。出力に `ATTACK-A1-TARGET-CHANGED-OK`（試験 hook で controller 照合後・`Runtime.callFunctionOn` 前の遷移を決定的に発生させ、broker `target_changed`・lease 消費済み・他 origin input 空を確認）と `ATTACK-A4-OOPIF-OK`（実 cross-site OOPIF target で controller・broker 双方 `target_mismatch`、OOPIF input 空）を確認。既存 A0〜A17 の判定は変更なし（`ATTACKS-ALL-DONE`）。
- A13（別 UID の実 process による `peer_uid_mismatch` 試験）は本 run でも未解決のまま残す。この host は同一 UID のため `unshare -r` の子も host からは同 UID に見え、別 UID の実 process を作れない（別 UID が使える host が必要）。本番 admission（`Attested`、同一 UID host では `SameUid` 拒否）も変更していない。
- コード変更なし。H3 観測停止・approve_once・短い lease・ADR-0102 D6 の起動前拒否は変更していない。本番昇格・本番設定変更・内部 origin 追加はしていない。

## 統合記録（2026-09-30、task 01M3PAX6RVE7AX8Z6118KADME3、WorkUnit land2）

`land` branch の統合 commit `4d65de6e` をこの worktree に fast-forward で取り込んだ。p4a・p4c・p4b・gaps・closeout の統合 commit は次のとおり。各 SHA と `celeris/01M3SPF94RDWTPWHNDEQD68VB9`、`celeris/01M3SHZGWGKG2VPP0G5DHG92C4`、`4d65de6e` について `git merge-base --is-ancestor <sha> HEAD` を実行し、すべて exit 0 を確認した。

| 子 WorkUnit | 統合 commit | メッセージ |
| --- | --- | --- |
| p4a（isolated runtime） | `08022768` | integrate wu/p4a (phase phase-4) |
| p4c（backend 適合 runner・fallback） | `44eab893` | integrate wu/p4c: resolve merge conflicts (browser.rs attempt-aware session on isolated runtime, docs union) |
| p4b（stronger injection） | `99d5d0bf` | integrate wu/p4b (phase phase-4-inject) |
| gaps（celeris/01M3SPF94RDWTPWHNDEQD68VB9） | `ac65208f` | integrate wu/h3-doc-sync (phase h3fix) |
| closeout（celeris/01M3SHZGWGKG2VPP0G5DHG92C4） | `92126674` | integrate wu/record (phase record) |

追跡表 `phase-browser-acceptance.md` と合わせ、P4-A/B/C は一部達成、別 host UID 実証と本番機密能力解放は名前付き後続 task とする。本番 admission は `Attested` 必須であり、この host の SameUid は引き続き拒否する。P4-A/B/C の行別判定と制約は上記追跡表を正とする。
# ADR-0115 権限分離 launcher（run 01M3X8SRB3X08AXW8WK5PY7P9N、2026-10-02）

- 実装統合: `celeris-browser-launcher` binary と固定 IPC、`SO_PEERCRED` 検査、session registry/回収、launcher 所有 user namespace の UID/GID map、daemon 側 runtime 選択と receipt を統合済み。従来 daemon 所有 runtime は既定経路として維持し、launcher は設定で選択する。root 配置用 socket/service unit と host 準備手順は [browser-launcher-host-setup.md](../ops/browser-launcher-host-setup.md) に記載。
- launcher 実 process 試験: `CELERIS_ISOLATION_TESTS=skip cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` → exit 0、1 件は `SKIPPED (not passed): celeris-browser user is absent`。この環境には `celeris-browser` user がなく、`celeris-browser-launcher.socket` unit も見つからない。従って Chrome の namespace owner・daemon UID 1001 からの ptrace/proc 読取り拒否は実証していない。
- 全体検査: `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo test --workspace` は exit 101。再試行 `CELERIS_ISOLATION_TESTS=skip cargo test --workspace` も exit 101。sandbox で `unshare -Ur` が `EPERM` となり、`celeris --test instance_handoff` 5 件が失敗。ログは ADR-0095 worker DB guard の namespace probe が `Operation not permitted` と報告し、残る handoff 試験もその結果として期待する dispatch/standby 状態に到達しなかった。browser の `CELERIS_ISOLATION_TESTS=skip` はこの daemon DB guard を無効化しないため、workspace test 合格とは扱わない。
- 未解決・依頼: host 管理者に上記手順書に沿った専用 user/subuid/subgid・root 所有 binary・systemd socket/service の準備を依頼する。準備後に skip なしで `browser_launcher_ptrace` を実行し、`NS_GET_OWNER_UID`、`uid_map`/`gid_map`、ptrace attach と `/proc/<pid>/{environ,mem}` の拒否、`verify_isolation` の成功を記録する。workspace test は user namespace 利用可能な環境で再実行が必要。`CredentialInjection` と `IdentityRestore` は未解放、本番昇格・設定変更なし。

## launcher 実 process 実証の再試行（run 01M3X9XQPGTKXCQ3X94NKX9RTT、2026-10-02）

指定された `CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` を UID 1001 の本 worktree で実行した。**exit 101、0 passed / 1 failed**。試験は `launcher test environment required: celeris-browser user is absent` で起動前に停止した。`getent passwd celeris-browser` は該当なし、`/run/celeris-browser/launcher.sock` は存在せず、`systemctl status celeris-browser-launcher.socket` と `.service` はいずれも `Unit ... could not be found`。この実行環境の `/proc/self/uid_map` は `1001 0 1` で、手順書が要求する初期 user namespace の root host ではない。host 本体の準備状態まではこの環境から断定できない。

この試行では Chrome session に到達していないため、`NS_GET_OWNER_UID`、Chrome の `uid_map` / `gid_map`、`PTRACE_ATTACH` / `strace -p` の errno、`/proc/<pid>/{environ,mem}` の結果、`verify_isolation` の結果は**未取得**。合格の証跡として扱わない。手順書に実装が必須とする `session_root`、`agent_browser`、`resolver` と私有 session dir の作成を明記した。管理者が [host 準備手順](../ops/browser-launcher-host-setup.md)を初期 namespace の host 上で終えた後、UID 1001 が同じ launcher socket に接続できる実行環境から指定コマンドを再実行し、上記の各値と exit 0 をここに追記する。root 操作、本番 DB・設定、昇格、機密能力の解放は行っていない。
