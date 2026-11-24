---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: research-goal
status: blocked
updated: 2026-11-24
---
# 戦略層 v1 子 8 research-goal: 研究ケースの goal と Outcome を 11.15 より前に書かれた task C の問いから作る

## 結論
- `scripts/dev/parent-eval-blind/research.py` の `build` は、goal と Outcome を `GOAL_SOURCES` の節から作るようにした。問い（Outcome の `title`）は 11.2 節の「人の答え」の箇条のうち「止ま」を含む文、判定方法（Outcome の `acceptance`）は 11.5 節の「停止の割合」の箇条、goal の背景は 11.1 節の「人の決定」の箇条。11.15 節の見出しと最初の段落は使わない。0 節と 11.15 節以降の節も使わない。
- 出典の節・規則・11.15 より前に書かれた根拠は ADR 2026-11-24-strategy-pilot-ablation の「D2.2 付記 1」に書いた。根拠は、task C の作業場所にある run 25 の前の写し（`target/.ctmp/c-results.pre-r25.md`、最後の節は 11.13）と最終の c-results.md で、出典の 3 節が同じ文であること（3 / 3 節）。
- CLI `research.py check-goal --src <c-results.md> --points 11.16,11.17,11.20 [--written-before <前の版>]` を足した。実データで exit 0。
- 研究ケースの本文は repo に写していない。commit したのは合成の testdata と、合成の testdata から生成した pilot-compare の testdata だけ。

## 実装前 brief
1. 根本原因: `build` が「問い = 11.15 節の見出し、判定方法 = 見出しの後の最初の段落」と決め打ちしていた。合成の testdata の 11.15 節は最初の段落が判定方法の形の作り物だったので試験は通ったが、実データの 11.15 節は sweep の結果の節で、最初の段落は「結論:」の結果の段落だった。症状は operator の stub の 1 回の goal.md に出た。
2. 責務 owner: `scripts/dev/parent-eval-blind/research.py` の `build`（seed の spec の goal と Outcome を作る唯一の場所）。driver・Rust の seed は spec を受けるだけで、直す必要はない。
3. 既存の再利用先: 子 0 の `research_case.py` の `sections`・`slices`・`leaks`（path で import のまま）、`research.py` の `heading`・`first_paragraph`・`later_headings`・`Leak` の検査、`make_pilot_runs.py`（pilot-compare の testdata の生成）。
4. 選択肢: (i) task C の DB の objective の「追加の依頼 C」を読む（不採用: WU の目的が c-results.md の節を出典に決めている。実行時に DB を読む口も要る）。(ii) 問いを人が書いた文で repo に置く（不採用: 研究ケースの本文を repo に写すことになる）。(iii) 11.15 より前の節を番号と箇条の印で選び、実行時に c-results.md から抜く（採用）。
5. 新しい状態・分岐・fallback・port: 無し。出典が無ければ `ValueError` で止まり、代わりの節を探さない。CLI の `check-goal` と任意の `--written-before`（出典の節が前の版と同じ文かを見る）を足した。
6. 撤去対象: 旧い規則（11.15 節の見出しと最初の段落を問いと判定方法にする）のコード・docstring・試験 `test_outcome_is_the_question_and_method_of_11_15_only`、`crates/task-api/tests/parent_checkpoint_live.rs` の `seed_research` の説明の 1 行。
7. 不変条件と試験: 下の節。

## 変えた file
- `scripts/dev/parent-eval-blind/research.py`: `GOAL_SOURCES`・`bullet`・`goal_parts`・`cutoff_hits`・`check_goal`、`build` の goal と Outcome、`main` の `check-goal`、docstring の seed の規約。
- `scripts/dev/parent-eval-blind/test_research.py`: 下の試験。
- `scripts/dev/parent-eval-blind/testdata/research/c-results.md`（合成）: 0 節（後の結論の形。漏れの語を含む）、9.8 節、11.1・11.2・11.5 節を足した。11.15 節の最初の段落を実データと同じ「結論:」の結果の形にした。
- `scripts/dev/parent-eval-blind/testdata/pilot-runs*/iter-1/research/`: `make_pilot_runs.py` で生成し直した（goal.md・results-slice.md・slice.json・files.json）。生成し直さないと `test_pilot_compare.py` の `test_testdata_is_what_the_generator_writes` が落ちる。
- `crates/task-api/tests/parent_checkpoint_live.rs`: `seed_research` の doc comment の 1 行（旧い規則の説明）だけ。
- `agent-docs/adr/2026-11-24-strategy-pilot-ablation.md`: D2.2 付記 1。

