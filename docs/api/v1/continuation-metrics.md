# 実行 metrics の continuation 比較

---
tasks: [01M3Y2KXVXJ6CFH2XRS98W452J]
---

`GET /api/v1/metrics/execution` の `continuation` は全タスクの worker run、`groups[].continuation` は既存の `group_by` に属する worker run の合計です。`continuation_by_work_unit` と `groups[].continuation_by_work_unit` は WU ID をキーにして同じ値を返します。atomic run のキーは `task:<task_id>` です。`GET /tasks/{id}/execution` の `metrics.continuation` はイベントから作るタスク単位の値です。

| 欄 | 意味 |
| --- | --- |
| `fresh` | `usage.session_resumed = false` の run |
| `resumed` | `usage.session_resumed = true` の run |
| `unknown` | 旧イベントなど `session_resumed` が無い run |
| `runs` | worker run 数 |
| `wall_ms` | 各 run の `RunMetrics.wall_ms` の和。並列 run の時間は重なっても加算 |
| `input_tokens` | `input_tokens + cache_read_tokens + cache_creation_tokens` の和 |
| `duplicate_reads` | run 内で重複した Read / Grep / Glob の観測回数の和 |
| `fresh_fallback_by_reason` | dispatch の `continuation session: fresh (reason=...)` の理由別件数。`not_continuation`、`independent_wu`、`role_fresh` は除く |

旧 run の未報告値は各合計では 0 として扱い、run 自体は `unknown.runs` に残します。run 索引が無い旧イベントはタスク合計に含めますが、WU 所属を復元できないため WU 別には出しません。before/after の比較では `fresh` / `resumed` を使い、導入前の run を fresh と推測しません。理由は run の進行イベントに記録されたものだけ数えます。`since` は従来どおり更新されたタスクの選択に適用し、選択されたタスクの全 run を集計します。
