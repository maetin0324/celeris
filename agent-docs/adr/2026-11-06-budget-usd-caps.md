---
tasks: [01M6QDK2PMJTXF17Z8TNDKF85M]
---
# ADR 2026-11-06: 自律化の予算の上限を料金換算（USD 相当）で判定する

- 状態: 採用（設計）。この ADR ではコードを変えない。実装は同じ task（01M6QDK2PMJTXF17Z8TNDKF85M）の WorkUnit core・scripts・ops・wire-rest・closeout。
- 決めた人: 人（2026-11-06、operator 経由の質問「token の上限を数えるとき、cache の分を含めるか」への回答「料金の換算で数える」）。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の進捗「人が決めること」5 を閉じる。
- 関係: [ADR-0061](0061-coding-harness-routing-foundation.md)（Phase 104 の静的単価表）、[ADR 2026-10-27-trial-observed-versions](2026-10-27-trial-observed-versions.md) D3（`PRICE_TABLE_VERSION`）、[ADR-0079](0079-recursive-task-decomposition.md) D3・D7（木の上限と raise-once）、[ADR 2026-11-04-project-controller](2026-11-04-project-controller.md)（案件の予算）、[ADR 2026-10-20-mlsys-main2-fixes](2026-10-20-mlsys-main2-fixes.md) D1（task の `Budget.max_tokens`）、[破壊的変更](2026-10-11-breaking-changes-by-default.md)。

## 結論
- 案件の日次上限と木の上限は、run の usage を静的単価表（`task_core::pricing`）で USD に換算した和で判定する。cache read・cache write はそれぞれの単価で数える。
- 単価の無い model と、usage の無い終わった run は「費用不明」として数え、不明が 1 件でもあれば上限に達したものとして止める（fail-closed）。黙って 0 USD にしない。
- local の model（llama.cpp・opencode の Qwen など）は、人が config `[budget_rates."<model の接頭辞>"]` に換算単価を明示すれば数えられる。明示が無ければ fail-closed で止まる。
- 既定値: 案件の日次上限 `max_usd_per_day = 100.0`（旧 `max_tokens_per_day = 20000000`）。木の上限 `max_tree_usd` は旧 `max_tree_tokens` と同じく既定は無し（設定したときだけ）。
- 旧欄 `max_tokens_per_day`・`max_tree_tokens` は消し、書いた config は読み込みエラーにする。表示用の token は残す。`crates/campaign` と task の `Budget.max_tokens`（研究の凍結評価）は変えない。

## 背景（現状、`git merge-base HEAD main` で読んだ実装）
token の上限は 3 か所あり、数え方が揃っていない。

| 上限 | 場所 | 数える量 | 範囲 | 既定 |
|---|---|---|---|---|
| `[project_controller.budget] max_tokens_per_day` | `task_ops::project_budget::{usage_on_day, exceeded, severity}`、store は `SqliteStore::project_run_usage_between` | input + output（cache を含めない） | 案件の task の全 run（`runs`。reviewer・planner を含む）と案件 CoS chat run（`chat_runs`）の和。UTC の 1 日。`usage_json IS NULL` の行は SQL で除外（= 0 扱い） | 20,000,000 |
| `[execution.tree] max_tree_tokens` | `task_core::tree::limits::{tree_counters, run_limit_breach, limits_with_allowances}`、`crates/celeris/src/config/execution.rs`、`TreePlannerContext.tokens_left`（`task-worker/src/protocol.rs`）、`TreeLimitsUsage.max_tokens`（`task-ops/src/tree_view.rs`）、`planner_flow.rs`・`tree_units.rs` | input + output。reviewer を含む | 木の全節点の run。`usage` の無い run は `continue`（= 0 扱い） | 無し |
| task の `Budget.max_tokens` | `task-dispatch/src/dispatcher/token_budget.rs` | `Usage::budget_tokens`（input + output + cache read + cache creation） | 予算を持つ最も近い祖先から下の全 run | 無し。campaign が試行ごとに設定 |

