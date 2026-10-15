---
task: 01M4Z2B2HA2FEK1ZRWKS8MXSRR
unit: cw-enter
status: done
completed: 2026-10-15
---
# cw-enter: credential 未登録で待機へ移し、受信箱 item と通知を 1 件に集約

ADR 2026-10-15-mlsys-min-decision-assisted D4 の名前をそのまま使う（`waiting_for_auth`・`secret_stored`・集約キー `credential:<secret_id>`）。新しい名前は作っていない。

## 実装
- 入口: `task-dispatch/src/dispatcher/credential_gate.rs`。経路が選ばれた後・lease を取る前（`dispatch_run.rs`）で、pick の source が `CredentialGates` に宣言され、秘密が未登録（空でない file が無い。値は読み捨て）なら、その work unit だけを `blocked(waiting_for_auth)`（`WorkUnitBlockedReason::WaitingForAuth` を追加）へ移し `credential_waits` に記録。task の Status・兄弟 unit は触らない。冪等（行を読み直す）。計画なし task は `task:<id>` を 1 unit として扱う。
- 宣言: `[llm_sources.<id>] wait_for_credential = true`（+ `credential_ref` 必須、`credential_purpose` 任意）。`Config::credential_gates()`、bootstrap・reload で `set_credential_gates`。
- 復元: bootstrap で `restore_credential_waits()`（`credential_waits_list_waiting`）。cancel 済み task の wait は閉じ、wait だけ書かれて unit が未遷移のものは待機へ移す。
- 受信箱: `InboxKind::CredentialWait`。`credential_waits`（開いている小さな表。events は走査しない・読み取り接続）から派生し、集約キーごとに 1 item（`credential_wait-<secret_id>`、created_at は最初の wait で固定）。用途・待つ task 一覧・`/accounts` リンク・「登録すれば自動で続行」・誤キー時は「更新してください」。answer は native 操作（PUT /api/v1/secrets/{id}）へ委ね、汎用 answer では拒否。
- 通知/Discord: 新しい送信経路は作っていない。受信箱 item は既存の CoS triage（source_kind=item.kind, key=item.id, revision=created_at）に載り、待つ task の増減・誤キー更新では revision が変わらないので通知は 1 件のまま。
- worker: `selected_llm_route` は credential 参照（名前）だけ。値は adapter/broker 境界。試験で値が wait 行・inbox・event・route に出ないことを確認。
- schema: `docs/api/v1/api-v1.schema.json` を再生成（enum 追加のみ）。

## 証拠
- `cargo test -p task-dispatch credential_gate`: 7 pass（該当 unit のみ待機、同一秘密 2 task で item 1 件、再起動復元・途中落ち補完、cancel 済み閉じ、計画なし、値が出ない）。
- `cargo test -p task-ops human_inbox`: 20 pass（kind 全種、同秘密 1 item・誤キー更新で id/created_at 不変）。
- config 試験 `wait_for_credential_builds_a_gate_and_requires_a_credential_ref`。
- 初回 `test-parallel.sh`: 5167 pass / 2 fail（kind 試験の fixture 不足、`request_lock_counts` の writer lock → `credential_waits_list_waiting` を読み取り接続へ）。両方修正。

## 未解決・提案
- 受信箱 item は既存 policy で CoS triage run を 1 回起こし得る（待機中の worker/LLM run ではないが CoS）。「CoS を起こさない」を厳密にするなら triage から `credential_wait` を除く決定が要る。
- 計画なし task は登録後、検証を待たず gate を通る（cw-resume で検証と claim を足す際に atomic 側も揃える）。
- web/gui の生成型は未再生成（cw-web 担当。`InboxKind` に `credential_wait`、blocked reason に `waiting_for_auth`）。
- Jev 用の source 宣言（`wait_for_credential`）は Jev adapter 側の設定で行う。本番 config の変更は無し。
