---
title: "健全性v2 R4 cred-cache: ExecutionCoordination の credential の待ちの写しを消す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 cred-cache

完了日: 2026-11-14

## 概要

- 最終 review の advisory（dual_source_of_truth・removal_missing）を直した。atomic task の credential の待ちの写し
  （`ExecutionCoordination` の field と、印を立てる・消す 2 つの method、structure guard の field の一覧の行、旧い doc comment）を消した。
- gate（`dispatcher/credential_gate.rs` の `credential_gate_hold`）は store だけで決める: atomic task で開いた wait があれば入り直さず hold、
  開いた wait が無く秘密が登録済みなら通す、それ以外は `enter_credential_wait`。`enter_credential_wait` は store の
  `credential_wait_enter` の `outcome.created` のときだけ受信箱の item の印と triage の reconcile を行う（従来どおり）。
- 安全確認（resumer の検証・再開の claim・予算の stop・`CredentialGate` の fact）は変えていない。`cex_lease_restart.rs` は変えていない。
- ADR 2026-11-14-atomic-task-credential-verified に付記「cred-cache」を足した（D3 を置き換える）。

## 実装前 brief

1. 根本原因: fix-cred が待ちの判断を store に移した後も、`ExecutionCoordination` が同じ集合をメモリに写し、doc comment は消えた復元の挙動を書いていた。
2. 責務 owner: `dispatcher/credential_gate.rs`（gate と待ちの入り）と `execution_coordination.rs`（写しの field）。
3. 既存の再利用先: store の `credential_wait_enter`（開いた wait を返す冪等）、`credential_waits_list_waiting`（読み取り接続）、
   試験の `credential_dispatcher`・`HumanGateAdapter`・`E2e`。
4. 選択肢: (a) 写しを消し、atomic task で開いた wait があれば入り直さない（採用。tick ごとの writer の lock も取らない）。
   (b) 写しを消して毎 tick `credential_wait_enter` を呼ぶ（正しいが tick ごとに Immediate の transaction を取る）。
5. 新しい状態・分岐・fallback・port: 無し（写しの分岐を store の読みの分岐に置き換えただけ）。
6. 撤去対象: 写しの field・2 つの method・structure guard の一覧の行・旧い doc comment（再起動で戻すという説明）。
7. 不変条件: atomic task の待ちは store の 1 行が正本で、tick と再起動を重ねても wait 行・受信箱の item は 1 件、run を起こさない
   （新しい試験）。resume の後にだけ起動する（`cex_credential_atomic_task_waits_for_verified_resume`）。

## 足した試験

- `crates/task-dispatch/src/dispatcher/tests/cex_review_authority.rs` の `credential_wait_for_atomic_task_is_entered_once_across_ticks`:
  atomic task が待ちに入った後、tick 3 回、再起動（同じ store で新しい dispatcher と `restore_credential_waits`）、tick 3 回で、
  開いた wait が最初の行と同じ 1 件、`credential_waits` の task の行が 1 件、受信箱の item 1 件、task の event の数が変わらない、
  runs 索引が空、task は ready。秘密 file が無い場合と、置いたが検証前（waiting）の場合の 2 通り。
- 置き場所: `tests/credential_gate.rs`・`credential_e2e.rs` は gate を直に呼び、tick の route の選択を通らない。tick で gate に入る
  道具（`credential_dispatcher`）と他の `cex_credential_` の試験がある `cex_review_authority.rs` に置いた。
- atomic task の待ちの入りは `Event` を書かない（WU の遷移が無い）。このため「入りの event」は、`created` のときだけ書かれる
  受信箱の item の印（`notice_item_id`）と受信箱の item、task の event の数が増えないことで確かめる。
- 改変での確認: gate の atomic の開いた wait の経路を「開いた wait を cancel して入れ直す」に一時的に変えると、新しい試験は
  `secret_saved=false tick 1` で落ちた（開いた wait の id が 1 から 2 に替わる）。既存の `cex_credential_atomic_task_waits_for_verified_resume`
  もこの改変で落ちたが、resume の後の検証の回数（left 2・right 1）で落ちた。改変は戻した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `grep -rn "credential_held\|hold_credential\|release_credential_hold" crates/task-dispatch/src` → 0 件（exit 1）。
- `cargo nextest run --locked -p task-dispatch -E 'test(/credential_wait_for_atomic_task_is_entered_once_across_ticks/) | test(/cex_credential_/) | test(/credential_gate::/) | test(/credential_e2e::/) | test(/structure_guard/)'`
  → exit 0、31 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、3942 passed、11 skipped）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## commit

- 「cred-cache: ExecutionCoordination の credential の待ちの写しを消し、gate を store の wait だけで決める」

## 未解決事項

- ADR 2026-11-11-dispatcher-state-ownership の D1 の表に、消した field の行が残っている（この WorkUnit の範囲外）。
- 開いた wait が無くなった直後に秘密 file が消えた場合に新しい wait が作られることは、コードを読んだ見込みで、試験では確かめていない。

## 提案

- record-d7 か次の記録の unit で、ADR 2026-11-11 D1 の表から消した field の行を除き、own-coord-a の数を 13 にする
  （structure guard の文言は 13 にした）。
