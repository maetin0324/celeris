---
title: browser 本番 admission の機密能力解放と launcher 身元確認
tasks: [01M3VFQZ2TX3W0KTDQHKCAVJR6, 01M3WV4BFJ71J9ZWJ020MP2Z4K, 01M3WW2RBB9QW9NPN862TZEK9P, 01M3ZFJ2DZ5TZPAFKACX45JNF4]
status: done
updated: 2026-10-03
---
# browser 本番 admission の機密能力解放と launcher 身元確認

> 旧 `docs/PROGRESS.md`（現 `agent-docs/PROGRESS.md`）に main が足した節を ADR-0128 D6 に従い sync-main-2 migrate-docs（task 01M3Z8CXYG1J6BZCQ87YS3FC67）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

## 本番 admission の機密能力解放（2026-10-01、task 01M3VFQZ2TX3W0KTDQHKCAVJR6）

[ADR-0138](../adr/0138-browser-prod-admission-confidential-release.md) は提案であり、本番解放の決定ではない。中間成果として `verify_isolation` に user namespace owner 検査を追加し、owner 不明・daemon owner（`OwnerUnknown` / `UsernsOwnedByDaemon`）を拒否する。これは main の `Attested` より厳しい。境界試験は `prod_admission.rs`（6 passed）と `browser_prod_admission.rs`（9 passed）、全体 gate は `cargo test --workspace`（3055 passed / 0 failed / 11 ignored）と `cargo clippy --workspace -- -D warnings`（exit 0）。ただし launcher 経由の実 process ptrace 拒否は未実証で、**解放は未**。実証されるまで `CredentialInjection`・`IdentityRestore` を許す本番 session は無い。

- H3: 認証区間の LLM 観測停止を維持。
- H4: task ACL・期限・失効時の再判定と認証区間中の Live View 停止を維持。
- H5: project + exact origin の束縛と期限・失効・削除を維持。
- H2（ADR-0080）: `approve_once`・短い一回限り lease を維持。
- 証拠コマンド: `cargo test -p celeris-credentiald --test prod_admission`; `cargo test -p task-worker --test browser_prod_admission`; `cargo test --workspace`; `cargo clippy --workspace -- -D warnings`。
- 未解決: subuid の親 user namespace map 外による EPERM、launcher 実装、実 process での ptrace 拒否/A13 は未解決。本番昇格は実証証拠に対する人の承認後に人が行い、この task では実施していない。

### 本番 Attested に launcher 証明と実 process ptrace 拒否を必須化（2026-10-02、子 task 01M3WV4BFJ71J9ZWJ020MP2Z4K、unit launcher-gated-release）

launcher（ADR-0115）実装と host 準備の完了を受け、owner 検査だけでなく launcher session 証明（`task_core::browser_isolation::LauncherSessionProof` / `verify_launcher_session`）を両 admission に必須化した（fail-closed）。証明が無い・検証失敗・`SameUid`・非隔離の runtime は owner 検査に通っても拒否する。

- 必須化の箇所: `celeris-credentiald::injection_ipc::Admission::Attested.admit`（CredentialInjection）、`task-worker::browser_runtime::RestoreAdmission::Attested`（IdentityRestore）。両方とも `task_core::browser_isolation` の共通条件に依る。
- ptrace 拒否は launcher 経由の別 UID runtime への daemon UID からの実 process 攻撃で**実証済み**（sandbox・host 双方）: `PTRACE_ATTACH errno=Some(1)`（EPERM）、`strace -p` は `Operation not permitted`、`/proc/<pid>/{environ,mem}` はいずれも `errno=Some(13)`（EACCES）。同 UID の positive control（`PTRACE_ATTACH=0`）で検査手段自体の有効性も確認済み。
- prod-facts の結論: 本番 Attested は別 UID の launcher runtime の `/proc/<pid>/ns` を daemon UID から直接読むと `EACCES` になるため、daemon が読めない namespace・userns owner は launcher の束縛（protocol v3 `SessionBinding.ns_inodes` / `ns_owner_uid`）から採り、daemon 自身が読める `/proc/<pid>/status` 等はそのまま daemon が読んで組み合わせる（`collect_launched_runtime_facts`）。読み取りエラーは安全値で埋めず `Err` のまま返す。
- 許可/拒否の対応表: synthetic（構造体を直接組んだ模擬観測、5 通り）は単体・結合試験で実証済み（`task-core browser_isolation` 31 passed、`celeris-credentiald` 58 passed、`task-worker --lib browser_launcher` 18 passed）。**実 session の表 `ADMISSION[real-session]`（launcher の実観測を本番入口に通した表）は未実証**: host で protocol v3 launcher に入れ替えて実行したが、systemd の socket activation 下で launcher への接続の `SO_PEERCRED` が `celeris-browser`（995）ではなく `systemd`（uid 0）に見え、前提チェックで失敗した（`launcher-admission-evidence.sh` 実行結果 `EXIT: 101`、6 本中 5 passed・1 failed）。後続 task で、launcher が accept 後に自分の pid/uid を伝える仕組みを ADR-0116 に追記して直す。
- 検査: `cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` exit 0（sandbox、6 passed、許可/拒否表は前提欠落で `SKIP:`）。`cargo clippy --workspace -- -D warnings` exit 0。
- 証跡: 親 task 01M3VFQZ2TX3W0KTDQHKCAVJR6 の成果物 `prod-admission-release-evidence.md`（試験コマンド・exit code・ptrace 拒否の実出力・対応表・host log 要点）と `launcher-host-run.log`。
- **本番昇格は未実施**。昇格は人が selfdeploy 手順（kb `projects/agent-platform/selfdeploy-release-verify-procedure.md`、証跡の `prod-admission-release-evidence.md` §6 参照）で行う。この run は本番 daemon を再起動・昇格していない。

