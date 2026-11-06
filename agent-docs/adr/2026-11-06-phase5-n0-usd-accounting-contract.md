---
tasks: [01M6QW0GSCHZZGYRNQMNRKRDZA]
---
# ADR 2026-11-06: USD 相当の会計の定義の確認と、1 時間 cache の書き込みを予算で小さく数えない（第5フェーズ N0）

- 状態: 採用。コードの変更は D3 だけ（`task_core::pricing::run_budget_cost`）。
- 決めた人: N0（01M6QW0GSCHZZGYRNQMNRKRDZA）の worker。人の決定（2026-11-06「料金の換算で数える」）と
  [ADR 2026-11-06-budget-usd-caps](2026-11-06-budget-usd-caps.md) の原則「小さく数えない」「0 円と偽らない」の範囲で決めた。
- 関係: [ADR 2026-11-06-budget-usd-caps](2026-11-06-budget-usd-caps.md)（D1-1 を D3 で改める）、
  [ADR 2026-10-27-trial-observed-versions](2026-10-27-trial-observed-versions.md) D3（`PRICE_TABLE_VERSION` は campaign が照合する）。

## 背景
第5フェーズの N1〜N7 は USD 相当の値で合否を決める（pilot の上限、P4-E の上限、費用対効果）。N0 は、USD の会計が
入力・出力・cache read・cache creation と CoS・worker・reviewer を 1 つの定義で数えているかを、`git merge-base HEAD main`
（USD 予算の task 01M6QDK2PMJTXF17Z8TNDKF85M の統合 commit「integrate wu/closeout (phase closeout)」を含む）で読んで確かめた。

## D1 会計の定義（確認した現行の挙動）
| 量 | 予算判定（daemon） | 根拠 |
|---|---|---|
| 1 run の費用 | `task_core::pricing::run_budget_cost(model, usage, rates)`。保存済みの `usage.cost_usd` は使わず、判定のたびに今の単価表で再計算する | `crates/task-core/src/pricing.rs` |
| input・output | 単価表の input・output の単価 | 同上 |
| cache read | cache read の単価（input の 1/10〜1/40） | 同上 |
| cache creation | D3 の単価（この ADR で改める） | 同上 |
| 範囲（案件の日次） | 案件の task の全 run（worker・reviewer・planner・repair。`runs`）と案件 CoS の chat run（`chat_runs`）。UTC の 1 日、`started_at` で日を決める。走っている run と harness 起動前に終わった run は数えない | `SqliteStore::project_run_usage_between`（`crates/task-core/src/store/project_guard.rs`） |
| 範囲（木） | 木の全節点の全 run（reviewer を含む） | `task_core::tree::tree_counters` |
| 統括 CoS（general thread）の chat run | 案件の日次にも木にも入らない（案件に属さない） | 同上の SQL は `chat_threads.project_id` で絞る |
| 単位 | micro-USD（1e-6 USD）に run ごとに切り上げて足す | `usd_to_micros_ceil` |

## D2 欠測は fail-closed（成功扱いにしない）
1. 次の run は「費用不明」1 件として数え、1 件でもあれば上限に達したものとして止める（案件は `BudgetKind::UsdUnknown`・
   `Dangerous`、木は `TreeLimitKind::TreeUsd` の超過）: model の記録が無い、usage が `NULL`・読めない、token 欄が 1 つも無い、
   単価表に無い model（`[budget_rates]` にも無い）、使った token 種の単価が `None`（`gpt-6-*` は全種、`gpt-5` は cache write）。
2. 0 USD として数えるのは、人が `[budget_rates]` に 0 を明示した model だけ。
3. 実験の道具（N1 の pilot・N3 の P4-E）も同じ規則に従う。`compare.py` は 1 run でも `usage.cost_usd` が無ければ費用を
   `null` にして残りの試行を始めない（`usd_unknown`）。pilot の集計は `usage_json` null（`cos_usage_null`）・cache 欄の欠落・
   USD 未算出のどれかがあれば、その試行を合格にしない（N1 の受け入れ条件）。

## D3 cache creation は予算判定で 1 時間 TTL の書き込み単価（input の 2 倍）以上で数える
- 事実: Anthropic の cache の書き込みは 5 分 TTL が input の 1.25 倍、1 時間 TTL が 2 倍（claude-api skill の
  `shared/prompt-caching.md`、2026-10-06 時点の写し）。`PRICE_TABLE` の `cache_write_per_million` は 5 分 TTL の値。
