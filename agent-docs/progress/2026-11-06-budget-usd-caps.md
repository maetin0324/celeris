---
title: 予算の token 上限を料金換算（USD 相当）の上限に置き換える
tasks: [01M6QDK2PMJTXF17Z8TNDKF85M]
status: done
updated: 2026-11-06
---
# 予算の token 上限を料金換算（USD 相当）の上限に置き換える

完了日: 2026-11-06（WorkUnit closeout まで）

人の決定（2026-11-06）「料金の換算で数える」を実装する task。設計は
[ADR 2026-11-06-budget-usd-caps](../adr/2026-11-06-budget-usd-caps.md)。

## 変更
- WorkUnit adr: ADR `2026-11-06-budget-usd-caps.md` とこの進捗ファイルの骨子を追加した。コードは変えていない。
- WorkUnit core: `task_core::pricing` に `BudgetRates`・`run_budget_cost`・`budget_cost`（micro-USD の切り上げ和、単価なし・usage なしは `Unknown`）を追加。`tree::TreeLimits.max_tree_tokens` を `max_tree_usd_micros` に置き換え、`TreeLimitKind::TreeUsd`・`TreeCounters.usd_micros/unknown_cost_runs`・`with_unknown_cost_note` を追加。`tree_counters` は `&BudgetRates` を引数に取る。現行 model の前方一致試験を追加（`PRICE_TABLE`・`PRICE_TABLE_VERSION` は変えていない）。下流 crate は後続 WorkUnit が追従する。
- WorkUnit ops: `task_ops::project_budget` を USD 判定に置き換えた。`BudgetLimits.max_usd_per_day_micros`（既定 100 USD）、`BudgetKind::Usd`・`UsdUnknown`、`BudgetUsage.usd_micros`・`usd_unknown_runs`（`tokens` は表示用で残す）、`DANGEROUS_USD_FACTOR`。`usage`・`usage_on_day` は `&BudgetRates` を取る。`tree::tree_counters` と `tree_view::task_tree` も `&BudgetRates` を取り、`TreeLimitsUsage` は `max_tokens` を消して `usd`・`max_usd`・`usd_unknown_runs` を置いた。`project_controller_view` の limits は `max_usd_per_day`。
- task-core の公開 API の変更（最小限）: `SqliteStore::project_run_usage_between` は `Vec<ProjectRunUsage { model, usage }>` を返す（走っている run と harness 起動前に終わった task run は SQL で除外、usage 無しは `usage: None`、CoS chat run の model は `resolved_config_json.model`）。`ControllerBudgetLimits.max_tokens_per_day` を `max_usd_per_day: f64` に置き換えたため、`ControllerBudgetLimits`・`ControllerSummary` 系の `Eq` derive を外した（f64）。
- 後続 WorkUnit への依存: task-api（`handlers/project_controller.rs` の `usage(...)` に rates を渡す・`budget_today` に `usd` 等・`execution.rs` の `task_tree` に rates）、task-dispatch（`planner_flow.rs`・`tree_units.rs` の `tree_counters`、`harness_tests.rs` と tests の `project_run_usage_between`・`usage`・`max_tokens_per_day`）、celeris config が `BudgetLimits`・rates を組み立てる。
- WorkUnit scripts・api-worker・dispatch・config・repair・size の変更は `2026-11-06-budget-usd-caps/<wu-key>.md` に書いた。要点: compare.py・driver.py・long-horizon-pilot-llm.sh は `max_usd` / `--max-usd-per-day` に一本化。worker protocol は `tokens_left` を `usd_left` に、tree view は `max_tokens` を `usd`・`max_usd`・`usd_unknown_runs` に置き換えた。config は旧欄を `deny_unknown_fields` で読み込みエラーにした。dispatcher.rs は `budget_rates` の欄を `tree_units.rs` へ移して source-size gate に戻した。
- WorkUnit closeout: 旧欄の走査（下の証拠）と、この進捗の完成、本番への移行節の確認手順。コードは変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- WorkUnit adr は文書だけの変更。cargo の検査は closeout で提出時の HEAD に対して実行して、ここに書く。
- 旧欄の走査（closeout、提出時の HEAD）: `grep -rnE "max_tokens_per_day|max_tree_tokens|max-tokens-per-day" . --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=target` の結果から `agent-docs/adr/`・`agent-docs/progress/`・`LEGACY_REJECT` を含む行を除くと 0 件。旧 field の識別子（`TreeTokens`・`tokens_left`・`BudgetKind::Tokens`・`DANGEROUS_TOKEN`）も scripts・crates・web/src・docs に 0 件。`max_tokens` は campaign・LLM 呼び出しの `max_tokens: None` と、旧欄を拒む試験（LEGACY_REJECT 付き）だけ。
- closeout の検査（提出時の HEAD。この WU の base と HEAD は同じなので `test-affected.sh` は「試験対象の変更なし」で exit 0。全体の変更に対する試験は config WU の `test-affected.sh` が 6252 passed・exit 0）: `bash scripts/dev/test-affected.sh` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。`python3 scripts/dev/check-source-size.py --base $CELERIS_WU_BASE` → 失敗 0。`crates/campaign` の差分なし。
- 受け入れ条件との対応: ADR が実装より先（commit 件名「adr」の WorkUnit が最初）。旧欄を拒む config 試験は config WU、cache read・write の単価どおりの判定と単価なし・usage null の `UsdUnknown` の試験は core・ops・dispatch WU に書いた。
- WorkUnit ops（提出時の HEAD で実行）: `cargo test -p task-ops` → 633 passed・0 failed。`cargo clippy -p task-ops -p task-core -- -D warnings` → exit 0。`cargo fmt --all -- --check` → 差分なし。新しい試験: cache read・cache write の単価どおりの判定、単価なし model・usage null・chat run の usage null が `UsdUnknown`、`budget_rates` 明示（0 の宣言を含む）、走行中・起動前 run を数えない、`<source>/` 接頭辞付き model。`--all-targets` の clippy は task-core の既存試験 3 件（project_retention_scale.rs ほか）で落ちる（この WU が触っていない file。main 由来）。

