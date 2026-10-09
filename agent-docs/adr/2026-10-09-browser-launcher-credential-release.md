# ADR 2026-10-09: launcher 経路の browser CredentialUse 解放

---
tasks: [01M4FRN13Z206RAWBZ0NHZQEGX]
---

- 日付: 2026-10-09
- 状態: 採用（launcher session に限る CredentialUse の条件と経路。実装・台帳更新・host 実証は後続工程）
- 決定者: 人（2026-10-09、manaba 課題監視 task 01M4FPA6ADFH639XYP894ES87J）
- 関連: [ADR-0116](0116-browser-launcher-implementation.md)、[ADR-0138](0138-browser-prod-admission-confidential-release.md)、[ADR-0080](0080-browser-phase2-policy-broker-approval.md)、[credential 台帳証拠 ADR](2026-10-09-browser-ledger-credential-evidence.md)

## 決定

launcher runtime で `CredentialUse`（必須能力 `CredentialInjection`）を使えるのは、同じ稼働 session について以下のすべてが成立した場合だけとする。

1. ADR-0138 D-L の `LauncherSessionProof` がある。
2. 証明の検証が成功し、launcher UID・session/instance・PID/starttime の束縛が一致する。
3. user namespace owner が daemon UID ではない。
4. `isolation_ok` が真で、実行時の隔離事実の採取・検証にも成功する。

一つでも欠ける、検証・IPC・採取が失敗する、launcher が無い場合は browser session 接続・credential 操作より前に拒否する。daemon runtime や別 backend へ fallback しない。拒否は理由を含む安全な固定 code とし、秘密・CDP payload は log、event、artifact、LLM 観測へ出さない。既存の承認・短い lease・origin/policy binding、H3 観測停止、credential lifecycle と cleanup は維持する。

## 証明の搬送と秘密の経路

launcher worker は既存の launcher IPC 応答から `LauncherSessionProof` を検証可能な形で得る。worker は credentiald の injection IPC にある `LauncherProofRegistration`（`session_id`, `instance_id`, `peer_uid`, `proof`）として登録し、broker の `LiveRegistry` に session 登録と結び付ける。credentiald は `Admission::Attested` の既存経路で `/proc` 等から実 process facts を採り直し、既存 `admit_attested` と `verify_launcher_session` を再利用する。登録された証明自体を信頼の根拠として無条件に受け入れない。証明なし・不一致・launcher UID 未設定・検査失敗は拒否する。daemon runtime は従来どおり証明なしで拒否される。

credentiald が secret を扱い、launcher 側の trusted controller / CDP sink が browser へ注入する。task controller、LLM、一般 action IPC に secret を返さない。注入は固定の認証操作として CDP の認証 sink へ渡し、既存の redaction と auth interval の遮断を適用する。controller は結果 status のみを受け取る。登録 IPC に含めるのは session 証明だけで credential 値は含めない。

## IdentityRestore

この決定では `IdentityRestore` を解放しない。restore は保存 identity の選択・復元とその後の session 全体にわたる観測停止、project/origin scope、期限・失効を伴う別の機密経路であり、今回の目的である手動承認付きの一回の login 注入からは必要性も lifecycle の適合も導けない。launcher の同じ隔離条件は将来の必要条件として維持するが、それだけで restore を許可しない。restore の解放は専用判断とその証拠を要する。

## 台帳と host 条件

`credential_backends` の credential 対応は、backend ID だけでなく証拠を生成した runtime（daemon / launcher）で区別する。`isolation_suite`・`egress_negative_suite` は launcher runtime の実証証拠、`injection_attack_suite`・`auth_section_observation_stop` は daemon runtime の P4-B 実証証拠を各 case ごとに要求する。いずれも指定 runtime と一致しない証拠を代用しない。launcher が無い host は launcher credential evidence を生成せず、理由を記録して credential を空にする（非機密 capability の台帳結果は独立に扱う）。

機密経路の試験名は `launcher_credential_`、台帳試験名は `browser_ledger_launcher_` を接頭辞とする。

host 実証（`CELERIS_LAUNCHER_TESTS=require` の必須モード stutter 3 回、および統合後 HEAD で `ADMISSION[real-session]` を再取得）は運用セッションが root で行う。本 ADR の採用や合成試験だけで host 実証済み・本番有効とは扱わない。既存の運用手順を更新し、結果と HEAD を記録してから人が本番昇格を判断する。

## 既存 ADR への付記

