---
title: P0-F 修正 pilot driver の usage 配線（CoS chat run と cache_creation）
tasks: [01M6P8HSXJRFFZ92NZEQCF22JS]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P0-F 修正: pilot driver の usage 配線

結論: `driver.py` の usage 集計を `collect_usage` に切り出し、CoS の chat run の usage を tokens に入れ、key 名を記録と同じにした。
usage が null の起動が 1 件でもあれば `usage.missing` に件数付きの理由を書き、tokens を null にする。実 LLM・本番には触れていない。
決定は ADR [2026-11-05-phase3-p0f-usage-counting](../../adr/2026-11-05-phase3-p0f-usage-counting.md)。

## 変更
- `tokens` は daemon の予算（`task_ops::project_budget`）と同じ input+output。cache（read+creation）は別欄 `usage.cache_tokens`。
- 旧い key `cache_creation_input_tokens` の読み取りは消した（記録の key は `cache_creation_tokens`）。
- 実 LLM の harness で usage が null: `missing` = `run_usage_null:<n>` / `cos_usage_null:<n>`（両方は `,` 区切り）、`tokens`・`cache_tokens` は null。
  grade は tokens が null の試行を「N trials have no usage」と数えるので summary に欠けが出る（`crates/campaign/src/long_horizon.rs` の `usage_missing`。変更なし）。
- 試験: `scripts/dev/long-horizon-pilot/test_driver_usage.py`（5 件）。`scripts/dev/test-long-horizon-pilot.sh` に登録。

## 証拠
提出時の HEAD で実行: `test_driver_usage.py` 5 件 OK（exit 0）、`test-affected.sh` exit 0（対象 crate なし・文書検査と python 試験 OK）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 提案
- `events_append_only` は driver の固定値 True のまま（任意項目 (d) は見送り）。決定的な検査（events の行数・hash が単調に増えることの照合）への置き換えは別 task で行う。
- operator は P0-F の手順で実 LLM pilot を再実施し、CoS の tokens が入ることを確かめる（未実施）。
