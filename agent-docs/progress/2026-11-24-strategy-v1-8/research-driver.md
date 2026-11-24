---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: research-driver
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 8 research-driver: driver に研究ケースを足し、合成の testdata の stub で流す

## 結論
- driver に事例 `research`（ADR 2026-11-24-strategy-pilot-ablation D2.2）を足した。起動の形: `bash scripts/dev/parent-eval-blind.sh --harness stub|config --arms a,b,c --case research --research-src <c-results.md> --points 11.17,11.20 --out <dir>`。時点は 11.16・11.17・11.20、方式 c の版 v1 の段は 11.15 節。出力は replay と同じ `<dir>/research/<point>/<arm>/`（`output.json`・`reads.json`・`run.json`・`input/`）と `<dir>/research/strategy-v1/`。
- slice の切り出し・未来の語・sha256 は子 0 の `research_case.py` の `sections`・`slices`・`leaks`・`SENTINELS`・`SHA` を `scripts/dev/parent-eval-blind/research.py` が path で import して使う（写していない）。
- `--harness config` では `--research-src` の sha256 が `research_case.SHA` と違えば exit 2（承認の検査の後、build の前）。承認の `scope.cases` の名は `research-<時点>`。
- 合成の testdata（`scripts/dev/parent-eval-blind/testdata/research/c-results.md`。節の見出しの形だけを合わせた作り物）で、stub の 11.17・11.20 × 方式 a・b・c と版 v1 の段が exit 0（約 4 分半）。方式 b・c の `input/prompt.md` 4 件に「（Outcome の記録は無い）」は 0 件。trigger は 11.17 が `children:3`、11.20 が `children:6` で、checkpoint は時点で 1 回だけ発火した（ADR の未確認「研究ケースの時点で checkpoint を 1 回だけ発火させられるか」は stub では確かめた）。
- この WU の worker は研究ケースの実データ（c-results.md）を開いていない。repo に commit した研究ケースの文書は合成の testdata だけ。

## 実装前 brief
1. 根本原因: driver と sh は事例 `replay` だけを受け（`--case research` は exit 2）、seed（`parent_checkpoint_blind_seed`）は replay の fixture の時点しか置けなかった。研究ケースの slice を切る口と、sha256 で版を固定する検査が無かった。
2. 責務 owner: pilot の driver（`scripts/dev/parent-eval-blind.sh`・`driver.py`）と seed の試験（`crates/task-api/tests/parent_checkpoint_live.rs`）。production（`crates/*/src`）は owner ではない。
3. 既存の再利用先: 子 0 の `research_case.py`（path で import）、arm-driver の一時 daemon・方式 a/b/c の流れ・出力の配置、seed の Roadmap・strategy root・子の置き方（`seed_tree` に分けて replay と共用）、`pilot_gate.py check-approval`。
4. 選択肢: (i) 研究ケース用に別の driver を作る（不採用: 一時 daemon の写し）。(ii) seed を API で置く（不採用: strategy root の起票の操作の記録は store の関数でしか置けない）。(iii) driver が slice と seed の spec を作り、Rust の seed が spec の `research` を受けて replay と同じ `seed_tree` で置く（**採用**）。
5. 新しい状態・分岐・fallback・port: production には無し。script と試験の seed に事例の分岐（replay・research）。fallback は無い（Outcome の判定方法が作れなければ止まる）。
6. 撤去対象: 無し（事例を足すだけ。replay の seed は `seed_tree` を呼ぶ形に変え、旧い置き方の写しは残していない）。
7. 不変条件と試験: 下の節。