- [ADR-0116](0116-browser-launcher-implementation.md) の末尾付記を参照。
- [ADR-0138](0138-browser-prod-admission-confidential-release.md) の末尾付記を参照。
- [ADR-0080](0080-browser-phase2-policy-broker-approval.md) の末尾付記を参照。

## 実装付記（launcher-credential）

実装は `CredentialUse` だけを解放し、`IdentityRestore` は引き続き拒否する。認証情報を CDP 操作の主体へ返さないため、credentiald の `LiveSessionRegistration.controller_pid` / `controller_start` は daemon worker の process facts とし、`runtime_pid` / `runtime_start` は launcher が隔離検査した browser runtime の process facts とする。注入要求を受ける injection IPC peer はこの controller である。launcher 自身は CDP の `SharedCdp` / `CdpController` を所有するため、daemon controller が秘密を受け取らず CDP command を launcher に依頼する中継を追加する。credentiald broker は `with_launcher_uid` で設定された launcher UID を launcher peer として認証し、daemon UID の controller に secret frame を返さない。Attested admission ではこの launcher peer に加え、`LauncherProofRegistration` と実 process facts の照合を必須にする。

daemon→launcher の protocol には固定 `Verb::Authenticate` と対応する固定引数型を追加する。引数は `session_id`、`auth_section_id`、`lease_id`、`origin`、`target` のみで、selector や credential 値を含めない。`Response` は固定 `status`（成功/拒否）だけを返し、自由文、receipt、CDP payload を返さない。launcher は自身の `SharedCdp` / `CdpController` を使い、既存の `browser.rs::inject_h3` および `browser_cdp_sink.rs` の認証 sink に接続する。credential 値は credentiald から launcher が所有する注入 sink へ一方向に渡し、一般 action 経路や daemon worker へ戻さない。

`LauncherRuntime::start_guarded` は既存の Started 応答を `launcher_session_proof` で検証し、同じ接続の responder UID を保持する。隔離検査を通過した後、daemon worker が broker client の `register_live_session` で `LiveSessionRegistration` を登録し、得られた `LauncherSessionProof`、`session_id`、`instance_id`、応答の `SCM_CREDENTIALS` 由来 `peer_uid` を `LauncherProofRegistration` として `attach_launcher_proof` する。proof が無い、UID を採れない、登録に失敗した場合は launcher session を使った credential 操作を開始しない。credentiald の `LiveRegistry::attach_launcher_proof` は稼働 session に一度だけ結び付け、`injection_ipc.rs::admit_attested` が実 process facts に対して `verify_launcher_session` を再実行する。

`browser_launcher_run.rs::refuse_confidential` は、proof の検証成功、namespace owner が daemon UID と異なること、`isolation_ok` が真であることの全条件を満たす場合だけ `CredentialUse` を許す。どれかが不成立・取得不能・IPC 失敗なら、接続前に従来どおり `browser credential use is not available through the launcher runtime` で拒否する。承認待ちを含めて fail closed とし、daemon runtime への fallback はしない。shim の `config.json` は解放条件成立時に限り `credential_use: true` と有効な `credential_policy_ids` を設定し、それ以外は従来どおり `false` と空配列にする。`browser.rs::inject_h3` の承認・短期 lease・origin/policy binding と `CdpController` の認証区間観測遮断は維持する。

実装試験は以下を追加し、secret が log/event/artifact/stdout に出ないことも各関連経路で固定する。launcher の protocol・実 process session は userns opt-in 試験に分離し、外部ネットワークや CPU 負荷試験は使わない。

| ケース | 主な crate / 箇所 |
| --- | --- |
| proof ありで登録・注入を許可、proof なし・偽造・期限切れ・UID 不一致を拒否 | `celeris-credentiald` `injection_ipc.rs`（`admit_attested` / `verify_launcher_session`）、`task-worker` `browser_launcher_run.rs` |
| namespace owner が daemon UID の session を拒否 | `task-worker` `browser_launcher_run.rs`（`launcher_session_proof` / `refuse_confidential`） |
| `isolation_ok` 不成立・証明採取失敗・登録 IPC 失敗で接続前に拒否 | `task-worker` `browser_launcher_run.rs`、`browser_launcher/` |
| 固定 `Authenticate` verb は許可された引数だけを受け、応答は status のみ | `task-worker` `browser_launcher/protocol.rs`・`backend.rs` |
| secret 非露出（log/event/artifact/stdout）と 인증 sink の遮断 | `task-worker` `browser.rs`（`inject_h3`）、`browser_cdp_sink.rs`、`browser_launcher/`、`celeris-credentiald` `injection_ipc.rs` |

