---
wu: size
status: done
completed: 2026-11-06
---
# size: dispatcher.rs の行数を source-size gate の上限以下に戻す

## 変更
- `crates/task-dispatch/src/dispatcher.rs` から `budget_rates` の欄（doc コメント 3 行 + 欄）と `Dispatcher::new` の初期化を消した（-4 行）。
- 欄は `dispatcher/tree_units.rs` の `StagnationWatch.budget_rates` に移した（木の状態）。`set_budget_rates`（project_wake.rs）、`planner_flow.rs`、`project_wake.rs` の参照は `self.stagnation.budget_rates` に変えた。`set_stagnation` は作り直しのとき `budget_rates` を引き継ぐ。
- 挙動は変えていない。`scripts/dev/source-size-baseline.json`・`source-size-report.toml`・`crates/campaign` は変えていない。

## 証拠（提出時の HEAD で実行）
- `python3 scripts/dev/check-source-size.py --base b8109768d162`: 失敗 0。dispatcher.rs は 2774 行（変更前 2778）。
- `cargo nextest run -p task-dispatch`: 1413 件すべて成功。
- `cargo clippy -p task-dispatch --all-targets -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0（一度差分が出たため `cargo fmt --all` で直した）。
