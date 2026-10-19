---
title: arm-set-e2e — 実 CLI・所有 stub で A/B のみ・B/C のみを検証し手順書を更新
status: done
completed: 2026-10-19
tasks: [01M59KXY8345HZEE06FVZ3FWVP]
---
# arm-set-e2e

## 区分
- **実装済み**: 全体台帳の試行数上限（`SetFixed.approved_trials`、宣言の最小）、上限は既存行より増やせない（`cap_*` 行欄。旧行は serde default で無制限＝互換）、判定と予約を flock 内で原子的に実施、自分の取り分の試行数は allocation か期待試行数。manifest の公開構造体に必須欄は増やしていない（`SetFixed` は Default）。
- **無 LLM 検証済み**（`cargo test -p celeris-campaign --test arm_set_e2e`、8 件）:
  - 実 CLI の validate→dry-run→env-lock→run(mock, `--global-ledger`)→check→export を A/B のみ（C が出ない・6 試行）と B/C のみ（A が出ない・4 試行）で通し、期待試行数・台帳の trials・export の configs/比較表が一致。同じ測定 ID で有効 arm を変えた resume は CLI で `resume rejected`。
  - 呼出し記録（Counting executor）で未使用 arm の呼出し 0、費用の集計一致。所有 stub の通常 `POST /api/v1/tasks` に B/C の payload（arm・routing_override）だけが届き、A は POST されない。
  - B1/C1 の候補・tier・harness 不一致と単一モデル pin の拒否。B0 の非 Sonnet run は既存 `experiment_sets_exec` の checker 試験（B0 全役割）が担保。
  - 全体台帳: 別測定 ID の budget 拡大でも上限不変（試行数・provider 枠）、同時 6 投入で 1 件だけ通る、resume の予約は二重計上しない、旧形式行の互換。
- **実 pilot 未確認**: 実 LLM・実 daemon の run、実 token の費用照合。
- **承認待ち**: 最終セットの preflight と本測定（統合 01M58VG5C4XGK4E70M6WJ6MEX0）。承認値は fixture のみ使用、推測していない。

## 証拠
- `cargo test -p celeris-campaign --test arm_set_e2e`: 8 passed。`--test experiment_sets_exec` と lib: 全 passed。
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 未解決・提案
- CLI の `preflight`/`lock` の実 API 経路（A 不要時の probe 0）は実 daemon の catalog・probe が要るため、CLI 通しでは env-lock の入力検査（B/C で A の program を探さない）までに留めた。最終 preflight で確認する。
- 予約は最初の試行の記録で実測値に置き換わる（実行中の in-flight 予約を保持する拡張は未実装）。
- `test-parallel.sh` は 5843 passed / 1 failed（`a_program_pin_and_watch::watch_stop_and_restart...`、単独 2 回は pass。負荷時の時間依存と見ている）。
- `--all-targets` の clippy は task-dispatch の既存 `type_complexity`（本 WU 外）で落ちる。
