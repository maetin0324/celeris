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

## judged-scope WU（2026-10-23、attempt 3）
- K4 と K3 の公開 case に `judged_scope: ["analysis"]` を明記した。review check を持たない grade-case と一致し、K4 の campaign report は `judged_scope stages=analysis not_judged=review` を出す。
- 全 case の照合で K1 にも同種の不一致を検出したため、K1 は実際の implementation / experiment 検査に限定した。K2・short-ml は変更していない。
- `crates/campaign/tests/k_case_judged_scope.rs` は CaseSet で全 case を読み、全実 case の判定範囲を grade-case の検査実体と比較する。grade-case の無い K0 は mock control であることを検査する。別の試験で K4 の campaign_scope と scope 行を検査する。
- hash 追跡: K3/K4 は公開 case.json の範囲宣言のみで、materials・KB・private・grade-case の bytes と各 pin は不変。K1 は generator が範囲宣言を grade-case に転記するため同ファイルも更新し、grade_case_ref を `10c180ed…` に再 pin した。旧 grade-case hash の参照は case.json のみだった。旧 K3/K4 case.json SHA の固定参照は repo 内に無い。測定 bundle の再 freeze は後続 bundle-nollm WU の対象で、旧測定の grades/raw は変更していない。
- この節は判定範囲の修正だけの記録。上記の新 bundle 検証済みとの記述や古い試験数の整合は、後続 preflight-reject / bundle-nollm / close-progress WU で解消する。本 WU では測定開始可とはしない。
- 必須検査中に既知の偽 webhook 試験の EOF 誤計数（空本文 2 件を通知に加算）を再現。HTTP の POST /hook 完了時だけ記録し、応答前に記録するよう試験 fixture を補修し、EOF・別 path・正規 POST を順に送る回帰試験を追加した。
- 最終検査: `cargo test -p celeris-campaign --test k_case_judged_scope` exit 0（2 passed）、`cargo test -p celeris --test credential_wait_notify` exit 0（2 passed）。`bash scripts/dev/test-affected.sh` exit 0（2603 passed / 5 skipped、177.293 秒）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、`git diff --check` exit 0。webhook 補修 commit は `7cf00c45`。
- この WU の検査ログは run 成果物ディレクトリの judged-scope-targeted-final.log / judged-scope-webhook.log / judged-scope-test-affected-final.log / judged-scope-clippy-final.log / judged-scope-fmt-final.log に保存した。初回失敗ログも別名で保持した。

### judged-scope WU 再試行（2026-10-23）
- 前回 run は crates/celeris/tests/credential_wait_notify.rs の偽 webhook 補修（7cf00c45）を含み、WU の範囲 check が範囲外として不合格。8102013e で revert した（同等の修正は main 165d76e1 にあり、main 取り込みで入る）。
- judged_scope 修正 98c29a8c は維持。`cargo test -p celeris-campaign --test k_case_judged_scope` 2 passed。
- test-affected exit 0（2602 passed / 5 skipped）、clippy・fmt exit 0。範囲 check の out-of-scope は空。

## preflight-reject WU（2026-10-23、attempt 3）
- 新しい試験 `crates/campaign/tests/preflight_input_archive.rs`（`cargo test -p celeris-campaign --test preflight_input_archive`: 12 passed）。
  fixture は `synth::healthy().install_k4_fixture`、tgz は `asset_assembly::create_tgz` で作る。外部には出ない。
  - preflight の正例: `preflight_accepts_the_synthetic_k4_archive`
  - preflight の負例（`check` が `graders` だけを不足にし、`run_gated` が transport 0 回・out 無しで止まる＝LLM run を起こさない）:
    `preflight_rejects_a_missing_grader_archive`（grade-case の隣に無い）、`preflight_rejects_an_archive_that_differs_from_the_pinned_sha256`、
    `preflight_rejects_a_grader_archive_whose_bytes_differ_from_the_public_input`、`preflight_rejects_an_archive_that_cannot_be_extracted`、
    `preflight_rejects_an_archive_whose_raw_differs_from_pinned_raw`、`preflight_rejects_an_archive_from_which_cells_cannot_be_recomputed`
  - stage-grader-inputs（`inputs::stage_grader_inputs` と CLI）: `stage_grader_inputs_places_the_pinned_archive_next_to_the_grade_case`、
    `stage_grader_inputs_rejects_a_missing_archive_and_places_nothing`、`stage_grader_inputs_rejects_a_hash_mismatch_and_places_nothing`、
    `stage_grader_inputs_validates_every_case_before_copying_any`、`stage_grader_inputs_cli_stages_or_exits_nonzero_without_placing_anything`
- 見つかった穴と修正（`crates/campaign/src/inputs.rs`）: `stage_grader_inputs` は case を順に照合しながら写していたため、後ろの case の archive が
  欠落・不一致だと前の case の tgz を置いたまま Err になっていた。全 case を照合してから写す 2 段に変え、写しの失敗時は `.partial` を消す。
  preflight.rs の `check_input_archive` は各 miss を既に区別して返しており変更なし。
- 必須検査: `bash scripts/dev/test-affected.sh` exit 0（2614 passed/5 skipped）、`cargo clippy --workspace -- -D warnings` exit 0、
  `cargo fmt --all -- --check` exit 0。1 回目の test-affected は範囲外の負荷 flake 4 件で exit 100（`celeris::root_done_summary` 3 件は
  偽 Discord hook が空の接続を数える、`task-dispatch` の `browser_policy_missing_worker_rejection_is_not_infra_requeue`）。単独再実行と 2 回目は全合格。
  WU の範囲外（crates/celeris・task-dispatch）なので本 WU では直さない。
