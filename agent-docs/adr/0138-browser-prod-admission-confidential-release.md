# ADR-0138: browser 本番 admission の機密能力解放条件

---
tasks: [01M3VFQZ2TX3W0KTDQHKCAVJR6, 01M3WV4BFJ71J9ZWJ020MP2Z4K]
---

- 日付: 2026-10-01
- 日付（更新）: 2026-10-02（launcher session 証明を必須条件に追加）
- 日付（更新）: 2026-10-02（子 task 01M3WV4BFJ71J9ZWJ020MP2Z4K で証明の必須化を実装し ptrace 拒否を実 process で実証。事実採取の経路を D-L 付記のとおり直した）
- 状態: owner 検査と launcher session 証明の検証・両 admission への必須化（fail-closed）は実装済み。launcher 経由の別 UID runtime への daemon UID からの ptrace 拒否は**実 process で実証済み**（sandbox・host 双方、証跡 `prod-admission-release-evidence.md` §2）。許可/拒否の対応表のうち synthetic（構造体を直接組んだ模擬観測）は単体・結合試験で実証済みだが、**実 session の表（`ADMISSION[real-session]`、launcher の実観測を本番入口に通した表）は未実証**（systemd socket activation 下で launcher の `SO_PEERCRED` が systemd を指すため。身元確認は応答の `SCM_CREDENTIALS` に替えた（§ D-L 付記、ADR-0116 launcher 実装の付記 D-P）。host での再取得は人が行う）。本番昇格は人が行う（未実施）
- 番号: 当初 ADR-0116 として起票したが、main の [ADR-0116](0116-browser-launcher-implementation.md)（launcher 実装）と重なったため 0138 に振り直した（2026-10-03）。旧番号の「ADR-0116 D-L」「ADR-0116 条件 1〜5」はこの ADR を指す
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md)、[ADR-0102](0102-browser-phase4-isolation-injection-routing.md)、[ADR-0109](0109-browser-p4b-injection-ipc-cdp-sink.md)、[ADR-0112](0112-browser-p4b-conformance-evidence-unlock.md)、[ADR-0115](0115-browser-ptrace-owner-ns-launcher.md)、[後続 task](../progress/browser-followups.md)

## 背景

別 host UID の browser runtime では Chrome 起動、`verify_isolation` の成功、`/proc/<pid>/environ` の読取り拒否と本番 `Attested` identity 復元が実証された。一方、daemon UID の独立 process から Chrome への `ptrace` attach は成功した。daemon が runtime の user namespace を作り、その owner UID になっていたためである（ADR-0115）。別 UID と既存の `verify_isolation Ok` だけをもって機密能力を本番で解放できない。

## 提案: 両 admission の共通条件

本番の `celeris-credentiald::Admission::Attested` による `CredentialInjection` と、`task-worker::RestoreAdmission::Attested` による `IdentityRestore` は、**同じ稼働中の隔離 session** について次の全条件を満たした場合に限り許可する案とする。現時点では owner 検査だけを実装しており、両能力を許す本番 session は無い。launcher session 証明（下記 5・D-L）の検証が両 admission に入り、launcher 経由での実 process ptrace 拒否が試験で実証されるまで解放しない。

