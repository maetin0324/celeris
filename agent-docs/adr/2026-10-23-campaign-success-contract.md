---
title: "ADR: campaign の成功契約を 1 つにし、human-op 集計・CSV・共有作業・B の event を同じ契約に揃える"
status: accepted
date: 2026-10-23
tasks: [01M5KVHDWYTACKJKBNKWQXGX0M]
---
# campaign の成功契約（success contract）と human-op 集計の整合

## 文脈
2026-10-23 の main 再監査（13d64acd）で、成否と人の作業の集計が部品ごとに違う規則を使っていた。
- checker/export は「executor passed AND 結合済み独立採点 final_pass」、cell は trial_no 最小の試行で判定する。
  human-op join は最後の trial_no の outcome だけを読み、report は `outcome == passed` を完遂にしていた
  （実 main-3 pilot で A の executor passed / grade model_failure を完遂と表示）。join の行は観測された identity だけで manifest の分母と合わない。
- 評価者 CLI は arm/case/rep 必須で shared scope の行を書けず、append-only の訂正 CLI も無い（共有 env-lock 修正 71 秒が A/B に二重記録）。
- join は Question/Approval の待機・自動 retry を取り込まず、評価者記録が 1 件あるだけで待ち・job を 0 秒と表示した。
- `protocol_step` は時間・介入から外すのに attempt（再作業）には入り、cutoff 後の介入も件数に入っていた。
- export の CSV は comma join で、`fault_breakdown` の JSON が複数 key を持つと列がずれる。

## 決定
### D1 成功契約を `crates/campaign/src/success.rs` に置く
`success::cells(manifest, trials, bound_grades)` が計画 cell（有効 arm × 問題 × repeats）ごとに `Cell`
（canonical trial_no・executor status・`SuccessClass`・未実行・記録数）を返す。checker の `recompute_graded` はこの関数で集計し、
human-op join は runner の out dir（`campaign.json` がある dir）から `success::load` で同じ判定を読む（lock を信用できなければ採点を無効にするのも checker と同じ）。
report は join の結論を写すだけで再判定しない。規則の文言は `SUCCESS_CONTRACT`・`RETRY_SELECTION`・`DENOMINATOR`・`OBSERVATION_WINDOW` の定数。

### D2 executor status と validated success を別欄にする
join/report は `executor_status`（canonical 試行の outcome）・`validated_success`・`success_class`（`needs_regrade` 付き）を出す。
旧 `outcome`（最後の試行）・`failure_reason`・暫定の `failure_cause` / `provisional-failure-cause/1` は消す（report schema は `/2`）。
manifest を渡さない入力の validated success は欠測。

### D3 shared scope と追記専用の訂正
`human-op record|span --scope shared [--experiment-set]` で case/arm/rep を持たない行を書く。`human-op correct` は correction 行を追記し、
`set.retracted = true` の対象は `resolve` で外れる（元の行は残る）。`resolve_with_history` が補正履歴を返し、report の `corrections` に出す。
correction の record_id は対象ごとに 1 つ（同じ対象をさらに直すときは correction の correction）。

### D4 B の通常 event を join する
`SqliteStore::decision_log_human_waits` が campaign の task 木（識別のある task と子孫）の events から
`QuestionRaised→Answered`・`ApprovalRequested→ApprovalDecided` を `wait` 行にし、`celerisctl campaign export` が records.jsonl に足す（DB には書かない）。
join は task_id で試行に結び `event_waits`、`run` 行の `is_retry` を `auto_retries`（人の再作業と別）、評価者の `celeris_event_ref` を
介入の source_event・待機の event・`run:<id>` と照合して `event_refs`（joined/unjoined）を出す。観測源の無い量は欠測（0 にしない）。

### D5 共通 filter と共通の観測窓
`counts_as_human_op`（protocol_step・intent protocol・automatic を除く）を介入・人の時間・再作業に共通で使う。除外した op を含む論理作業の
attempt は再作業に数えない。観測窓の終端は cutoff、無ければ試行終了（ledger start + wall）で、その後に始まった op・attempt は数えない。
試行開始前の同じ identity の記録（準備・再起動）は数える（終端だけで切る）。
観測の有無は項目ごとに持つ（付記 2026-10-23、差し戻し対応）: op 記録（`op_start`/`op_end`）が無い試行の介入・人の時間、
attempt を持つ記録が無い試行の再作業は欠測（null）で、cutoff・job・wait だけの記録から観測済みの 0 を作らない。

### D6 CSV は RFC 4180
`crate::csv`（writer と parser）で quoting する。外部 crate は足さない。`export::csv_table` が表を作り、試験は parser で列数と JSON セルを確かめる。

### D7 消費の範囲
主比較の消費は試行の run だけで、CoS call と共有 overhead を含めない（`export::COST_SCOPE` を table.json と report.json に出す）。
全 CoS の帰属は今回やらない。list/cash は final-accounting.json の別欄のまま。

## 影響
- report.json の欄名が変わる（`completion` の source は `grade`、`failure_cause` は無い）。旧 report は再生成する（生記録は不変）。
- 旧 main-3（mlsys-main-3）の生記録は旧採点定義のまま再集計できる（A/B とも validated success 0）。

## 付記 2026-10-25: 未実行 cell の消費・records 欠如・照合 script（main-4 grade-integrate）
- **未実行 cell の消費**: report の `tokens_missing_trials`・`cost_micro_usd_missing_trials` は、cell の判定に使った試行が実行されなかった cell
  （`TrialRecord::ran` が false: 予算停止・依存 blocked・arm 予算切れで未投入）を数えない。代わりに `*_not_run_trials` と arm の `not_run_trials` に数え、
  値は null のまま（欠測にも観測済み 0 にもしない）。これで欠測数の分母が checker の `ran_trials` と同じ「実行した試行」になる。
  `success::Cell.ran`（join の `success_class.ran`）が判断の元。記録の無い cell（`missing`）は従来どおり欠測に数える。
  理由: main-4 の無 LLM 通しで B の arm 予算切れ 1 件を report だけが欠測 2（checker 1）と数え、分母が食い違った。
- **records 無しの export**: `--records`（launcher の RECORDS）が無い export は `consumption` に `scope: missing`・`reason` を書き、
  前回の export の `final-accounting.json` を消す（別入力の値を表と並べて残さない）。launcher は警告を出す（RECORDS は collect/grade では作られず、
  測定 DB の写しからの `celerisctl campaign export` が元）。
- **照合**: `scripts/campaign/check-out-consistency.py --out <out>` が生記録から成功契約を再計算し、checker totals・export 行・report の arm を突き合わせる。
  採点の結合（hash・lock）は checker の判断に従う。report（`success::load`）は保存 lock だけで結合し期待 lock を作らないので、checker は launcher と同じ
  `--measurement-id`・資産の引数で呼ぶ（省くと期待 lock が合わず checker だけが grade を外し、report とずれる。照合はこれをずれとして出す）。
