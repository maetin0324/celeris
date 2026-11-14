---
title: "健全性v2 R4 fix-promote: WU の done の後に落ちても依存 WU が ready に上がるよう、dispatch gate の昇格を計画の版によらず行う"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 fix-promote

完了日: 2026-11-14

## 概要

cex-lease の未解決の反例（WU a の done を書いた直後に落ちると、v1 の計画で b・c が pending のまま残り Task が進まない）を
owner の dispatch gate で直した。`Dispatcher::wu_dispatch_gate`（`crates/task-dispatch/src/dispatcher/work_units.rs`）は
`promote_newly_ready` を計画の版によらず呼ぶ。決定は ADR `agent-docs/adr/2026-11-14-dependency-promotion-every-plan-version.md`。
反例は試験 `cex_restart_after_wu_done_promotes_dependents_once` で固定し、修正の前に落ち、後に通ることを確かめた。

## 原因

- `finish_worker_result_with`（`worker_finish.rs`）は WU a の done（`work_unit_transition_with_events`）、依存 WU の
  `dependency_ready`（`side_effect_rows` ごとの `work_unit_transition`）、Task の遷移・`runs` の閉じ・`WorkerFinished` を
  別々に書く。`test_hooks.halt_after_wu_transition` は a の done の直後で打ち切る。
- 再起動の後、新しい active は run A を孤児として回収し、Task を ready に戻す（`infra_requeue`）。
- dispatch gate は依存の状態からの昇格を v2 の計画だけで行っていた。v1 の計画では `task_core::next_work_unit` が
  `Stuck` を返し、失敗した WU も依存の失敗も無いので `Skip` を返し続けた。

## 修正

- ADR 2026-11-14-dependency-promotion-every-plan-version を先に書いた。ready は SQLite の依存の状態から
  `ready_promotions`（`task_core::newly_ready`）で導く値で、v2 の gate・`finish_phase_integration`・`reconcile_tree_units` は
  既に同じ関数で上げている。
- `work_units.rs` の `promote_newly_ready` の呼び出しから計画の版の条件を外した。`reopen_stale_stage_integrations` は
  段のある計画だけなので v2 のまま。
- v1 の計画で gate と finish の側の集合が同じであることはコードで確かめた（ADR D2）。finish の側は a を done にした行に
  置き換えた集合を、gate は store の行を `newly_ready` に渡す。違いは a の `updated_at`・`last_run_id` だけで、判定はこれらを
  読まない。v1 の計画には統合 WU も段も無い。昇格を取り消す `hold_for_auto_leaf`・`hold_for_worker_decisions` は a を
  `blocked(decision)` にするので、store の行からも上がらない。試験でも b・c の `dependency_ready` は 1 件ずつだった。違いは無く、
  check を落とす理由は無い。

### Objective の前提との違い

Objective は 1 transaction 化を採らない理由を「Task の trigger を伴わない複数 WU 行の store method が無い」と書いていた。
今の HEAD には `TaskStore::work_units_apply`（WU 行の挿入・更新と events を 1 transaction で書き、Task の状態は変えない）がある。
ADR D3 には事実どおりに書き、採らない理由を次の 3 つにした: 1 回にしても Task の遷移・`runs` の閉じ・`WorkerFinished` との
窓は残り前進は再起動の gate に頼ること、finish の書き込みの順の組み直しはこの修正の範囲を超えること、finish の書き込みの成否を
正本にすると正本が 2 つになること。

## 実装前 brief

1. 根本原因: dispatch gate が依存の状態からの昇格を v2 の計画だけで行い、v1 では finish の側の書き込みだけが昇格させていた。
   症状は再起動の後に Task が ready のまま進まないこと。
2. 責務 owner: `Dispatcher::wu_dispatch_gate`（Ready な Task の次の一手を決める）。
3. 既存の再利用先: `promote_newly_ready` → `phase_integration::ready_promotions` → `task_core::newly_ready`。試験は
   `cex_lease_restart.rs` の helper（`new_active`・`run_until_state`・`wu_transitions`）。
4. 選択肢: (a) gate の条件を外す（採用。正本は store の依存の状態の 1 つ）。(b) `work_units_apply` で a の done と依存 WU を
   1 transaction にする（不採用。ADR D3）。
5. 新しい状態・分岐・fallback・port: 無し（分岐を 1 つ消した）。
6. 撤去対象: gate の v2 の条件。既存の試験 `cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent` の
   「今の実装で成り立たない」の記述。
7. 不変条件: 依存の満たされた pending の WU は、計画の版によらず次の dispatch で ready に上がり、同じ WU の
   `dependency_ready` は 1 件。試験 `cex_restart_after_wu_done_promotes_dependents_once`。

## 試験

### 足した試験

