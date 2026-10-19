---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 全dispatcher run開始点にworker pinを渡す（dispatch-worker-pin）
work_unit: dispatch-worker-pin
status: done
updated: 2026-10-19
---
# dispatch-worker-pin

## 変更（task-dispatch のみ）
- `variant_pin.rs`: `variant_start_check` を `variant_run_pin(task, selected)` に置き換え（旧関数は削除）。祖先連鎖の解決・org drift・assignee を再検査し、variant 付きなら `CampaignRunPin`（identity・全 node digest `org_effective_digest`・selected=実際の route・daemon_instance_id・config_sha256）を組む。観測値が無い・pin の `validate` が通らないなら `Err`（欠落を許さない）。variant なしは `Ok(None)`。`Dispatcher::set_variant_observation(instance_id, config_sha256)` と `VariantObservation` を追加。
- `dispatch_run.rs`: lease・`WorkerStarted` の前の再検査で pin を作り `extras.campaign` に入れる。失敗は従来どおり `block_variant_task`（run 無し）。`dispatch_one` は retry・continuation・repair・並列 WU・子孫の共通起動点。
- `worker_task.rs`: worker_task の `RunContext` literal に `campaign: extras.campaign.clone()`。`run_context.rs` の `RunExtras` は `campaign: None` で初期化。
- 対象外 run（eval_trials・cos_chat・review）の literal は `..RunContext::default()` で `campaign: None`（変更なし。review は pin の対象外）。

## 試験（`tests/variant_pin.rs`、名前に `variant` を含む 17 件のうち新規 4）
正: root と子の run が adapter（stub）に同じ identity・digest・selected・instance/config 付きの `context.campaign` を届ける／variant なし task は `None`。負: daemon 観測値なしは blocked で run 無し／起動時の org drift は blocked で run 無し。

## 証拠
- `cargo test -p task-dispatch --lib variant` → 17 passed
- `cargo test -p task-dispatch` → 966 passed、0 failed
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- 参考: `clippy -p task-dispatch --all-targets` は既存の `tests/trial_context.rs` の type_complexity で落ちる（本変更と無関係、未変更ファイル）

## 未解決事項
- `crates/celeris/src/daemon/run.rs` から `set_variant_observation(identity.instance_id, config.source_sha256)` を呼ぶ配線は範囲外（daemon 所有）。未配線の daemon では variant 付き task は blocked になる（安全側）。
- 他 callsite（api/celeris/ctl）と protocol schema は後続葉。
