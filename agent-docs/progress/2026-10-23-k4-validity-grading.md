---
title: 研究評価再監査 1/P0: K4 の妥当性を独立採点し、入力 archive・generator・preflight・freeze を実資産まで接続する
tasks: [01M5KVHA4222TK6VEKW599SNMA]
status: done
updated: 2026-10-23
completed: 2026-10-23
adr: agent-docs/adr/2026-10-23-k4-cells-validity-grading.md
---
# 研究評価再監査 1/P0: K4 の妥当性を独立採点し、入力 archive・generator・preflight・freeze を実資産まで接続する

監査対象 main: 13d64acd。作業を始めたときに最新 main で再現を確認した。K4 の grade-case は 5 件数 metric だけで、`parse_report` は metric 行以外を読まなかった。
K3/K4 の grade-case には `input_archive` が無く、generator・preflight・launcher にも接続されていなかった。
この時点の main ではどれも解消されていなかった。最新 main で既に解消済みだった項目は無い。

## commit（本 branch `celeris/01M5KVHA4222TK6VEKW599SNMA`。main には未反映）

`git log --reverse --format=%s $(git merge-base HEAD main)..HEAD` の全 commit（古い順）。基点は `git merge-base HEAD main`。
rebase で SHA が変わるため、hash ではなく件名で挙げる。main への反映は、人の取り込み（delivery）を待つ。

| 件名 | 内容 |
|---|---|
| phase 1: K4 の採点をセル有効性の再計算にし、固定入力 tgz を generator→stage→preflight→grader まで接続、main-4 bundle の pin 検査 | 実装本体 |
| phase 1: architecture-map の提出物 path を文書 path 検査から外す | 文書 path 検査の修正 |
| fix(campaign): align public case judged scope with evaluator checks | K1/K3/K4 の judged_scope を grade-case に揃えた |
| phase 1: judged-scope WU の範囲外変更を revert し、検査結果を記録 | 進捗 |
| phase 1: preflight と stage-grader-inputs の input archive 事前拒否を試験し、stage を全件照合後の複写に直す | 事前拒否の試験、stage の 2 段化 |
| fix(campaign): freeze declared asset locks and audit real K4 inputs offline | `celeris-campaign lock`、pins の pending 検査、実原票の無 LLM 監査試験 |
| phase 1: bundle-nollm WU の範囲外 merge を外し、再検査を記録する | 進捗 |
| phase 1: close-progress — 進捗を実 commit・試験数・blocker・開始可否で書き直す | 進捗 |
| phase 1: progress-hashfree — 進捗の commit 表を件名で列挙し、提出時の HEAD の検査を記録する | この文書だけを変更。コードは変えていない |
| phase 1: progress-hashfree — 提出時の HEAD の検査結果を記録する | この文書だけを変更。コードは変えていない |
| phase 1: progress-hashfree — source-size-browser 進捗の front matter を main 版に揃え、提出時の HEAD の検査を記録する | 他 task の進捗 1 file を main と同じ内容にし、この文書を更新。コードは変えていない |
| phase 1: progress-hashfree — 提出時の HEAD の再検査の結果を記録する | この文書だけを変更。コードは変えていない |

## 実装の要点
- `crates/campaign/src/cells_validity.rs`: 固定 raw から次を決定的に計算する。期待セル集合（`planned-cells.json`）、完了記録（`cell.json`/`exit.json`）、
  IOR の実効設定（fsync・checkRead・api、fallback 警告）、mdtest、`qstat -xf` の job 終了状態（F と Exit_status）。
  そのうえで `results/cells.csv`（行集合・valid・主理由・数値）と報告の件数を照合する。LLM による採点はしない。
- `grader.rs`: `RecomputeSpec.cells`。`preflight.rs check_input_archive`: grade-case の隣の tgz について、実在・sha256・公開入力と同じ bytes か・展開・`pinned_raw`・cells の再計算可否を見る。
- `inputs::stage_grader_inputs` と CLI `stage-grader-inputs`: 全 case を照合してから複写する。`measure-launcher.sh prepare` から呼ばれる。
- `fixtures/grade-cases/tools/gen_recompute.py`: K3/K4 に `input_archive`、K4 に `cells` を生成する。short-ml は対象外（ADR D4）。
- K4 資産（task.md・private.json・case.json）を再 freeze した。K1/K3/K4 の `judged_scope` を grade-case の検査に揃えた。
- `scripts/campaign/check-main-pins.py`: `--pins` に対応した。RELEASE 等が未記入なら pending として非 0 で終わる。既定（main-1）の挙動は変えていない。`celeris-campaign lock` を追加した。