- 事実: Claude Code（worker・CoS）の stream-json の usage は `cache_creation.ephemeral_1h_input_tokens` に書き込みを計上する。
  2026-11-06 07:30Z に直近 39 本の Claude Code run の `stdout.jsonl` の result 行を読むと、cache creation 3,074,916 token は全部
  1 時間 TTL（5 分 TTL は 0）。`Usage` は TTL を分けず `cache_creation_tokens` の合計だけを持つ。
- 影響: Opus 5.5 の単価で数えると、5 分 TTL の単価の判定は 32.62 USD、実際の単価では 41.84 USD で、22.0% 小さく数えていた。
  これは「小さく数えない」に反する（上限を超えても止まらない向き）。
- 決定: `run_budget_cost` は単価表の行の cache creation を `max(cache_write_per_million, 2 × input_per_million)` で数える。
  TTL が分からないので大きい方（1 時間 TTL）に倒す。5 分 TTL だけの run は最大 1.6 倍に多く数えるが、止まる向きの誤差で許す。
- 変えないもの:
  - `PRICE_TABLE` の値と `PRICE_TABLE_VERSION`（`2026-09-approx-1`）。campaign が `GET /health` の `pricing_version` を
    凍結した比較条件として照合する（K2/K4/IEEE Access の凍結を変えない）。
  - `estimate_cost_usd` と、worker が run の終わりに保存する `usage.cost_usd`（campaign の計上と表示に使う）。
  - `[budget_rates]` の行（人が宣言した `cache_write_per_million` をそのまま使う。1 時間 TTL を使う model なら人がその値を書く）。
  - cache write の単価が `None` の行（`gpt-5`）は従来どおり費用不明。
- 却下した案: `Usage` に TTL 別の欄を足して正確に数える。`Usage` の構造体の字句は 34 file・49 か所、API schema・web の型・
  campaign の記録に及ぶ。N0 の範囲を超えるので提案に回す（進捗の「提案」）。

## D4 単価表の妥当性（2026-11-06 の照合）
出典は claude-api skill の model 表（2026-10-06 時点の写し）。値は USD / 100 万 token。

| model | 表の行 | input / output / cache read / cache write（表） | 出典 | 判定 |
|---|---|---|---|---|
| claude-fable-5-1 | `claude-fable-5-1` | 10 / 50 / 0.25 / 12.5 | 10 / 50 / 0.25、write は 1.25 倍 = 12.5 | 一致 |
| claude-opus-5-5 | `claude-opus-5-5` | 4 / 20 / 0.20 / 5.0 | 4 / 20 / 0.20、write 5.0 | 一致 |
| claude-sonnet-5-5・claude-sonnet-5 | `claude-sonnet-5` | 2 / 10 / 0.2 / 2.5 | 2 / 10 / 0.20、write 2.5 | 一致 |
| claude-haiku-5-5 | `claude-haiku`（汎用の行） | 0.8 / 4.0 / 0.08 / 1.0 | 0.10 / 0.50（prompt 10 万 token 以下）、0.50 / 2.50（超え） | 表が 1.6〜8 倍大きい。止まる向きなので予算では許す。費用対効果の表では過大と注記する |
| claude-opus-5（5.5 でない） | `claude-opus`（旧 Opus の行） | 15 / 75 / 1.5 / 18.75 | 5 / 25 | 表が 3 倍大きい（止まる向き） |
| gpt-6-*（codex） | `gpt-6-*` | 全種 None | — | 費用不明で止まる。codex の worker を使う案件は `[budget_rates]` が要る |

- Haiku 5.5・Opus 5 の行の修正は `PRICE_TABLE_VERSION` を変えるので、K2/K4 の凍結を解く人の判断が要る。N0 では変えない。

## 結果
- 予算判定の cache creation は D3 の単価になる。2026-11-06 時点の Claude Code の run（1 時間 TTL だけ）では実際の単価と一致する。
- 予算判定（D3 の単価、再計算）と保存値 `usage.cost_usd`（5 分 TTL の単価、run の時点の表、aider の自己申告を含む）は
  一致しない。実験の道具（`compare.py`・`driver.py` の集計）は保存値を足すので、cache creation の多い試行では予算判定より
  小さい。N1・N3 は USD の上限を保存値で判定するときに、この差を記録する（runbook の受け入れ条件）。
