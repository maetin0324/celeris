---
tasks: [01M52D2AM5K6GCFGEJZW4REWM2]
unit: grade-export
status: not-run
updated: 2026-10-16
---
# 本測定 pending: 未実施（not-run）と昇格後の実行手順

## 1. 現状（事実）
- 本測定は **未実施（not-run）**。昇格前で実行不可。実測も消費も無い。採点・check・export も **していない**（実施済み・採点済みではない）。
- `data/` には `README.md` だけがあり、`data/final-out` も `data/final-export` も **存在しない**。結果・数値・消費量は作っていない。
- 根拠: 人の決定（final-run = 実行しない／できない）。実装（migration 0084–0086、campaign/adapter）が main に入り本番へ昇格するまで走らせられない。pilot も未実施（auth-ready=defer）。

## 2. 昇格後の実行順（運用セッション。worker・agent は実行しない）
1. root 完了 → 統合・昇格（H1。sha12 を控える）。
2. **runbook Part P の pilot**（`../2026-10-16-mlsys-final-connect/runbook.md`）→ 係数から上限を確定。
3. **`run-final.md`** の §1〜§4（置換 → validate / dry-run → 入力配送 → 本測定）。
   ```sh
   B=$CARGO_TARGET_DIR/debug/celeris-campaign; P=agent-docs/progress/2026-10-16-mlsys-final; D=$P/data
   $B validate --manifest $P/final.json
   $B dry-run  --manifest $P/final.json
   $B run --executor celeris --manifest $P/final.json --out $D/final-out \
      --api-url http://127.0.0.1:<task-api port>/api/v1 --token-env CELERIS_CAMPAIGN_TOKEN \
      --task-template $P/task-template.json --repo-src $R --materials-src $M --kb-src $K
   ```
   中断・`waiting>0` の後は同じコマンドをそのまま再実行して再開する。上限は試行 12・tokens 1,000,000・r=1,n=5,m=2。
4. check / grade / export（run-final.md §5〜§6）。
   ```sh
   $B check --manifest $P/final.json --out $D/final-out
   $B grade --case $P/grade-case/<case-id>.json --artifact $D/final-out/trials/<A|B|C>/<case-id>/rep1 > $D/grade-<arm>-<case-id>.json
   $B export --manifest $P/final.json --out $D/final-out --dest $D/final-export
   ```
   全試行（失敗・timeout・cancelled・未完了を含む）を分母に残す。`grade-case/` は未作成なので、grade の前に作る。
5. `result.md` を実測値で更新（現状の `result.md` は not-run の記録）。

## 3. 注意
`check`（`data/final-out/run-meta.json` が要る）と `data/final-export` の存在確認は、手順 3〜4 の後でなければ通らない。昇格前に通すには結果を捏造するしかないので、通していない。
