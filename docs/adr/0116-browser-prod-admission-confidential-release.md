# ADR-0116: browser 本番 admission の機密能力解放条件

---
tasks: [01M3VFQZ2TX3W0KTDQHKCAVJR6]
---

- 日付: 2026-10-01
- 状態: 採用（契約。本番への昇格・設定変更は未実施）
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md)、[ADR-0102](0102-browser-phase4-isolation-injection-routing.md)、[ADR-0109](0109-browser-p4b-injection-ipc-cdp-sink.md)、[ADR-0112](0112-browser-p4b-conformance-evidence-unlock.md)、[ADR-0115](0115-browser-ptrace-owner-ns-launcher.md)、[後続 task](../progress/browser-followups.md)

## 背景

別 host UID の browser runtime では Chrome 起動、`verify_isolation` の成功、`/proc/<pid>/environ` の読取り拒否と本番 `Attested` identity 復元が実証された。一方、daemon UID の独立 process から Chrome への `ptrace` attach は成功した。daemon が runtime の user namespace を作り、その owner UID になっていたためである（ADR-0115）。別 UID と既存の `verify_isolation Ok` だけをもって機密能力を本番で解放できない。

## 決定: 両 admission の共通条件

本番の `celeris-credentiald::Admission::Attested` による `CredentialInjection` と、`task-worker::RestoreAdmission::Attested` による `IdentityRestore` は、**同じ稼働中の隔離 session** について次の全条件を満たした場合に限り許可する。

1. 対象 runtime の実 process から採った `RuntimeFacts` に対し `verify_isolation` が `Ok(IsolationAttestation)` を返す。別 host UID（root でも daemon の `host_uid` でもない）、必要な user/pid/net/mount/ipc/uts namespace、読取専用 root、限定された書込み先、broker/host IPC の不可視、CDP の非 TCP 経路、`no_new_privs`、capability 全落とし、専用 process group など ADR-0102 D1 の全条件を維持する。違反を一つだけ無視して attestation を作らない。
2. `/proc/<runtime pid>/ns/user` の namespace FD に `ioctl(NS_GET_OWNER_UID)` を行い、**runtime の user namespace の owner UID** を得る。その値を runtime の親 user namespace における daemon の `host_uid` と比較し、等しくないことを確認する。数値の比較は両 UID が同じ親 namespace で解釈されるように行い、解釈できなければ owner 不明として拒否する。`uid_map` の別 UID 値や launcher 設定から owner を推定しない。
3. 採取対象の PID と starttime、session 登録、namespace FD と稼働状態を照合し、PID 再利用や採取後の process 入替えで別 session の事実を流用しない。broker と worker の両 admission が同じ条件を検査し、どちらか一方の成功を他方の検査の代用にしない。実際の機密操作時にも対象 session と lease の有効性を再確認する。
4. ADR-0115 の権限分離 launcher 経由で namespace が作られ、daemon に owner の資格情報、CDP pipe、開封済み state が返らない。daemon UID の独立 process と worker から Chrome への `ptrace` attach と `/proc/<pid>/mem`・`fd`・`environ` 読取りが拒否されることを**実 process で実証済み**の配置に限る。owner UID の不一致は必要条件だが、この攻撃試験や controller の秘密非露出の代わりにはならない。launcher 未配置、試験未実施、試験失敗の host では本番の機密能力を有効化しない。

`RuntimeFacts` に owner UID の採取結果を追加し、`verify_isolation` による違反として `UsernsOwnedByDaemon`（owner が `host_uid`）と `OwnerUnknown`（FD/ioctl/namespace の解釈、照合に失敗）を定義する。`SameUid`、他の隔離違反、事実採取・検証失敗、owner 不明、daemon 所有のいずれも fail closed とする。取得エラーを安全な owner 値に置き換えない。`SameUidHarness` と `SameUidHarnessFacts` は試験 feature に閉じ、本番 `Attested` へ渡さない。成功例や conformance 記録にも harness の結果を本番 attestation として数えない。

## 維持する承認と観測の境界

- **H3（認証区間の LLM 観測停止）**: credential 注入中および復元 identity を使う session では、既存の LLM 入力・event・artifact・Live View の遮断を維持する。admission 成功を観測再開の理由にしない。秘密は trusted sink/controller にだけ渡し、receipt や診断に含めない。
- **H4（Live View の task ACL）**: 本人の task/run/session に束縛した live proxy と期限・失効時の再判定を維持する。共有 dashboard を開放しない。H3 の認証区間中は H4 の本人にも表示しない。
- **H5（project+origin の期限付き identity）**: 人が確認した需要に対する project と exact HTTPS origin の束縛、期限・失効・削除を維持する。個人 Chrome profile や他 project/origin の identity を復元しない。
- **ADR-0080 H2（`approve_once` と短い lease）**: 人による一回承認を省略しない。credential lease は操作直前に発行し、既定 60 秒・上限 300 秒、`max_uses=1`、task/run/session・origin・policy/revision に束縛する。期限切れ、再利用、cancel、policy 変更で拒否する。隔離の成功は承認や lease の代替ではない。

ADR-0112 の実測 conformance と ADR-0102 D6 の起動前拒否も維持する。能力の宣言だけ、fixture 名だけ、または `SameUidHarness` の試験結果で機密要求を起動しない。機密能力を満たさない場合は非機密 backend へ能力を落として fallback しない。

## 検証と本番反映

- 別 host UID・全隔離条件成立・owner が daemon 以外・launcher 経由・ptrace 拒否が実証された session で、両方の本番 `Attested` admission が成功する試験を置く。broker は実 process の事実を採り直し、worker の復元経路も同じ owner 条件を通す。
- `SameUid`、namespace 等の非隔離、採取/検証失敗、`OwnerUnknown`、`UsernsOwnedByDaemon`、launcher なし、ptrace 未拒否を個別に拒否する。資格情報の漏洩と復元時の H3 観測停止も回帰試験で確認する。
- ADR-0115 の launcher、controller、lifecycle の実 process 攻撃試験結果を記録してから有効化を判断する。A13 の broker `peer_uid_mismatch` と ptrace 拒否は別の判定として記録する。
- 本番への昇格と設定変更は、統合済み commit と試験証拠を人が確認・承認した後に**人が行う**。エージェントは昇格しない。この ADR の採用だけでは本番を有効化しない。
