---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: arm-driver
status: running
---
# 戦略層 v1 子 8 arm-driver: driver を方式 a/b/c に広げ、replay の 3 時点を stub で流す

## 結論（この run の時点）
- 子 6 の driver（`scripts/dev/parent-eval-blind.sh` → `scripts/dev/parent-eval-blind/driver.py`）を、方式 a・b と事例 `replay` の時点 C-mid・D-done・F-done に広げた。一時 daemon の起動・停止・API・TS の配備は子 6 のものをそのまま使い、写しを作っていない。
- 起動の形: `bash scripts/dev/parent-eval-blind.sh --harness stub|config --arms a,b,c --case replay --points C-mid,D-done,F-done --out <dir> [--approval <json> --llm-config <toml>]`。
- 出力は `<dir>/<case>/<point>/<arm>/` の `output.json`（verdict・next・claims〈根拠の参照つき〉・usage。方式 b は最終の記録 `record` も持つ）・`reads.json`・`run.json`、方式に渡した入力の写し `input/`（`request.json`・`prompt.md`・`files.json` と `tree/` の本文）。
- stub で replay の 3 時点 × 方式 a・b の 6 組が exit 0 で書けた。方式 b の `input/prompt.md` 3 件に「（Outcome の記録は無い）」は 0 件で、`outcome \`arch-v4\`` の行がある。
- **方式 c は未実装**。版 v1 を書く run と Critic の段（ADR D1.3）を driver にまだ足していない。sh は `--arms` に c があると exit 2 で断る（手で書いた版で代えない。ADR D1.5）。続きの run で足す（下の「残り」）。

## 実装前 brief
1. 根本原因: 子 6 の driver は時点 E-mid・D-done と方式の名 `parent-eval-blind` に固定で、方式 a（親の途中評価なし）を流す口が無く、出力は `<out>/<point>/evaluation.json` だけだった。さらに WU eval-seed の後、seed が承認済み Roadmap の版 1 を置くのに driver が API で Roadmap を `base_version: null` でもう一度提案しており、版 1 と食い違っていた。
2. 責務 owner: pilot の driver（`scripts/dev/parent-eval-blind.sh`・`driver.py`）と、seed の試験（`crates/task-api/tests/parent_checkpoint_live.rs` の `parent_checkpoint_blind_seed`）。production（`crates/*/src`）は owner ではない。
3. 既存の再利用先: 子 6 の driver の一時 daemon・TS の配備・待ち、`pilot_gate.py check-approval`（方式の名をそのまま `--arms` に渡す）、seed の関数 `seed`、本物の prompt の関数 `parent_checkpoint_section`・`result_section`、`parent-eval-blind-compare.py` の `check_reads`。
4. 選択肢: (i) 方式ごとに別の driver を作る（不採用: 一時 daemon の起動の写しになる）。(ii) 方式の違いを controller の mode だけで作る（**採用**。ADR D1）: a は mode `shadow`（evaluate は `suppressed`、reviewer run 0）、b は mode `enabled`。
5. 新しい状態・分岐・fallback・port: production には無し。script の中に方式の分岐（a は suppressed の判断を待つ、b は最終の記録を待つ）。
6. 撤去対象: 子 6 の出力の配置 `<out>/<point>/evaluation.json` と時点の制限 E-mid・D-done、方式の名 `parent-eval-blind`、driver の Roadmap の API での提案と承認（seed が置く）。commit 済みの子 6 の記録は新しい配置へ移した（旧と新を両方読む切替は作っていない）。
7. 不変条件と試験: 下の節。

## 変えた file
- `scripts/dev/parent-eval-blind.sh`: `--arms`・`--case` を足し、時点を事例 replay の C-mid・D-done・F-done に限った。承認の検査に `--arms "$arms"` を渡す。時点 × 方式ごとに driver を起こし、出力を `<out>/<case>/<point>/<arm>/` に置く。`CELERIS_RUN_ID` の拒否（引数の検査より前）は前のまま。
- `scripts/dev/parent-eval-blind/driver.py`:
  - `--case`・`--arm` を足した。方式 a は controller を mode `shadow` にし、`project_controller_decisions` に evaluate の判断が `effect_state = suppressed` で載るまで待ち、root の reviewer run が 0 件であることを確かめる（1 件でもあれば失敗）。
  - 方式 b は前と同じく最終の記録 `parent-evaluation/<判断 id>.json` を待つ。
  - Roadmap を API で提案・承認する手順を消した（seed が版 1 を置く。WU eval-seed）。
  - `[project_controller.checkpoint] max_evaluation_attempts = 1`、`max_usd_per_day` は `min(承認の max_usd, 3.0)`（ADR D3.1）。
  - `input/`: seed の直後（daemon と run が `artifacts/`・`runs/` を書く前）に作業 tree の一覧と本文を写す。run の後に `runs/<run_id>/request.json` を写し、harness が `prompt.txt` を書いていればそれを `prompt.md` に、fake の harness（stub）では本物の prompt の関数で描く（下の試験 `parent_checkpoint_blind_render`）。`run.json` の `prompt_source` が `harness` か `rendered` を示す。
  - **漏れの修正**: 前の driver は時点の dir に無い slice の file を E-mid の file で埋めていた。C-mid では E-mid の `reports.json`（C-mid の cutoff より後の報告）が作業 tree に入る。全時点で同じ `goal.md`（子 0 の manifest の sha256 が同じ）だけを E-mid から取り、`reports.json` は時点の file が無ければ置かない。
