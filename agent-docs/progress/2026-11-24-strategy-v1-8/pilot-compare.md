---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: pilot-compare
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 8 pilot-compare: 比較（指標・漏れ・secret）と盲検の採点材料の script

## 結論
- `scripts/dev/parent-eval-blind-compare.py` に `--pilot` を足した（ADR 2026-11-24-strategy-pilot-ablation D4・D6）。起動の形:
  `python3 -I scripts/dev/parent-eval-blind-compare.py --pilot --runs <iter-1> [--runs <iter-2>] --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --out <dir> [--research-src <c-results.md>]`。
  `<dir>/metrics.json`（同じ入力で同じ byte。時刻を書かない）に、反復 × 方式 × 事例の指標を書く。漏れがあれば exit 1（metrics.json は書く）。
- `scripts/dev/parent-eval-blind/pilot_score.py` を足した（ADR D5・D7）。`blind`・`check-blank`・`check-scored`・`tally` の 4 つ。
- 合成の testdata（`testdata/pilot-runs/iter-1`、漏れを混ぜた `testdata/pilot-runs-leak/iter-1`）は `testdata/make_pilot_runs.py` が作り、試験が生成物と commit した file の一致を確かめる。研究ケースの作業 tree は合成の `testdata/research/c-results.md` から切った（実データを開いていない）。
- 子 6 の呼び方（`--pilot` なし）は変えていない。commit した fixture（`crates/task-api/tests/fixtures/parent_checkpoint_live/blind/`）から同じ `compare.json` を byte で再現する試験を置いた。
- production（`crates/*/src`・control-ts・web）の差分は 0。

## 実装前 brief
1. 根本原因: 子 6 の比較は方式 b の verdict・next と読みしか見ず、方式 a・c と研究ケースの指標、出力側の漏れ、secret・mutation を数えなかった。盲検の採点材料（方式の伏せ方・採点欄・開封）を作る道具が無かった。
2. 責務 owner: pilot の比較と採点材料の script（`scripts/dev/parent-eval-blind-compare.py`・`scripts/dev/parent-eval-blind/pilot_score.py`）。production は owner ではない。
3. 既存の再利用先: 子 6 の `check_reads`（`extra` の禁じた名を足せる形にした）、子 0 の `replay.py`（`load_events`・`checkpoints`・`norm`・`cost_so_far`・`ANSWER_KEY_MARK`）と `research_case.py`（`leaks`・`sections`・`POINTS`。`research.py` 経由）を path で import、driver の `CASES`・`ARMS`・`STRATEGY_POINT`・`STRATEGY_STAGE`、`task_core::strategy` の `static SECRET` の正規表現（Rust の source から読む）、rubric.md の節（そのまま写す）、子 6 の試験 `parent_checkpoint_blind_no_answer_key_or_future` の漏れの規則（印・理由の文・期待の組・未来の語・後の時刻と本文）。
4. 選択肢: (i) secret の規則を Python に写す（不採用: 規約 2）。(ii) task-api の試験の binary から `contains_secret` を呼ぶ（不採用: 比較のたびに cargo の build が要り、`contains_secret` は真偽だけで数を返さない）。(iii) Rust の source の `static SECRET` の literal を実行時に読む（**採用**。読めなければ止まる）。採点材料は compare の出力（metrics.json）から answer key に依らない欄だけを白い list で写す。
5. 新しい状態・分岐・fallback・port: production には無し。compare に `--pilot` の分岐（`--pilot` なしの経路は変えない）。
6. 撤去対象: 無し（子 6 の呼び方は人の条件で残す。旧い仕組みの置き換えではない）。
7. 不変条件と試験: 下の節。