1. 対象 runtime の実 process から採った `RuntimeFacts` に対し `verify_isolation` が `Ok(IsolationAttestation)` を返す。別 host UID（root でも daemon の `host_uid` でもない）、必要な user/pid/net/mount/ipc/uts namespace、読取専用 root、限定された書込み先、broker/host IPC の不可視、CDP の非 TCP 経路、`no_new_privs`、capability 全落とし、専用 process group など ADR-0102 D1 の全条件を維持する。違反を一つだけ無視して attestation を作らない。
2. `/proc/<runtime pid>/ns/user` の namespace FD に `ioctl(NS_GET_OWNER_UID)` を行い、**runtime の user namespace の owner UID** を得る。その値を runtime の親 user namespace における daemon の `host_uid` と比較し、等しくないことを確認する。数値の比較は両 UID が同じ親 namespace で解釈されるように行い、解釈できなければ owner 不明として拒否する。`uid_map` の別 UID 値や launcher 設定から owner を推定しない。
3. 採取対象の PID と starttime、session 登録、namespace FD と稼働状態を照合し、PID 再利用や採取後の process 入替えで別 session の事実を流用しない。broker と worker の両 admission が同じ条件を検査し、どちらか一方の成功を他方の検査の代用にしない。実際の機密操作時にも対象 session と lease の有効性を再確認する。
4. ADR-0115 の権限分離 launcher 経由で namespace が作られ、daemon に owner の資格情報、CDP pipe、開封済み state が返らない。daemon UID の独立 process と worker から Chrome への `ptrace` attach と `/proc/<pid>/mem`・`fd`・`environ` 読取りが拒否されることを**実 process で実証済み**の配置に限る。owner UID の不一致は必要条件だが、この攻撃試験や controller の秘密非露出の代わりにはならない。launcher 未配置、試験未実施、試験失敗の host では本番の機密能力を有効化しない。

5. ADR-0115 の launcher が発行した **session 証明**（`LauncherSessionProof`、D-L）があり、その検証に成功している。証明は次の 3 点を同じ session について示す: (a) runtime が launcher 経由で起動された（launcher の応答に kernel が付けた `SCM_CREDENTIALS` の送り手 UID が設定された launcher UID であり（[ADR-0116 launcher 実装の付記 D-P](0116-browser-launcher-implementation.md)）、`Response::Started` の `Receipt.isolation_ok` が真）、(b) runtime の user namespace の owner UID（`SessionFacts.ns_owner_uid`）が daemon の `host_uid` ではない、(c) runtime の PID と starttime（launcher 内の `SessionRecord{pid, starttime}`）に束縛されている。証明が無い、検証に失敗した、または `SameUid`・非隔離の runtime は、条件 2 の owner 検査に通っていても拒否する（fail-closed）。

### D-L: launcher session 証明の形と検証

- launcher は `Response::Started` と `observe` の応答に、`SessionRecord` から採った `pid`・`starttime` を含む証明を返す。daemon 側の型は `task_core::browser_isolation::LauncherSessionProof{session_id, instance_id, pid, starttime, ns_owner_uid, launcher_uid, isolation_ok}` とする（`browser_launcher::protocol::{Receipt, SessionFacts}` と `registry::SessionRecord` の値から組み立てる。今は `Receipt` に pid/starttime が無いので、protocol に足す）。
- 検証（`verify_isolation` と同じ task-core の共通条件。broker と worker の両 admission が自分で呼ぶ）:
  - 証明の `launcher_uid` が、launcher の応答に kernel が付けた `SCM_CREDENTIALS` の送り手 UID（[ADR-0116 launcher 実装の付記 D-P](0116-browser-launcher-implementation.md)。当初は `SO_PEERCRED` としたが socket 起動では systemd を指すため改めた）と設定上の launcher UID の両方と等しい。daemon の `host_uid` や root ではない。
  - `isolation_ok` が真で、`ns_owner_uid` が `Some` かつ daemon の `host_uid` と異なる。条件 2 で daemon 自身が採った owner UID とも一致する。
  - `pid` の `/proc/<pid>/stat` の starttime が証明の `starttime` と一致し、`RuntimeFacts` を採った process と同じである。PID 再利用・process 入替え・session 終了後は不一致として拒否する。
  - `session_id`・`instance_id` が admission の対象 session と一致する。他 session の証明を流用しない。
- 違反として `LauncherProofMissing`（証明なし。`BrowserRuntimeKind::Daemon` で起動した runtime を含む）と `LauncherProofInvalid`（上記のどれかが不一致・採取不能）を定義する。どちらも他の違反と同じく fail closed とし、owner 検査の成功で上書きしない。`SameUidHarness` から証明は作れず、試験用の偽証明を本番 `Attested` へ渡さない。