各試験名は `launcher_credential_` を接頭辞とする。userns を要する実 process 試験は `CELERIS_USERNS_TESTS=1` の opt-in とし、通常試験では決定的な証明・登録・policy 分岐を検査する。

## 付記 2026-10-09: 接続前 gate

final review は「条件を満たさない session も launcher に接続・session 起動した後に拒否している」として差し戻した。launcher session 証明（ns owner・ns inode・responder UID）は launcher が session を作らないと採れないため、人の決定（preconnect-meaning = a）に従い、「接続前に拒否」を次の二段 gate として実装する。

1. **接続前 gate**（`browser_launcher_run.rs::preconnect_credential_gate`、`open_launcher_session` の最初）。CredentialUse を求める run（policy の `CredentialUse`、または最後の wait が credential_use の Registered / Approved）について、launcher socket の設定がある・`[browser] launcher_uid`（credentiald の `--launcher-uid` と同じ値）が設定され 0 でも daemon UID でもない・credentiald の制御経路（credentiald 設定の `runtime_dir`、無ければ `XDG_RUNTIME_DIR`）が取れる・wait store が読める、の全部を `LauncherRuntime::start_guarded` の `spawn_blocking` より前に検査する。不成立なら launcher socket に接続せず `browser credential use is not available through the launcher runtime` で拒否する。admission の対象にならないが admission 無しでは必ず拒否される run（credential 以外の登録済み wait、操作の無い承認）もここで拒否する。
2. **証明依存 gate**（session 起動・隔離検査の後、credentiald 登録より前）。`launcher_session_proof` の検証成功・ns owner が daemon UID でない・`isolation_ok`・証明の launcher UID が `[browser] launcher_uid` と一致、のどれかが欠ければ `runtime.stop()` で session を止めてから同じ文言で拒否する。credentiald への `register_live_session` / `attach_launcher_proof`、shim の `credential_use: true`、`Authenticate`、harness 起動はどれも起きない。credentiald 登録の失敗も stop してから同じ文言で拒否する。上の実装付記の `refuse_confidential` はこの二段（`credential_demand` で拒否対象を決め、`open_launcher_session` で判定）に置き換えた。

境界: 接続前に決まる条件（設定・UID 分離・credentiald 経路・wait store）は launcher に接続する前、証明に依存する条件は launcher session 作成後・credentiald / CDP / harness への接続と秘密の注入より前。秘密が launcher・Chrome に渡るのは両方の gate と credentiald の Attested admission を通った後だけである。CredentialUse を求めない run は接続前 gate を素通しし、`launcher_uid` 未設定でも従来どおり動く。

試験（`task-worker` の `launcher_credential_preconnect_`、偽 launcher は試験内の `UnixListener` / `LauncherServer`、userns・外部ネットワーク・CPU 負荷なし）: 接続前条件の不成立 5 通りと admission 不能な wait で launcher socket の接続数 0・従来文言、証明依存の不成立で session stop・credentiald の control socket 接続数 0、条件成立で launcher に接続、CredentialUse を求めない run は従来どおり。同一 process の偽 launcher では responder UID が daemon UID と同じになるため、admission 成立から credentiald 登録までの経路は host 実証（運用セッション）で確かめる。

## 人の決定（2026-10-09）: 接続前の意味（preconnect-meaning）と二段 gate の承認

- 論点 `preconnect-meaning`（「接続前」を何と解するか）は子 task `01M4FS9M260F7VBBPANZCF8M23` の計画で人に示された。
- 回答: 2026-10-09、運用セッション（人から受信箱の対応を任されている）が **a（二段 gate）** を選んだ。
- 回答の要旨: 接続前に決まる条件（設定・uid 分離・credentiald 経路・wait store）は launcher 接続前に拒否する。証明に依存する条件（証明の検証・owner≠daemon・isolation_ok）は session 作成後かつ credentiald 登録・shim 有効化・`Authenticate`・harness 起動より前に検査し、不成立なら session を止めて拒否する。上の「付記 2026-10-09: 接続前 gate」の二段がこれに当たる。
- この task の段の決定 `confirm-two-stage-gate`（二段 gate を『接続前』の意味として認めるか）に対し、2026-10-09 に **承認する**（推奨どおり、選択肢: 承認する / 不承認）と回答された。回答者は上記の運用セッションである。
- 不承認だった場合は、この付記を有効な決定としない。本節の記録と実装の扱いは、その時点で replan により決める。

## 付記 2026-10-09: launcher runtime の credential-request → WaitingForAuth wait（共有段）