## 変えた file
- `scripts/dev/parent-eval-blind-compare.py`: `--pilot`・`--runs`（繰り返し可。dir の名が反復の名）・`--research-src`。`check_reads` に `extra` を足した（子 6 の既定は同じ）。`--pilot` なしで `--runs`・`--research-src` を渡すと exit 2。
  - 指標（`metrics.json` の `definitions` に定義を書く）: 時点ごとに verdict・next と answer key の一致（replay）、誤った pivot（answer key で on_track・unknown の時点に pivot・discriminating_experiment）、見逃し（answer key が continue 以外の時点に continue）、根拠の参照が無い・slice の外の claim の数（uncertainty は除く。slice の中 = seed の root と子の task・artifact、input/ の本文にある commit・url・knowledge・chat の値）、usage（run 数・USD）、secret の形の数（値は書かない）、run の記録の mutation（`input/files.json` と `input/tree/` の sha256 の食い違い、review.json の外への Write・Edit・Bash の redirect）、`（Outcome の記録は無い）` の有無（ADR D8 で採点から外す）。
  - 方式 × 事例: 未達発見の時点（verdict が at_risk・off_track で next が continue 以外の最初の時点）、未達発見までの task 数（replay は fixture の `children_done` の数、検知が無ければ史実の 8。研究ケースは 11.15 節からの節の数）、検知の後の史実の worker run と USD（`after_detection`）、answer key の検知できた時点から検知までの史実の worker run と USD（`unneeded_after_first_detectable`。方式 a は 137 run・264.2525 USD で rubric のコストの史実と一致）、誤った pivot・見逃し・一致の数、usage（方式 c は版 v1 の段を含む）、反復の間の verdict・next の一致（`agreement`）。
  - 漏れ（exit 1）: replay は sentinels.json の語（時点より後）・answer key の印（`ideal_verdict`・`ideal_action`・`answer_key`・`celeris.replay-answer-key/1`）と理由の文・期待の組（input/ だけ）・cutoff 後の event の時刻と本文・`events.jsonl` の cutoff 後の行。研究ケースは `research_case.leaks`（初出が時点より後の SENTINELS）・後の節の見出し（`--research-src` があるとき）・`research-case.md` の参照の表の文（input/ だけ）。禁じた読み（子 6 の名と answer_key・answer-key・research-case.md・c-results.md・pilot-key）も漏れに数える。版 v1 の段（`<case>/strategy-v1/`）は事例の段の時点（A-mid・11.15）で検査する。
- `scripts/dev/parent-eval-blind/pilot_score.py`（新規）:
  - `blind --runs <dir> [--runs …] --metrics <json> --out <pilot-scoring.md> --key-out <pilot-key.json> [--seed N] [--repo-copy <md>]`: 事例ごとに方式を乱数の id（8 文字）に替え、時点の中の記録の順を乱数で並べ、時点ごとの判定・axes・理由・主張と根拠・方式 c の版 v1 の候補・費用と、answer key に依らない指標、id ごとの決定的な指標の表、rubric の 6 軸 × 0〜2 点と根拠の空欄・採点者・レビュー時間（分）の欄、rubric.md の採点の手順・軸と基準・合格線の節（そのまま）、`pilot-key.json` の sha256 を書く。方式の名・run id・判断 id・harness の名・answer key の値・史実の参照の点は書かない。記録の文の中の方式の名に当たる語は `［伏せ字］` に替える。compare が漏れか修正前の入力として外した記録には「採点から外す」と書く。`--repo-copy` は研究ケースの理由・主張・版の文を除いた写し。
  - `check-blank <md>`: 節・sha256 の行・rubric の節（rubric.md と同じ byte）・6 軸の採点欄がそろい、採点欄が空で、rubric の節の外に方式の名が無い。
  - `check-scored <md>`: 全軸が 0〜2、根拠が空でない、採点者が `human:<名>`、レビュー時間が数。
  - `tally --scoring <md> [--scoring …] --key <pilot-key.json> [--metrics <json>] [--out <json>]`: sha256 を確かめて開封し、方式 × 軸 × 採点者の表と rubric.md の合格線の判定（方式 b・c ごとに、両事例で pivot の時点 2・unsupported claims 1 以上・parent goal achievement 0 でない、両事例の合計が a を 3 点以上上回る、execution 2 で goal 0 の記録があれば失格）、`--metrics` があれば誤った pivot 2 回以上で pivot の時点を 0、2 名の軸の差が 1 を超えた軸と、合計の差が 4 を超えたときのやり直しを出す。