`task_core::pricing::estimate_cost_usd(model, usage)` は前方一致で単価表を引き、4 種の token をそれぞれの単価で掛けて足す。単価表に無い model、使った token 種の単価が `None` の model（`gpt-6-*` は全種 `None`、`gpt-5` は cache write が `None`）、token が 1 件も無い usage は `None` を返す（「0 円と偽らない」）。worker（claude_code・codex・aider）と `routing_outcome` は run の終わりにこの値を `usage.cost_usd` に入れる。aider は aider 自身の `Cost:` 行を優先する。`tree_counters` の `cost_usd`・`cost_usd_complete` はこの保存値の和で、表示にだけ使っている。

input + output は実際の処理量を小さく数える。2026-11-05 の P0-F 再実行で C06 は input + output 約 5,500 token、cache read + creation 約 47 万 token だった。cache 込みの token 数は、単価が input の 1/10〜1/20 の cache read を input と同じ重さで数える。人はどちらでもなく料金の換算を選んだ。

## 決定

### D1 判定に使う量
1. run 1 本の費用 = `estimate_cost_usd(model, usage)`。model は run の記録（task run は `runs.model`、CoS chat run は `chat_runs.resolved_config_json` の model）を使う。cache read は cache read の単価、cache creation は cache write の単価で数える（`estimate_cost_usd` の現行の式のまま）。
2. 判定は保存済みの `usage.cost_usd` を使わず、判定のたびに model と usage から再計算する。理由: 保存値には harness の自己申告（aider の `Cost:`）と run の時点の単価表の版が混ざる。判定は 1 つの規則（今の `PRICE_TABLE_VERSION` の表）で決まり、同じ DB から同じ答えが出る。
3. 範囲は現行の token の和と同じ。案件の日次は案件の task の全 run（worker・reviewer・planner・repair）と案件 CoS chat run、木は木の全節点の全 run（reviewer を含む）。走っている run は、木では数えない（現行どおり、次の run を起こす前の判定）。
4. 内部の表現は整数の micro-USD（1e-6 USD、`u64`）。run ごとの費用を micro-USD に切り上げて足す（小さく数えない）。config・API・表示は USD の小数（JSON の number）。整数にする理由: `BudgetLimits` の `Eq`、raise-once の余裕（上限の半分）、`>=` 判定を浮動小数の誤差なしで行える。
5. 換算の関数は `task_core` に 1 つ置き（core WU が名前を決める。例 `task_core::pricing::budget_cost`）、案件・木・表示（`tree_counters` の `cost_usd`）がこれを共有する。返り値は「micro-USD の和」と「費用不明の run の数」の組。
6. model 名に route の `<source>/` 接頭辞が付く記録があるかは未確認。core WU が `runs.model` と `resolved_config_json` の実際の値を確かめ、付くなら引く前に最後の `/` より後を使う規則を同じ関数に置いて試験する。

### D2 単価の無い model と usage の無い run
選択肢と判断:

| 案 | 判断 |
|---|---|
| A. 0 USD として数える | 不採用。pricing.rs の原則「0 円と偽らない」に反し、local model の暴走が上限に掛からない |
| B. fail-closed（費用不明が 1 件でもあれば上限到達として止める） | 採用（既定の挙動） |
| C. config で換算単価を明示させる | 採用（B の上で、人が明示したときだけ数える） |
| D. 近い model の単価で推定する | 不採用。推定の根拠が無く、人が見ない値で判定が変わる |

