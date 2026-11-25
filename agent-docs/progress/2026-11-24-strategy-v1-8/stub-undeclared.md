---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: stub-undeclared
status: done
updated: 2026-11-25
completed: 2026-11-25
---
# 戦略層 v1 子 8 stub-undeclared: stub を claude-code と同じ未申告の書き方にし、直した driver を両 case で通して operator の手順を書く

## 結論
- `scripts/dev/parent-eval-blind/stub_reviewer.py` の版を書く役は、`artifacts/strategy/<id>/v1.json` に書くだけで `artifact` 行を出さない（claude-code の worker と同じ）。この stub で方式 c の版 v1 の段が done になり、記録の名は `strategy/pilot-replay/v1.json`・`strategy/pilot-research/v1.json` だった。名を付けたのは WU artifact-name が直した run 後の走査（`scan_undeclared_artifacts_in_dir`）だけで、production の変更はこの WU に無い。
- sh（`scripts/dev/parent-eval-blind.sh`）は、同じ方式の dir を `rm -rf` していた。これをやめた。出力先に `<out>/<case>/<point>/<arm>/` か段の終わっていない `<out>/<case>/strategy-v1/` があれば、build の前に exit 1 で止まり、何も消さない（`pilot_runs.py check-new`）。driver も自分の出力の dir があれば exit 1。段の終わった `strategy-v1/`（`run.json` と `strategy-v1.json` がある）は、同じ `--out` の方式 c が流し直さずに使う。operator が時点を 1 つずつ流し、各回の前に予算を検査できるようにするため。
- stub で replay の方式 a・b・c × C-mid・D-done・F-done と、研究ケース（合成の testdata）の方式 a・b・c × 11.16・11.17・11.20 を同じ out に流し、両方 exit 0。その out に `compare --pilot` が exit 0（漏れ 0）、`pilot_score.py blind`・`check-blank`（本体と repo 用の写し）が exit 0。版の段の dir を含む配置は、直さずに読めた。漏れの規則は変えていない。
- `agent-docs/progress/2026-11-24-strategy-v1-8/arm-driver.md` の operator の手順を、残りの run（replay の方式 c ×2、研究ケースの方式 a・b・c ×2）に書き直した。
- 評価器の prompt（`crates/task-worker/src/change_rules/`）・`control-ts`・`web` の差分は 0。本番の DB・daemon・config には触れていない。LLM の run は起こしていない（`--harness stub` だけ）。task の進捗 `agent-docs/progress/2026-11-24-strategy-v1-8.md` は変えていない。

## 実装前 brief
1. 根本原因: stub の版を書く役が fake の `artifact` 行で名を申告していた。claude-code・codex・acp の worker は ArtifactProduced を出さないので、名は run 後の走査が付ける。stub は申告に頼って版の段を通したため、走査が basename の名を付ける不具合（WU artifact-name で修正）が stub では見えなかった。もう 1 つ、sh が実行の前に同じ方式の dir を `rm -rf` していた。operator が `--arms` を誤ると、iter-1・iter-2 の replay の方式 a・b の結果が消える。
2. 責務 owner: stub の振る舞いは `stub_reviewer.py`。出力の配置は sh と driver で、配置の規則は `pilot_runs.py`（反復の root の解決を compare・pilot_score と共有する module）に置いた。
3. 既存の再利用先: `pilot_runs.py`（配置の module）、`driver.STRATEGY_STAGE`（`pilot_runs.STRATEGY_STAGE` から導くようにした）、`test_driver.py` の `run_sh`・`_driver`・`run_stub`。
4. 選択肢:
   - (a) sh の `rm -rf` を残し、`--force` の flag を足す。新旧の切替の flag になるので不採用。
   - (b) sh と driver で別々に dir の有無を見る。規則が 2 つになるので不採用。
   - (c) 規則を `pilot_runs.run_dirs`・`existing_run_dirs` に置き、sh は build の前に `check-new` を呼び、driver は自分の dir だけを `out_path()` で見る。採用。
   - 版の段の dir: 常に拒むと、方式 c を時点ごとに流せない（2 回目は `strategy-v1/` があるので止まる）。段の終わったものは使い、終わっていないものは拒む、にした。
