---
title: "健全性v2 R4 fix-cred: 計画の無い task も credential の検証と resume を経るまで起動しない"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 fix-cred

完了日: 2026-11-14

## 概要

- 反例（`cex-review.md` の未解決 2）を owner で直した。計画の無い（atomic）task は、秘密 file が置かれた後、検証が一時失敗
  （`VerifyOutcome::Transient`）しても次の tick で run を起こしていた。
- 直した後は、`credential_gate_hold` が SQLite の `credential_waits` の `task:<id>` と今の route の秘密の開いた wait
  （waiting・rejected・verified）を読み、resumer の claim で resumed になるまで run を起こさない。daemon の再起動の後も同じ。
- 判断は ADR [2026-11-14-atomic-task-credential-verified](../../adr/2026-11-14-atomic-task-credential-verified.md) に書いた
  （正本は SQLite、`ExecutionCoordination` の `credential_held` は写し、WU のある task の挙動と `CredentialGate` の fact の意味は変えない）。
- `crates/task-core` は変えていない（既存の読み取り `credential_waits_list_waiting` を使った）。

## 作業前 brief

1. 根本原因: `dispatcher/credential_gate.rs` の `credential_gate_hold` は `CredentialGates::is_registered`（秘密 file が空でないか）
   だけで通していた。atomic task には WU の Blocked 行が無いので、開いた wait が残っていても file があれば run が起きた。
   症状は `dispatch_one` の CredentialGate の fact（`credential_hold = false`）から run の起動に出ていた。
2. 責務 owner: `Dispatcher::credential_gate_hold`（credential の待ちの入口）。正本は store の `credential_waits`。
3. 既存の再利用先: store の `credential_waits_list_waiting`（trait `CredentialWaitStore`。`dyn TaskStore` から呼べる。
   `credential_wait_get` は `SqliteStore` の inherent method で、dispatcher からは呼べない）。試験は `credential_e2e::E2e`・`Scripted`・
   `cex_review_authority.rs` の `credential_dispatcher`・`HumanGateAdapter`・`run_until_state`。
4. 選択肢:
   - 案 A（採用）: gate が store の開いた wait を見て hold する。task-core を変えずに済み、再起動の後も store から同じ判断になる。
   - 案 B: `ExecutionCoordination` の `credential_held` を gate で読む。メモリの写しを正本にし、再起動・resume の判定を写しに頼るので採らない。
   - 案 C: atomic task にも WU 相当の行を作る。store の変更が要り、この unit の範囲（task-core を変えない）の外なので採らない。
5. 新しい状態・分岐・fallback・port: 新しい状態は無し。分岐は「atomic task で開いた wait があれば hold」の 1 つ
   （`atomic_credential_wait_open`）。`task:<id>` の書式は `atomic_wait_unit_id` に 1 か所にした。fallback・flag は無し。
6. 撤去対象: `enter_credential_wait` の「`coord` の印があれば wait を作らずに戻る」判断（印は写しになり、判断は store で行う）。
   `credential_held` の field 自体は `execution_coordination.rs` がこの unit の書き込み範囲の外にあるので残した
   （撤去の条件と責任者は ADR D3 と下の「提案」）。試験 11 の doc comment の「atomic task の待機は成立しない」を新しい試験への案内に替えた。
7. 不変条件: atomic task は開いた wait が resumed（または cancelled）になるまで run を起こさない。WU のある task は従来どおり
   WU の Blocked 行が止める。秘密の値は読み捨て、どこにも写さない。確かめる試験は
   `cex_credential_atomic_task_waits_for_verified_resume` と既存の `credential`・`cex_`・`dispatch_admission::` の試験。

## 変更