## 本番への移行
1. 消す config 欄: `[project_controller.budget] max_tokens_per_day`、`[execution.tree] max_tree_tokens`。新しい binary は、この欄を書いた config を読み込みエラーにする。
2. 新しい欄:
   - `[project_controller.budget] max_usd_per_day`（既定 100.0 USD 相当）
   - `[execution.tree] max_tree_usd`（既定は無し）
   - `[budget_rates."<model の接頭辞>"]`（単価表に無い model の換算単価。4 欄必須。既定は無し）
3. 本番 config の書き換え: 不要の見込み。2026-11-06 時点で、本番 config（`~/.config/celeris/config.toml`）に `max_tokens_per_day`・`max_tree_tokens` は書かれていない（task の依頼文の前提。この run は本番 host を読んでいないので未確認）。人は promote の前に次で確かめる。
   - `grep -nE 'max_tokens_per_day|max_tree_tokens' ~/.config/celeris/config.toml` → 何も出ないこと。出たら同じ値の意図を USD で `max_usd_per_day`・`max_tree_usd` に書き換える。
4. `[project_controller]` を有効にした案件が local model（opencode-qwen など単価表に無い model）を使うなら、promote の前に `[budget_rates]` を書く。書かないと費用不明で日次の予算が止まる（ADR D2）。
5. promote の前に、未回答の木の上限の決定で key に `max_tree_tokens` を含むものが無いことを人が確かめる（新しい binary は旧い種類を読まない）。決定の key は `limit:max_tree_tokens`（`TreeLimitKind::decision_key`）。旧 binary の API（`GET /decisions?open=true`、`DecisionList`）に次を実行する。`celerisctl` に決定の一覧の command は無い。
   ```bash
   curl --fail-with-body -H "Authorization: Bearer $CELERIS_API_TOKEN" \
     "$CELERIS_API/api/v1/decisions?open=true" \
     | jq '[.items[] | select(.decision.key | test("max_tree_tokens"))] | length'
   ```
   `0` なら移行できる。1 以上なら、その決定（`.decision.id`）に `POST /decisions/{id}/answer`（`option` に選択肢の key）で答えるか、`POST /decisions/{id}/withdraw` で取り下げてから promote する。回答済み・取り下げ済みの決定と events の `"max_tree_tokens"` の文字列は書き換えない（追記専用）。判定はそれらを読まない。
6. release・promote は人が行う（この task は本番 host に触れていない）。手順は `scripts/selfdeploy/release.sh` で release を作り、通ってから `promote.sh`（`SD_ACTOR` を付ける）。release の gate が全体の試験を走らせる。
7. 運用中の pilot の手順（`long-horizon-pilot-llm.sh`）は `--max-tokens-per-day` を `--max-usd-per-day` に変える。

## 未解決
- route の `<source>/` 接頭辞付きの model 名が `runs.model` に入るかは未確認。core は引く前に最後の `/` より後を使う規則で吸収し、試験した（ADR D1-6）。
- harness 起動前に終わった run の規則（ADR D2-3）: 終わった run のうち usage・model・adapter・account・session・metrics が全部無いものは数えない。走っている run も数えない。ops WU は案件の日次でも同じ規則に合わせること（`chat_runs` は `started_at` がある行だけ）。

## 提案
- task-core の既存試験 4 件が `cargo clippy --workspace --all-targets -- -D warnings` で落ちる（`project_proposal_tests.rs:194`、`script_exhaustive.rs:296`、`tests/project_retention_scale.rs:100`、`chat/plan_rejudge_tests.rs:78`）。この task は触っていない。別 task で直す。
- `celerisctl` に決定の一覧・回答の command が無い。移行の確認を API の curl に頼っているので、将来足す余地がある。