理由: final review が「launcher runtime（`browser_launcher_run.rs::run`）は run 後に `approval-request.json` だけを読み、shim の `credential-request.json` を wait に変えない」ことを指摘した。このままだと launcher 経路の run は credential 登録を人に聞かずに終わり（`Completed` / `WaitingForHuman`）、`WaitingForAuth` の wait も `auth:<task>:<run>` の resume key も残らない。daemon 経路（`browser.rs`）には同じ意味の段が既にあったので、両経路が同じ関数を呼ぶ形にして差を無くした。

決定:

1. **wait の組み立ては 1 箇所**。`browser.rs::credential_wait(task_id, run_id, session_id, policy, &CredentialRequest) -> NewBrowserWait`（`pub(super)`）を approval の `operation_wait` と同じ置き方で追加し、`WaitingForAuth` wait（`credential_policy_id = Some(policy_id)`、`credential: None`、`resume_key = auth:<task>:<run>`、policy binding 固定）はここでだけ組む。daemon 経路の inline 組み立てはこれを呼ぶ形に置き換えた（挙動不変）。
2. **request file を見る段も共有**。`browser.rs::shim_request_wait(runtime, task_id, run_id, session_id, policy, sink, outcome) -> (outcome, Option<BrowserRunState>)` を追加し、daemon 経路（`run_with_executable_attempt`）と launcher 経路（`browser_launcher_run.rs::run`）の両方が run 後にこれを呼ぶ。
3. **順は daemon 経路に合わせる**。`credential-request.json` があればそれを先に処理して `WaitingForAuth` wait を開き、outcome は `Terminal::Question`、最終 `browser.state` は `WaitingForAuth`（launcher 経路では従来の `waiting_for_approval` フラグの代わりに `Option<BrowserRunState>` を state 決定に入れる）。`credential-request.json` が無いときだけ `approval-request.json` を見て `WaitingForApproval`（ADR 2026-10-08 D2）。この順はコードの comment にも書いた。
4. **fail closed**。`read_credential_request` が policy 不一致で `Err` なら wait を開かず `Err`、`sink.browser_wait_open` が失敗しても `Err`（resume できない Question を返さない）。
5. **秘密は wait・event・log に入れない**。wait に入るのは `origin` / `purpose` / `credential_policy_id` だけで `credential` は常に `None`。Question の文面も固定文（origin も秘密も含まない）。request の未知の欄は `deny_unknown_fields` で拒否し、拒否の文言に request の中身を写さない。

試験: `browser_launcher_run_tests.rs` に `launcher_credential_request_` 接頭辞で 5 件（wait が 1 件開く・resume key・Question・state、policy 不一致 4 通りの拒否、credential 優先と approval への fallback、wait store 不成立の fail closed、秘密文字列が wait / browser 更新 / progress / outcome に出ないこと）。偽 sink と tempdir の request file だけで回り、userns・実 launcher・実 process・socket・外部ネットワーク・CPU 負荷を使わない。進捗は `agent-docs/progress/2026-10-09-browser-launcher-credential-release/launcher-cred-wait.md`。

## 付記 2026-10-09: stutter 台本の signal 競合（ESRCH）

理由: 必須モード stutter の再実行で、試験 process group が終わる瞬間に `kill -STOP` / `kill -CONT` が ESRCH で失敗し、台本が `code=1` にして偽の失敗を出した（2 回中 1 回）。

決定:

1. **試験 group の終了と競合した signal の失敗（ESRCH）は失敗にしない。** その時点で stutter は終わっているので loop を抜けるだけにする。合否は従来どおり `stops>0` と `wait` の終了コードで決める。抜けた後の `kill -CONT` は残す（停止したまま残さないため）。
2. **pgid が取れる前に子が終わった場合は失敗のまま。** 停止が一度も live な試験に届いていないので、`stops=0` の回は証跡にならない。`STUTTER[stutter-N]: stops=0` を記録し `EXIT` は非 0 にする。これは signal の競合とは別で、偽の失敗ではなく停止が届かなかった回として扱う。
3. 台本は dash で動く形（`kill -STOP -"$pgid"`、`--` は使わない）のままとする。

試験: `crates/task-worker/scripts/tests/launcher-admission-evidence-stutter.sh` に、`LAUNCHER_EVIDENCE_TEST_CMD='sleep 0.3'` の `--stutter 3` を 10 回繰り返す段を足した（全回 exit 0、各回 `stops>0`、末尾 `EXIT: 0`）。sleep だけで CPU を使わず、launcher・userns・実 process は使わない。進捗は `agent-docs/progress/2026-10-09-browser-launcher-credential-release/stutter-race.md`。