- `scripts/dev/parent-eval-blind/testdata/make_pilot_runs.py`（新規）と生成物 `testdata/pilot-runs/iter-1`（replay の C-mid・D-done・F-done と研究ケースの 11.17 × 方式 a・b・c、事例ごとの版 v1 の段）・`testdata/pilot-runs-leak/iter-1`（4 件の漏れ）。
- 試験 `scripts/dev/parent-eval-blind/test_pilot_compare.py`（11 件）・`test_pilot_score.py`（9 件）（新規）。

## 守る不変条件と、既存の試験で検出できない理由
- 方式 b・c の input/ と出力に、時点より後の語・answer key の印と理由の文・cutoff 後の event・禁じた読みがあれば exit 1（ADR D4.1）。
- 指標は同じ入力で同じ byte で、時刻を書かない。比較は run の記録を書き換えない。
- 漏れの規則と secret の規則は子 0 の script と Rust の source を使い、写さない。
- 採点材料は方式の名と answer key の値を書かず、採点欄は空で、rubric の節は rubric.md と同じ。tally は sha256 が違う key を開封しない。
- 子 6 の呼び方（`--pilot` なし）は同じ `compare.json` を書く。
- 既存の試験で検出できない理由: `parent-eval-blind-compare.py` には試験が無く、採点材料の script は無かった。

## 試験
- 否定対照（受け入れ条件 0・1・2）:
  - `testdata/pilot-runs-leak` → exit 1。`leaks` に C-mid/b の `events.jsonl` の cutoff 後の event（F の worker_finished。時刻と本文も）、C-mid/c の prompt.md の answer key の理由の文、D-done/b の output.json の未来の語 `parent-unmet`、研究 11.17/b の slice の `1 / 488`（初出 11.20）が出る。
  - 1 件ずつ混ぜた対照（`test_each_negative_control_alone_is_exit_1`）: answer key の印・理由の文（出力）・期待の組・後の節の見出し・参照の表の文・後の event の時刻（出力）、と禁じた読み（`test_forbidden_read_is_a_leak`）。どれも exit 1。
  - `check-blank` は方式の名（`方式 b`・`Evidence + 複数戦略`・`fixed_plan`・`"arm": "c"`）の混入、採点欄の記入、合格線の文の変更で exit 1。`check-scored` は空欄・3 点・根拠の空・`operator:` の採点者で exit 1。`tally` は key の sha256 の不一致と空欄の採点で exit 1。
- 変更前に落ちた試験（`git archive "$CELERIS_WU_BASE"` の tree に新しい試験と testdata だけを置いて実行）:
  - `python3 -I scripts/dev/parent-eval-blind/test_pilot_compare.py` → `Ran 11 tests`、`FAILED (failures=12, errors=1)`。落ちたのは `test_clean_runs_write_metrics_without_time_and_same_bytes`・`test_leak_testdata_is_exit_1_with_each_kind`・`test_each_negative_control_alone_is_exit_1`（6 件の subTest すべて）・`test_forbidden_read_is_a_leak`・`test_secret_shapes_are_counted_without_their_value`・`test_mutation_of_the_record_is_reported`・`test_iterations_agreement_and_pre_fix_input`、ERROR は `test_secret_pattern_is_read_from_the_rust_source`（`secret_pattern` が無い）。base でも通ったのは `test_testdata_is_what_the_generator_writes`・`test_compare_does_not_write_the_runs`・`test_child6_compare_without_pilot_is_unchanged`（子 6 の挙動の回帰の確認で、base でも通るのが正しい）。
  - `python3 -I scripts/dev/parent-eval-blind/test_pilot_score.py` → exit 1（`ModuleNotFoundError: No module named 'pilot_score'`）。
