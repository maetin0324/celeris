---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: compare-reads
status: done
updated: 2026-11-25
completed: 2026-11-25
---
# 戦略層 v1 子 8 compare-reads: compare の check_reads で書き込みの heredoc と名の部分一致を漏れに数えない。invalid の記録を採点材料に示す

## 結論
- `scripts/dev/parent-eval-blind-compare.py` の `check_reads` は、Bash の heredoc の本文と redirect（`>`・`>>`）の書き先を禁じた名と照合しない。末尾が `/` の禁じた名（`replay/`・`agent-docs/adr/`）は path の成分の頭でだけ一致とする。`reads_outside` とほかの漏れの規則は変えていない。
- 直した compare を operator の実 run（root の artifacts の `pilot-runs/iter-1`・`iter-2`）に `--research-src` 付きで当て、`pilot-compare-v4/metrics.json` を書いた。exit 0、`pass: true`、漏れ 0、`excluded` 0、`failed` 0。`pilot-compare`・`pilot-compare-v3`・`pilot-runs` は変えていない。
- 変更前の 16 件（8 点）は、どれも実際の読みではなかった。6 件は書き込みの heredoc の本文にある `c-results.md`、10 件は版の path `artifacts/strategy/pilot-replay/` の `pilot-replay/` が `replay/` に部分一致したもの。
- `pilot_score.py blind` は、record の status が `invalid` の記録に「評価の記録の状態: invalid（run: …、…）。判定と次の手は gate の既定値で、評価器の答えではない」の 1 行を出す。実 run では方式 c の 4 点に出て、`check-blank` は通る。
- ADR 2026-11-24-strategy-pilot-ablation に「D4.1 付記 2」を書いた。実 LLM の run は起こしていない。

## 実装前 brief
1. 根本原因: `check_reads` が reads の 1 件の文（Bash では command の全体）を、禁じた名と部分一致で照合していた。そのため (A) 作業 tree の `artifacts/` に review.json を書く command の heredoc の本文に入った名（driver の入力 `request.json`・`prompt.md` にもある `c-results.md`）と、(B) 方式 c の版の path `artifacts/strategy/pilot-replay/v1.json` の中の `replay/` が、読みとして数えられた。症状は `compare --pilot`（`pilot-compare-v3`）の exit 1。
2. 責務 owner: `parent-eval-blind-compare.py` の `check_reads`（子 6 の呼び方と `--pilot` が共有する、読みの検査の唯一の場所）。invalid の記録の見せ方は `pilot_score.py` の `record_lines`。
3. 既存の再利用先: redirect の書き先は、`mutation` が書き込みに数える `REDIRECT_RE` をそのまま使う（書き込みに数えるものを読みに数えない。同じ正規表現を二重に持たない）。外の絶対 path の切り出しは `check_reads` の既存の正規表現を `ABS_PATH_RE` として共有した。試験は `test_pilot_compare.py` の `copy_clean`・`compare` と、`test_pilot_score.py` の `score`。
4. 選択肢: (i) `c-results.md` と `pilot-replay/` を名指しで除く（不採用: 症状の箇所への特例）。(ii) Bash の command を読みの検査から外す（不採用: `cat c-results.md` を見逃す）。(iii) shlex で command を字句に分ける（不採用: heredoc を扱えず、引用符の閉じない command で落ちる）。(iv) heredoc の本文と redirect の書き先だけを照合から除き、末尾が `/` の名は path の成分の頭で照合する（採用。人の決定 2026-11-25 の (1)(2) どおり）。
5. 新しい状態・分岐・fallback・port: 無し。`check_reads` の中の照合の文の作り方と、名の照合の関数 `forbidden_name_in` だけ。
6. 撤去対象: `check_reads` の部分一致だけの照合（置き換え。旧い照合を残す切替は置かない）。`REDIRECT_RE` は pilot の節から定数の節に移した（定義は 1 つ）。
7. 不変条件: 実際の読み（Read の path、`cat` の引数、heredoc の外の引数、heredoc の本文の中の外の絶対 path、heredoc の後の行）は漏れのまま。`reads_outside` は変更の前後で同じ。`testdata/pilot-runs-leak` は exit 1 で `metrics.json` は同じ byte。子 6 の呼び方の `compare.json` は同じ byte（`test_child6_compare_without_pilot_is_unchanged`）。

## 16 件の分類（変更前 = base の script、`pilot-compare-v3` と同じ byte）
番号は表の並べ替えのための通し番号（`metrics.json` の `leaks` の順ではない）。変更前の script を同じ引数で当てた `metrics.json` は `pilot-compare-v3/metrics.json` と `cmp` で同じ byte（exit 1）。下の「変更 1」は heredoc の本文と redirect の書き先を除く規則、「変更 2」は末尾が `/` の名を成分の頭で照合する規則。「単独で消えるか」は、その規則だけを当てたときに名が照合の文から消えるか。