判定規則: job 43706 は `F / Exit_status = -20`。完了記録のある 30 セルは有効。BenchFS W2 の 2 セルは `missing; job_failed(exit=-20)` で無効。分母は 32。
F かつ Exit 0 は正常として扱う。

## 実行して確認した範囲（検証済みと言えるのはここまで）
「単体」は repo の試験、「実原票」は host 上の実 tgz、「fixture」は合成データまたは偽 Host を意味する。
- 単体＋実原票: `cargo nextest run -p celeris-campaign --test k4_cells_validity`（10 passed）。正解の通過、欠測を分母に残すこと、全セル valid の拒否、
  実効 fsync=0 を valid とした報告の拒否、failed job を valid とした報告の拒否、F/Exit 0 の通過、表と報告の不一致の拒否を確かめた。
  `real_k4_archive_grades_thirty_of_thirty_two` は実原票で 32/30 を出す。
- 単体（fixture）: `cargo test -p celeris-campaign --test preflight_input_archive`（12 passed）。preflight について、合成 tgz の正例と、
  欠落・sha256 不一致・公開入力と bytes 違い・展開失敗・pinned_raw 不一致・cells 再計算不能の負例を確かめた（負例では transport を 0 回しか呼ばない）。
  stage-grader-inputs の正例と負例も確かめた。どれも合成 fixture と偽 Host によるもので、実 launcher/daemon は使っていない。
- 単体: `cargo test -p celeris-campaign --test k_case_judged_scope`（2 passed）。`python3 scripts/campaign/test_check_main_pins.py`（36 OK）。
- 無 LLM の CLI 通し（記録: mlsys-main-4/nollm/summary.json）: 実原票 tgz（sha256 `128057cf…`）を使った。結果は次のとおり。
  - 正例（exit 0）: stage-grader-inputs、grade、grade-out、check、export
  - 負例（exit 1）: 全セル valid の grade、archive 欠落の stage、sha 不一致の stage
  - preflight の正例・負例（preflight-positive/missing/sha.json）は偽 Host によるもので、executor は simulated。
    summary.json も `fake_host: true`、`simulated_executor: true`、`real_launcher_connected: false`、`measurement_claim: false` と記録している。
- 新版 bundle への実 CLI preflight（nollm/bundle-preflight-pending.json）: graders と case-inputs は通った。
  K2 repo・cap-env・認証/runtime・project ID・測定設定が不足しており exit 1（paid_calls_made=0）。実 bundle の preflight はまだ通っていない。
- 旧 pilot の再採点（新 grader 版、旧 grades.jsonl は不変）: A・B とも `model_failure`（旧契約の成果物）。B は UnifyFS W3 を無効と誤判定している。
- 実行していないこと: 実 launcher・daemon への接続、cap-probe、env-lock、有料 pilot。

## 必須検査（提出時の HEAD で実行）
最後の commit の後、提出時の HEAD（working tree clean）で次を実行した。ログは run 成果物 root の final-test-affected.log・final-clippy.log・final-fmt.log。
- `bash scripts/dev/test-affected.sh`: exit 0。基点は `git merge-base HEAD main`（`CELERIS_WU_BASE` にその値を渡して branch 全体の変更を対象にした）。2634 tests run、2634 passed、7 skipped、195.268 秒。最後の commit（結果の記録）の後にも同じ 3 検査を再実行し、exit と件数が一致することを確かめた。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0（差分なし）。
- 文書検査: `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`: すべて exit 0。前回は main 由来の agent-docs/progress/2026-10-23-source-size-browser.md の front matter（title・updated）欠落で progress-index --check が exit 1 だった。main の同じ file の版（front matter 2 行の追加だけ）を写して直した。本文は変えていない。
  progress-index は close-progress の時点で、main 由来の他 task の進捗 8 件の front matter（title/tasks/updated）欠落で exit 1 だった。
  別 branch の「docs: fill missing progress front matter fields」の該当 8 file 分を写して直した。各 file の本文は変えていない。