## 試験
守る不変条件と、既存の試験で検出できない理由:
- goal と Outcome は 11.15 節より前に書かれた節（`GOAL_SOURCES`）だけから作り、11.15 節の見出し（番号の有無とも）・最初の段落・0 節を含まない。旧い試験は 11.15 節の見出しと最初の段落を期待していたので、この誤りを合格にしていた。
- 出典が無い・0 節・11.15 節以降なら止まる。
- 漏れの否定対照（16 節の語・後の節の見出し）は残り、goal の出典に混ぜた未来の語も `Leak` で止まる。
- `check-goal` は本文を出さない。

試験（`python3 -I scripts/dev/parent-eval-blind/test_research.py -v`、16 件）:
- 新規 `test_goal_and_outcome_come_from_the_question_written_before_11_15`（変更前に落ちる）、`test_goal_sources_are_sections_written_before_11_15`、`CheckGoal` の 5 件（`test_synthetic_passes_and_prints_only_numbers`・`test_goal_with_the_11_15_heading_or_first_paragraph_is_exit_1`・`test_leak_is_exit_1_without_the_leaked_words`・`test_written_before_compares_the_source_sections`・`test_unknown_point_is_exit_2`）。
- `test_mixed_inputs_are_stopped_before_the_run` に否定対照 3（11.2 節の問いの文に 11.20 で初めて現れる語を混ぜると `Leak`）を足した。否定対照 1・2（16 節の語、11.21 節の見出し）はそのまま。
- 消した試験: `test_outcome_is_the_question_and_method_of_11_15_only`（旧い規則の契約を廃止した。代わりは上の新規の 2 件）。

変更前に落ちた出力（`git archive "$CELERIS_WU_BASE"` の tree に新しい `test_research.py` と合成の testdata を置いて実行）:
- `FAILED (failures=6, errors=1)`。落ちたのは `test_goal_and_outcome_come_from_the_question_written_before_11_15`（title が 11.15 節の見出しだった）・`test_goal_sources_are_sections_written_before_11_15`（ERROR: `GOAL_SOURCES` が無い）・`test_mixed_inputs_are_stopped_before_the_run`（否定対照 3 で `Leak not raised`。goal が 11.2 節を使っていない）・`CheckGoal` の 4 件（`check-goal` が無く exit 2）。
- 変更前の `research.py` で実データの goal・Outcome を作ると、11.16・11.17・11.20 の 3 時点とも 11.15 節の見出しが 1 件、最初の段落が 1 件入っていた（件数だけを数えた）。変更前の `check-goal` は使い方の誤りで exit 2。

変更後（提出時の HEAD で実行）:
- `python3 -I scripts/dev/parent-eval-blind/test_research.py` → `Ran 16 tests` OK。
- `python3 -I scripts/dev/parent-eval-blind/test_driver.py` → OK。`test_pilot_compare.py` → `Ran 13 tests` OK。`test_pilot_score.py` → OK。
- 合成の testdata の判定方法の箇条は作り物の語と数（実データの語・値を写さない）。

実データでの check-goal（提出時の HEAD で実行。本文は出していない）:
```
$ python3 -I scripts/dev/parent-eval-blind/research.py check-goal \
    --src /local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md --points 11.16,11.17,11.20 \
    --written-before /local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/target/.ctmp/c-results.pre-r25.md
11.16: sections 11.15..11.16（2 節）、leak 0 件、11.15 節の見出し・最初の段落 0 件
11.17: sections 11.15..11.17（3 節）、leak 0 件、11.15 節の見出し・最初の段落 0 件
11.20: sections 11.15..11.20（6 節）、leak 0 件、11.15 節の見出し・最初の段落 0 件
goal の出典: 11.2・11.5・11.1 節（11.15 節より前）
前の版と同じ文の出典: 3 / 3 節
exit 0
```