`cex_restart_after_wu_done_promotes_dependents_once`（`crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs`）。
v1 の計画 a → b → c を `halt_after_wu_transition` の dispatcher で走らせ、a の done の直後で finish を止める（Task は Running、
A の `WorkerFinished` 無し、b・c は pending）。新しい active で再起動し、Task が Done に届くこと、b・c の `dependency_ready` が
1 件ずつ、a の遷移は dispatch と completed の 2 件だけ（走らせ直さない）、A の `WorkerFinished` は 1 件、`WorkerStarted` は 3 件を
確かめる。もう一度再起動して `reclaimed == 0`、event の数が変わらないことを確かめる。
守る不変条件と既存の試験で検出できない理由は試験の doc comment に書いた。

### 修正の前後

- 修正の前（試験だけを足した状態）: `cargo nextest run --locked -p task-dispatch -E 'test(cex_restart_after_wu_done_promotes_dependents_once)'`
  は exit 100、1 failed。`run_until_state` の保険（60 秒）まで進まず、60.3 秒で `status=Ready b=[] c=[]` の assert で落ちた
  （b・c の遷移が 1 件も無い）。
- 修正の後: 同じ試験は合格（下の証拠）。

### 変えた既存の試験

`cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent` は、v1 の gate が昇格しない（Task が ready で止まる）
ことを前提に、最初の再起動の 1 tick の後で dispatcher を落とし、2 回目の再起動で何も足さないことを見ていた。修正の後は最初の
tick で b が上がって走るので、そこで落とすと b の run が新しい孤児になり、2 回目の再起動が正しく回収した（`reclaimed` が 1 で
落ちた）。最初の再起動を Task が Done に届くまで回し、その後の再起動で何も足さないことを見る形に変え、`WorkerStarted` の確認を
「A だけ」から「3 件、A は 1 件」に変えた。理由は doc comment に書いた。回収の冪等性（A の `WorkerFinished` は 1 件、a は
走らせ直さない、2 回目の再起動は event を足さない）の assert は残した。

## 変更（件名）

- 「fix-promote: dispatch gate の依存 WU の昇格を計画の版によらず行い、WU の done の後の再起動の反例を試験で固定する」
- 「fix-promote: promote_newly_ready の doc comment を元の行数に戻し、work_units.rs の source-size の伸びを無くす」

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- 受け入れ条件 0: `grep -n 'v2 && self.promote_newly_ready' crates/task-dispatch/src/dispatcher/work_units.rs` は 0 件（exit 1）。
  ADR `agent-docs/adr/2026-11-14-dependency-promotion-every-plan-version.md` がある。
- 受け入れ条件 1・2: `cargo nextest run --locked -p task-dispatch -E 'test(/cex_/) | test(/orphan_work_units/) | test(/phase_integration/)'`
  は exit 0、59 passed。
- `bash scripts/dev/test-affected.sh`: exit 0、3942 passed（11 skipped）。対象は celeris・celeris-campaign・celerisctl・
  llm-proxy・task-dispatch。repo health は verdict=pass、fail_grade 0、new 1（`migration.remnant` の warning。この変更と無関係で、
  前回の最終 review の記録にも同じものがある）。
- 最初の test-affected は repo health の `size.responsibility-growth`（`wu_dispatch_gate` が 204 行で閾値 200 を超えた）で
  exit 1 だった。gate のコメントを 2 行に戻して再実行し、上の結果になった。
- 1 回目の提出の後、celeris の WU の check `celeris:repo-health/1`（基準 `$CELERIS_WU_BASE`）が 2 件の fail で落ちた:
  `size.responsibility-growth`（`work_units.rs` の本体 2085 行が閾値 2000 を超えたまま伸びた）と `source-size`
  （`check-source-size.py` の `production_size` が value 2086・allowed 2085）。原因は `promote_newly_ready` の doc comment を
  3 行から 4 行に伸ばしたこと。doc comment を 3 行に戻して `work_units.rs` を基準と同じ 2085 行にした（コードは変えていない）。
  `python3 scripts/dev/repo-health/repo_health.py --repo scripts/dev --base "$CELERIS_WU_BASE" --head worktree --policy-version 1`
  は exit 0、verdict=pass、fail_grade 0、new 0。`check-source-size.py --base "$CELERIS_WU_BASE"` は exit 0（0 failure）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は ADR・`work_units.rs`・`cex_lease_restart.rs`・この進捗の 4 件。

## 未解決

- 未確認: finish が a の done を書いた後、worker の決定の要求（`DecisionRequested` と他の unit の `blocked(decision)`。
  `work_units_apply` で書く）の前で落ちると、その要求は残らず、再起動の後に gate が依存 WU を上げる。finish の側も
  `dependency_ready` を決定の要求の前に書くので、この順序は修正の前から v2 の計画で同じで、この変更で新しく生じたものではない。
  試験は無い。

## 提案

- `finish_worker_result_with` の WU の書き込み（a の done・依存 WU・決定の要求の行）を `work_units_apply` の 1 回にまとめると、
  上の未確認の窓も閉じる。finish の書き込みの順を組み直すので、別の unit で扱う。
