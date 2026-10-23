# ソース健全性の必須差分ゲート

---
tasks: [01M5KWPP2W6FZAGWXFDFBQJ5S0]
---

`bash scripts/dev/test-affected.sh` は、crate 選択の前に
`bash scripts/dev/check-health.sh` を必ず実行する。文書だけの変更にも適用し、
`--list` は従来どおり対象表示だけにする。
workspace の check はこの入口・clippy・fmt の3本を維持する。
タスク5（`01M5KWPP519YW28Q6FQRNP5184`）が候補SHAの必須検査集合を導入するときも、
この入口または `check-health.sh` をその集合に含める。

軽量検査は source-size 差分、architecture-map、文書参照、ADR番号、
それらの正負例試験。全workspace試験・web e2e・実browser/usernsを追加しない。
release.sh の warning-only report は全負債を見る棚卸しであり、必須差分ゲートの代用ではない。

## 固定基準と悪化の判定

`scripts/dev/source-size-baseline.json` の SHA は
`13d64acd17806d73a1797bffbf831efc9cb5ab6a`（2026-10-23着手時main）。
分類規則は `source-size-report/1`、production > 2,000行、inline test module > 300行。
Rust/TS production、Rust inline/external test、TS test、generated、schema、fixture、
migration、tooling、docs、lockfileを区別する。外部test自体に行数上限は設けない。
`cfg(test)` の外部module宣言もtestとして数え、移動でproductionが増えたとは扱わない。

判定キーは path・規則・inline module名。現在値が閾値を超えたとき、許容値は
`max(閾値, min(固定基準値, 比較元値))`。基準にないキーの許容値は閾値。
比較元は `CELERIS_WU_BASE`、なければ `git merge-base HEAD main`。
固定基準が負債の上限を与え、比較元が着地済みの改善の再悪化を防ぐ。
未追跡（ignore対象外）のソースも検査する。基準SHA・比較元が読めない場合は失敗する。
基準内の各値はそのSHAのソースから再計測し、書き換えた数値だけで通せない。

基準の自動更新・定期再作成はしない。SHA・分類・閾値を変える際は、差分、理由、
負債の追跡先を明記した独立レビューを必須とする。例外はTOMLの正確なpath・規則・理由で指定する。
今回の新規例外はゼロ。既存のtask-api wire型の例外だけを維持する。

## 証拠と追跡

入口の標準出力には候補HEAD（作業treeも対象）、基準SHA、比較元SHA、
違反path・規則・閾値・基準値・比較元値を出す。JSONは
`python3 scripts/dev/check-source-size.py --json` で取得できる。
reviewは実行ログと検証対象SHAを対応させる。作業treeの結果だけを未検証の将来SHAへ流用しない。
全負債の棚卸しは `python3 scripts/dev/source-size-report.py --format json`。
解消・残件は [この変更の記録](../progress/2026-10-23-source-health.md) に列挙した。

追跡起票は自動ゲートから行わない。残件のキー（規則＋path）を既存の未完了taskと照合し、
既存taskがあれば再利用する。同一キーにつき新規起票は最大1回、再試行上限2回。
上限後は既存taskで人の判断を受け、新しいtaskを作って回数を戻さない。
今回、Dispatcherの3キーは既存タスク4へ、browserの2キーは1件のfollow-upへまとめた。
follow-upはこのrun終了時にdraft化される。draft化失敗時も新規起票せず元の提案を確認する。
