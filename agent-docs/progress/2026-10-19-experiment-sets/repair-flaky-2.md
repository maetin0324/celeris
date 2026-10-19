---
title: repair-flaky-2 — 負荷下で落ちる campaign の実時計依存試験 2 件の決定化
status: done
completed: 2026-10-19
tasks: [01M59XT2H8KMZZDBFZYC81WGNR]
---
# repair-flaky-2

- `a_program_pin_and_watch.rs`: `RealSleep`（1s × 最大 60 回）を `StepClock` に置換。間隔を待たず 1ms 譲るだけで進め、反復上限は `u32::MAX`（回数で偶然打ち切らない）、保険の壁時計上限 300s を超えたら panic。終了は「外部 process の停止確認で台帳が閉じる」出来事で決まる。再起動で再投入しない・上限超過で停止の主張は不変。
- `launcher.rs`: 固定の `HEALTH_WAIT_SECS=60`（daemon 起動の保険上限）を 600 に。負荷で起動が遅れると偽の失敗になっていた。拒否の主張は不変。
- 本体 src は触っていない。
