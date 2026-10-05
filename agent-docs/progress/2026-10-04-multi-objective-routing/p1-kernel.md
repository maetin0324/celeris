# Phase 1: model_router kernel

`task-core::model_router` に `profiles`、`context`、`policy`、`estimator`、`optimizer`、`trace` を追加した。ModelProfile と DeploymentProfile を別の型にし、同じモデルの複数 deployment が wire model 名と価格 override を独立に持てる。SourceState は観測値を Option として保持し、未観測を 0 とみなさない。

Policy は ADR §3.3 の lane 別初期値と mode を持ち、weights と正規化値を検証する。HeuristicEstimator は profile の品質指数だけを参照する純粋関数。Optimizer は制約を先に検査し、品質 floor を通った候補だけを量子化した score と設定順で並べ、provider allowlist を返す。account の確定と予約は呼び手に残す。legacy_rank は入力の既存選択順を保存する。

RoutingRecord.optimizer は optional で、旧 event JSON の欠落を許容する。指定された四つの task-core 試験で profile 分離、制約の優先、安定順位と未知値、旧 event の読み取りと audit の互換を確認した。

検証: `cargo test -p task-core routing_ --lib` 成功、`cargo clippy --workspace -- -D warnings` 成功。
