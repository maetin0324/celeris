---
task: 01M5KVHA4222TK6VEKW599SNMA
status: done
completed: 2026-10-23
adr: agent-docs/adr/2026-10-23-k4-cells-validity-grading.md
---
# 研究評価再監査 1/P0: K4 の妥当性を独立採点し、入力 archive・generator・preflight・freeze を実資産まで接続する

監査対象 main: 13d64acd。再現確認: K4 grade-case は 5 件数 metric のみ（`cells_with_cell_json` 等）、`parse_report` は metric 行以外を読まず、
mlsys-main-3 の実 pilot（A 30/32、B 28/32）をどちらも `model_failure`（旧 `inputs/` 展開先不一致）としか言えなかった。`input_archive` は
実装済み（61c9b78a）だが K3/K4 の grade-case に無く、generator・preflight・launcher は接続されていなかった。すべて最新 main でも未解消だった。

## 実装（main 反映 commit は本 branch の `phase 1:` commit。人が取り込む）
- `crates/campaign/src/cells_validity.rs`（新）: `CellsSpec`、`compute`（期待セル集合 = `planned-cells.json`、完了記録 `cell.json`/`exit.json`、IOR 実効設定
  fsync/checkRead/api と fallback 警告、mdtest `all` 行、`qstat -xf` 記録の job 終了状態 F/Exit_status、値）、`check`（`results/cells.csv` の行集合・valid・
  主理由の語・有効セルの数値、報告の `cells_total`/`cells_valid`/`cells_*_<fs>` と表の件数の一致）、試験用 `synth`（合成 raw・`install_k4_fixture`）。
- `grader.rs`: `RecomputeSpec.cells`。raw 側の不整合は archive 利用時 `env_failure`、成果物側は `model_failure`。`safe_join`/`read_json` を pub に。
- `preflight.rs check_input_archive`: grade-case の隣の tgz の実在・sha256・公開 `case.inputs[]` と同一・展開・`pinned_raw`・`cells` 参照を投入前に確かめる。
- `inputs::stage_grader_inputs` + CLI `celeris-campaign stage-grader-inputs [--manifest] --grade-case-root --inputs-src`、`measure-launcher.sh prepare` が呼ぶ。
  tgz は `.gitignore`（repo に置かない）。
- `fixtures/grade-cases/tools/gen_recompute.py`: K3/K4 に `input_archive`、K4 に `cells`（run・jobs=`review28/qstat-43706.txt`・api・tolerance）。
  K3 の metrics/pins は再生成しても不変（差分は `input_archive` の 5 行だけ）。
- K4 資産の再 freeze: `materials/task.md`（判定規則と出力契約）、`private.json`（期待 30/32・job F/-20・UnifyFS W3 有効の罠）、`case.json`
  （materials_sha256 `77aa6847…`、grade_case_ref `34de0c88…`、private_ref `2aeea567…`）。K3 の case.json は grade_case_ref のみ更新。
- `scripts/campaign/check-main-pins.py`: `--pins <json>` / `pinned/main/pins.json` で測定ごとの承認値（既定は main-1 の値のまま）、grade-case の
  `input_archive` の実在・sha256 検査（PROBLEMS の case だけ）、公開入力と同一 bytes の archive は leak と数えない、`repeats` は experiment_sets から、
  cap-probe 証跡は PROBLEMS に応じる。`measure-run-final.md`・`architecture-map.md` 更新。

## 判定規則の要点（人が承認した final-v3 と一致）
job 43706 は `review28/qstat-43706.txt` で `F / Exit_status = -20`（BenchFS W2 rep1 の host panic）。完了記録を持つ 30 セルは有効、BenchFS W2 の
2 セル（rep1 途中で切れ・rep2 未実行）は `missing; job_failed(exit=-20)` で無効。分母 32。`job_state = F` だけで失敗にしない（F かつ Exit 0 は正常）。
記録なし／F 以外の job のセルは有効にできない。

## 証拠（無 LLM）
- `cargo nextest run -p celeris-campaign --test k4_cells_validity` → 10 passed: 正解通過・欠測は分母・「件数は正しいが全セル有効」拒否・
  実効 fsync=0（Parameters と fallback 警告）を有効→拒否・failed job（F/271）/記録なし/R を有効→拒否、F/Exit 0 通過、F を失敗と読む→拒否・
  表と報告の不一致拒否・数値ずれ拒否・主理由の語なし拒否・raw 側不整合は env_failure・実原票で 32/30（benchfs 6, 他 8）、A 相当通過、B 相当（UnifyFS W3 無効）拒否、全有効拒否。
