---
title: 健全性v2 G3 loop-points integration-path — WU の check・段の統合・統合の修復の後に health を当てる
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: running
updated: 2026-11-09
---
# 健全性v2 G3 loop-points integration-path — WU の check・段の統合・統合の修復の後に health を当てる

WorkUnit `integration-path`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D1.2・D2.2・D3.2・D3.4 を WU の check と段の統合に配線した。基準は `$CELERIS_WU_BASE`。1 回目の run は turn の上限の前に区切った（未完。下の「未解決」）。

## 変更（commit の件名）
- 「G3 loop-points integration-path: WU の check・段の統合・統合の修復の後に health を当て、Fail を既存の repair へ、Error を Blocked へ」
- 「G3 loop-points integration-path: health の項目の評価・対象・質問を health_review::check_item に移し、dispatcher の行を減らす」
- 内容
  - `health_review/check_item.rs`（新規）: 入口 `gate::health_gate` の結果を検査の列の 1 行に写す唯一の所（`evaluate`）。Pass・PolicyOnly は合格、Repair は `health:<rule_id>:<hash>…` の不合格、Error・Unavailable は `health_check_error: <理由>` の不合格、NotApplicable は列に足さない。`run_item` が開始・終了の event（`health` 欄に証跡）を書く。`phase_base`（段の基点 = 段の最初の unit の `base_commit`）、`work_unit_target`（base = `wu.base_commit`、head = WU の HEAD）、`error_question`、`repair_buckets`（rule ごとの bucket）、`check_error`。
  - `dispatcher/phase_integration.rs`: `run_integration_checks` に引数 `health` を足し、検査の後に health の項目を同じ列の最後に足す（WU の check と段の統合の両方がここを通る）。`start_integration` は検査が無くても統合後に health を評価する。`on_integration_finished` は Error を `integration_gives_up` に渡し、`integration_gives_up_with` は `health_check_error` の理由では replan しない（Blocked と受信箱の質問）。`schedule_integration_check_repair` は health の Fail を `RepairClass::Health` で修復し、`max_repairs_per_class` を `health:<rule_id>` ごとに数える。修復の後の再統合は新しい head で評価し直す（前の証跡を使わない）。
  - `dispatcher/work_units.rs`: WU の check に health の対象を渡す。Fail は checks の不合格として WU の repair（retry）に、Error は run を質問（`health_check_error: …`）にすり替えて Blocked にする。
  - `task-core/src/execution.rs`: `RepairClass::Health`（bucket `health`、予算 30/1800）。`classify_command` は cmd `HEALTH_CHECK_ID` を `health:` の reason だけ Health に分け、Error は修復しない。
  - 試験の調整: `integration_check_progress.rs`・`scope_checks.rs`（引数 `None` と `.results`）、`target_env.rs`（health の項目は command の log が無いので除く）。

## 試験（守る不変条件）
- `dispatcher/tests/health_enforce.rs`: `health_enforce_work_unit_check_records_evidence`・`_fail_routes_to_work_unit_repair`・`_error_blocks_task`、`health_enforce_phase_integration_records_evidence`・`_fail_schedules_repair`・`_unavailable_blocks_task`、`health_enforce_integration_repair_reevaluates_new_head`。入口の試験は分類だけを見て、経路が列に足すか・repair と Blocked のどちらに回すか・修復の後に評価し直すかを通らない。
- `health_review/check_item_tests.rs`: `health_enforce_integration_repair_buckets_per_rule`・`_error_is_read_only_from_health_item`。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_enforce_(work_unit_check|phase_integration|integration_repair)_/)'` → exit 0（9 passed）
- `cargo nextest run -p task-dispatch -E 'test(/health_enforce_|integration_check|scope_check/)'` → 50 passed・1 failed（`target_env` は直した後に単独で passed）
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 1（下の未解決）
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings` は未実行

## 未解決
- check-health が exit 1: `size.responsibility-growth` が worsened（`Dispatcher::start_integration` 221 行、`work_units.rs` の本体 2206 行、dispatcher module の合計の純増）、`deps.cycle` が worsened（dispatcher ↔ health_review の辺が増えた）、source-size（`work_units.rs` が比較基準 2181 行を超える）。次の run は dispatcher の追加の行を `health_review::check_item` に移すか既存の行を減らして純増 0 にし、`deps.cycle` は dispatcher から health_review への新しい file の組を増やさない形（呼ぶ file を `phase_integration.rs` の 1 か所にまとめる）を試す。
- WU の health の head は commit（WU の HEAD）なので、未 commit の WU の変更は WU の check では評価されない（段の統合で評価される）。D2.7 の dirty の report は commit の head では起きない。
- 段の統合の health は先頭の git の repo だけを評価する（検査と同じ）。参照の無い単一の作業場所は repo の id を全桁 0 にした。

## 提案
- 無し。
