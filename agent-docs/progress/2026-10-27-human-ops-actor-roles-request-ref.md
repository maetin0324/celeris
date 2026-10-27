---
title: human-ops /2 — 主体の種類・役割・論理要求の参照と、主体別・論理要求単位の report
tasks: [01M5XXF7Q8133JDMR051R4MHJX]
status: done
updated: 2026-10-27
---
# human-ops /2 — 主体の種類・役割・論理要求の参照と、主体別・論理要求単位の report

完了日: 2026-10-27

判断は [ADR 2026-10-27-human-ops-actor-roles-request-ref](../adr/2026-10-27-human-ops-actor-roles-request-ref.md)
（ADR 2026-10-19-eval-raw-human-ops-record に付記）。

## 変更

- `crates/campaign/src/human_ops.rs`: schema `celeris.eval-human-op/2`。`ActorKind` を 7 種（human・script・cos_project・
  cos_general・external_operator・mcp_client・unknown）に。`decided_by`・`executed_by`・`approved_by`・`credential`（op で必須、
  `CREDENTIAL_KINDS` か `operator:<name>`/`mcp:<id>`）・`request_ref`/`request_revision`（op・wait、両方か無しか、record_id の材料）・
  `policy_ref` を追加（全行で欄の存在が必須）。役割・credential・policy_ref は correction で直せる。CLI の `--actor-kind` を必須にし
  既定の human を消した。`--decided-by/--executed-by/--approved-by <kind>:<id>`・`--credential`（既定 unknown）・
  `--request-ref/--request-revision`・`--policy-ref` を追加。
- `crates/campaign/src/human_ops_report.rs`: `counts_as_human_op` は actor.kind=human かつ manual のみ。介入を request_ref で
  1 件にまとめ `intervention_items[].history`（event initial/revision/repeat）に並べる。`interventions_by_actor_kind`・
  `unknown_actor`（件数と区間）・`excluded_ops.non_human` を試行・arm・shared に出す。report.md に主体別・unknown 主体の列。
- 文書: `docs/ops/eval-raw-record-definitions.md`（§1・§2.1・§2.3・§3.1・§3.3 新設・§5・§6）、`docs/ops/eval-human-ops-runbook.md`、
  `docs/ops/mlsys-main-4-pilot-runbook.md` の command に `--actor-kind human`。

## 試験（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `crates/campaign/tests/human_ops_report.rs::request_revisions_count_once_and_only_human_manual_time_counts`:
  同じ request_ref の 3 revision → 介入 1・history 3（initial/revision/revision）、CoS の 30 分・unknown の 10 分・問いが開いていた 35 分は
  人の時間に入らず union 600 秒、unknown は `unknown_actor` に 1 件 600 秒、主体別件数。
- `crates/campaign/tests/human_ops.rs::v2_fields_validate_and_old_rows_are_rejected`: 新しい欄の無い行・未知の kind・/1 schema を拒否。
- `crates/campaign/tests/human_ops_cli.rs::record_rejects_invalid_input_and_conflicting_rewrite`: `--actor-kind` 無しを拒否、/2 欄が行に入る。
- 結果は下の「検査」。

## 検査

- `bash scripts/dev/test-affected.sh`: exit 0。nextest 2722 passed・9 skipped、check-doc-links ok、progress-index --check ok。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 本番への移行

- 既存の /1 の `human-ops.jsonl` は新しい版では読めない（拒否）。file は消さず残す。本測定は /2 で記録を始める。/1→/2 の変換は作らない（ADR 移行節）。
- config・DB の変更は無い。

## 未解決事項

- 主体の種類は記録者の申告で、認証 principal（起票元の A1/A3）との照合は無い。A3 を実装するときはこの語彙を使う。

## 提案

- CoS・MCP の操作を human-ops に自動で書く経路（cos_operations・mcp_calls から actor.kind と credential を導出）があると申告の誤りが減る。

## delivery-repair: verify の CoS 背景処理を停止

- commit: `phase 1: verify の CoS 背景処理を外して煙試験への干渉を防ぐ`
- main をこの worktree に fast-forward で取り込んだ。元の checkout・本番サービス・staging は変更していない。
- prepare.log は build・全体試験の成功後、verify の smoke が reviewing のまま60秒で失敗したことを示す。
  staging のログでは dispatch 区間が約21秒・38秒。ready task の eligible filter は既に存在したが、
  同区間で呼ばれる CoS intake・fallback・root 完了通知は filter を通らず snapshot 全体を対象にしていた。
- [決定](../adr/2026-10-27-verify-without-cos-background.md): verify の最初の tick 前に CoS launch 登録を外す。
  `cos.enabled=false` だけでは intake が残るため登録自体を解除する。通知ループ起動時の CoS route cutover も verify では抑止する。通常運用の fallback と煙試験の60秒条件は維持。
- 回帰試験を既存 `instance_handoff` の verify 試験へ追加。`cos.enabled=true` でも CoS inbox・chat run・
  triage/root-done cursor を書かず、fake smoke の worker→review→done を通す。
- 修正前: 同試験は exit 101、CoS cursor が期待0に対して2行あり失敗。時間・負荷に依存せず不要な経路を検出した。
- 修正後と必須検査の結果は以下へ記録する（提出時の HEAD、最後の main 同期後。基準は `git merge-base HEAD main`）。

### 残る確認

- 実 snapshot 上で60秒以内に完了するかは次回配送の verify check 6 で確認する。今回 release・deploy は実行しない。
- phase の個別計測が無いため、過去ログの dispatch 時間すべてが CoS に由来するとは断定しない。

### 再検査結果（提出時の HEAD、最後の main 同期後）

- `cargo test -p celeris --test instance_handoff verify_mode_never_dispatches_and_never_touches_daemon_instances -- --nocapture`: exit 0、1 passed（1.49秒）。CoS cursor/inbox/chat run は0件、smoke は done。
- `bash scripts/dev/test-affected.sh`: exit 0、2729 passed・9 skipped（183.412秒）。human_ops・human_ops_cli・human_ops_join・human_ops_report を含む。文書・進捗・source-size 検査も成功。既存 `dagbench_pbs_e2e` の `ExtBackend` 未使用import警告1件。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `git diff --check`: exit 0。
