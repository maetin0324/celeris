# 短い ML 研究キャンペーン case: 延期状態の運用手順（採用していない）

---
tasks: [01M59KY5FTRJTJ80PACNW3K533]
---

状態: **延期（deferred）**。決定 2026-10-19、期限 2026-10-21T00:00:00Z、ラベル dev、base commit `c9654bff56332c160523d904b2248ae527d9b820`。
この文書は延期を確定した状態の運用手順であり、採用手順ではない。**追加 case の採用・無 LLM の正負例・実 pilot はいずれも未成立**。効果や汎化は主張しない。

## status の所在

- 正本: `crates/campaign/optional/short-ml-campaign/status.json`（`cases/` の外。loader は case として読まない）
- 検査: `crates/campaign/scripts/verify-short-ml-case.py`
- 経過: `agent-docs/progress/2026-10-19-short-ml-case/case-controls.md`
- `cases/dev/` と `cases/final/` に `short-ml-campaign` ディレクトリを置かない。`case.json` も置かない。

## 延期理由（status.json の reasons）

- R1: 再利用できる小規模 ML 資産が無い。local の python3 3.13.3 に numpy/scipy/sklearn/torch/pandas が無く、導入は依存追加になるため行わない。pegasus-lm は LLM 配信基盤で対象外。sirius の system numpy 1.23.5 はあるが、`/work/NBB/rmaeda` に ML repo・固定データが無く新規資料が要る。
- R2: 既存 grader の experiment 判定は scheduler 形の証拠（数字[.host] の job id、exit_status=0 の scheduler_record、評価側が集めた `celeris.observed-jobs/1`）だけを受理する。local process の実行を評価側が記録する収集器は無く、grader/harness の変更はしない。PID を job id と偽ることは捏造になるので行わない。
- R3: 代替（固定 raw の recompute のみ／hidden test の実装のみ）は既存の分析 case・実装 case と重複し、ML campaign を判定しない。

## 未検証

- sirius の batch job（scheduler 証拠あり）で動く標準ライブラリのみの ML case を作れるか。
- sirius の job 投入と qstat/sacct 記録の収集を ML case に使えるか。
- 正例（正しい結果の受理）と負例（誤計算・条件違い・未実行の拒否）を通常 grader で動かす検査（case 不在のため未実施）。

## 適用限界

- 本記録は dev ラベルの延期判断で、final 汎化や実 pilot の確認を意味しない（status.json の `claims` は全て false）。
- 既存 case（dev: k0-fixture-control / k1-bf-fsync-campaign / k3-locusta-ab-analysis、final 一式）、loader、grader、manifest は不変。

## 主比較との関係

- 主比較（A0/B0）、Jev 副比較（B1/C1）、論文の依存先に影響しない。依存を足さない。
- A0/B0 と B1/C1 は別測定 ID。既存 arm の実行器は保持する。
- K2 / K4 は置き換えない。

## 検査の実行方法

```
python3 crates/campaign/scripts/verify-short-ml-case.py --check
```

標準ライブラリのみ、LLM・ネットワーク不使用。成功時は `ok: deferred record is consistent (not an adoption pass)` を出し exit 0。

検査するのは**延期記録の整合だけ**: 必須欄・task ID・inventory 証拠・再開条件・`claims` が全て false・証拠パスの存在、延期前提（grader.rs の sha256、既存 case 集合、local に numpy が無いこと）が今も成立すること、延期用ディレクトリが `cases/` に無いこと。
これは **採用の合格ではない**。status が adopted の記録は、通常 grader での正負例を実行する検査が未実装のため受理しない（失敗する）。
前提が変わった場合（grader.rs 変更、case 追加、numpy が import 可能）は FAIL になり、延期理由の再検討を促す。

## 再開条件

1. sirius 等 scheduler 証拠が得られる環境で、標準ライブラリのみまたは承認済み依存の ML 資料（固定データ・実行手順）が用意できる。
2. 既存 grader の種別を変えずに、正例（正しい結果）と負例（誤計算・条件違い・未実行）を LLM なしで通せる。
3. 人が依存追加/クラスタ job を承認する。

再開時は status.json を更新し、adopted 用の検査（通常 grader での正負例実行）を実装してから採用とする。本番手順の実行や測定の開始は、この文書の範囲外。