## 保存場所（run 成果物 root = /local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts）
- raw（変更なし）: K4 は /local/celeris/data/measure/main-3/out/snapshot/inputs/main-v3-n2-43706.tgz（sha256 `128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1`）。
  K3 は /local/celeris/data/scratch/mlsys-pilot-2/inputs/main-43223.tgz。
- 旧 pilot の新 grader 版での再採点: artifacts/regrade-main-3-k4/（A.json・B.json・README.txt）。旧 main-3/out/grades.jsonl は不変。
- 次の測定 bundle: artifacts/mlsys-main-4/。再 freeze 版は revisions/judged-scope-v2（campaign_id mlsys-main-4-v2、campaign_sha256 `c678b60a999f…`、
  宣言資産の lock の sha256 `abdf0b6147ad…`、task-template の max_turns 100）。旧版の main-4 も残している。
- freeze の状態: artifacts/mlsys-main-4/freeze-status.json。45 frozen、14 pending、`measurement_ready: false`。
- 無 LLM の通し: artifacts/mlsys-main-4/nollm/（summary.json、各 CLI の stdout/stderr、fixture/positive-out に grades・集計・export）。
- 旧資産が不変であることの照合: mlsys-main-4/old-assets-before.json、old-assets-after.json。
- 集計: 新しい試行は無いので、新しい集計も無い。

## 未解決の blocker（freeze-status.json の pending 14 件）
- RELEASE、A_VERSION、A_PROGRAM、A_PROGRAM_SHA、CAMPAIGN_BIN_SHA: 承認する release と A harness の実体が決まっていない（運用者が pins.json に記入する）。
- env-lock.json、cap-env.json: 測定環境で採取した実証跡が無い。
- daemon.toml、launcher.env、canaries.json: 測定専用の設定と path が決まっていない。
- repo-src: K2 の初期木（BenchFS commit 086c4618…）を配置していない。
- cap-probe と通常 launcher の preflight 証跡: 実 launcher に接続していない。
- task-template.project_id: 測定用 project が未作成（本 run は DB を変更しない）。
- shared-ledger: 新しい測定 root の共有台帳をまだ初期化・承認していない。
- その他:
  - 原票 tgz の `review28/` に旧監査の `cells-43706.csv` が同梱されている（sha256 を固定済みのため差し替えられない）。独立性は grader の再計算で担保する。
  - short-ml の `input_archive` 化は実施していない（次の測定では有効にしない）。

## 次の測定の開始可否
- **不可。** freeze-status.json に pending が残る限り開始できない。上の 14 件を解消し、`check-main-pins.py --root … --pins …` を ok にし、
  `measure-launcher.sh prepare` から通常 launcher の `preflight` まで実環境で通す必要がある。
- K4 pilot は、上記に加えて task 2（成功契約と human_ops 集計）との統合を確認してから判断する。

## 提案
- 次の原票からは、監査の答え（`review28/cells-*.csv`）を同梱しない tgz を作る（解答が漏れる経路を減らすため）。

## 経緯（WU ごと、2026-10-23）
- main: 実装（件名「phase 1: K4 の採点をセル有効性の再計算にし、…」）。attempt 2 で architecture-map の path 検査の不合格を直した（「phase 1: architecture-map の提出物 path を文書 path 検査から外す」）。
- judged-scope: 「fix(campaign): align public case judged scope with evaluator checks」。
- preflight-reject: 「phase 1: preflight と stage-grader-inputs の input archive 事前拒否を試験し、…」。stage が後ろの case の失敗時に前の tgz を残していた穴を、全件照合してから複写する方式に直した。
- bundle-nollm: 「fix(campaign): freeze declared asset locks and audit real K4 inputs offline」。main merge の commit を branch から外し、「phase 1: bundle-nollm WU の範囲外 merge を外し、再検査を記録する」で再検査した。
- close-progress: 「phase 1: close-progress — …」。
- progress-hashfree: commit 表を件名に直し、提出時の HEAD で必須検査を再実行した。各 WU の詳細なログは run 成果物 root の judged-scope-*.log・bundle-*.log・final-*.log にある。
