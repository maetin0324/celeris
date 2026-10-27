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
