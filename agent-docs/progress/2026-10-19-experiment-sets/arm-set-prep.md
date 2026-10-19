---
task: 01M59KXY8345HZEE06FVZ3FWVP
wu: arm-set-prep
status: done
completed: 2026-10-19
---
# arm-set-prep
- 実装: crates/campaign の preflight.rs / lock.rs / env_lock.rs / main.rs（env-lock の A 解決のみ）。ADR 付記は agent-docs/adr/2026-10-19-experiment-sets.md。
- 試験: tests/preflight.rs（A/B のみ・B/C のみ・有効 arm 欠落は失敗）、tests/env_lock.rs（A/B・B/C・宣言替えの拒否）、
  tests/campaign_lock.rs（有効 arm のみ・set hash・宣言なしは欄なし）。
- 証拠: `cargo test -p celeris-campaign --test preflight --test env_lock --test campaign_lock` 全通過（preflight 25 / env_lock 17 / campaign_lock 7）。
- lock_exec/lock_tree/env_check/cap_probe/measure_env/jev_smoke は arm 直書きが無い／有効 arm の config のみを使うため変更なし
  （env_check の manifest_release は存在する B/C だけを見る）。
- 未解決: main.rs の run 経路で A 無効時の a_program 取り扱いは arm-set-exec 側。