- `crates/task-dispatch/src/dispatcher/credential_gate.rs`:
  - `credential_gate_hold`: atomic task（`wu = None`）は、`atomic_credential_wait_open(task.id, secret_id)` が真なら `true` を返す。
    偽なら従来どおり `is_registered` で通すか、wait を作る。
  - `atomic_credential_wait_open`（新）: `credential_waits_list_waiting` から `task:<id>` と `secret_id` の組の行を探す。
  - `enter_credential_wait`: atomic task の重複の判断を `coord.hold_credential` の戻り値から store の開いた wait に替えた
    （以前は印があると wait を作らずに戻り、resume の後に秘密 file が消えると wait も受信箱の item も無いまま止まり得た。コードを読んだ見込みで、試験では確かめていない）。
  - `atomic_wait_unit_id`（新）: `task:<id>` の書式。
- `crates/task-dispatch/src/dispatcher/tests/cex_review_authority.rs`: 試験 11 の後に
  `cex_credential_atomic_task_waits_for_verified_resume` を足した。試験 11 の doc comment の範囲の行を直した。
- `agent-docs/adr/2026-11-14-atomic-task-credential-verified.md`（新）。

commit: 「fix-cred: 計画の無い task は開いた credential_wait が resumed になるまで起動しない」

## 試験 `cex_credential_atomic_task_waits_for_verified_resume`

`crash_before_claim` の 2 変種で回す。どちらも atomic task の wait を作り、`put_secret` で秘密 file と保存の記録を置き、resumer の
検証を `Transient` で 1 回処理した後に次を確かめる。

- 一時失敗の後の tick: `TickReport.dispatched = 0`、runs 索引に行なし、task は ready。
- daemon の再起動（同じ store で新しい dispatcher、`restore_credential_waits` が 1 件を返す）の後の tick: 同じ。
- 時計を 31 秒進めた（retry の期限の後の）tick: 同じ。
- `crash_before_claim = true`: `verified` を書いて claim の前に落ちた跡を作り、その tick と、もう一度の再起動の後の tick: 同じ。
- resumer の claim で `resumed` になった後に初めて run が起き、runs 索引の行は 1 本。verifier の呼び出しは 2 回
  （`Transient`・`Valid`）、`crash_before_claim = true` では 1 回（verified は再検証しない）。

### 修正の前に落ち、修正の後に通ったこと

- 修正の前（試験だけを足した木）: `cargo nextest run -p task-dispatch -E 'test(cex_credential_atomic_task_waits_for_verified_resume)'`
  → exit 1。`cex_review_authority.rs` の `held` の assert（`crash_before_claim=false after transient`）で
  `TickReport { dispatched: 1, in_flight: 1, .. }`（left 1、right 0）。cex-review.md の未解決 2 の観測と同じ。
- 修正の後: 同じコマンド → exit 0（1 passed）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(/cex_/) | test(/credential/) | test(/dispatch_admission::/)'` → exit 0（75 passed）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris celeris-campaign celerisctl llm-proxy task-dispatch。
  3933 passed、11 skipped。script bundle の SKIP 0 件）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲 check（計画の check 3 と同じ式。`sh "$CELERIS_WU_SCOPE_PATHS"`）→ exit 0。変更 path は credential_gate.rs・
  cex_review_authority.rs・ADR・この進捗の 4 件で、`crates/task-core` は 0 件。

## 未解決

- `ExecutionCoordination` の `credential_held` は、この変更の後は判断に使われず、書くだけの写しになった。
  `execution_coordination.rs` がこの unit の書き込み範囲の外にあるため消していない（ADR D3）。
- 別の秘密を要する route に替わった atomic task では、前の秘密の wait は開いたまま残る（ADR D2 の照合の範囲）。
  受信箱の item もその wait から出続ける。従来と同じ挙動で、この unit では変えていない。

## 提案

- record-fix（または後の task）で、`ExecutionCoordination` の `credential_held` と `hold_credential`・`release_credential_hold`、
  ADR 2026-11-11 D1 の表の `credential_held` の行を消す。撤去の条件（判断で読まれていない）は満たしている。