`RuntimeFacts` に owner UID の採取結果を追加し、`verify_isolation` による違反として `UsernsOwnedByDaemon`（owner が `host_uid`）と `OwnerUnknown`（FD/ioctl/namespace の解釈、照合に失敗）を定義する。`SameUid`、他の隔離違反、事実採取・検証失敗、owner 不明、daemon 所有のいずれも fail closed とする。取得エラーを安全な owner 値に置き換えない。`SameUidHarness` と `SameUidHarnessFacts` は試験 feature に閉じ、本番 `Attested` へ渡さない。成功例や conformance 記録にも harness の結果を本番 attestation として数えない。

### D-L 付記（2026-10-02、本番での事実採取、prod-facts）

条件 2・D-L は daemon が自ら `/proc/<runtime pid>/ns/user` を開いて `ioctl(NS_GET_OWNER_UID)` する前提で書いたが、本番の別 UID launcher runtime では daemon UID からのこの open が実測 `EACCES` になり、直す前は毎回 `IsolationRequired` で拒否していた（意図したとおり fail-closed だが、owner 検査自体が成立しない）。

- 修正: daemon が直接読めない namespace・userns owner は、launcher の束縛（protocol v3 `Receipt`/`SessionFacts` の `ns_inodes` / `ns_owner_uid`、`task_core::browser_isolation::LauncherSessionProof`）から採る。daemon 自身が読める `/proc/<pid>/status`・`mountinfo`・`getpgid` はそのまま daemon が読んで組み合わせる（`collect_launched_runtime_facts`）。読み取りエラーは安全値で埋めず `Err` のまま返す。
- 条件 2 の「daemon 自身が採った owner UID」と D-L の「`ns_owner_uid` が条件 2 の owner UID と一致する」は、launcher-managed runtime では「daemon が独立に再計測した値」ではなく「launcher の証明に載った値を daemon が受理した値」を指すと読み替える。これは owner 検査を弱めない: 証明の発行者（launcher）は daemon と異なる UID で動き、daemon は証明の `launcher_uid`・PID/starttime・`session_id`/`instance_id` の一致を独立に検査する（D-L 既定の検証）。
- 実証: `task-core browser_isolation`（31 passed）・`celeris-credentiald`（58 passed）・`task-worker --lib browser_launcher`（18 passed）、`browser_launcher_ptrace.rs` の sandbox・host 両実行。詳細は `prod-admission-release-evidence.md` §5。

### D-L 既知の未実証（実 session の許可/拒否表）

synthetic（構造体を直接組んだ模擬観測）での 5 通り（launcher-proof=allow、same-uid/non-isolated/no-proof/proof-invalid=deny）は単体・結合試験で実証済みだが、**launcher の実観測を本番入口（両 `Attested`）に通した `ADMISSION[real-session]` の表は未実証**。host で protocol v3 launcher に入れ替えて試験台本（`launcher-admission-evidence.sh`）を実行すると、表を作る直前の前提チェック（当時の版では launcher 接続の `SO_PEERCRED` が設定上の launcher UID であること）で失敗した。原因は launcher が systemd の socket activation（`Accept=no`）で動き、listen socket を最初に作るのが systemd（root）であるため、接続の `SO_PEERCRED` が `celeris-browser` ではなく `systemd` に見えること。sandbox でも同様の理由（userns 越し）で前提が成立しない。この前提は task 01M3ZFJ2DZ5TZPAFKACX45JNF4 で応答の `SCM_CREDENTIALS` に置き換えた（下の付記と ADR-0116 launcher 実装の付記 D-P）。host で `ADMISSION[real-session]` を再取得する必要がある（手順 `docs/ops/browser-launcher-admission-evidence-run.md`）。それまでは D-L の owner 検査・証明必須化は synthetic と単体試験の間接証跡、ptrace 拒否そのものは実 process で確認済みという状態にとどまる。

