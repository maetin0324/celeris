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
- 開いた wait が無く、秘密が未登録なら、従来どおり wait を作る（`enter_credential_wait`）。
- 開いた wait が無く、秘密が登録済みなら通す。最初から登録済みで一度も待たなかった task は検証を経ない（ADR 2026-10-15 D4 と同じ。
  検証は待ちに入った wait にだけかかる）。
- 照合は今の route の `secret_id` で行う。別の秘密の wait は、その秘密を要する route でだけ hold する。

### D3 `ExecutionCoordination` の `credential_held` は cache で、hold の判断に使わない

`credential_held` はメモリの写しで、正本ではない。run を起こすかの判断（gate の `true`・`false`）は D1 の store の wait だけで決める。
印は `enter_credential_wait` が「この process で同じ wait を既に入れた」ことを覚え、tick ごとに `credential_wait_enter`
（writer の lock を取る冪等の入れ直し）を呼ばないための cache としてだけ使う。gate を通したときに消す。

- daemon の起動時の `restore_credential_waits` は、atomic task の印を立て直さない。gate が store の wait で hold し、最初の
  `enter_credential_wait` が印を立てる（そのときの `credential_wait_enter` は開いた wait をそのまま返し、新しい行も受信箱の item も作らない）。
- 印が残ったまま開いた wait が無くなり、秘密 file も消えた場合（resume の直後に file が消えた場合）は、印のために新しい wait が
  作られない。これは従来からの挙動で、この変更では変えていない（コードを読んだ見込み。試験では確かめていない）。

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

## 付記 2026-11-14 cred-cache: atomic task の待ちのメモリの写しを消す（D3 を置き換える）

task 01M6TYVJT4S29ZMP6P0D4831JG の最終 review の advisory（dual_source_of_truth・removal_missing）を受け、WorkUnit cred-cache で
D3 の「印」（`ExecutionCoordination` が持っていた atomic task の集合）と、印を立てる・消す 2 つの method を消した。
atomic task の credential の待ちは D1 の store の `credential_waits` だけが持つ。D3 の「再起動で `restore_credential_waits` が
印を戻す」という旧い説明も、写しと一緒に消えた（fix-cred が復元の処理を既に消していた）。

- gate（`credential_gate_hold`）: atomic task で、今の route の `secret_id` の開いた wait が store にあれば、入り直さずに `true`。
  開いた wait が無く秘密が登録済みなら `false`。それ以外は `enter_credential_wait` で待ちに入って `true`。WU のある task は
  開いた wait を見ず、従来どおり（D4）。
- `enter_credential_wait` は写しを見ずに store の `credential_wait_enter` を呼ぶ。store は同じ unit と秘密の開いた wait が
  あればそれを返し（`created = false`）、行も受信箱の item も増やさない。受信箱の item の印と triage の reconcile は
  `outcome.created` のときだけ行う（従来どおり）。lane assist の入口（`lane_assist.rs`）も同じ関数を通る。
- D3 が写しを置いた理由（tick ごとに writer の lock を取る入れ直し）は、gate が開いた wait を読み取り接続
  （`credential_waits_list_waiting`）で見つけたら入り直さないことで無くなった。
- D3 の最後の項目（印が残ったまま開いた wait が無くなり、秘密 file も消えた場合に新しい wait が作られない）は、印が無いので起きない。
  開いた wait が無く秘密が未登録なら、gate は新しい wait を作る（コードを読んだ見込み。試験では確かめていない）。
- 安全確認は変えない: resumer の検証、再開の claim（`claim_resume`）、予算の stop、`dispatch_admission` の `CredentialGate` の
  fact の意味と順序。
- 試験: `crates/task-dispatch/src/dispatcher/tests/cex_review_authority.rs` の
  `credential_wait_for_atomic_task_is_entered_once_across_ticks`（tick 3 回と再起動の後の tick 3 回で、wait 行 1 件・受信箱の
  item 1 件・task の event の数が変わらない・run を起こさない。秘密 file の無い場合と、置いたが検証前の場合）。
- [ADR 2026-11-11 Dispatcher の状態の owner](2026-11-11-dispatcher-state-ownership.md) の D1 の表には、消した field の行が
  残っている。表の更新は WorkUnit cred-cache の範囲外で、進捗
  `agent-docs/progress/2026-11-11-health-v2-r4/cred-cache.md` の提案に書いた。
