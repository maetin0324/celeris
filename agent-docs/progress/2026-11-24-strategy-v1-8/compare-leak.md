---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: compare-leak
status: done
updated: 2026-11-25
completed: 2026-11-25
---
# 戦略層 v1 子 8 compare-leak: 比較の漏れの規則で、時点以前にも同じ本文がある後の event を漏れに数えない

## 結論
- `scripts/dev/parent-eval-blind-compare.py` の `Rules.leaks`（replay）は、時点より後の event の本文（`outcome`、40 字超）のうち、cutoff 以前の event の本文のどれかに同じ文字列が含まれるものを漏れに数えない。ほかの規則は変えていない。
- operator の実 run（root の artifacts の `pilot-runs/iter-1`・`iter-2`）で `compare --pilot` が exit 1 から exit 0 になった。`testdata/pilot-runs-leak` は exit 1 のままで、`metrics.json` は変更の前後で同じ byte。`testdata/pilot-runs` は exit 0。
- 生成器 `testdata/make_pilot_runs.py` の出力は変わらない（`test_testdata_is_what_the_generator_writes` が通る）。testdata は生成し直していない。
- ADR 2026-11-24-strategy-pilot-ablation に「D4.1 付記 1」を書いた。

## 実装前 brief
1. 根本原因: `scripts/dev/parent-eval-blind-compare.py` の `Rules.leaks`（replay）は、時点の cutoff より後の event の `outcome`（40 字超）が入力の文にあれば「時点より後の event の本文」として漏れに数える。本文が時点以前の event にも同じ文字列で現れるかを見ていない。アーキv4 の replay では、定型文 `interrupted: aborted (task no longer running under this lease)` が C-mid の cutoff（2026-11-18T07:41:56Z）より前の task C の `worker_finished` に 57 件、後（task D）に 27 件ある。C-mid の入力 `input/tree/events.jsonl` は時点以前の行だけを持つ（`cutoff 後の event` の規則は当たらない）のに、この定型文が後の event の本文と一致して漏れとされた。症状は operator の実 run（iter-1・iter-2 の C-mid の方式 a・b）で `compare --pilot` が exit 1 になったこと。
2. 責務 owner: `parent-eval-blind-compare.py` の `Rules.leaks`（replay の漏れの規則を持つ唯一の場所）。driver・`replay.py`・seed の側は入力を正しく作っており、直す必要はない。
3. 既存の再利用先: `Rules` が既に持つ `self.events`・`self.cutoffs`・`self.later` と子 0 の `replay.norm`。試験は `test_pilot_compare.py` の `copy_clean`・`inject`・`compare` を使う。
4. 選択肢: (i) 定型文を名指しで除く（不採用: 症状の箇所への特例。別の定型文で同じ誤検出が起きる）。(ii) 40 字の閾値を上げる（不採用: 規則を弱め、後にだけある短めの本文を見逃す）。(iii) 後の event の本文 b が、cutoff 以前の event の本文のどれかに文字列として含まれる（同じ本文を含む）なら数えない（採用。入力にある b が時点以前の本文から来うるとき、未来の情報と区別できないので漏れの根拠にならない。後にだけある本文は従来どおり数える）。
5. 新しい状態・分岐・fallback・port: 無し。`Rules.leaks` の中で本文の集合を 1 つ絞るだけ。
6. 撤去対象: 無し（旧い規則の 1 条件を置き換える。旧い条件を残す切替は置かない）。ADR D4.1 の「子 6 の試験と同じ規則」の記述は付記で差分を書いた。
7. 不変条件: ほかの規則（sentinels の語・answer key の印と理由の文・期待の組・後の event の時刻・cutoff 後の event の行・研究ケースの規則）は変えない。`testdata/pilot-runs-leak` は exit 1 のまま。試験は下の節。

## 変えた file
- `scripts/dev/parent-eval-blind-compare.py`: `Rules.leaks` の replay の本文の集合（cutoff 以前の本文に含まれるものを除く）と `Rules` の docstring。
- `scripts/dev/parent-eval-blind/test_pilot_compare.py`: 試験 2 件と補助（`replay_rows`・`inject_event_row`・`compare_ts_in`）。
- `agent-docs/adr/2026-11-24-strategy-pilot-ablation.md`: D4.1 付記 1。
- この進捗 file。