- `scripts/dev/parent-eval-blind-compare.py`: 新しい配置（`<case>/<point>/<arm>/output.json`）を読む。方式 a は評価器を起こさないので status を見ない。`check_reads` は変えていない。
- `crates/task-api/tests/parent_checkpoint_live.rs`（試験だけ）: seed が C-mid・F-done も受ける（`PILOT_POINTS`。同じ規則で cutoff 以前の記録だけ）。`#[ignore]` の `parent_checkpoint_blind_render` を足した。`parent_checkpoint_blind_live_checks` は `replay/<時点>/<b|c>/output.json` の `record` を検査し、`output.json` の verdict・next が記録の effective と同じことも確かめる。
- 子 6 の記録の移動: `crates/task-api/tests/fixtures/parent_checkpoint_live/blind/{E-mid,D-done}/` → `blind/replay/{E-mid,D-done}/b/`。`evaluation.json` は `output.json` の `record` にそのまま入れ（`output_from_record` で変換）、`run.json` に `case`・`arm`・`llm_runs`・`prompt_source` を足した。`blind/compare.json` は新しい配置で比較を流し直した（変わったのは時点の key 2 つだけ。値は同じ）。
- `scripts/dev/parent-eval-blind/test_driver.py`（新規）。

## 守る不変条件と、既存の試験で検出できない理由
- 方式の引数は a・b・c、時点は事例 replay の C-mid・D-done・F-done だけ（他は exit 2）。
- `--harness config` は worker の run の中（`CELERIS_RUN_ID`）で引数の検査より前に exit 2。承認の `scope.arms`・`scope.cases` に無い方式・時点は exit 2。
- 方式 a は reviewer run 0（LLM 0）。方式 b の入力の prompt に「（Outcome の記録は無い）」が出ない。
- 作業 tree に時点より後の file（他の時点の `reports.json`）を置かない。
- output.json は verdict・next・claims（根拠の参照つき）・usage を持つ。
- 既存の試験で検出できない理由: driver と sh には試験が無かった。子 6 の形のままでは方式 a・b・c の引数・承認の arms・出力の形を誰も確かめず、E-mid の `reports.json` を他の時点に流用する漏れは E-mid・D-done だけを流していたので表に出なかった。

## 試験
### 変更前に落ちた試験
新しい `test_driver.py` を、base（`$CELERIS_WU_BASE`）の `parent-eval-blind.sh`・`driver.py`・`stub_reviewer.py`・`pilot_gate.py` と base の fixture を `$TMPDIR` に取り出した tree で実行した（`python3 -I -W ignore::ResourceWarning scripts/dev/parent-eval-blind/test_driver.py`）。13 件のうち 10 件が落ちた（failures=5, errors=5）:
```
ERROR: test_arm_a_output_is_the_fixed_plan_without_llm
ERROR: test_arm_b_output_has_verdict_next_claims_with_refs_and_usage
ERROR: test_driver_cases_and_arms_match_the_shell
ERROR: test_suppressed_evaluate_picks_only_the_suppressed_evaluate_decision
ERROR: test_tree_listing_excludes_git_and_hashes_files
FAIL: test_approval_without_the_arm_is_refused
FAIL: test_approval_without_the_point_is_refused
FAIL: test_arm_c_is_refused_until_the_strategy_stage_exists
FAIL: test_point_outside_the_case_is_refused
FAIL: test_unknown_arm_is_refused
Ran 13 tests ... FAILED (failures=5, errors=5)
```
base でも通った 3 件（`CELERIS_RUN_ID` の拒否・未知の事例と `--arms` の欠け・承認の無い config の拒否）は、base の sh が既に断っていた振る舞いで、回帰の確認として残した。

### 変更後
- `python3 -I -W ignore::ResourceWarning scripts/dev/parent-eval-blind/test_driver.py` → Ran 13 tests, OK。
- `env -u CELERIS_RUN_ID bash scripts/dev/parent-eval-blind.sh --harness stub --arms a,b --case replay --points C-mid,D-done,F-done --out $TMPDIR/stub-out` → exit 0（約 3.5 分。build は cache 済み）。
  - 方式 a: 3 時点とも verdict 無し・next `continue`・status `fixed_plan`・`llm_runs` 0、`controller_decision.effect_state` は `suppressed`。claims は cutoff 以前の子の局所の review の合格（C-mid 2・D-done 4・F-done 6）。
  - 方式 b: 3 時点とも `checked`・`unknown`/`ask_human`（stub の固定の出力）・`llm_runs` 1・`prompt_source` `rendered`。
  - `input/files.json`: C-mid・F-done は `events.jsonl`・`goal.md`・`slice.json`、D-done は加えて `reports.json`。
  - `grep -c "（Outcome の記録は無い）" replay/*/b/input/prompt.md` → 3 件とも 0。
