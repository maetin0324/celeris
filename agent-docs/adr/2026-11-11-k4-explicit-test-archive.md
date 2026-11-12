---
tasks: [01M75722QN630BBASEQ759KG4T]
---
# ADR 2026-11-11: K4 の実物検証は明示した原票を使う

- 状態: 採用
- 関係: [試験の指針](../guides/testing.md)、[K4 採点](2026-10-23-k4-cells-validity-grading.md)

## 原因と owner

`crates/campaign/tests/k4_cells_validity.rs` の実物試験は `CAMPAIGN_K4_TGZ` が無くても別 task の本番 workspace の絶対パスを探す。
そのファイルがあるホストだけで実物の展開と採点を繰り返し、今回の受け入れ検査では 600 秒で timeout した。
原票の選択は試験 fixture の責務であり、製品の grader や timeout を変える問題ではない。

## 決定

実物検証は既存の `CAMPAIGN_K4_TGZ` を明示した場合だけ行う。指定が無ければその旨を表示して戻る。
指定したファイルが存在しなければ失敗する。別 task の絶対パスを探す旧経路を削除する。
新しい設定・fallback・製品の分岐は追加しない。

## 検査の意味

実物試験の SHA 照合、30/32 の期待値、誤答の拒否条件は維持する。通常検査では同じファイル内の合成原票の試験が
正常解、欠測の分母、fsync、失敗 job、表と報告の不一致、数値誤りを検査する。
実物検証を行う人は原票を用意して `CAMPAIGN_K4_TGZ=<原票> cargo nextest run -p celeris-campaign --test k4_cells_validity` を実行する。
通常検査の実行内容が、無関係な task の成果物の存在で増えるという契約は廃止する。
