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
   （free function の `atomic_wait_open`）。`task:<id>` の書式は `wait_unit_id` の 1 か所にした。fallback・flag は無し。
   `Dispatcher` に method は足していない（health の `size.responsibility-growth` が Dispatcher の method 数と dispatcher module の
   本体行の増加を fail にするため。下の「health」）。
6. 撤去対象: `restore_credential_waits` が atomic task の `coord` の印を立て直す分岐（gate が store の wait で hold するので要らない。
   ADR D3）。試験 11 の doc comment の「atomic task の待機は成立しない」を新しい試験への案内に替えた。
   `coord` の印（`credential_held`）は、同じ wait を tick ごとに入れ直さないための cache として残した（hold の判断には使わない）。
7. 不変条件: atomic task は開いた wait が resumed（または cancelled）になるまで run を起こさない。WU のある task は従来どおり
   WU の Blocked 行が止める。秘密の値は読み捨て、どこにも写さない。確かめる試験は
   `cex_credential_atomic_task_waits_for_verified_resume` と既存の `credential`・`cex_`・`dispatch_admission::` の試験。

## 変更

- `crates/task-dispatch/src/dispatcher/credential_gate.rs`:
  - `credential_gate_hold`: atomic task（`wu = None`）は、`atomic_wait_open` が真なら、秘密 file があっても `is_registered` で
    通さず、`enter_credential_wait` へ進んで `true` を返す（`enter_credential_wait` は印があれば何も書かない）。
  - `atomic_wait_open`（新、free function）: `credential_waits_list_waiting` から `task:<id>` と `secret_id` の組の行を探す。
  - `wait_unit_id`（新、free function）: wait の `work_unit_id`（WU の id か `task:<id>`）。`enter_credential_wait` と共有する。
  - `restore_credential_waits`: atomic task の印を立て直す分岐を消した。task が無い・cancel 済みの 2 分岐は同じ処理（wait の cancel）
    なので 1 つにまとめた。
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

- 修正の後の guard の確認: `atomic_wait_open` の結果を見ない形（`if !open && …` を `if …` に戻す）に一時的に変えると、同じコマンドが
  exit 1（同じ assert、left 1、right 0）。戻すと exit 0。

## health

最初の実装（`Dispatcher` に method `atomic_credential_wait_open` を足した版）では、
`bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` が exit 1 だった。`size.responsibility-growth` の worsened が 2 件
（Dispatcher の method 数 382 → 383、dispatcher module の本体行 31607 → 31629）。判断を free function に移し、restore の分岐を
上のとおり整理した版で exit 0（verdict pass、fail_grade 0、new 0、worsened 0。method 数 382 → 382、本体行 31607 → 31605）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(/cex_/) | test(/credential/) | test(/dispatch_admission::/)'` → exit 0（75 passed）
- `bash scripts/dev/test-affected.sh` → TEST_AFFECTED
- `cargo clippy --workspace -- -D warnings` → CLIPPY
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict pass、worsened 0）
- 範囲 check（計画の check 3 と同じ式。`sh "$CELERIS_WU_SCOPE_PATHS"`）→ exit 0。変更 path は credential_gate.rs・
  cex_review_authority.rs・ADR・この進捗の 4 件で、`crates/task-core` は 0 件。

## 未解決

- `coord` の印が残ったまま開いた wait が無くなり、秘密 file も消えた場合（resume の直後に file が消えた場合）、印のために新しい
  wait が作られず、受信箱の item も出ないまま task が待つ。従来からの挙動で、この unit では変えていない（コードを読んだ見込み。
  試験では確かめていない。ADR D3）。
- 別の秘密を要する route に替わった atomic task では、前の秘密の wait は開いたまま残る（ADR D2 の照合の範囲）。
  受信箱の item もその wait から出続ける。従来と同じ挙動で、この unit では変えていない。

## 提案

- 上の未解決 1 を直すなら、`ExecutionCoordination` の `credential_held` を消し、`enter_credential_wait` の重複の判断を store の
  開いた wait にする（`execution_coordination.rs` の変更なので、この unit の範囲の外。tick ごとの writer の lock を増やさない形にする）。