## 変えた file
- `scripts/dev/parent-eval-blind/research.py`（新規）: `research_case.py` を path で import し、時点 p の作業 tree（`goal.md`・`results-slice.md`・`slice.json`）と seed の spec（`goal`・`outcome`・`children`）を作る。入力に初出が p より後の `SENTINELS`（`rc.leaks`）か p より後の節の見出しがあれば `Leak`（driver は exit 1 で、seed も run も起こさない。D4.3 の run の前の検査）。`check-sha --src` は sha256 の不一致で exit 2。
  - seed の規約（cutoff 以前の節だけ）: 子は 11.15 節から p 節までの節ごとに 1 件（終端の done、title は節の見出し、報告は節の本文、受け入れ条件は「節の測定の結果と解釈を c-results.md に記録する」の reviewer の 1 件）。c-results.md に review の判定の記録は無いので、判定の event は置かない（prompt では `[reviewer] unknown`、方式 a の claims は 0 件）。
  - Outcome（id `research-question`）は 11.15 節（全時点の cutoff 以前）だけから作る: title = 11.15 節の見出し（問い）、acceptance = 見出しの後の最初の段落（判定方法）。案件の目的（goal.md）も同じ 2 つ。
- `scripts/dev/parent-eval-blind/driver.py`: `CASES["research"]`・`STRATEGY_POINT["research"]`、`--research-src`、研究ケースの slice と spec の `research`、`children_per_checkpoint` を時点の子の数（1〜8）にする config、版 v1 の段の objective に置いた file の名を並べる。
- `scripts/dev/parent-eval-blind.sh`: `--case research`・`--research-src`・時点 11.16・11.17・11.20、承認の cases の名 `research-<時点>`、`--harness config` の sha256 の検査、版 v1 の段の時点を事例ごとに（replay は A-mid、研究ケースは 11.15）。
- `crates/task-api/tests/parent_checkpoint_live.rs`: `seed` を replay の slice の読みと共通の `seed_tree` に分け、spec の `research` を `seed_research` で置く（時点は `RESEARCH_POINTS`）。
- `scripts/dev/parent-eval-blind/test_research.py`（新規）・`testdata/research/c-results.md`（新規、合成）。`test_driver.py` は `CASES` の比較を replay の項目だけにした。

## 守る不変条件と、既存の試験で検出できない理由
- slice と漏れの語と sha256 は `research_case.py` のものを使い、写さない（規約 2）。
- 時点 p の入力に p より後の節の本文・見出し・初出が p より後の語が入らない。混ぜた入力は run の前に止まる。
- Outcome（問いと判定方法）は 11.15 節だけから作り、時点で変わらない。方式 b・c の prompt に「（Outcome の記録は無い）」が出ない。
- `--harness config` は sha256 が違う `--research-src` を exit 2 で断る。
- 既存の試験で検出できない理由: driver と sh は事例 replay しか受けず、研究ケースの slice・漏れ・sha256 を確かめる試験が無かった。

## 試験
- 新しい試験 `scripts/dev/parent-eval-blind/test_research.py`（10 件）: `test_research_case_is_imported_by_path_not_copied`・`test_slice_is_the_sections_up_to_the_point`・`test_outcome_is_the_question_and_method_of_11_15_only`・`test_inputs_have_no_section_after_the_cutoff`・`test_mixed_inputs_are_stopped_before_the_run`（16 節の語を 11.17 に混ぜる・11.21 節の見出しを 11.16 に混ぜる）・`test_driver_stops_a_leaking_slice_with_exit_1`・`test_config_with_a_research_src_of_another_sha256_is_exit_2`・`test_approval_scope_uses_research_point_names`・`test_research_needs_its_src_and_its_points`・`test_driver_and_shell_agree_on_the_research_points`。
- 変更前に落ちた試験（`git archive "$CELERIS_WU_BASE"` の tree に新しい試験と testdata を置いて実行）:
  - そのまま: `python3 -I scripts/dev/parent-eval-blind/test_research.py` → exit 1（`ModuleNotFoundError: No module named 'research'`）。
  - base の tree に `research.py` だけを足す: `FAILED (failures=4, errors=1)`。落ちたのは `test_driver_and_shell_agree_on_the_research_points`（ERROR）・`test_approval_scope_uses_research_point_names`・`test_config_with_a_research_src_of_another_sha256_is_exit_2`・`test_research_needs_its_src_and_its_points`・`test_driver_stops_a_leaking_slice_with_exit_1`。
  - base の sh に `--case research --research-src …` を渡すと exit 2（使い方の誤り）。
