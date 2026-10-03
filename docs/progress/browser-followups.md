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
| prod-admission-release | 01M3VFQZ2TX3W0KTDQHKCAVJR6 | browser: 本番 admission での機密能力（CredentialInjection・IdentityRestore）解放 | 子 task 01M3WV4BFJ71J9ZWJ020MP2Z4K（unit launcher-gated-release）で launcher session 証明（`LauncherSessionProof` / `verify_launcher_session`）を両 admission（`celeris-credentiald::injection_ipc::Admission::Attested`・`task-worker::browser_runtime::RestoreAdmission::Attested`）に必須化（fail-closed）。owner 検査に通っても証明が無ければ拒否する。ptrace 拒否は launcher 経由の別 UID runtime への daemon UID からの実 process 攻撃で sandbox・host 双方で実証済み（`errno=1 EPERM`、`strace Operation not permitted`、`/proc/<pid>/{environ,mem}` は `errno=13 EACCES`）。身元確認は `SO_PEERCRED`（socket 起動では systemd＝uid 0）から応答の `SCM_CREDENTIALS`（送り手 uid 995）に替えた（[ADR-0116 付記 D-P](../adr/0116-browser-launcher-implementation.md)、main の protocol v3）。許可/拒否の対応表は synthetic（模擬観測、5 通り）に加え、**実 session の表 `ADMISSION[real-session]` は main `3527c8e3`（protocol v3）の launcher に対する host の 1 回の通常実行（EXIT 0、6 passed）で実証済み**（生ログ `/var/tmp/launcher-evidence-main.log`、リポジトリ外）。merge 前の v4 実装の host 実行（`launcher-host-run-v2.log`、EXIT 0）も同じ表を出したが、merge 後の実装の証跡ではない。未確認は必須モードの host stutter 3 回と merge 後 HEAD の host 再取得（人が行う）。証跡: 親 task 01M3VFQZ2TX3W0KTDQHKCAVJR6 の成果物 `prod-admission-release-evidence.md`（試験コマンド・exit code・ptrace 拒否の実出力・対応表を記載。リポジトリ外の成果物ディレクトリにあるため相対リンクなし）。**本番昇格は未実施・人が行う**。[ADR-0138](../adr/0138-browser-prod-admission-confidential-release.md)。境界試験: `prod_admission.rs` / `browser_prod_admission.rs` / `browser_launcher_ptrace.rs`。 |
| restore-observation-stop | 01M3SRZ4X8NHRE0BB1QXMBTPKJ | browser: 復元 identity の session で LLM 観測と Live View を止める（ADR-0101 D4 / ADR-0080 H3） | **解決済み（2026-09-30、この task 01M3SPF94RDWTPWHNDEQD68VB9 の WorkUnit restore-obs-stop で修正）**。`restore_in_session`（`crates/task-api/src/browser_identity.rs:307`）が controller への投入前に `observation_stopped` を記録し、session 終了まで解除しない。test: `restore_enters_observation_stop_until_session_end`（task-api）、`restored_session_refuses_agent_observation`（task-worker）。celerisctl で cancel 済み |