## 試験
- `test_body_also_before_the_cutoff_is_not_a_leak`: C-mid の方式 a の `input/tree/events.jsonl` に、replay の events から取った cutoff 以前の行（本文が上の定型文）を 1 行足す。漏れ 0・exit 0 を見る。前提として定型文が cutoff より後の event にもあることを assert する。
  - 守る不変条件: 時点以前にも同じ本文がある後の event の本文は漏れに数えない。既存の試験で検出できない理由: testdata の replay の events.jsonl は合成の 2 行だけで、時点の前後に同じ本文を持たない。
  - 変更前（base の `parent-eval-blind-compare.py`）で落ちた出力:
    ```
    AssertionError: Tuples differ: (1, ['iter-1/replay/C-mid/a: input:input/tree/events.jsonl: 時点より後の event の本文']) != (0, [])
    Ran 2 tests in 0.479s
    FAILED (failures=1)
    ```
- `test_body_only_after_the_cutoff_is_a_leak`（否定対照）: C-mid の後にだけある本文（40 字超、cutoff 以前の本文に含まれず、時刻と時点より後の sentinels の語を含まないもの）を `input/tree/goal.md` に足す。exit 1 で、漏れがちょうど `時点より後の event の本文` の 1 件であることを見る。変更の前も後も通る。
  - 既存の否定対照の試験 `test_each_negative_control_alone_is_exit_1` にこの種類が無かったので足した。
- 既存の `test_leak_testdata_is_exit_1_with_each_kind`（`pilot-runs-leak` の C-mid/b の「cutoff 後の event」ほか）と `test_testdata_is_what_the_generator_writes` は変えずに通る。

## 検査（提出時の HEAD で実行）
- `python3 -I scripts/dev/parent-eval-blind/test_pilot_compare.py -v`: Ran 15 tests、OK（exit 0）。
- 同じ dir の `test_pilot_score.py`（10 件）・`test_research.py`（16 件）・`test_driver.py`（16 件）: すべて OK（exit 0）。
- `parent-eval-blind-compare.py --pilot --runs scripts/dev/parent-eval-blind/testdata/pilot-runs-leak --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --research-src scripts/dev/parent-eval-blind/testdata/research/c-results.md --out $TMPDIR/leak`: exit 1、漏れ 6 件（C-mid/b の cutoff 後の event F#920・時刻・本文、C-mid/c の理由の文、D-done/b の `parent-unmet`、research 11.17/b の `1 / 488`）。base の script で同じ入力を流した `metrics.json` と `cmp` で同じ byte。
- 同じ command で `--runs .../testdata/pilot-runs`: exit 0。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check`: 結果は下の「提出時の検査」節。

## 提出時の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0（「試験対象の変更なし」。変更は Python の script・試験と文書だけで、Rust の crate と web は対象外。repo_health_gate: all checks passed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0（ok）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は上の「変えた file」の 4 件だけ。

## 実データの結果（operator の実 run、root の artifacts の pilot-runs）
- 変更前（base の script）:
  ```
  python3 -I scripts/dev/parent-eval-blind-compare.py --pilot --runs <root artifacts>/pilot-runs/iter-1 --runs <root artifacts>/pilot-runs/iter-2 \
    --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --out $TMPDIR/before
  exit=1
  parent-eval-blind-compare: FAIL: iter-1/replay/C-mid/a: input:input/tree/events.jsonl: 時点より後の event の本文; iter-1/replay/C-mid/b: input:input/tree/events.jsonl: 時点より後の event の本文; iter-2/replay/C-mid/a: input:input/tree/events.jsonl: 時点より後の event の本文; iter-2/replay/C-mid/b: input:input/tree/events.jsonl: 時点より後の event の本文
  ```
- 変更後（同じ command）: exit 0。`leaks: []`・`failed: []`・`pass: true`・`excluded: []`・`usage: {llm_runs: 6, usd: 0.753975}`。方式 b の検知は iter-1 で無し、iter-2 で D-done。
- `--runs <root artifacts>/pilot-runs`（反復を含む親 dir）: exit 0、`metrics.json` は上と同じ byte。
- replay の events で、cutoff より後の本文（40 字超）のうち cutoff 以前の本文にもあるものを時点ごとに数えた（`Rules` と同じ読み方）: C-mid と C-done で 1 種（上の定型文）、ほかの 10 時点で 0。子 6 の盲検の時点（E-mid・D-done）は 0 種。

## 未確認
- 実 run の metrics.json の中身（一致・検知の数）の妥当性はこの WU では見ていない。見たのは漏れの判定（exit）だけ。採点と集計は pilot-materials・close-audit の仕事。
- 方式 c と研究ケースの実 run はまだ無い（stub-undeclared の後に operator が流す）。この規則の変更がそれらに当たるかは、その出力で確かめる。
