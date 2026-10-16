---
title: "MLSys 本測定: case × grader × isolation の統合試験と case-grader 全体の記録"
tasks: [01M51QRH7TD3FHER7HVF9W3TAH]
unit: e2e
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# e2e（case-grader 全体の記録）

統合試験 `crates/campaign/tests/measure_e2e.rs`（4 件）。grader／cases／isolation の本体に不備は見つからず、変更していない。

## 状態の区分

| 区分 | 内容 |
|---|---|
| 実装済み | cases（`cases.rs`、case 5 件: dev k0/k1/k3・final k2/k4）、grader（`grader.rs`、4 検査＋control の ControlOnly）、isolation（`isolation.rs`）、これらをつなぐ `tests/measure_e2e.rs`。詳細は `cases.md`・`grader.md`・`isolation.md` |
| mock 検証済み | ①dev case k1 を A/B/C の分離 workspace に展開し入力 hash が一致（KB・資料は case 固定 hash と一致）、A の KB 書き込みが B/C に出ない。②制御用 mock の Passed（証拠種別 mock）は control 以外で不合格、control（k0）は `ControlOnly`・`final_pass=false`、`select_for_measurement` は control を拒否。③評価テスト弱体化／削除、未実行なのにもっともらしい Markdown（証拠・生データなし）、数値捏造、成功宣言だけ、が不合格。④final case k2 の隠し command・欠陥 id が公開部に無く、dev 経路（trial dir・`cases/dev`・`grader-private/dev`）に final の値が無い。dev と final は同時に選べない。実証拠つき成果は Pass（対照） |
| 実経路検証済み | 実 case file のローダ（hash 固定の検証）と、実 case の snapshot（materials/KB）を入力にした隔離の hash 一致。外部原票・初期 commit の hash 計算は WU cases で実施済み。評価 command の実行は stub（実 cargo ではない） |
| 未実施 | **実クラスタ（Sirius）での実験と実 LLM による試行は未実施**。実 BenchFS の受入 command 実行、submodule 固定、K1〜K4 実 case の private から grader case（`celeris.grade-case/1`）への変換（試験は既知欠陥から合成）、実 trial の A/B/C 運用 |

## 本番移行
無し（config 欄・DB・API の変更なし）。

## 証拠コマンド
- `cargo test -p celeris-campaign --test measure_e2e` → 4 passed, 0 failed。
- 全体検査（test-parallel／clippy／fmt）の結果は下の「全体検査」。

## 提案
- private.json から `celeris.grade-case/1` を生成する変換（pristine テスト・metrics・defect の file/keywords）を実測定の前に作る。現状は private が欠陥の id と説明しか持たず、grader が検出判定に使う file・keywords が無い。
- 評価 command を実際に走らせる `CommandRunner` 実装を計測側 crate に置く（src の process 禁止 guard を保つため）。
- 未解決（cases.md 引き継ぎ）: R1 node-h 上限の承認、R2 submodule 固定元、R3 K2 隠し試験の移植確認、R5 K3 統計量の確認。

## 全体検査
- `bash scripts/dev/test-parallel.sh`: passed 5418, failed 0, ignored 14、exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