### land-main3: 最新 main の統合 — 2026-10-02

main `0d438ec19d9a` を merge し、`docs/PROGRESS.md` の両側の節を保持した。main の ADR-0122 完了・ui-ux 外部 skill の結合試験・planner 指針・最終検査記録に加え、browser launcher の実 process 証跡、tick_prunes の単独再実行、過去の land-main/land-main2 記録も残した。

- main 由来の launcher 関連差分を確認: `crates/task-worker/src/browser_runtime.rs` は main 側の init 待ち変更を含み、launcher/sandboxd/egress の起動経路に効く。この変更は既に land-main2 の記録に記載済みで、host の binary 入れ替えと require 試験の再実行が必要。
- `git merge-base --is-ancestor 0d438ec19d9a HEAD` → exit 0。`git merge-tree --write-tree main HEAD` → exit 0（tree `d7c0705a6e14f6dc89fbd842b679f4078c084bd5`）。main の ADR-0122 / ui-ux 記録と launcher 節は両方保持。
- 最終検査: `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test --workspace` → exit 101。`instance_handoff` 8件中3 passed / 5 failed。`cargo test -p celeris --test instance_handoff` 単独再実行も exit 101、同じ5件を再現。3件は ADR-0095 worker db guard の user namespace 作成が `Operation not permitted` で失敗。残り2件（新旧 daemon の dispatch/standby 引継ぎ）も同じ環境で失敗した。検査は pass 扱いにしない。
- launcher binary に効く main 差分は `crates/task-worker/src/browser_runtime.rs` の init 起動待ち処理である。既存の記録どおり host の binary 入れ替えと require 試験の再実行が必要。

### pick-chrome: 並走 session での launcher Chrome 特定 — 2026-10-02

`browser_launcher_ptrace.rs` の Chrome 特定が並走 session で曖昧になって落ちていた件を、試験 file だけで直した。launcher は daemon から読める `/proc` に session の印を出さないため、launcher 子孫の新しい Chrome 候補を全部検査して 1 件以上を要求し、自分の session の停止で検査済みの session root が消えることを確かめる。選択は純粋な関数に分け、単体試験 `chrome_pick_*` 4 件を足した。詳細は `docs/progress/phase-browser-4.md`『並走 session での Chrome 特定』。