1. **単価が引けない run**（単価表に無い model、引いた行で使った token 種の単価が `None`、model の記録が無い）は、`[budget_rates]` に一致する行があればその単価で数える。無ければ「費用不明」1 件として数える。
2. **`[budget_rates."<model の接頭辞>"]`**: `input_per_million`・`output_per_million`・`cache_read_per_million`・`cache_write_per_million` の 4 欄すべて必須、有限で 0 以上。前方一致で、長い接頭辞を先に引く。0 も書けるが、それは人の明示の宣言として扱う（黙っての 0 ではない）。単価表で 4 種すべての単価がある行に一致する接頭辞は読み込みエラー（表を上書きさせない。表の正本は `PRICE_TABLE` と `PRICE_TABLE_VERSION`）。この単価は予算の判定にだけ使い、`usage.cost_usd`・campaign の計上・`PRICE_TABLE_VERSION` には入れない。
3. **usage の無い終わった run**（`finished_at` があり usage が `NULL`、または token 欄が 1 つも無い usage）は「費用不明」1 件。例外は harness を起動する前に終わった run だけで、core WU が run の記録から決定的に区別する関数を置いて試験する。区別できない run は費用不明に倒す。CoS chat run は `started_at` のある行だけを数える（queued のままの行は起動していない）。
4. **費用不明が 1 件以上あれば上限到達**として扱う。案件の日次では新しい種類 `BudgetKind::UsdUnknown`（`"usd_unknown"`）で止め、`severity` は `Dangerous`（翌日の自動再評価をしない。翌日も同じ model なら同じく止まるので、人が `[budget_rates]` を書くか route を変える）。木では `TreeLimitKind::TreeUsd` の超過として止め、決定の要求の本文に費用不明の run の数を書く。人の raise-once の回答は不明分を数えないことを意味しないので、不明が残る限り再び止まる（解除は `[budget_rates]` の追記か、上限 `max_tree_usd` を外すこと）。
5. 根拠: compare.py は既に「費用の分からない試行が 1 件でもあれば USD の上限を守れないので止める」（`usd_unknown`）を採っている。本番の cheap lane は local provider（opencode-qwen）を優先するので、C が無いと B だけでは local を使う案件が毎日止まる。C は人が config に値を書いた事実が残るので「偽らない」を満たす。
6. 費用不明で止まるまでに走るのは、判定の前に起きた run だけ（判定は次の run・次の CoS 入力を起こす前）。最大で止まる前の 1 本ぶん不明が増える。現行の token の上限も run 1 本ぶん超えうるので、同じ程度。

### D3 既定値と根拠
| 欄 | 既定 | 根拠 |
|---|---|---|
| `[project_controller.budget] max_usd_per_day` | 100.0 | 実測の換算費用: TS 制御 S6 の worker 50 run の `cost_usd` 和 87.90 USD（1 run 平均 1.76 USD、input + output 1,278,839 token。`agent-docs/progress/2026-11-02-ts-control-layer-s6.md`）、P2-G の小さい root は 1 本 0.87〜1.86 USD（worker 1 + reviewer 1〜2）、P0-F の 2 試行で 0.92 USD。既定の `max_roots_per_day = 5` の root が各 10 run（1.76 USD/run）なら 88 USD、CoS 入力 10 回を足して 100 USD 弱。旧既定 2,000 万 token は S6 の比率（input + output 100 万 token あたり約 69 USD）で約 1,370 USD/日に当たり、事実上効いていなかった。危険の判定（`Dangerous`）は現行どおり上限の 2 倍（200 USD） |
| `[execution.tree] max_tree_usd` | 無し（設定したときだけ） | 旧 `max_tree_tokens` の既定が無しで、木の費用は task の大きさで桁が変わる。上限は人が案件・org profile で決める |
| `long-horizon-pilot/driver.py --max-usd-per-day` | 20.0 | 旧 `--max-tokens-per-day 1000000` の置き換え。P1-h で人に示した停止上限（32 試行で 20 USD 相当）と同じ桁。P0-F の 2 試行は 0.92 USD |
| `general-cos-compare/compare.py` の `max_usd` | 現行の値のまま | 既に `max_usd` がある。`max_tokens` だけを消す |

すべて USD 相当で、OAuth の定額枠では請求額ではない（単価表の概算）。

### D4 config 欄・API field・旧欄
1. config:
   - `[project_controller.budget] max_tokens_per_day` を消し、`max_usd_per_day`（number、有限で > 0）を置く。0・負・NaN は読み込みエラー（現行の「0 は上限なしではなく誤り」I12 を引き継ぐ）。
   - `[execution.tree] max_tree_tokens` を消し、`max_tree_usd`（number、任意、有限で > 0）を置く。
   - 新しい表 `[budget_rates."<model の接頭辞>"]`（D2-2）。
   - 旧欄は両方とも `deny_unknown_fields` で読み込みエラーになる。試験は旧欄を書いた TOML が拒否され、メッセージに旧欄名が出ることを固定する（`config/script.rs` の付記 S5-1 と同じ形）。
