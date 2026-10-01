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
| a13-real-process | 01M3SPN8HE6A1TZ54GBZGHMZYJ | browser P4-B A13: 別 UID 前提の実 process 攻撃試験 | A13 は別 host UID の実 process が前提で、この host では起動できない（sep-uid-runtime に依存）。A13 の判定は『部分』。根因は daemon が user namespace の持ち主となり `CAP_SYS_PTRACE` を持つこと。権限分離 launcher の設計は [ADR-0115](../adr/0115-browser-ptrace-owner-ns-launcher.md) を参照。実装と実 process 検証は後続段階。 |
| prod-admission-release | 01M3VFQZ2TX3W0KTDQHKCAVJR6 | browser: 本番 admission での機密能力（CredentialInjection・IdentityRestore）解放 | owner 検査を追加（`UsernsOwnedByDaemon` / `OwnerUnknown` を拒否し、main の `Attested` より厳しい）。**解放は未**: launcher 経由で実 process の ptrace 拒否が実証されるまで、`CredentialInjection`・`IdentityRestore` を許す本番 session は無い。[ADR-0116](../adr/0116-browser-prod-admission-confidential-release.md)。境界試験: `prod_admission.rs` / `browser_prod_admission.rs`。 |
| restore-observation-stop | 01M3SRZ4X8NHRE0BB1QXMBTPKJ | browser: 復元 identity の session で LLM 観測と Live View を止める（ADR-0101 D4 / ADR-0080 H3） | **解決済み（2026-09-30、この task 01M3SPF94RDWTPWHNDEQD68VB9 の WorkUnit restore-obs-stop で修正）**。`restore_in_session`（`crates/task-api/src/browser_identity.rs:307`）が controller への投入前に `observation_stopped` を記録し、session 終了まで解除しない。test: `restore_enters_observation_stop_until_session_end`（task-api）、`restored_session_refuses_agent_observation`（task-worker）。celerisctl で cancel 済み |