| # | 点 | 禁じた名 | command の形 | 名のある所 | 変更 1 単独 | 変更 2 単独 |
|---|---|---|---|---|---|---|
| 1 | iter-1/research/11.16/b | c-results.md | `mkdir … && cat > …/artifacts/review.json <<'EOF'`（heredoc 2 つ） | heredoc の本文 | 消える | 残る |
| 2 | iter-1/research/11.17/b | c-results.md | `cd …/artifacts && python3 -I - <<'EOF'`（review.json を書く） | heredoc の本文 | 消える | 残る |
| 3 | iter-1/research/11.17/c | c-results.md | `cd …/slice-repo; python3 -I - <<'EOF'`（review.json を書く） | heredoc の本文 | 消える | 残る |
| 4 | iter-1/research/11.20/b | c-results.md | `mkdir … && cd …/artifacts && python3 - <<'E'` | heredoc の本文 | 消える | 残る |
| 5 | iter-2/research/11.16/b | c-results.md | `mkdir … && cd …/artifacts && python3 -I - <<'EOF'` | heredoc の本文 | 消える | 残る |
| 6 | iter-2/research/11.17/b | c-results.md | `cd …/artifacts && python3 -I - <<'EOF'` | heredoc の本文 | 消える | 残る |
| 7 | iter-1/replay/strategy-v1 | replay/ | `mkdir -p artifacts/strategy/pilot-replay; cat > artifacts/strategy/pilot-replay/v1.json <<'EOF'`、heredoc の後の行で `json.load(open('artifacts/strategy/pilot-replay/v1.json'))`・`sha256sum` | redirect の書き先と、heredoc の後の行の版の読み | 残る | 消える |
| 8 | iter-1/replay/strategy-v1 | replay/ | `cat > artifacts/result.json <<'EOF'` | heredoc の本文 | 消える | 消える |
| 9 | iter-1/replay/strategy-v1 | replay/ | `ls; cat goal.md; cat artifacts/strategy/pilot-replay/v1.json; …` | 版の読み（`pilot-replay/`） | 残る | 消える |
| 10 | iter-1/replay/strategy-v1 | replay/ | `sha256sum artifacts/strategy/pilot-replay/v1.json goal.md; … json.load(open('artifacts/strategy/pilot-replay/v1.json'))` | 版の読み | 残る | 消える |
| 11 | iter-1/replay/strategy-v1 | replay/ | `cd …/artifacts; cat > review.json <<'EOF'`（heredoc 2 つ） | heredoc の本文 | 消える | 消える |
| 12 | iter-2/replay/strategy-v1 | replay/ | `mkdir -p artifacts/strategy/pilot-replay && cat > artifacts/strategy/pilot-replay/v1.json <<'EOF'`、heredoc の後の行で `json.load(open(…))`・`sha256sum` | redirect の書き先と、heredoc の後の行の版の読み（`mkdir` の引数は末尾に `/` が無く当たらない） | 残る | 消える |
| 13 | iter-2/replay/strategy-v1 | replay/ | `cat > artifacts/result.json <<'EOF'`（heredoc 2 つ） | heredoc の本文 | 消える | 消える |
| 14 | iter-2/replay/strategy-v1 | replay/ | `ls -la; …; sha256sum artifacts/strategy/pilot-replay/v1.json goal.md; …` | 版の読み | 残る | 消える |
| 15 | iter-2/replay/strategy-v1 | replay/ | `cat slice.json; …; cat artifacts/strategy/pilot-replay/v1.json` | 版の読み | 残る | 消える |
| 16 | iter-2/replay/strategy-v1 | replay/ | `cd …/artifacts; python3 -I - <<'E'`（critic の判定を書く） | heredoc の本文 | 消える | 消える |

- 形 (A) 書き込みの heredoc の本文: #1〜#6（研究ケースの 6 点。方式 b が 5 点、方式 c が 1 点）。変更 1 でだけ消える。名は評価の文（claim・reason）の中の `c-results.md` で、`c-results.md` の file を開いた読みは無い（driver の入力にもこの名がある）。
- 形 (B) 版の path の `pilot-replay/`: #7〜#16（replay の方式 c の版の段、2 反復で 5 件ずつ）。変更 2 で全部消える。うち #8・#11・#13・#16 の 4 件は名が heredoc の本文にだけあり、変更 1 でも消える。読んだのは方式 c が自分で書いた作業 tree の版（`artifacts/strategy/pilot-replay/v1.json`）で、子 0 の `replay/` の dir ではない。
- 分類の command: 進捗に写した表は、base の `metrics.json` の 16 件の command を、新しい `split_heredocs`・`REDIRECT_RE`・`forbidden_name_in` で 1 件ずつ分けた結果（`python3 -I` の一時 script。run の `$TMPDIR` に置き、commit していない）。

