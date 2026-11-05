# ADR 2026-11-05: 第 3 フェーズ P0-F — pilot の facts.usage は daemon の予算と同じ input+output で数え、cache は別欄に出す

- 状態: 採用。
- 根拠: `agent-docs/progress/2026-11-05-phase3/p0f-usage.md`。実 LLM pilot（C06・C08 new、2026-11-05T16:13Z）で
  `facts.usage` が実消費の一部しか数えていなかった。

## 問題
`scripts/dev/long-horizon-pilot/driver.py` の `collect` に 2 つの欠陥があった。
1. CoS の起動（`chat_runs`）を `invocations` に足すとき usage を `None` に固定し、`chat_runs.usage_json` を読まなかった。
   CoS 14 件の消費が tokens に入らず、`usage.missing` も立たなかった。
2. `runs.usage_json` の和の key が `cache_creation_input_tokens` で、記録の key（`task_core::Usage` の
   `cache_creation_tokens`）と合わず、常に 0 を足していた。

## 決定
- D1: `facts.usage.tokens` は daemon の予算（`task_ops::project_budget`、`input_tokens + output_tokens`）と同じ数え方にする。
  対象は案件の task の runs と、案件の CoS thread の chat_runs の和。pilot の facts と daemon の guard の上限が同じ量を指す。
- D2: cache は `Usage::budget_tokens` に含めず、別欄 `facts.usage.cache_tokens`（`cache_read_tokens + cache_creation_tokens`）に出す。
  理由: cache 込みにすると予算（input+output）と食い違い、pilot の消費と guard の停止が比べられなくなる。別欄なら両方を後から読める。
- D3: key 名は記録と同じ `input_tokens` `output_tokens` `cache_read_tokens` `cache_creation_tokens`。旧い
  `cache_creation_input_tokens` の読み取りは残さない。
- D4: 実 LLM の harness で usage が null の run があれば `usage.missing` に `run_usage_null:<件数>`、chat run なら
  `cos_usage_null:<件数>`（両方なら `,` 区切り）を書き、`tokens` と `cache_tokens` を null にする。grade は `tokens` が null の試行を
  「N trials have no usage」と数えるので、欠けは summary に出る。stub は従来どおり `tier=stub`・tokens null。
- D5（任意項目）: `events_append_only` は driver が書く固定値のままで、決定的な検査に置き換えない。この ADR では変えない（提案として進捗に残す）。

## 試験
`scripts/dev/long-horizon-pilot/test_driver_usage.py`: 一時 SQLite（runs・chat_runs・chat_threads）に usage 入りの行と null の行を入れ、
`collect_usage` に当てる。