- `cargo nextest run -p task-api --test parent_checkpoint_live` → 6 passed, 4 skipped（`#[ignore]`）。
- `cargo nextest run -p task-api --test parent_checkpoint_live --run-ignored only -E 'test(parent_checkpoint_blind_live_checks)'`（commit した子 6 の記録）→ 1 passed（E-mid・D-done とも `Checked`、参照は slice の中）。
- 同じ試験を `PARENT_CHECKPOINT_BLIND_DIR=<stub の出力>` で → 1 passed（C-mid・D-done・F-done の stub の記録が `Checked`）。
- `python3 -I scripts/dev/parent-eval-blind-compare.py --out <stub の出力> --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json` → exit 0、forbidden 0・outside 0。

## operator が worker の外で実 LLM の pilot を流す手順
人の答え（2026-11-24）: 2 反復・最大 48 run・25 USD、account は `claude_max_personal`、model は `claude-sonnet-5-5`（子 6 と同じ）。研究ケースは内部の評価だけ（ChatGPT・Muse に送らない）。測定用の account（`claude_max_lab`・`/local/celeris/measure-accounts`）は使わない。本番の DB・daemon・config に触れない。

1. 承認の json を人が書き、root の artifacts に置く（commit しない）。例（`/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-approval.json`）:
   ```json
   {
     "approval_id": "strategy-v1-8-pilot-2026-11-24",
     "approved_by": "human:rmaeda",
     "approved_at": "2026-11-24T09:00:00Z",
     "expires_at": "2026-12-01T00:00:00Z",
     "account": "claude_max_personal",
     "model": "claude-sonnet-5-5",
     "harness": "claude_code",
     "scope": {
       "arms": ["a", "b", "c"],
       "cases": ["C-mid", "D-done", "F-done", "research-11.16", "research-11.17", "research-11.20"],
       "environment": "non-production"
     },
     "limits": {"max_usd": 25, "max_wall_secs": 600}
   }
   ```
2. `--llm-config` の TOML は子 6 の 4 run と同じ形（`claude_code` の harness・`claude_max_personal`・`claude-sonnet-5-5` の route）を operator が書く。全方式・全時点で同じ 1 つの file を使う（ADR D3.1）。
3. 反復ごとに出力先を分けて流す（worker の外の shell で。`CELERIS_RUN_ID` があると exit 2）:
   ```sh
   P=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-runs
   for it in 1 2; do
     bash scripts/dev/parent-eval-blind.sh --harness config --arms a,b --case replay \
       --points C-mid,D-done,F-done --out "$P/iter-$it" \
       --approval /local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-approval.json \
       --llm-config <operator の toml>
   done
   ```
   方式 c は driver に入ってから `--arms a,b,c` にする（この commit では c は exit 2）。研究ケースは research-driver の WU が `--case` に足す。
4. 上限（ADR D3.3・D3.4）: 全体で 48 run・25 USD。replay の方式 b は 1 時点 1 run（3 時点 × 2 反復 = 6 run）、方式 a は 0 run。1 時点の daemon の上限は `max_usd_per_day` 3.0 USD。各時点の前に、使った USD ＋ 3.0 が 25 を超える、または使った run ＋ 計画の run が 48 を超えるなら、その時点を始めずに止め、残りを「未実施」として人に聞き直す。使った USD と run は各 `run.json` の `usd`・`llm_runs` を足して数える。
5. 流した後、`python3 -I scripts/dev/parent-eval-blind-compare.py --out "$P/iter-<n>" --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json` を operator が走らせ、forbidden・outside が 0 であることを確かめる（answer key を開くのはこの script だけ）。

## 欠け（plan_issue の候補）
- 方式 c（版 v1 を書く run・Critic・v2 の起票）は driver に無い。production を変えずに既存の入口で流せるか（fake の worker run が `strategy/<id>/v1.json` を成果物に書き、その final review に `strategy_critic` が付き、一時 DB の決定 `strategy-approve-v1` に答えられるか）を、続きの run が確かめる。流せなければ ADR D1.5 のとおり未確認として報告する。

## 未確認
- 方式 c の全部（上の欠け）。
- 実 LLM（`--harness config`）での run。harness が `prompt.txt` を書く経路（`prompt_source: harness`）は stub では通っていない。
- D3.4 の方式の順の乱数（`pilot-key.json`）と D5 の盲検の id は、この driver では振っていない（出力の配置は objective のとおり方式の名を使う）。盲検は pilot-materials の段で行う前提。

## 残り（続きの run）
- 方式 c の段を driver と stub に足し、stub で 3 時点 × 方式 c を流す。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` を提出時の HEAD で実行し、この進捗を done にする。