- `cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` → exit 0（5 passed、実 launcher 試験も実行）。
- `cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `crates/task-worker/src/` は不変（host の binary 入れ替え不要）。


## launcher の身元確認を SCM_CREDENTIALS に（socket 起動対応）— 2026-10-03

- 完了日: 2026-10-03（task 01M3ZFJ2DZ5TZPAFKACX45JNF4）。Attested task branch（celeris/01M3WV4BFJ71J9ZWJ020MP2Z4K）を取り込んだ上で修正。
- 原因: launcher は systemd の socket 起動で、listen socket を作ったのが systemd（root）。`SO_PEERCRED` は listen 時の資格情報を返すので daemon からは uid 0 に見え、`ADMISSION[real-session]` の前提が成り立たなかった。
- 方法: `LauncherClient` が `SO_PASSCRED` を立て、各応答に kernel が付ける `SCM_CREDENTIALS`（応答を書いた process の pid/uid/gid）を `MSG_PEEK` で読む。全応答で一致しなければ `None`（fail closed）。`LauncherRuntime::start` と `browser_launcher_ptrace.rs` はこの値を使う。launcher binary・protocol は不変。根拠は ADR-0116（launcher 実装）付記 D-P。
- 試験: `client_identifies_the_responding_process_not_the_listener_creator`（listen した process と応答する子 process を分け、`SO_PEERCRED` は前者・responder は後者を指す）、`client_records_a_consistent_responder_across_requests`。
- 証拠: `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace --all-targets -- -D warnings` exit 0、`cargo test -p task-worker --lib browser_launcher` 20 passed、`--lib launcher_run` 16 passed、`--test browser_launcher_ptrace --test browser_prod_admission` 6 + 18 passed（sandbox）。
- ついで: 取り込みで呼び出しを失って未使用になった `browser_injection_wire.rs` の `wait_cdp_ready` を削除（clippy の dead_code）。
- ADR 番号: 取り込んだ本番 admission の ADR（旧 `0116-browser-prod-admission-confidential-release.md`）は main の ADR-0116（launcher 実装）と重なるため [ADR-0138](../adr/0138-browser-prod-admission-confidential-release.md) に振り直した（main と全 celeris/* ブランチの最大は 0137）。コード・試験・台本・unit の「ADR-0116 D-L」「ADR-0116 条件 1〜5」を ADR-0138 に、D2〜D7・付記 D-P は ADR-0116 のまま。条件 5(a) と未実証節の `SO_PEERCRED` の記述も D-P に合わせた。
- 2026-10-03 訂正: `ADMISSION[real-session]` は main `3527c8e3`（protocol v3）の launcher に対する host の 1 回の通常実行（`/var/tmp/launcher-evidence-main.log`、EXIT 0、6 passed）で実証済み。`launcher-host-run-v2.log`（EXIT 0）は merge 前の v4 実装の証跡で、merge 後の実装の証跡ではない。未確認: host の必須モード stutter 3 回、merge 後 HEAD（試験の待ち時間 20→60 秒のみ差分）の host 再取得。いずれも人が `docs/ops/browser-launcher-admission-evidence-run.md` の手順で行う。本番昇格は未実施（人が行う）。

## main の更新（2026-10-03、旧 docs/PROGRESS.md から移した）

> 旧 `docs/PROGRESS.md`（main 4c354a7f）の節を ADR-0128 D6 に従い sync-main-3（task 01M40D0QW6HX3XEZK3GBCQV5ZM）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

冒頭の節の main 側の最新の本文を、見出しを 1 段下げて残す。「launcher の身元確認を SCM_CREDENTIALS に」節の未解決の行は main の 2026-10-03 訂正に置き換えた。上の記述と食い違う点（`ADMISSION[real-session]` の実証）は、こちらが新しい。

### 本番 admission の機密能力解放（2026-10-01 起票、task 01M3VFQZ2TX3W0KTDQHKCAVJR6）

2026-10-01 時点の経緯: 中間成果として `verify_isolation` に user namespace owner 検査を追加し、owner 不明・daemon owner（`OwnerUnknown` / `UsernsOwnedByDaemon`）を main の `Attested` より厳しく拒否した。境界試験は `prod_admission.rs`（6 passed）・`browser_prod_admission.rs`（9 passed）。当時は launcher 経由の実 process ptrace 拒否が未実証で、ADR-0138 は提案のままだった。

2026-10-03 更新（子 task 01M3WV4BFJ71J9ZWJ020MP2Z4K、unit launcher-gated-release）: launcher session 証明（`verify_launcher_session`）を両 admission（credentiald `Admission::Attested`・task-worker `RestoreAdmission::Attested`）に必須化し、sandbox・host 実 process の双方で daemon UID からの ptrace 拒否を実証した。launcher 証明つきの別 UID 隔離 session だけが `CredentialInjection`・`IdentityRestore` を許可され、`SameUid`・非隔離・証明なし・検証失敗の runtime は拒否される。H3（認証区間の LLM 観測停止）・H4（ACL・期限・Live View 停止）・H5（origin 束縛・失効）・ADR-0080 H2（`approve_once`・短い lease）はいずれも弱めていない。詳細・試験・host log は下記「本番 Attested に launcher 証明と実 process ptrace 拒否を必須化」節と成果物 `prod-admission-release-evidence.md` を参照。

- 証拠コマンド: `cargo test -p celeris-credentiald --test prod_admission`; `cargo test -p task-worker --test browser_prod_admission`; `cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture`; `cargo test --workspace`; `cargo clippy --workspace -- -D warnings`。
- 未確認: `CELERIS_LAUNCHER_TESTS=require` での host の SIGSTOP stutter 3 回、merge 後 HEAD の host 再取得（手順は下記節）。
- **本番昇格は未実施**。実証証拠に対する人の承認後、人が selfdeploy 手順（kb `projects/agent-platform/selfdeploy-release-verify-procedure.md`）で行う。