5. 新しい状態・分岐・fallback・port: sh に「段の終わった `strategy-v1/` があれば版の段を流さない」分岐を 1 つ足した。理由は上の 4。fallback・port・config の欄は無し。
6. 撤去対象: sh の `rm -rf "${out:?}/$case/strategy-v1"` と `rm -rf "${out:?}/$case/$p/$m"`。stub の `artifact` 行。申告を要求していた試験 `test_worker_writes_and_declares_the_strategy_document_and_critic_judges_its_candidates`（名を変え、申告の assert を消した。申告しないことは新しい試験が見る）。
7. 不変条件: 下の節。

## 守る不変条件と、既存の試験で検出できない理由
- stub の版を書く役は `artifact` 行を出さず、版は artifacts の下の `strategy/<id>/v1.json` だけにある（`test_stub_writer_does_not_declare`）。
- 既にある `<out>/<case>/<point>/<arm>/` と段の終わっていない `strategy-v1/` を、sh も driver も消さず上書きしない。sh は build の前に exit 1、他の方式の dir も作らない（`test_existing_arm_dirs_are_kept`）。
- 他の方式の dir と、段の終わった `strategy-v1/` は妨げない（`test_new_arm_dirs_pass_the_check`）。
- 既存の試験で検出できない理由: stub の試験は申告の行を要求していた（逆の不変条件）。sh の `rm -rf` を見る試験は無かった。

## 試験
### 変更前に落ちた試験
base（`$CELERIS_WU_BASE`）の `parent-eval-blind.sh`・`driver.py`・`stub_reviewer.py`・`research.py`・`pilot_runs.py`・`research_case.py` を `$TMPDIR/oldcheck` に `git show` で出し、新しい `test_driver.py` を置いて実行（`CARGO_TARGET_DIR` を外し、build に進まないようにした）:

```
env -u CARGO_TARGET_DIR python3 -I scripts/dev/parent-eval-blind/test_driver.py Stub.test_stub_writer_does_not_declare Args.test_existing_arm_dirs_are_kept
FAIL: test_stub_writer_does_not_declare
AssertionError: Lists differ: [{'type': 'artifact', 'name': 'strategy/pi[67 chars]on'}] != []
FAIL: test_existing_arm_dirs_are_kept
AssertionError: 'exists: …/o/replay/C-mid/a' not found in '…parent-eval-blind.sh: line 92: CARGO_TARGET_DIR: CARGO_TARGET_DIR is required\n'
Ran 2 tests … FAILED (failures=2)
```

base の sh は既存の dir があっても build の手前まで進んだ（`CARGO_TARGET_DIR` が無いので止まった。あれば build し、`rm -rf` で消していた）。

### 変更後（提出時の HEAD で実行）
- `python3 -I scripts/dev/parent-eval-blind/test_driver.py` → `Ran 19 tests` `OK`
- `python3 -I scripts/dev/parent-eval-blind/test_pilot_compare.py` → `OK`
- `python3 -I scripts/dev/parent-eval-blind/test_pilot_score.py` → `OK`
- `python3 -I scripts/dev/parent-eval-blind/test_research.py` → `OK`

## stub の端から端まで（同じ out。`A=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/wu/stub-undeclared/artifacts`）
1. `bash scripts/dev/parent-eval-blind.sh --harness stub --arms a,b,c --case replay --points C-mid,D-done,F-done --out "$A/stub-e2e" --state-root "$TMPDIR"` → exit 0（448 秒、build を含む）。
   - `replay/strategy-v1/run.json`: `task_status` done、`strategy.name` `strategy/pilot-replay/v1.json`、run 2 件（worker・reviewer）。`strategy-v1.critic.json`: review_verdict 2 件とも pass、`review_json.critic` あり。
   - 方式 c の 3 時点の `output.json`: `status` checked、`strategy.name` `strategy/pilot-replay/v1.json`、`strategy.stage_status` done。方式 b は checked、方式 a は `fixed_plan`（LLM 0）。