## 検査
提出時の HEAD で実行（基準は `$CELERIS_WU_BASE`）:
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web（`parent_checkpoint_live.rs` の comment の 1 行で task-api が入った）。nextest `6377 tests run: 6377 passed (13 slow), 32 skipped`、web の unit `773 passed (773)`、tsc -b は通った。終わりに scratch の trash の消し残しの `rm: Permission denied` が 3 行出た（exit は 0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path は上の「変えた file」と同じ。評価器の prompt（`crates/task-worker/src/change_rules/`）・control-ts・web・task の進捗 `agent-docs/progress/2026-11-24-strategy-v1-8.md` は変えていない。
- 研究ケースの本文が repo に無いこと: 実データの出典の問い・判定方法・背景の先頭 40 字を `git grep -F` → 0 / 3 件。

## 計画の check の食い違い（2 回目の run。計画の問題として申告した）
前回の run は done を返したが、範囲の check（`"scope": true`）が exit 1 で落ちた。2 回目の run で原因を確かめた。

- 範囲の check が許す path は `research.py`・`test_research.py`・`testdata/research/`・ADR・この進捗だけで、次の 2 つが範囲の外に出た。
  - `scripts/dev/parent-eval-blind/testdata/pilot-runs/` と `testdata/pilot-runs-leak/` の研究ケースの 16 file と 4 file（`testdata/make_pilot_runs.py` が `research.files` と合成の `testdata/research/c-results.md` から生成する goal.md・results-slice.md・slice.json・files.json）。
  - `crates/task-api/tests/parent_checkpoint_live.rs` の `seed_research` の doc comment の 1 行（旧い規則の説明）。
- 同じ WU の check `python3 -I -m unittest discover -s scripts/dev/parent-eval-blind -p 'test_*.py'` は `test_pilot_compare.py` の `test_testdata_is_what_the_generator_writes` を含む。この試験は生成器の出力と commit した testdata の一致を見る。
- 提出時の HEAD で両方を確かめた:
  - 生成し直した testdata のまま: unittest は `Ran 55 tests` OK。範囲の check は exit 1。
  - testdata を範囲の基点の版に戻す: unittest は `FAILED (failures=1)`（`test_testdata_is_what_the_generator_writes` の digest の不一致）。範囲の check は Rust の 1 行を除けば通る。
- goal と Outcome の作り方を変えると、生成器の研究ケースの出力は必ず変わる（goal.md に 11.15 節の見出しを入れないことが受け入れ条件 0 だから）。研究ケースの出力を変えずに直す方法は無い。2 つの check は同時に通らない。
- ほかの check（check-goal の実データと合成の 11.16・11.17・11.20、sha256・doc-links・adr-numbers、進捗と ADR の grep）は提出時の HEAD で exit 0。
- 申告した直し方: この WU の範囲の check の正規表現に `scripts/dev/parent-eval-blind/testdata/`（生成した testdata を含む）と `crates/task-api/tests/parent_checkpoint_live\.rs`（doc comment の 1 行）を足す。後続の compare-leak は research-goal に依存し、範囲に `testdata/` を含むので、書く場所は重ならない。Rust の file を範囲に入れないなら、次の run がその 1 行を戻し、未確認に書く。

## 未確認
- 実データで作った goal と Outcome が、評価器にとって研究ケースの問いと判定方法として十分かは、人と operator が読んで確かめていない（この WU の worker は実データの goal を手元で読んだだけで、repo にも進捗にも写していない）。operator は実 LLM の前に `check-goal` を流し、stub の 1 回の `research/<時点>/b/input/tree/goal.md` を読むとよい。
- 根拠に使った run 25 の前の写しは task C の作業場所の `target/.ctmp/` の下にあり、掃除で消える恐れがある。sha256 は ADR の付記 1 に書いた。残すなら人か operator が root の artifacts に写す。
- 並行の WU compare-leak が `make_pilot_runs.py` か pilot-compare の testdata を変えると、統合の後に testdata の生成し直しが要るかもしれない（integrate-fix の段で `test_pilot_compare.py` を流して確かめる）。
- 研究ケースの子は引き続き review の記録を持たない（research-driver の未確認のまま）。
