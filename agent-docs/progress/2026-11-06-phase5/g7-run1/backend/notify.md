---
title: process 起動直前の永続通知を spawn と同じ関数に入れる（WU notify）
tasks: [01M6SQJF9YD3WPTD8NSB191PZW]
status: done
completed: 2026-11-07
---
# process 起動直前の永続通知（WU notify）

完了日: 2026-11-07。正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md)（D1・D2・D4）。新しい設計判断は無く、ADR の追加は無い。

## 変更（commit は件名で列挙）
- `phase 1: 起動前通知の契約試験(修正前に失敗)と EventSink::before_spawn`
- `phase 2: spawn 直前に起動前通知を送る(全 adapter)`

## 実装
- `EventSink::before_spawn(&self) -> Result<(), String>`（`crates/task-worker/src/adapter.rs`、既定 `Ok`）。呼び出し側（dispatcher の sink）が実体（`chat_run_mark_launched`）を渡す。
- `subprocess::spawn_after_notice(sink, &mut command)`: 通知 → `Err` なら `AdapterError::Other`（spawn しない）→ `command.spawn()`（失敗は `AdapterError::Spawn`、通知は取り消さない）。
- 使用箇所（`command.spawn()` を直接呼ばない）: `run_subprocess`（fake・config-stub）、claude_code、codex、pi、acp（opencode）、aider、langmem、local_deep_research、paperqa の `pqa ask`。
- 再送: route failover の 2 本目以降や codex の再試行でも通知は呼ばれる。初回時刻を書き換えないのは永続側 `chat_run_mark_launched` の冪等性（durable-contract.md）が担う。この WU は dispatcher を変えない。

## 試験と修正前後の記録（`crates/task-worker/src/launch_notice_tests.rs`、実 process の stub、順序は 1 本のログ file の並びで判定）
- 修正前（phase 1 の commit 時点）: `cargo test -p task-worker --lib launch_notice` は exit 101、4 件とも FAILED
  （例: `left: ["spawned"] right: ["notify", "spawned"]`、`notice attempted left: 0 right: 1`）。
- 修正後: 同じコマンドで 4 passed, 0 failed。対応する条件:
  - 0 順序: `notice_is_called_before_the_process_is_spawned`（fake・claude-code・codex・pi。`["notify","spawned"]`、通知 1 回）
  - 1 通知失敗で spawn なし: `a_failed_notice_prevents_the_spawn`（stub は 1 行も書かず、通知の理由が `AdapterError::Other` に載る）
  - 2 fake / config-stub も通る: 上の 2 試験が fake（`run_subprocess`）と claude-code・codex・pi の stub command を同じ契約で検証
  - 3 spawn 失敗後も通知が残る: `a_spawn_failure_keeps_the_notice`（存在しない program で `AdapterError::Spawn`、通知 1 回）
  - 追加: `a_second_launch_sends_the_notice_again`（2 本目の起動でも通知→spawn の順）

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 5560 件 pass（skip 21）、doctest ok。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 未解決・後続へ
- acp と aider は同じ helper に切り替えたが、専用の stub 試験は無い（コンパイルと helper の共通化で担保）。
- dispatcher の sink が `before_spawn` で `chat_run_mark_launched` を呼ぶ配線は launch-wire WU。`Conflict` を `Err` に写して spawn を止める点もそちらで試験する。

## 提案
- launch-wire は CoS chat run の sink だけ `before_spawn` を実装し、task run の sink は既定のままにする。