2. `bash scripts/dev/parent-eval-blind.sh --harness stub --arms a,b,c --case research --research-src scripts/dev/parent-eval-blind/testdata/research/c-results.md --points 11.16,11.17,11.20 --out "$A/stub-e2e" --state-root "$TMPDIR"` → exit 0（302 秒）。版の段は `strategy/pilot-research/v1.json` で done。
3. `grep -l "（Outcome の記録は無い）" "$A"/stub-e2e/*/*/[bc]/input/prompt.md` → 0 件（grep exit 1。対象 12 file）。
4. `python3 -I scripts/dev/parent-eval-blind-compare.py --pilot --runs "$A/stub-e2e" --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --research-src scripts/dev/parent-eval-blind/testdata/research/c-results.md --out "$A/stub-e2e-metrics"` → exit 0、`metrics.json` の `leaks` は `[]`。
5. `python3 -I scripts/dev/parent-eval-blind/pilot_score.py blind --runs "$A/stub-e2e" --metrics "$A/stub-e2e-metrics/metrics.json" --out "$A/stub-e2e-score/pilot-scoring.md" --key-out "$A/stub-e2e-score/pilot-key.json" --seed 1 --repo-copy "$A/stub-e2e-score/pilot-scoring.repo.md"` → exit 0。`check-blank` は本体・repo 用の写しとも exit 0。
6. 同じ out に 1 と同じ command を流し直す → build の前に exit 1（`exists: …/replay/C-mid/a` ほか 10 件と `the output already has these arm dirs … (they are not deleted)`）。out の全 file（209 件）の sha256 は前後で同じ。
7. 時点を 1 つずつ流す経路: `$A/stub-e2e-perpoint/replay/` に 1 の `strategy-v1/` を写し、`--arms c --case replay --points C-mid --out "$A/stub-e2e-perpoint"` → exit 0（34 秒）。`reusing …/strategy-v1` を出して版の段を流さず、`strategy-v1/` の sha256 は前後で同じ、`C-mid/c/output.json` は checked で `strategy.name` `strategy/pilot-replay/v1.json`。

1・2 は、段の終わった `strategy-v1/` を使う分岐を sh に足す前に流した。版の段を流す側の command は、その後は字下げを変えただけ。分岐の側は 7 で流した。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0（`repo_health_gate: all checks passed`、`試験対象の変更なし（docs のみ等）`。この WU は Rust・web を変えていない。`scripts/dev/parent-eval-blind/` の python の試験は集合に入っていないので、上の 4 file を直接走らせた）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `arm-driver.md`・`parent-eval-blind.sh`・`driver.py`・`pilot_runs.py`・`stub_reviewer.py`・`test_driver.py` とこの file だけ。
- `git diff --name-only "$CELERIS_WU_BASE" -- crates/task-worker/src/change_rules control-ts web` → 0 件。

## LoC（`git diff --numstat "$CELERIS_WU_BASE"`、この file を除く）
- 本体（script）: `parent-eval-blind.sh` +15 −8、`driver.py` +12 −6、`pilot_runs.py` +45 −1、`stub_reviewer.py` +3 −4。計 +75 −19。production の Rust の差分は 0。
- 試験: `test_driver.py` +66 −6。
- 文書: `arm-driver.md` +37 −31。
- 純増の理由: 出力の dir を消さない検査（`pilot_runs.run_dirs`・`complete_stage`・`existing_run_dirs` と CLI `check-new`）と、その試験。

## 未確認
- 実 LLM（claude-code）の worker が書いた版が `strategy/pilot-<case>/v1.json` の名で記録され、方式 c の版の段が done になるか。stub は申告しない書き方で同じ経路を通したが、実 LLM の run は operator が流すまで未確認。
- 研究ケースの実データ（`c-results.md`）での切り出し。この WU の worker は実データを開いていない（合成の testdata だけ）。arm-driver.md の手順 3 で operator が stub で確かめる。
- 評価器の入力の欠け（CoS のコメントの「既知の欠け」）は WU eval-input・eval-seed で直した後の HEAD で流した。stub の b・c の `input/prompt.md` 12 件に「（Outcome の記録は無い）」は 0 件。

## 提案
- `compare --pilot` の `secrets` は stub の out で方式ごとに 12（1 時点 4）だった。値を出さずに位置を調べると、全部が `daemon.log` の `…/scratch/targets/task-01M…` の `sk-01M…` の部分（`task-` の末尾の `sk-` が secret の正規表現に一致）で、secret ではない。pilot-materials か close-audit で、`daemon.log` を数える対象から外すか、件数の注記を採点材料に書くことを検討する（この WU では compare を変えていない）。