## 試験
- 変更前に落ちる 3 件（`test_pilot_compare.py`。不変条件: heredoc の本文と redirect の書き先は読みでない、末尾が `/` の名は成分の頭でだけ一致。既存の試験で検出できない理由: 禁じた読みの試験は Read の path の 1 件だけで、書き込みの heredoc と方式 c の版の path の形が無かった）:
  - `test_cat_heredoc_body_is_not_a_read`: research/11.17/b に `cat > …/artifacts/review.json <<'EOF'` の本文に `c-results.md` を持つ command を足す。漏れ 0・exit 0。
  - `test_python_heredoc_writing_review_is_not_a_read`: research/11.17/c に `python3 -I - <<'E'` で review.json を書く script（本文に `c-results.md`）を足す。漏れ 0・exit 0。
  - `test_strategy_path_pilot_replay_is_not_the_replay_dir`: replay/strategy-v1 に `sha256sum artifacts/strategy/pilot-replay/v1.json …` と Read の path `…/artifacts/strategy/pilot-replay/v1.json` を足す。漏れ 0・exit 0。
  - 変更前（base の script を作業 tree に戻して `python3 -I scripts/dev/parent-eval-blind/test_pilot_compare.py Pilot.<4 件>`）の出力:
    ```
    FAIL: test_cat_heredoc_body_is_not_a_read (__main__.Pilot.test_cat_heredoc_body_is_not_a_read)
    AssertionError: Tuples differ: (1, ['iter-1/research/11.17/b: reads: c-re[205 chars]F)']) != (0, [])
    FAIL: test_python_heredoc_writing_review_is_not_a_read (__main__.Pilot.test_python_heredoc_writing_review_is_not_a_read)
    AssertionError: Tuples differ: (1, ['iter-1/research/11.17/c: reads: c-re[231 chars]E)']) != (0, [])
    FAIL: test_strategy_path_pilot_replay_is_not_the_replay_dir (__main__.Pilot.test_strategy_path_pilot_replay_is_not_the_replay_dir)
    AssertionError: Tuples differ: (1, ['iter-1/replay/strategy-v1: reads: re[298 chars]n)']) != (0, [])
    Ran 4 tests in 2.109s
    FAILED (failures=3)
    ```
    （4 件目の否定対照は変更前も通った。）
- 否定対照 `test_real_reads_of_forbidden_names_stay_leaks`（変更の前も後も通る）: research/11.17/b に 1 件ずつ足し、どれも exit 1 で、`reads: <名> (<command>)` の漏れが出ることを見る。
  - Read の path `/synthetic/home/slice-repo/replay/answer_key.json` と相対の `replay/answer_key.json`（`replay/`・`answer_key`）
  - `cat c-results.md > …/artifacts/notes.txt`（redirect があっても読みは残る）
  - `cat agent-docs/progress/2026-11-22-strategy-v1-6.md`（`strategy-v1-6`）
  - heredoc の外の引数 `python3 -I - /work/…/2026-11-21-strategy-v1-0/replay/answer_key.json <<'EOF'`（`replay/`・`answer_key`、`reads_outside` にも残る）
  - heredoc の本文の中の外の絶対 path `open('/work/agent-platform/agent-docs/adr/…')`（`agent-docs/adr/`、`reads_outside` にも残る）
  - heredoc の後の行の `cat c-results.md`
- `test_heredoc_split_follows_bash`: `<<-` の行頭の tab、1 行に 2 つの heredoc、`<<<`（here-string）を本文にしないこと、`forbidden_name_in` の成分の頭（`'replay/x'`・`a/replay/x` は一致、`pilot-replay/` は不一致、`c-results.md` は部分一致のまま）。
- `test_pilot_score.py` の `test_invalid_record_says_the_verdict_is_the_gate_default`（不変条件: invalid の記録には gate の既定値である旨の 1 行を出し、方式の名を出さず `check-blank` を通る。既存の試験で検出できない理由: testdata の記録は全て checked）: testdata の replay/D-done/c を invalid（violations `shape parent_evaluation: 節が無い`、run.json の status `budget_exhausted`）に替えた写しで blind を流す。変更前の出力:
  ```
  FAIL: test_invalid_record_says_the_verdict_is_the_gate_default (__main__.Score.test_invalid_record_says_the_verdict_is_the_gate_default)
  AssertionError: 0 != 1 : # 対照 pilot の盲検の採点材料（戦略層 v1 子 8）
  Ran 1 test in 0.457s
  FAILED (failures=1)
  ```