付記（2026-10-02、task 01M3ZFJ2DZ5TZPAFKACX45JNF4）: daemon 側の身元確認を応答の `SCM_CREDENTIALS` に替えた（[ADR-0116 launcher 実装の付記 D-P](0116-browser-launcher-implementation.md)）。launcher の binary・protocol は変えていない。`ADMISSION[real-session]` の再取得は人が [`docs/ops/browser-launcher-admission-evidence-run.md`](../../docs/ops/browser-launcher-admission-evidence-run.md) の手順で行う（未実施）。

## 維持する承認と観測の境界

- **H3（認証区間の LLM 観測停止）**: credential 注入中および復元 identity を使う session では、既存の LLM 入力・event・artifact・Live View の遮断を維持する。admission 成功を観測再開の理由にしない。秘密は trusted sink/controller にだけ渡し、receipt や診断に含めない。
- **H4（Live View の task ACL）**: 本人の task/run/session に束縛した live proxy と期限・失効時の再判定を維持する。共有 dashboard を開放しない。H3 の認証区間中は H4 の本人にも表示しない。
- **H5（project+origin の期限付き identity）**: 人が確認した需要に対する project と exact HTTPS origin の束縛、期限・失効・削除を維持する。個人 Chrome profile や他 project/origin の identity を復元しない。
- **ADR-0080 H2（`approve_once` と短い lease）**: 人による一回承認を省略しない。credential lease は操作直前に発行し、既定 60 秒・上限 300 秒、`max_uses=1`、task/run/session・origin・policy/revision に束縛する。期限切れ、再利用、cancel、policy 変更で拒否する。隔離の成功は承認や lease の代替ではない。

ADR-0112 の実測 conformance と ADR-0102 D6 の起動前拒否も維持する。能力の宣言だけ、fixture 名だけ、または `SameUidHarness` の試験結果で機密要求を起動しない。機密能力を満たさない場合は非機密 backend へ能力を落として fallback しない。owner 検査の追加は中間成果であり、機密能力の本番解放を意味しない。

## 検証と本番反映

- 別 host UID・全隔離条件成立・owner が daemon 以外・launcher session 証明の検証成功・ptrace 拒否が実証された session で、両方の本番 `Attested` admission が成功する試験を置く。broker は実 process の事実を採り直し、worker の復元経路も同じ owner 条件を通す。
- `SameUid`、namespace 等の非隔離、採取/検証失敗、`OwnerUnknown`、`UsernsOwnedByDaemon`、`LauncherProofMissing`（証明なし・launcher なし）、`LauncherProofInvalid`（UID・owner・PID/starttime・session の不一致）、ptrace 未拒否を個別に拒否する。owner 検査に通るが証明の無い runtime の拒否も個別に確かめる。資格情報の漏洩と復元時の H3 観測停止も回帰試験で確認する。
- 実 process 試験（`crates/task-worker/tests/browser_launcher_ptrace.rs` 系）で、launcher で起動した別 UID runtime への daemon UID からの `ptrace` が拒否され、その session だけが両能力を許可されることを示す。host の前提（launcher socket・subuid の親 userns map）が無ければ明示的に skip し、`CELERIS_LAUNCHER_TESTS=require` では skip せず失敗させる。
- ADR-0115 の launcher、controller、lifecycle の実 process 攻撃試験結果を記録してから有効化を判断する。A13 の broker `peer_uid_mismatch` と ptrace 拒否は別の判定として記録する。
- 証跡: 試験コマンド・exit code・ptrace 拒否の実出力・許可/拒否の対応表・host log は親 task 01M3VFQZ2TX3W0KTDQHKCAVJR6 の成果物 `prod-admission-release-evidence.md`・`launcher-host-run.log` に記録済み（リポジトリ外）。実 session の表 `ADMISSION[real-session]` は上記「D-L 既知の未実証」のとおり後続 task に残す。
- 本番への昇格と設定変更は、統合済み commit と試験証拠に対する人の承認を経て**人が行う**。エージェントは昇格しない。この ADR、owner 検査、launcher session 証明の実装、ptrace 拒否の実証だけでは本番を有効化しない。
