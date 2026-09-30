---
tasks: [01M3SPF94RDWTPWHNDEQD68VB9]
---
# browser capability の後続 task（決定 sep-uid=a）

決定 sep-uid=a により、task 01M3SPF94RDWTPWHNDEQD68VB9 は本番 admission で SameUid が拒否されることの試験だけで閉じ、
別 host UID を要する次の 3 件を範囲外の後続 task として `celerisctl add` で起票した（2026-09-30、いずれも Draft。
案件 agent-platform を GUI で付けてから accept する）。確認は `celerisctl --db /var/lib/celeris/celeris.sqlite3 show <task id>`。

共通の理由: この host では rmaeda の subuid/subgid（165536:65536）が daemon の親 user namespace の uid_map
（`0 100000 1001 / 1001 1001 1 / 1002 101002 64534`）の範囲外で、newuidmap が EPERM になり別 host UID の
browser runtime を起動できない（手順: `docs/ops/browser-isolated-runtime-subuid.md`）。

| key | task id | 題名 | 理由 |
|---|---|---|---|
| sep-uid-runtime | 01M3SPN8H05EJ3DHPVEGEYTMEH | browser: 別 host UID（subuid）の隔離 runtime の実証と隔離下 identity 復元の本番成功 | subuid が親 uid_map の範囲外で newuidmap が EPERM。手順書で人が host を用意した後でないと実証できない |
| a13-real-process | 01M3SPN8HE6A1TZ54GBZGHMZYJ | browser P4-B A13: 別 UID 前提の実 process 攻撃試験 | A13 は別 host UID の実 process が前提で、この host では起動できない（sep-uid-runtime に依存） |
| prod-admission-release | 01M3SPN8HPHPWZ32F0AG986TWS | browser: 本番 admission での機密能力（CredentialInjection・IdentityRestore）解放 | 解放は別 UID 隔離の実証と A13 合格が前提（上 2 件に依存）。それまで本番は SameUid と機密能力を拒否する |
| restore-observation-stop | 01M3SRZ4X8NHRE0BB1QXMBTPKJ | browser: 復元 identity の session で LLM 観測と Live View を止める（ADR-0083 D4 / ADR-0080 H3） | 決定適合監査で見つけた矛盾。本番は SameUid で復元を拒否するため未到達。別 UID 解放の前に直す（sep-uid-runtime に依存） |