- 変更後（提出時の HEAD）: `test_pilot_compare.py` → `Ran 11 tests` OK。`test_pilot_score.py` → `Ran 9 tests` OK。既存の `test_driver.py` → OK、`test_research.py` → OK。
- 子 6 の比較（受け入れ条件 3）: `test_child6_compare_without_pilot_is_unchanged` が fixture の `replay/` を写した dir で `--pilot` なしの compare を走らせ、exit 0 と commit した `compare.json` と同じ byte を確かめる。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。crate の試験の対象は無し（「試験対象の変更なし」。変えたのは scripts/dev と進捗だけ）。script の試験 `Ran 126 tests` OK、repo_health_gate: all checks passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `git diff --stat "$CELERIS_WU_BASE" -- 'crates/*/src' control-ts web` → 差分 0。
- 範囲（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変えた path は `scripts/dev/parent-eval-blind-compare.py`・`scripts/dev/parent-eval-blind/{pilot_score,test_pilot_compare,test_pilot_score}.py`・`scripts/dev/parent-eval-blind/testdata/{make_pilot_runs.py,pilot-runs/,pilot-runs-leak/}`・この進捗。

## LoC（`git diff --numstat "$CELERIS_WU_BASE"`）
- production（`crates/*/src`・control-ts・web）: 0。
- 道具の script: `parent-eval-blind-compare.py` +363・−2（`--pilot`）、`pilot_score.py` +428（新規）。
- 試験: `test_pilot_compare.py` +237、`test_pilot_score.py` +214、`testdata/make_pilot_runs.py` +209（すべて新規）。生成した testdata（JSON・Markdown、163 file、約 41 KB）。
- 純増の理由: 指標・漏れ・secret・mutation の比較（ADR D6 の表のうち決定的な欄）と、盲検の採点材料・開封の道具を足した。新しい Manager・trait・状態機械・DB 表・Agent サービスは無い。

## operator・後の WU への申し送り
- operator の実 LLM の run の後、比較は反復の dir を並べて渡す:
  `python3 -I scripts/dev/parent-eval-blind-compare.py --pilot --runs $P/iter-1 --runs $P/iter-2 --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --research-src /local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md --out /local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-compare`
  （`$P` は arm-driver の進捗の operator の手順の `pilot-runs`）。replay と研究ケースを同じ `--out` の反復の dir に流しておく（driver は `<dir>/<case>/` に分けて書く）。
- pilot-materials は `pilot_score.py blind --runs $P/iter-1 --runs $P/iter-2 --metrics <上の metrics.json> --out /local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-scoring.md --key-out /local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-key.json --repo-copy <repo の写し>` を使い、`check-blank` を通す。`--seed` は付けない（乱数の id を本番の採点で予測できないようにする）。
- close-audit は人の採点の後に `pilot_score.py tally --scoring <採点済みの md> --key <pilot-key.json> --metrics <metrics.json>` で開封する。

## 未確認
- 実 LLM の run の記録での動き（operator の run の後に pilot-materials が確かめる）。特に claude-code の reads に出る Bash の redirect と system の path の数え方、`daemon.log` の secret の形の誤検出。
- 研究ケースの不要な実行（HPC の job）を検知の時点から数えること（ADR D6）。研究ケースの `after_detection`・`unneeded_after_first_detectable` は出していない。
- 研究ケースの verdict・next の一致・誤った pivot（研究ケースの参照の表は文で、answer key の形の値が無い。採点の後に人が表で見る）。
- 盲検は名前だけを伏せる。内容から方式を推測できるか（ADR D5）。
- `unneeded_after_first_detectable` と `after_detection` のどちらを rubric のコストの軸に使うかは人の採点で決まる（材料には両方を書いた。rubric の史実 137 run・264.25 USD は前者の方式 a の値と一致する）。
- 合格線の「合計点が (a) を 3 点以上上回る」は両事例の合計で判定した（事例ごとの差は tally の出力に無い。人が別の読みを取るなら close-audit で直す）。