- 残り: 新 bundle（main-4）での実 preflight と無 LLM の通し（bundle-nollm WU）、進捗の SHA・試験数の書き直し（close-progress WU）。

## bundle-nollm WU（2026-10-23）

この節は上の旧 bundle に関する記述を更新する。測定開始は不可。進捗本文全体の整理は後続 close-progress WU の対象。

- 旧 main-4 は保持し、成果物 root の mlsys-main-4/revisions/judged-scope-v2 に再 freeze。
  campaign ID は mlsys-main-4-v2、campaign_sha256 は c678b60a999f95ed8f34d792d3ec3bb6ff2a4f2344885e0951adaf92d9a8048e。
  K3/K4 の analysis 宣言を含む現行 case を配置した。task-template.max_turns は 100。
- `celeris-campaign lock` を追加。通常 preflight の expected_lock を stdout に出すだけで環境や台帳を作らない。
  新版 lock.json の内部 sha256 は abdf0b6147ad830551200218206545a113591d585f4e59bb8498359815d2ae7a。
  宣言資産の lock であり、環境・取り込み元の実在を承認するものではない。
- `check-main-pins.py` は pins.json の RELEASE/A_VERSION/CAMPAIGN_BIN_SHA/A_PROGRAM が未記入なら
  「pending: 人が記入」で非 0 にする。既定の main-1 release による検査は続けない。pins を指定しない旧 bundle の挙動は維持。
- 成果物 root は /local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts。
  mlsys-main-4/freeze-status.json に各 pin の frozen/pending、取得元、不足物、運用者の手順を保存。
  RELEASE、A executable、環境証跡、K2 初期木、project ID 等は未確定。旧 manifest の harness version は引き継いだ候補であり承認 pin ではない。
- 新版 bundle への実 CLI preflight は mlsys-main-4/nollm/bundle-preflight-pending.json。
  graders と case-inputs は通過し、K2 repo、cap-env、認証/runtime、project ID、測定設定の不足で exit 1（paid_calls_made=0）。
  A program は /usr/bin/false を指定し、LLM CLI や daemon を起動していない。
- 旧 bundle、再採点済み grades、main-3 の原票と grades.jsonl は mlsys-main-4/old-assets-before.json と old-assets-after.json で hash を照合して不変。
- 無 LLM の実原票監査: `cargo test -p celeris-campaign --test preflight_input_archive real_archive_offline_audit -- --ignored`（K4_AUDIT_DIR と K4_REAL_ARCHIVE を指定）exit 0、1 passed。
  実 tgz は sha256 128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1、期待 30/32。
  stage / grade / grade-out / check / export の CLI はすべて exit 0。archive 欠落・sha 不一致の stage と全セル valid の grade は exit 1。
  preflight の正負例は偽 Host による check / run_gated（負例の transport 呼び出し 0 回）。executor は simulated。
  実 launcher / daemon の接続成功を意味しない。新版 bundle の実 CLI preflight は上記 blocker により非 0。
  nollm/summary.json に argv・exit・出力・新版 source hash との一致を保存。nollm/fixture/positive-out に grades・集計・export を保存。
  原票を展開した fixture/raw も保存した。fixture は KB canary とその hash、監査専用 manifest/runtime だけを変更し、K4 の raw/grade-case は新版と同じ。
- 初回の監査 fixture では副比較の subset と、正例 out の残存による負例の未作成チェック不一致を修正した。失敗ログは bundle-real-audit.log / bundle-real-audit-v2.log / bundle-real-audit-v3.log、
  成功ログは bundle-real-audit-final.log。以前の出力は nollm/attempts/v3 に分けた。
- 最終検査: `python3 scripts/campaign/test_check_main_pins.py` 36 passed、asset_lock_cli_needs_no_runtime_and_rejects_changed_grade_case 1 passed。
  `bash scripts/dev/test-affected.sh` exit 0（2615 passed / 6 skipped、342.347 秒）、`cargo clippy --workspace -- -D warnings` exit 0、
  `cargo fmt --all -- --check` exit 0、`git diff --check` exit 0。
  初回の test-affected は dagbench_harness の graded_limits_and_failed_costs_use_the_formal_entry が compute 時の ENOENT で不合格（2614 passed / 1 failed）。
  同時ビルド終了後の単独再実行で全合格。範囲外のコード変更は行っていない。
  ログ: bundle-python-tests.log、bundle-lock-test.log、bundle-test-affected-final.log、bundle-clippy.log、bundle-fmt-final.log。
- freeze-status は 45 frozen / 14 pending。実 launcher 接続、環境承認、K2 初期木、project ID、共有台帳の確認が残る。
  K4 pilot はこれらの解消と task 2 の成功契約との統合確認まで開始不可。
- attempt 3（範囲 check の不合格対応）: 前回 run は flaky 回避のため main 81fd646a を merge した commit を branch に残し、WU の範囲 check が
  50 件の範囲外 file（crates/celeris・task-worker・web 等）で不合格だった。merge commit を branch から外し、実装 commit
  「fix(campaign): freeze declared asset locks and audit real K4 inputs offline」を HEAD に戻した（成果物・bundle・summary は不変）。
  偽 webhook の補修は main 側に既にあり target 同期で入る。再検査: 範囲 check の out-of-scope 空、
  `bash scripts/dev/test-affected.sh` exit 0（2615 passed / 6 skipped、133.958 秒）、`cargo clippy --workspace -- -D warnings` exit 0、
  `cargo fmt --all -- --check` exit 0。ログ: bundle-test-affected-attempt3.log、bundle-clippy-attempt3.log。
