---
title: 健全性v2 G3 loop-points integration-path — WU の check・段の統合・統合の修復の後に health を当てる
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: running
updated: 2026-11-09
---
# 健全性v2 G3 loop-points integration-path — WU の check・段の統合・統合の修復の後に health を当てる

WorkUnit `integration-path`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D1.2・D2.2・D3.2・D3.4 を WU の check と段の統合に配線した。基準は `$CELERIS_WU_BASE`。2 回目の run で check-health の fail と test-affected の 5 件の不合格を直した。全体の test-affected の再実行は wall clock の残りが足りず次の run に回した（下の「未解決」）。

## 変更（commit の件名）
- 「G3 loop-points integration-path: WU の check・段の統合・統合の修復の後に health を当て、Fail を既存の repair へ、Error を Blocked へ」
- 「G3 loop-points integration-path: health の項目の評価・対象・質問を health_review::check_item に移し、dispatcher の行を減らす」
- 「G3 loop-points integration-path: check_item を dispatcher::phase_integration の子 module にし、検査の項目の event 記録を共通化して dispatcher の行と依存の循環を戻す」
- 「G3 loop-points integration-path: WU の check の health の項目に合わせて G2 の scenario の stub の report と drain の引き継ぎの試験を直す」
- 内容
  - `health_review/check_item.rs`: `dispatcher::phase_integration` の子 module として `#[path]` で読む（`design_gate.rs`・`policy_review.rs` と同じ形。dispatcher から `crate::health_review` を参照すると `deps.cycle` に health_review が加わるため）。持つもの:
    - 検査の列の 1 項目の開始・終了の event（`record_start`・`record_finish`）。command の検査と health の項目が同じ関数で `WorkUnitCheck*`・`IntegrationCheck*` を書く（1 回目の run で二重に書いていた event の組み立てを 1 か所にした）。
    - health の項目（`run_item` → `evaluate` → 入口 `gate::health_gate`）。Pass・PolicyOnly は合格、Repair は `health:<rule_id>:<hash>…` の不合格、Error・Unavailable は `health_check_error: <理由>` の不合格、NotApplicable は列に足さない（event には証跡）。評価の thread が落ちたら Error。
    - 対象: `phase_target`（段の基点 = 段の最初の unit の `base_commit`、無ければ統合前の HEAD。head は merge の後に `at_head`）、`Dispatcher::work_unit_health`（base = `wu.base_commit`、head = WU の HEAD）。
    - 経路の扱い: `put_rows`（段の統合の記録の行。検査の無い段は auto_resolve の行を残す）、`health_error`、`Dispatcher::settle_health_row`（WU の列の health の行に cmd を付け、Error なら run を質問にすり替える）、`repair_count`（修復の bucket。health は `health:<rule_id>` ごとに数える）。
  - `dispatcher/phase_integration.rs`: `run_integration_checks` は引数 `health` を受け、検査の後に health の項目を同じ列の最後に足す（WU の check と段の統合の両方がここだけを通る）。`start_integration` は検査が無くても統合後に health を評価する。`on_integration_finished` は Error を `integration_gives_up` に渡し、`integration_gives_up_with` は `health_check_error` の理由では replan しない（Blocked と受信箱の質問）。`schedule_integration_check_repair` は health の Fail を `RepairClass::Health` で修復する。修復の後の再統合は新しい head で評価し直す。
  - `dispatcher/work_units.rs`: WU の check に health の対象を渡し、結果を `settle_health_row` に通す。Fail は checks の不合格として WU の repair（retry）、Error は質問で Blocked。
  - `task-core/src/execution.rs`: `RepairClass::Health`（bucket `health`、予算 30/1800）。`classify_command` は cmd `HEALTH_CHECK_ID` を `health:` の reason だけ Health に分け、Error は修復しない。
  - 試験の調整: `integration_check_progress.rs`・`scope_checks.rs`（引数 `None`）、`target_env.rs`（health の項目は log が無いので除く）、`drain_hand_off.rs`（新しい active の WU の検査は 2 件の後に health の項目〈index 2、total 3、Pass〉）、`health_review/scenario_tests.rs`（stub の report の new の finding を fail 級でない warning・verdict pass・exit 0 にした。G3 では new の fail 級の error は WU の check で repair に回り final review に届かないため。final review が new の finding を材料に受け取ることの確認は残る）。

## 試験（守る不変条件）
- `dispatcher/tests/health_enforce.rs`: `health_enforce_work_unit_check_records_evidence`・`_fail_routes_to_work_unit_repair`・`_error_blocks_task`、`health_enforce_phase_integration_records_evidence`・`_fail_schedules_repair`・`_unavailable_blocks_task`、`health_enforce_integration_repair_reevaluates_new_head`。入口の試験は分類だけを見て、経路が列に足すか・repair と Blocked のどちらに回すか・修復の後に評価し直すかを通らない。
- `health_review/check_item_tests.rs`: `health_enforce_integration_repair_buckets_per_rule`・`_error_is_read_only_from_health_item`、`health_enforce_phase_integration_rows_keep_auto_resolve_without_checks`（検査の無い段で auto_resolve の行が消えない。流れの試験は検査のある段しか通らない）。

## 証拠（2 回目の run の HEAD で実行）
- `bash scripts/dev/check-health.sh` → exit 0（verdict pass、fail_grade 0、new 1 = facts の WU の `dup.contract` の warning）。`Dispatcher::start_integration` 205 行（基準 207）、`work_units.rs` 本体 2180 行（基準 2180）、dispatcher module の合計 35122 行（基準 35163）、`deps.cycle` の dispatcher の循環は 3 単位（基準と同じ）。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo nextest run -p task-dispatch -E 'test(/health_review_flow|health_review_scenario|drain_hand_off|health_enforce_|health_review_/)'` → 111 passed（自分の prefix の試験は 10 件）。
- `bash scripts/dev/test-affected.sh`（test の直しの前の commit で実行）→ exit 100: 8069 件中 8064 passed・5 failed（`drain_hand_off` 1 件、`health_review_flow` 2 件、`health_review_scenario` 2 件）。5 件は上の試験の直しで合格した（上の 111 件に含む）。直した後の全体の再実行はまだ。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と計画の正規表現）→ 範囲外の path なし。

## 未解決
- 直した後の HEAD で `bash scripts/dev/test-affected.sh` を通しで再実行していない（約 19 分。この run の wall clock の残りが足りなかった）。次の run で実行し exit 0 を確かめる。
- WU の health の head は commit（WU の HEAD）なので、未 commit の WU の変更は WU の check では評価されない（段の統合で評価される）。D2.7 の dirty の report は commit の head では起きない。
- 段の統合の health は先頭の git の repo だけを評価する（検査と同じ）。参照の無い単一の作業場所は repo の id を全桁 0 にした。

## 提案
- 無し。
