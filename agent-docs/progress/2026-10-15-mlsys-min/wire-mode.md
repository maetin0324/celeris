---
unit: wire-mode
status: done
completed: 2026-10-15
---
# wire-mode

ADR `agent-docs/adr/2026-10-15-mlsys-min-routing-mode-wiring.md`。

## 変更
- `[routing] mode`（既定 fixed、未知は読み込みエラー）: `crates/celeris/src/config/routing_mode.rs`、bootstrap/reload で `Dispatcher::set_routing_mode`。
- 純粋部 `task-core/route_select/assist.rs`（`assist_lane`、`ClassifierStatus`、`LaneAssistRecord`、`REAL_API_VERIFIED=false`）。`RouteDecision.lane_assist`（None は出力しない）。
- dispatcher: `dispatcher/lane_assist.rs`（`ClassificationBoard`、`apply_lane_assist`）。`dispatch_run.rs` に 2 か所のフックのみ。
- 分類段（Jev 呼び出し）はこの unit に含めない（classify-stage が書く）。
- 配線の試験: `daemon::admin::routing_mode_wire_tests`（build_dispatcher と reload_providers の両方で Dispatcher に届く）。
- schema 再生成（`UPDATE_SCHEMA=1`、`node web/scripts/gen-types.mjs`）。

## 受け入れ条件と証拠
0. 既定 fixed・未知は読み込みエラー: `cargo test -p celeris --lib routing_mode` 3 passed。
1. fixed は従来と同一: 既存 route_select 試験は無変更で通過。`default_mode_is_fixed_and_writes_no_lane_assist`（board に分類があっても無視、lane 同一、`lane_assist` なし）。
2. 明示・天井・利用可否が分類に勝つ: `assist_tests`（`explicit_lane_beats_classification` `ceiling_and_allowed_tiers_beat_classification` `unavailable_lane_beats_classification`）と dispatcher の `explicit_human_lane_beats_classification`。
3. 失敗で fixed へ戻り `fell_back_to_fixed`: `injected_failures_fall_back_to_fixed_and_real_api_stays_false`（breaker_open / timeout / key_missing、`real_api_verified=false`）。KeyMissing/Auth は既存の credential 待ちへ: `missing_key_enters_the_existing_credential_wait_for_that_unit_only`。
4. 分類は cheap へ引き下げない: `classification_never_lowers_and_never_makes_cheap`、`classification_only_raises_and_never_lowers_below_fixed`。成功率は分類にも board にも載せない（confidence は分類の確信）。
5. dispatcher/store に Jev 直接呼び出しなし: `dispatcher_and_store_never_reference_the_jev_classifier`（task-dispatch・task-core の src と Cargo.toml を走査）。
- `bash scripts/dev/test-parallel.sh`: 5328 passed / 0 failed / 13 ignored、`test-parallel: ok`。`cargo clippy --workspace -- -D warnings`: exit 0。

## 本番への移行
不要。`[routing]` は任意で既定 fixed のため、本番 config は書き換えない。decision-assisted を試すときだけ人が `[routing] mode = "decision_assisted"` を足す（その前に下の未解決事項の分類段の起動が要る）。

## 未解決事項
- 分類段（`RequestClassifier` 注入・`JevClassifier` 呼び出し・`ClassificationStage` の構築・起動）は classify-stage の範囲。その（`JevConfig` の欄、`ApiKeyProvider` の secrets_dir 実装、ready な unit を分類して board へ置く tick 連携）は未配線。mode=decision_assisted でも board が空なら `not_attempted` で fixed と同じ。live smoke（有料・人の承認）の後に配線する。
- Jev 実 API 統合は未検証（`real_api_verified=false`）。
- decision_log（`classifier` / `routing_mode` 欄）への書き込みは未接続。判断 record の `lane_assist` が正で、log-wire の投影への追加は後続。
- replan 提案は frontier 要求に丸めた（計画の直し自体は wire-escalation）。

## 提案
- 分類段の起動 unit で、WU の ready 化時に分類して board を埋め、鍵登録後は board を差し替えて unit を再開する。