2. Rust の型（名前は WU が合わせてよいが、旧名を残さない）:
   - `project_budget::BudgetLimits.max_tokens_per_day` → `max_usd_per_day_micros: u64`。`BudgetKind::Tokens`（`"tokens"`）→ `Usd`（`"usd"`）と `UsdUnknown`（`"usd_unknown"`）。`DANGEROUS_TOKEN_FACTOR` → `DANGEROUS_USD_FACTOR`（2）。
   - `BudgetUsage` に `usd_micros`・`usd_unknown_runs` を足す。`tokens` は表示用として残す（D5）。
   - `TreeLimits.max_tree_tokens` → `max_tree_usd_micros: Option<u64>`。`TreeLimitKind::TreeTokens`（`"max_tree_tokens"`）→ `TreeUsd`（`"max_tree_usd"`）。`TreeCounters` に `usd_micros`・`usd_unknown_runs`。`tokens` は表示用で判定に使わない。`limit_allowance_step` は micro-USD の半分（切り上げ）。
3. API・protocol:
   - 案件の controller の view（`project_controller_view`）: `limits.max_tokens_per_day` を消し `limits.max_usd_per_day` を置く。`budget_today` に `usd`・`usd_unknown_runs`・`price_table_version` を足し、`tokens` は表示用で残す。
   - `GET /tasks/{id}/task-tree` の `TreeLimitsUsage`: `max_tokens` を消し `usd`・`max_usd?`・`usd_unknown_runs` を置く。`tokens` は表示用で残す。
   - worker protocol `TreePlannerContext.tokens_left` を消し `usd_left: Option<f64>` を置く。
   - `docs/api/v1/api.md`・schema・web の生成型と `web/e2e/parity/project-controller.spec.ts` を同じ変更で直す。
4. 過去の記録: events と判断の記録に残った `"tokens"`・`"max_tree_tokens"` の文字列は書き換えない（追記専用）。判定はそれらを読まない。未回答の決定の key が `max_tree_tokens` のものが残っていると新しい binary は種類を読めないので、本番への移行の前に件数を確かめる（進捗の「本番への移行」節）。
5. 道具: `compare.py` は plan・approval の `limits.max_tokens` と `stop_reason` の `"max_tokens"` を消し、`max_usd` と `usd_unknown` に一本化する（approval に `max_tokens` があれば preflight の問題として拒否）。`driver.py` と `long-horizon-pilot-llm.sh` は `--max-tokens-per-day` を消して `--max-usd-per-day` にし、生成する config に `max_usd_per_day` を書く。旧引数は argparse のエラー。

### D5 変えないもの
1. 表示・記録としての token: facts の usage、`RollupMetrics.tokens`、`BudgetUsage.tokens`、`TreeLimitsUsage.tokens`、compare・pilot の raw の token 欄は残す。
2. `crates/campaign` の計上・研究の凍結評価・task の `Budget.max_tokens` と `dispatcher/token_budget.rs`・`Usage::budget_tokens` は変えない。`Budget.max_tokens` は campaign が試行ごとに設定する凍結済みの比較条件で、この決定（自律化の予算）の範囲外。
3. `PRICE_TABLE` の値と `PRICE_TABLE_VERSION` はこの task で変えない。`[budget_rates]` は表に入らない。

## 結果
- cache の多い run は cache read の単価（input の 1/10〜1/20）で数えられ、上限が実際の換算費用に比例する。同じ token 数でも model の単価で判定が変わる。
- local model を使う案件は、人が `[budget_rates]` を書くまで費用不明で止まる。本番 config に `[project_controller]` を書いていない間は影響が無い。
- 試験で固定すること（受け入れ条件 5）: cache read の多い run と少ない run で判定が単価どおりに変わる、単価の無い model と usage の無い run が費用不明として止まる、`[budget_rates]` を書くと数えられる、表の上書きが拒否される、旧欄が読み込みエラーになる。
