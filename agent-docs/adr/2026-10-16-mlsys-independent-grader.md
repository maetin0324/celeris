---
title: "ADR: MLSys 本測定の独立 grader（実装テスト・実行証拠・独立再計算・欠陥検出）"
status: accepted
date: 2026-10-16
tasks: [01M51QRH7TD3FHER7HVF9W3TAH]
---
# MLSys 本測定の独立 grader

## 文脈
本測定では外側の 1 problem を「実装→検証→実験→分析・報告→レビュー」の研究キャンペーン 1 本とする
（棚卸し: `agent-docs/progress/2026-10-16-mlsys-measure/inventory.md` の K0〜K4）。
前 root の `crates/campaign` には、trial 記録から合否を再計算する `checker.rs` と、報告の数値行を再計算値と照合する
`report.rs` がある。しかしそれは campaign runner の記録（mock 実行器の `Passed` を含む）を検査するもので、
研究成果物そのもの（コード・実験原票・報告・レビュー指摘）を採点しない。エージェントの成功宣言・ファイルの存在・
mock の `Passed` で合格にならない、成果物から独立に判定する grader が要る。

## 決定
- D1: `crates/campaign/src/grader.rs`（単一 file。内部は `case`・`implementation`・`evidence`・`recompute`・`review` の
  inline mod）に grader を置く。`src/` 直下にディレクトリを作らない（既存試験 `runner_checker.rs` が `src/` の全 entry を
  file として読み、子 process・network の語を禁じているため）。入力は **trial の成果物 directory と grader 側の case 定義
  （`celeris.grade-case/1` の JSON）だけ**。成果物中の自己申告（`result.json` の summary、trial 記録の outcome、
  報告の「成功」の文言）は読まない。`lib.rs` に `pub mod grader;`、`main.rs` に `grade` サブコマンド 1 行だけ足す
  （`celeris-campaign grade --case <file> --artifact <dir> [--work <dir>]`、合格で exit 0、不合格で exit 1、
  どちらも判定 JSON を出す）。
- D2: 判定は構造化 JSON（`celeris.grade/1`）。`checks.{implementation,evidence,recompute,review}` の各々に
  `status`（`pass`／`fail`／`not_applicable`）・`reasons`・`details`。4 検査は互いの結果を参照せず独立に行う。
  `verdict` は `pass`／`fail`／`control_only`。case の節が無い検査は `not_applicable`、適用検査が 0 件なら `fail`。
  **split=`control`（K0 fixture/mock）の case は全検査が通っても `control_only` で `final_pass=false`**
  （mock の Passed を最終合格にしない）。
- D3（実装テスト）: grader が pristine な評価側テスト（case 定義の dir に置き sha256 を case に記録）を持つ。
  (a) pristine 自体の sha256 を case の記録と照合（grader 側の改変を検出）。(b) 成果物 repo の同じ path の file を
  pristine と比べ、削除・sha256 不一致を**失格**にする。不一致のときは `#[ignore]`・skip 印（`#[cfg(any())]` 等）の
  増加、`assert` の減少、`#[test]` の減少を詳細に出す。(c) 成果物 repo を作業 dir に写し（`.git`・`target` は除く）、
  pristine を重ねてから case の command を**注入可能な runner**（`CommandRunner` trait、試験は stub）で実行する。
  grader の library と CLI は子 process を起こさない（`runner_checker.rs` の src 走査が `process::Command` を禁じる。
  この guard は弱めない）。CLI の既定 runner `NoRunner` は常に Err を返すので、実行器を注入しない採点では実装テストが
  「未実行」で fail になる（fail-closed）。評価 command を実際に走らせる実行器は呼び出し側（e2e／campaign 計測の
  外側）が `CommandRunner` を実装して注入する。exit 0 かつ `must_pass` の各試験名が出力に `test <name> ... ok` として現れることを要求
  （0 件実行で通る抜け道を塞ぐ）。
- D4（実験条件と実行証拠）: 成果物の `evidence/jobs.json` の各 job に job ID・scheduler 記録・ログ・生データ
  （path と sha256）・実行条件・`evidence_kind` を要求する。scheduler 記録は job ID を含み、終了状態
  （`Exit_status = 0` か `exit_status=0`）を示すこと。ログは空でないこと。生データは存在し sha256 が記録と一致すること。
  条件は case の `conditions`（node 数等）と JSON 値で一致すること。`evidence_kind=mock` は control 以外で不合格。
  成果物内の path は相対で、`..`・絶対 path・symlink による成果物外参照を拒否する。
- D5（独立再計算）: case の `metrics` が生データ（JSON pointer か CSV 列）と集約（mean/median/sum/min/max/count）、
  派生の比（`ratio`）を定義し、grader が生データから再計算する。報告の `metric <name> = <value>` 行
  （`report.rs` の `result` 行と同じ「行で数値を宣言し再計算値と照合する」方式）と相対許容差で照合する。
  報告に行が無い・数値でない・許容差外は不合格。case の `pinned_raw` に固定入力の sha256 があれば一致を要求する
  （K3/K4 の入力原票の改竄検出）。分母（有効件数）を `count` 指標として宣言させられる（`report.rs` の分母検査の考え方）。
- D6（レビュー）: case の `known_defects`（file・行範囲・keywords）と、list 外だが実在が確認済みの `also_valid` を持つ。
  成果物の `review/findings.json` の各指摘を file・行・keyword で決定的に照合し、検出率と誤指摘
  （どちらにも当たらない指摘）を数える。指摘側の defect id 自己申告は使わない。`min_detection_rate` 以上かつ誤指摘
  `max_false_positives` 以下で合格。
- D7: 秘密値を判定 JSON に出さない（command の stdout/stderr は末尾 2 KiB に切り、成果物の内容は数値と path だけ出す）。
  外部 network・LLM は呼ばない。実クラスタ job は grader が起動しない（記録の検査だけ）。

## 帰結
- case の中身（K1〜K4 の pristine テスト・参照値・欠陥一覧）は WU `cases` が作り、grader は schema と採点を持つ。
  pristine テスト・欠陥一覧は試行の作業場所・KB snapshot に入れない（WU `isolation`）。
- 実 cargo の実行（BenchFS は UCX 必須で login node が要る）での受入 command の実経路検証は未実施。
  grader の試験は stub runner と合成成果物で行う。