- `cargo nextest run -p celeris-campaign --test k_cases_grade --test real_asset_grade --test preflight --test lock_cli --test lock_execution --test lock_grading --test launcher --test cap_probe --test env_check --test arm_set_cli_normal --test dagbench_graded --test grader_input_archive --test preflight_contract` → 135 passed, 2 skipped（実 K3/K4 tgz は host の既定 path にあり、K3/K4 の正しい報告=合格・6% ずれ=不合格を実 archive で確認）。
- `python3 scripts/campaign/test_check_main_pins.py` → 35 tests OK（pins 上書き・archive の未配置/改変 NG・配置済みは leak でない）。
  `test_sub_pins.py` は 1 件が本 task 以前から落ちる（`CELERIS_BIN … c9654bff5633 … does not exist`: host に無い release。未変更）。
- `bash scripts/dev/test-affected.sh` → exit 0（2594 passed, 4 skipped）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 旧 pilot の再採点（新 grader 版、旧 grades.jsonl は不変）: `artifacts/regrade-main-3-k4/{A,B}.json`。どちらも `model_failure`
  （旧契約の成果物: metric 行なし・列名違い）。期待件数は 32/30。A の本文は 30/32、B は 28/32（UnifyFS W3 を無効）で、新規則では B の判定が誤り。
- 次の測定 bundle（`artifacts/mlsys-main-4/`）: fixed 資産・`measure-src/main/{materials,kb,inputs}`・`stage-grader-inputs` で grade-case の隣に
  tgz 配置済み・`final.json`（campaign_id mlsys-main-4、K2+K4、materials `ae9007f5…`、`validate` → campaign_sha256 `7cd45b3c9b0b…`、dry-run within_budget true）・
  `task-template.json`（`max_turns: 100`）・`pins.json`。`check-main-pins.py --root` の残り 12 NG は運用側の物だけ（launcher.env・canaries・cap-env・daemon.toml・
  env-lock/lock・cap-probe 証跡・K2 preflight 証跡・RELEASE の pin）。`check-main-pins.out` に保存。

## 保存場所
- raw: 変更なし（`/local/celeris/data/measure/main-3/out/snapshot/inputs/main-v3-n2-43706.tgz`、sha256 `128057cf…`。K3 は `/local/celeris/data/scratch/mlsys-pilot-2/inputs/main-43223.tgz`）。
- grade: 旧 `main-3/out/grades.jsonl` 不変。新 grader 版の再採点は `artifacts/regrade-main-3-k4/`。
- 集計: なし（新しい試行は無い）。

## 未解決事項・blocker
- 次の測定の `pins.json` に RELEASE/A_VERSION/CAMPAIGN_BIN_SHA/A_PROGRAM(_SHA) を人が入れる（承認する release が決まってから）。env-lock/lock は測定 daemon を立てた後に launcher の `lock` 段で作る。
- 原票 tgz の `review28/` に旧監査の `cells-43706.csv`（答えに近い表）が同梱されている。tgz の sha256 は固定済みで差し替えられない。task.md は「同梱の集計は証拠にしない」と明記したが、解答の独立性は grader の再計算（表の数値・理由・分母の照合）で担保する。
- short-ml の `input_archive` 化は未実施（次の測定で有効にしない。ADR D4）。
- 実 pilot（有料）は本 task の範囲外で未実施。

## 次の測定の開始可否
- K4 の採点・入力接続・preflight・pin 検査は無 LLM で接続・検証済み。K4 pilot は task 2（成功契約と human_ops 集計）と合わせて判断する。
  開始前に人が行う: bundle の運用側 file（上記）を置き `check-main-pins.py --root … --pins …` を ok にし、`measure-launcher.sh prepare`（stage-grader-inputs を含む）→ `preflight` を通す。

## 提案
- 原票 tgz に監査の答え（`review28/cells-*.csv`）を同梱しない版を次の原票から作る（解答の漏れ経路を減らす）。

## attempt 2（2026-10-23）
- 前回の不合格は test-affected.sh の文書 path 検査だけ（docs/architecture-map.md の `` `results/cells.csv` `` を repo の path として扱った）。提出物の path なので backtick を外した。
- `bash scripts/dev/test-affected.sh` exit 0（nextest 2594 passed / 4 skipped、check-doc-links ok）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
