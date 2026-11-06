| label/cond | 窓の秒 | 停止 | RSS 開始→終了 kB | 傾き 0→1529 秒 MB/時 | 傾き 600→1529 秒 MB/時 | RssAnon 傾き 0→1529・600→1529 MB/時 | RssAnon Δ kB | malloc in_use Δ kB | malloc free Δ kB | SQLite 終了 kB | V8 used 最大 kB | threads | arena 推定 | 残差 kB | dispatch p50/p95 ms |
|---|---:|---|---|---:|---:|---|---:|---:|---:|---:|---:|---|---|---:|---|
| main/off | 34560 | None | 81984→201236 | -8.3 | -153.1 | 45.7・23.1 | 117524 | 52334 | 75281 | 0 | 0 | 27→26 | 26→41 | -10092 | 463024/11745252 |
| main/v8_load | 34560 | None | 81984→217128 | -4.9 | -146.1 | 50.5・29.4 | 133224 | 54600 | 81515 | 0 | 0 | 27→35 | 26→42 | -2892 | 312424/11578559 |
| main/shadow | 34560 | None | 110784→386780 | -28.9 | -175.9 | 57.9・38.6 | 287900 | 53412 | 253807 | 0 | 5255 | 33→41 | 32→52 | -20344 | 311723/11667716 |
| main/enabled | 34560 | None | 124032→405112 | -60.9 | -187.8 | 47.1・29.7 | 298936 | 60989 | 270022 | 0 | 5344 | 33→43 | 33→53 | -30796 | 326786/11810373 |

本番の観測（2026-11-04、shadow）: 181820→588668 kB／1,529 秒、傾き 全体 935.5・定常 746.9 MB/時。
- main/off: FAIL {"anon_steady_slope_vs_off_le_10mb_h": true, "v8_generations_le_2": true, "not_auto_stopped": false}
- main/v8_load: FAIL {"anon_steady_slope_vs_off_le_10mb_h": true, "v8_generations_le_2": true, "not_auto_stopped": false}
- main/shadow: FAIL {"anon_steady_slope_vs_off_le_10mb_h": false, "v8_used_heap_end_lt_2x_start": false, "v8_generations_le_2": true, "not_auto_stopped": false}
- main/enabled: FAIL {"anon_steady_slope_vs_off_le_10mb_h": false, "v8_used_heap_end_lt_2x_start": true, "v8_generations_le_2": true, "not_auto_stopped": false}
