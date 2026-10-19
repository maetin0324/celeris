---
task: 01M59KY5FNM0K4XGPPS61D0CA2
wu: ext-sim
status: done
completed: 2026-10-19
---
# ext-sim: 外部 job stub と独立観測 ledger

- 実装: `crates/campaign/src/dagbench/{mod,ext}.rs`（`ExtSim`）。論理 ID 冪等 submit・`lookup` 再照合・論理時計 `advance`・`record_consumed`。ledger は別 JSONL file（受理/物理起動/完了/通知/消費/lookup/fault_fired）。`open` で再生して状態を復元。
- fault: `FaultSpec{event, seed, occurrences}` の `nth = 1 + seed % occurrences` 件目で発火（`DropResponse` / `DuplicateNotice`）。`fault_fired` に fault_id・seed・nth・event_seq・logical_secs・論理 job ID を記録。再読込後は再発火しない。
- 実 process を起こさないため kill 対象は無い（本番 daemon・無関係 job に触れない）。
- 試験 `cargo test -p celeris-campaign --test dagbench_ext`: 4 passed（応答喪失で実起動 1、重複通知で消費 1、再読込で状態復元、枠超過）。sleep なし。
- `tests/runner_checker.rs` の src 走査を再帰化（dagbench/ サブディレクトリで落ちたため。禁止語の検査は dagbench にも効く）。
- 検査: test-parallel は 5819 passed / 1 failed（上記 runner_checker、修正後は通過を個別に確認）、clippy --workspace 0、fmt 0。
## 未解決・提案
- baseline / celeris-drv が実子 process を起こす場合、runner_checker の `process::Command` 禁止の許可名単位に追加が要る。
- `cargo clippy --all-targets` は既存 tests（env_lock.rs 等）で落ちる（本 WU 外）。
