| label/cond | 窓の秒 | 停止 | RSS 開始→終了 kB | 傾き 0→1529 秒 MB/時 | 傾き 600→1529 秒 MB/時 | RssAnon 傾き 0→1529・600→1529 MB/時 | RssAnon Δ kB | malloc in_use Δ kB | malloc free Δ kB | SQLite 終了 kB | V8 used 最大 kB | threads | arena 推定 | 残差 kB | dispatch p50/p95 ms |
|---|---:|---|---|---:|---:|---|---:|---:|---:|---:|---:|---|---|---:|---|
| before/off | 390 | wall | 79104→114676 | None | None | None・None | 12724 | None | None | None | None | 27→28 | 26→30 | None | 69/188 |
| before/shadow | 390 | wall | 122304→139464 | None | None | None・None | 6408 | None | None | None | None | 33→35 | 33→37 | None | 82/149 |
| after/off | 390 | wall | 91392→113392 | None | None | None・None | 9904 | 7901 | 5738 | 0 | 0 | 27→28 | 27→30 | -3736 | 54/148 |
| after/shadow | 390 | wall | 118464→137408 | None | None | None・None | 9152 | 11202 | 5670 | 0 | 2034 | 33→34 | 33→36 | -6440 | 89/174 |

本番の観測（2026-11-04、shadow）: 181820→588668 kB／1,529 秒、傾き 全体 935.5・定常 746.9 MB/時。
- before/off: pass {"runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
- before/shadow: pass {"runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
- after/off: pass {"v8_generations_le_2": true, "runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
- after/shadow: pass {"v8_used_heap_end_lt_2x_start": true, "v8_generations_le_2": true, "runs_and_leases_after_restart_0": true, "not_auto_stopped": true}