- testdata と生成器は変えていない（生成器の出力は変わらない。`test_testdata_is_what_the_generator_writes` が通る）。

## v4 の結果（実 run。root の artifacts）
- command（WU の 4 番目の check と同じ引数）:
  ```
  ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts
  python3 -I scripts/dev/parent-eval-blind-compare.py --pilot --runs $ART/pilot-runs/iter-1 --runs $ART/pilot-runs/iter-2 \
    --research-src /local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md \
    --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --out $ART/pilot-compare-v4
  ```
  exit 0。`metrics.json`: `pass: true`、`leaks: []`、`excluded: []`、`failed: []`、`usage: {llm_runs: 32, usd: 8.300332}`（operator の報告の 34 run・8.6335 USD との差は、版 v1 の段で失敗した 2 run・0.333 USD。その出力は `pilot-runs` の外に移されている）。
- 変更前の `metrics.json`（= `pilot-compare-v3`）と比べ、`leaks`・`excluded`・`pass` と各点の `leaks`・`forbidden_reads` のほかは全て同じ（`reads_outside` も同じ）。
- 方式ごとの検知（compare の標準出力）: replay の方式 b は iter-1 で無し・iter-2 で D-done、方式 c は iter-1 で C-mid・iter-2 で無し。研究ケースの方式 b は両反復で 11.20、方式 c は iter-1 で 11.20・iter-2 で 11.16。方式 a は検知無し（親の途中評価が無い）。数値の妥当性と採点は段 score・close の仕事で、この WU では見ていない。
- `pilot_score.py blind --runs …/iter-1 --runs …/iter-2 --metrics $ART/pilot-compare-v4/metrics.json`（出力は `$TMPDIR`）: exit 0。`評価の記録の状態: invalid` の行は 4 件（どれも `run: budget_exhausted、shape parent_evaluation: 節が無い`）。`check-blank` exit 0。
- `pilot-compare/metrics.json`・`pilot-compare-v3/metrics.json`・`pilot-runs` は、この run の開始（2026-11-25T03:0xZ）より後に変わった file が無い（`find -newermt`）。

## 検査（提出時の HEAD で実行）
- `python3 -I -m unittest discover -s scripts/dev/parent-eval-blind -p 'test_*.py'`: Ran 66 tests、OK。
- `parent-eval-blind-compare.py --pilot --runs scripts/dev/parent-eval-blind/testdata/pilot-runs-leak …`: exit 1、漏れ 6 件。base の script の `metrics.json` と `cmp` で同じ byte。`--runs …/testdata/pilot-runs`: exit 0。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`: 結果は下の「提出時の検査」節。

## 提出時の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0（「試験対象の変更なし」。変更は Python の script・試験と文書だけで、Rust の crate と web は対象外。repo_health_gate: all checks passed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0（ok）。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh`: exit 0（ok、482 files）。
- WU の check: 範囲（`sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は「LoC」節の 6 file だけ）、unittest discover（66 件 OK）、testdata の leak exit 1・clean exit 0、実 run の compare（exit 0、出力は `pilot-compare-v4/metrics.json` と同じ byte）、blind（invalid の行 4 件、check-blank exit 0）。どれも exit 0。

## LoC（`git diff --numstat`、基準は `$CELERIS_WU_BASE`）
- script（試験でない）: `parent-eval-blind-compare.py` +43/−5、`pilot_score.py` +13/−6。
- 試験: `test_pilot_compare.py` +73、`test_pilot_score.py` +32。
- 文書: ADR +31、この進捗 file。
- crates・web・評価器の prompt・control-ts・testdata は変えていない。純増の理由: heredoc の切り出しと成分の頭の照合は既存に無い規則で、人の決定 (1)(2) が求めたもの。

## 未確認
- heredoc の切り出しは Bash の字句を全部は扱わない。引用符の中の `<<` を heredoc と読む、引用符の中の `>` の後を redirect の書き先と読む、の 2 つは誤る（読みを少なく数える側）。実 run の 16 件と試験の形では当たらなかった。外の絶対 path は heredoc の本文の中でも照合するので、禁じた file の読みは外の絶対 path を通る限り検出される（ADR D4.1 付記 2 の前提）。
- `--pilot` は `reads_outside` を漏れに数えない（変更前から。この WU では変えていない）。実 run の `reads_outside` には claude-code の tool-results の path（`/home/rmaeda/.claude/projects/…/tool-results/*.txt`）や `/request.json` などの切り出しがあるが、禁じた名を含むものは無い。
- 方式 c の 4 点の invalid が採点に与える影響は、人の採点（段 score・close）で見る。
