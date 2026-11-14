---
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: accepted
---
# ADR 2026-11-14: 計画の無い task も credential の検証と resume を経るまで起動しない

- 状態: 採用（実装は task 01M6TYVJT4S29ZMP6P0D4831JG の WorkUnit fix-cred）
- 関係: [ADR 2026-10-15-mlsys-min-decision-assisted](2026-10-15-mlsys-min-decision-assisted.md) の D4（credential 待ち）を、
  計画の無い（atomic）task について具体にする。[ADR 2026-11-11 Dispatcher の状態の owner](2026-11-11-dispatcher-state-ownership.md)
  の D1（`credential_held` の owner は `ExecutionCoordination`）と D7（反例は repair か replan に回す）に従う。
  [ADR 2026-11-14 run を起こすかの順序](2026-11-14-dispatch-admission-order.md) の `CredentialGate` の fact の意味は変えない。

## 背景

ADR 2026-10-15 D4 は、credential 待ちの遷移を `credential_waits` の `waiting|rejected → verified → resumed` の一方向と決め、
resumer（`credential_resume::CredentialResumer`）が検証に成功して `claim_resume` を取ったときだけ待ちを解くとした。

WU のある task では、待ちに入った WU は `blocked(waiting_for_auth)` の行になり、resumer の `requeue_work_unit` が戻すまで
dispatch の対象にならない。計画の無い task（atomic task）には WU の行が無い。`work_unit_id = "task:<id>"` の wait だけがあり、
dispatch の gate（`dispatcher/credential_gate.rs` の `credential_gate_hold`）は `CredentialGates::is_registered`（秘密 file が
空でないか）だけを見ていた。このため、秘密 file が置かれた後は、検証が一時失敗（`VerifyOutcome::Transient`）しても、
誤キー（`rejected`）でも、次の tick で run が起きた（`agent-docs/progress/2026-11-11-health-v2-r4/cex-review.md` の未解決 2）。
daemon の再起動の後も同じで、`restore_credential_waits` が `ExecutionCoordination` の印を戻しても、gate は印を見ずに file を見ていた。

## 決定

### D1 正本は SQLite の `credential_waits`

atomic task の credential の待ちの正本は、SQLite の `credential_waits` の `task:<id>` の行である。gate は毎回 store の
開いた wait（`credential_waits_list_waiting`、`waiting`・`rejected`・`verified`）を読み、その task と今の route の
`secret_id` の組の行があるかで判断する。`task-core` の store・migration・列は変えない（既存の読み取りだけを使う）。

### D2 atomic task は開いた wait が resumed になるまで hold を外さない

`credential_gate_hold(task, None, pick)` は、`pick` の source に credential の宣言があり、`task:<id>` と その `secret_id` の
開いた wait があれば、秘密 file の有無に関わらず `true`（run を起こさない）を返す。wait が resumer の `claim_resume` で
`resumed` になる（または cancel で `cancelled` になる）と開いた wait が無くなり、そのあとは従来どおり `is_registered` で決める。

- 検証の一時失敗（`waiting` のまま）・誤キー（`rejected`）・検証済みで claim 前（`verified`）は、どれも hold を保つ。
- 開いた wait が無く、秘密が未登録なら、従来どおり wait を作る（`enter_credential_wait`）。wait を作る前の重複の判断も
  store の開いた wait で行う（D3）。
- 開いた wait が無く、秘密が登録済みなら通す。最初から登録済みで一度も待たなかった task は検証を経ない（ADR 2026-10-15 D4 と同じ。
  検証は待ちに入った wait にだけかかる）。
- 照合は今の route の `secret_id` で行う。別の秘密の wait は、その秘密を要する route でだけ hold する。

### D3 `ExecutionCoordination` の `credential_held` は写しで、判断に使わない

`credential_held` はメモリの写しで、正本ではない。gate と `enter_credential_wait` は印の有無で判断しない
（以前は印があると wait を作らずに戻ったため、resume の後に秘密 file が消えると、wait も受信箱の item も無いまま task が止まり得た。コードを読んだ見込みで、試験では確かめていない）。
写しは開いた wait を見たとき・wait を作ったときに立て、gate を通したときに消す（daemon の起動時の `restore_credential_waits` も立てる）。

写しを残すのは、この変更の書き込み範囲（`crates/task-dispatch/src/dispatcher/`）に `execution_coordination.rs` が無いためである。
撤去の条件は「`credential_held` を読む判断が無いこと」で、この変更の後は満たす。撤去は `ExecutionCoordination` の owner の変更で行い、
R4 の record-fix（D1 の表を締め直す unit）か後の task が、D1 の表の `credential_held` の行と一緒に消す。

### D4 WU のある task の挙動は変えない

`credential_gate_hold(task, Some(wu), pick)` は従来どおり、未登録なら wait を作って WU を `blocked(waiting_for_auth)` にし、
登録済みなら通す。WU の Blocked 行が resume まで dispatch を止める。`dispatch_admission` の `CredentialGate` の fact
（`credential_hold`、「この unit を credential の待ちに置いたか」）の意味と、admit の順序は変えない。

## 試験

`crates/task-dispatch/src/dispatcher/tests/cex_review_authority.rs` の `cex_credential_atomic_task_waits_for_verified_resume`:
atomic task の wait を作り、秘密 file を置いて検証を一時失敗させた後の tick、daemon の再起動（同じ store で新しい dispatcher と
`restore_credential_waits`）の後の tick、retry の期限の後の tick、`verified` で claim 前の tick（再起動の前後）で、
`TickReport.dispatched = 0`・runs 索引に行なし・task は ready のままを確かめる。resumer の claim で `resumed` になった後に初めて
run が 1 本起きることを確かめる。

## 戻し方

`git revert`（store の列・migration は変えていないので、DB の移行は要らない）。