- 変更後: `python3 -I scripts/dev/parent-eval-blind/test_research.py` → `Ran 10 tests` OK。`python3 -I scripts/dev/parent-eval-blind/test_driver.py` → `Ran 16 tests` OK。`cargo test -q -p task-api --test parent_checkpoint_live` → 6 passed、4 ignored。
- stub の e2e（提出時の HEAD の前、同じ内容で実行）: `bash scripts/dev/parent-eval-blind.sh --harness stub --arms a,b,c --case research --research-src scripts/dev/parent-eval-blind/testdata/research/c-results.md --points 11.17,11.20 --out "$TMPDIR/rd-out"` → exit 0。`research/{11.17,11.20}/{a,b,c}/output.json` の 6 件と `input/`、`research/strategy-v1/` が書かれた。b・c の verdict `unknown`・next `ask_human`（stub の固定）、a は `fixed_plan`・next `continue`・LLM run 0。`grep -l "（Outcome の記録は無い）" research/*/[bc]/input/prompt.md` → 0 件（grep exit 1）。11.17 の `input/` に SYN-1118〜SYN-1121・SYN-16 の印は 0 件、11.20 の `input/` に SYN-1121・SYN-16 は 0 件、版 v1 の段の作業 tree に 11.16・11.17 の印は 0 件。

## operator の手順への追記（arm-driver の進捗の「operator が worker の外で実 LLM の pilot を流す手順」に足す）
- 研究ケースは `--research-src` に host の実データの path を渡す: `/local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md`（`research_case.SRC`。sha256 は `research_case.SHA` と一致しなければ exit 2）。実データを repo に写さない。ChatGPT・Muse に送らない（人の答え: 内部の評価だけ）。
  ```sh
  P=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-runs
  for it in 1 2; do
    bash scripts/dev/parent-eval-blind.sh --harness config --arms a,b,c --case research \
      --research-src /local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md \
      --points 11.16,11.17,11.20 --out "$P/iter-$it" \
      --approval /local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-approval.json \
      --llm-config <operator の toml>
  done
  ```
- 実 LLM の前に、operator が同じ `--research-src` で `--harness stub` を 1 回流し、`research/<時点>/b/input/tree/goal.md` の問い（11.15 節の見出し）と判定方法（見出しの後の最初の段落）が研究ケースの問いと判定方法になっているかを読んで確かめる。なっていなければ実 LLM を流さず、research.py の規約の直しを計画に戻す。
- 研究ケースの run の数: 方式 b は 1 時点 1 run、方式 c は 1 時点 1 run と反復ごとの版 v1 の段 2 run。3 時点 × 2 反復で b 6 run・c 10 run、計 16 run（replay の 16 run と合わせて 32 run。上限 48 run・25 USD の中）。

## 未確認
- 実データの c-results.md での切り出し（worker は開いていない）: 11.15〜11.21 の節がすべて `research_case.sections` で取れるか、11.15 節の見出しと最初の段落が問いと判定方法になっているか、`Leak` で止まらないか。operator の stub の 1 回で確かめる（上の手順）。
- 実 LLM（`--harness config`）での研究ケースの run の全部。
- 研究ケースの子は review の記録を持たない（`[reviewer] unknown`）。方式 a の局所の review の claims は 0 件で、replay の方式 a と形が違う。
- 研究ケースの不要な実行（HPC の job）を検知の時点から数えられるか（ADR D6。pilot-compare の範囲）。
- arm-driver の欠け（方式 c の v2・起票の gate、一時 DB の `strategy-approve-v1`）は研究ケースにも同じく残る。
