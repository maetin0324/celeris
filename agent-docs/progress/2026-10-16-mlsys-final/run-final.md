---
title: "MLSys 本測定 run-final: 運用セッションが人の shell で実行するコマンド列"
tasks: [01M52D2AM5K6GCFGEJZW4REWM2]
unit: freeze-final
status: ready-for-operator
updated: 2026-10-16
---
# run-final（運用セッション用。worker・agent は実行しない）

根拠: runbook（`../2026-10-16-mlsys-final-connect/runbook.md`）Part M・Part H の H5、条件は `freeze-final.md`。**値（token・鍵）はこのファイルに書かない。** `--token-env` には環境変数の**名前**だけを渡す。
本測定は人の決定「pilot の提案どおり」で承認済みの枠（試行 12 以内・tokens 1,000,000・fable 不使用）の内側。枠を超える変更は新 campaign_id と新しい決定が要る。

## 0. 記号と前提
```sh
cd <この repo の checkout>               # worktree の agent-platform
B=$CARGO_TARGET_DIR/debug/celeris-campaign
CT=$CARGO_TARGET_DIR/debug/celerisctl    # 本番では ~/.local/celeris/current/bin/celerisctl
P=agent-docs/progress/2026-10-16-mlsys-final
D=$P/data                                # 出力置き場（final-out / final-export）
```
前提（人）: H1 昇格（sha12 を控える）、H2 `GET /api/v1/secrets` で `jev_api_key` の有無、H4 自分の shell で `export CELERIS_CAMPAIGN_TOKEN=...`（値は argv・file に残さない）。7 日使用率の開始値 `U0`（codex / claude）を控える。

## 1. 置換（H1 後に 1 回だけ。置換したら hash が変わるので 2 で控え直す）
`$P/final.json` の次を編集する（それ以外は変えない）:
- B・C の `harness_version` の `<release sha12 at H1>` → 昇格した sha12。
- C に `routing.evidence_table_id` を足す場合は `$CT routing evidence` の版（pilot と同じ版）。`lower_policy`/`online_update` は変えない。
- `$P/task-template.json` の `project_id` `<案件 id>` → 案件 id。acceptance は API の形 `{"type": "reviewer", "text": …}`（入れ子の `check` は起動時に拒否）。
- `final.json` の `budget.token_basis` の `<人の決定: …>` → 人が決めた基準（`input_output` / `input_output_cache_read` / `all`）。**既定は無い**。埋めないと manifest が読めず run は始まらない。試行記録・summary・export には基準に関係なく input / output / cache_read / cache_creation が全部残る。
`harness_version` の置換後の `final.json` の sha256 が実条件。以後 manifest・入力・snapshot を変えない（変えるなら新 campaign_id）。

## 2. 条件固定（Part M-1, M-2）
```sh
$B validate --manifest $P/final.json         # sha256 を控える（freeze-final.md の置換前 hash と別物になる）
$B dry-run  --manifest $P/final.json         # within_budget true、total_trials 6 を確認
```

## 3. 入力の配送（試行へ渡す公開部だけ。grader-private は渡さない）
```sh
M=$TMPDIR/final-materials; K=$TMPDIR/final-kb; R=$TMPDIR/final-repo   # NFS 外
mkdir -p $M $K $R
for c in k2-bf-xattr-restart-campaign k4-cmp4-validity-analysis; do
  cp -r crates/campaign/cases/final/$c/materials $M/$c
done
cp -r crates/campaign/cases/final/k2-bf-xattr-restart-campaign/kb/. $K/
# R: k2 の初期 tree（BenchFS 086c4618、case.json の initial.fetch に従い asset store の tar を展開）。k4 は repo 不要。
```
`$M` の hash は `final.json` の `snapshot.materials_sha256`（`freeze-final.md` に値）と一致しなければ run が拒否する。

## 4. 本測定（Part M-3。A/B 主・B/C 副）
```sh
$B run --executor celeris --manifest $P/final.json --out $D/final-out \
   --api-url http://127.0.0.1:<task-api port> --token-env CELERIS_CAMPAIGN_TOKEN \
   --task-template $P/task-template.json \
   --repo-src $R --materials-src $M --kb-src $K
```
`--api-url` は server の origin だけ（`/api/v1` を付けると拒否。実行器が足す）。`--out` は相対でもよい（起動時に絶対 path にする）。`$D` は NFS 外。
stdout の JSON（`executed / waiting / parallel_deferred / no_artifact / executor_errors / blocked / budget_cancelled / tokens_total / tokens_missing / max_overshoot_*`）を `$D/final-run-N.json` に保存する（N は実行回数）。
`budget.max_parallel`（1）は in-flight の上限: 1 回の run は 1 試行だけを投入して `waiting`（`parallel_deferred`）で戻る。**`waiting=0` になるまで同じコマンドを繰り返す**（前の試行の結果・使用量を記録してから次を出す）。`executor_errors>0` なら `trials.jsonl` の `error` 欄（HTTP status と本文）を見る。
**再開（Part M-4）**: 中断・`waiting>0`・daemon 再起動の後は上と**同じコマンドをそのまま**再実行する（`resumed_skipped` に数えられる）。別 hash で同じ `--out` は拒否される。cancel された試行の cluster job は `$D/final-out/*/cluster-cancel.jsonl` の `scancel`/`qdel` を人が実行（H6）。

## 5. 検査・独立採点（Part M-5, M-6）
```sh
$B check --manifest $P/final.json --out $D/final-out          # ok true、violations 空
# 試行ごと（対応表は freeze-final.md §4）。grade-case は $P/grade-case/<case-id>.json（grade-export WU が作る）。評価 command は実際に走る。
$B grade --case $P/grade-case/<case-id>.json --artifact $D/final-out/trials/<A|B|C>/<case-id>/rep1/workspace > $D/grade-<arm>-<case-id>.json
# まとめて: grade-case dir の <problem_id>.json を引き、$D/final-out/grades.jsonl を書く（grade-case が無い problem は missing_cases に出て exit 1）
$B grade-out --manifest $P/final.json --out $D/final-out --cases $P/grade-case
```
exit 0 = final_pass、非 0 = 判定 JSON が stderr（`2> $D/grade-….err` に残す）。**6 試行すべてに判定を付ける**。失敗・timeout・cancelled・未完了も分母に残す（除外しない）。

## 6. export（Part M-7, M-8 と H5）
```sh
$B export --manifest $P/final.json --out $D/final-out --dest $D/final-export
# Celeris 側の消費・判断ログ。本番 DB は読むだけ、写しに対して実行
$CT db backup $TMPDIR/final-db.sqlite3 --src <本番 DB path>   # 読み取り専用接続の VACUUM INTO。稼働中でも 1 snapshot で終わる
$CT --db $TMPDIR/final-db.sqlite3 campaign export mlsys-final-1 --out $D/exp-db --verify-connected --manifest-hash <手順 2 の sha256>
echo $?            # 0 = 接続 OK。非 0 は stderr の `not connected:` 行
```
`summary.json` の `shared_overhead`、`elapsed_ms` と `run_wall_sum_ms`、`total.cash_cost_usd`（取れなければ null）、終了時の 7 日使用率（codex 21%・claude 24% 以内か）を進捗（`../2026-10-16-mlsys-final/` の次 WU の file）に転記する。

## 7. 注意
- 本番 host 操作（昇格・systemctl・config 変更）はここに含めない。H1 は GUI/`promote.sh`。
- pilot の結果は較正専用で、本測定の主張に混ぜない。dev の case・入力・設定を final に持ち込まない。
