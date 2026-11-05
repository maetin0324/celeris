| label/cond | 窓の秒 | 停止 | RSS 開始→終了 kB | 傾き 0→1529 秒 MB/時 | 傾き 600→1529 秒 MB/時 | RssAnon 傾き 0→1529・600→1529 MB/時 | RssAnon Δ kB | malloc in_use Δ kB | malloc free Δ kB | SQLite 終了 kB | V8 used 最大 kB | threads | arena 推定 | 残差 kB | dispatch p50/p95 ms |
|---|---:|---|---|---:|---:|---|---:|---:|---:|---:|---:|---|---|---:|---|
| main/off | 1500 | wall | 92928→71672 | -49.8 | -176.5 | 39.3・21.5 | 16760 | 11774 | 7849 | 0 | 0 | 27→29 | 27→31 | -2864 | 167/519 |
| main/v8_load | 1500 | wall | 92736→72776 | -46.8 | -175.8 | 41.0・20.0 | 17480 | 11577 | 8970 | 0 | 0 | 27→29 | 27→31 | -3068 | 169/477 |
| main/shadow | 1500 | wall | 120192→88368 | -74.6 | -220.8 | 45.6・23.8 | 19440 | 13648 | 12151 | 0 | 2539 | 33→35 | 33→37 | -6104 | 173/488 |
| main/enabled | 1500 | wall | 120000→86268 | -79.1 | -218.7 | 42.4・24.3 | 18108 | 12935 | 11992 | 0 | 2538 | 33→35 | 33→37 | -6564 | 181/561 |

本番の観測（2026-11-04、shadow）: 181820→588668 kB／1,529 秒、傾き 全体 935.5・定常 746.9 MB/時。
- main/off: pass {"anon_steady_slope_vs_off_le_10mb_h": true, "v8_generations_le_2": true, "runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
- main/v8_load: pass {"anon_steady_slope_vs_off_le_10mb_h": true, "v8_generations_le_2": true, "runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
- main/shadow: pass {"anon_steady_slope_vs_off_le_10mb_h": true, "v8_used_heap_end_lt_2x_start": true, "v8_generations_le_2": true, "runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
- main/enabled: pass {"anon_steady_slope_vs_off_le_10mb_h": true, "v8_used_heap_end_lt_2x_start": true, "v8_generations_le_2": true, "runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
