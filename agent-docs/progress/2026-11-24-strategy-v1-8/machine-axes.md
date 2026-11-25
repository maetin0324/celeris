---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: machine-axes
status: done
updated: 2026-11-25
completed: 2026-11-25
---
# 戦略層 v1 子 8 machine-axes: 採点の分担（人は 4 軸、pivot の時点とコストは開封の後に機械）を rubric と pilot_score.py に入れる

## 結論
- 人の決定（2026-11-25、段 score の後）を道具と文書に入れた。人 rmaeda は 4 軸（execution success・epistemic progress・parent goal achievement・unsupported claims）とレビュー時間（分）だけを付ける。pivot の時点とコストの 2 軸は、close-audit が `pilot-key.json` を開いた後に `pilot_score.py machine-axes` で付ける。
- 人がすること: claude.ai の非公開の Artifact で 4 軸とレビュー時間を付ける。operator が root の artifacts の `pilot-scoring.md` に書き写し、`pilot_score.py check-human` が exit 0 になることを確かめてから continue する。
- この WU は道具と文書だけを変えた。root の artifacts（`$ART`）の file は書き換えていない。`pilot-key.json` は開いていない（sha256 を計算しただけ）。machine-axes と tally は実データに当てていない。repo の写し `pilot-scoring.md` は作り直していない。実 LLM の run は流していない。合格線は変えていない。

## 変更（commit の件名）
- `strategy-v1 8 machine-axes: 採点の分担を rubric の手順 3 と pilot_score.py（check-human・machine-axes）に入れる`
  - `agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md`: 「採点の手順」の 3 だけを直した（人は 4 軸とレビュー時間、機械は 2 軸、規則と数値は子 8 の進捗、子 9 は人の確認待ち）。「軸と基準」から末尾までは base と同じ byte。
  - `scripts/dev/parent-eval-blind/pilot_score.py`:
    - `check-human <md>` を足した。人の 4 軸が 0〜2 で、根拠が空でなく「機械:」で始まらないこと、採点者が `human:<名>`、レビュー時間（分）が数であること、pivot の時点とコストの行が空か「機械:」の根拠と 0〜2 の点であることを見る。
    - `machine-axes` を足した（規則は下）。
    - `check-scored` は check-human に加え、機械の 2 軸が 0〜2 の点と「機械:」の根拠で埋まっていることを見る。
    - `tally` から wrong_pivots による上書きと `--metrics` を消した（machine-axes に移した。二重にしない）。表の 2 軸の見出しに「（機械）」を付け、出力の json に `machine_axes` を書く。
    - 埋め込んだ rubric の節のうち「採点の手順」は有無だけを見る。「軸と基準」「合格線」は rubric.md との一致を見る。
  - `scripts/dev/parent-eval-blind/test_pilot_score.py`: `test_check_human_*` 3 件・`test_machine_axes_*` 6 件・`test_committed_repo_copy_passes_check_blank` を足し、既存の check-scored・tally の試験を分担に合わせた。
  - `agent-docs/adr/2026-11-24-strategy-pilot-ablation.md`: D5 付記 1（分担・採点の場所・作り直さないもの・検査の口・close-audit の手順）と D7 付記 1（機械の 2 軸の規則と数値）を足した。
- `strategy-v1 8 machine-axes: WU の進捗に提出時の検査の結果を書く`（この file）

## 機械が付ける 2 軸の規則（ADR D7 付記 1）
規則は反復ごとに当て、id の点は**反復の最小**とする。
- pivot の時点
  - 検知できた時点: replay は answer key の `first_detectable` の **E-mid**、研究ケースは ADR D2.2 の反証の **11.20**。
  - 検知: metrics の `first_detection`。record の status が invalid の記録は検知に数えず、同じ定義（compare の `DETECT_VERDICTS` を import）で後の時点を見る。
  - 時点の差（符号付き）: replay は answer key の `order`（C-mid 3・E-mid 6・D-done 7・F-done 10）、研究ケースは `research_case.py` の `POINTS`（11.15〜11.21）の順。
  - 点: 差が 1 以下で 2（前の時点の検知は負で 2）、2〜3 で 1、4 以上か検知無しで 0。その反復の `wrong_pivots` が 2 以上なら 0。研究ケースは answer key が無いので wrong_pivots を数えない。
  - この pilot で取りうる差は replay が C-mid −3・D-done +1・F-done +4、研究ケースが 11.16 −4・11.17 −3・11.20 0。したがって 1 点は出ない（計算で確認）。
- コスト
  - replay: 比 = max((`unneeded_after_first_detectable` の worker run ＋ 方式の `llm_runs`) ÷ **137 run**、(同じ USD ＋ 方式の `usd`) ÷ **264.2525 USD**)。比が **25 %** 以下で 2、**75 %** 以下で 1、超えたら 0。基準は metrics の `history` から読む。
  - 研究ケース: 史実の基準が無いので全方式 0。根拠に「史実の基準が無い・未確認」と書く。
- 埋め方: 採点欄の 2 行だけを `| pivot の時点 | <点> | 機械: <規則> <反復ごとの数値> |` の形で埋める。ほかの byte は変えず、もう一度流しても同じ byte になる。`--out` に id ごとの方式・規則・反復ごとの数値（`first_detection`・数えた検知・差・wrong_pivots・比）を書く。
- 開封の前の確かめ: check-human を通ること。`pilot-key.json` の sha256 が採点の file の値と一致すること。`metrics.json` の sha256 が key の `metrics_sha256` と一致すること。answer key の sha256 が metrics の `answer_key_sha256` と一致すること。採点欄の id が key と同じこと。どれかが違えば exit 1 で、file を変えない。

## close-audit の手順（ADR D5 付記 1）
`ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`、repo の root で実行する。
```
python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-human $ART/pilot-scoring.md
python3 -I scripts/dev/parent-eval-blind/pilot_score.py machine-axes --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json \
  --metrics $ART/pilot-metrics/metrics.json --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json \
  --out $ART/pilot-machine-axes.json
python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-scored $ART/pilot-scoring.md
python3 -I scripts/dev/parent-eval-blind/pilot_score.py tally --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --out $ART/pilot-tally.json
```
WU pilot-materials の進捗に書いた `tally … --metrics …` は、tally が `--metrics` を受けなくなったので使えない（exit 2）。

## 試験（変更前に落ちる負の試験を含む）
- `python3 -I scripts/dev/parent-eval-blind/test_pilot_score.py`: 変更後は 21 件すべて OK（exit 0）。
- 変更前に落ちること: `pilot_score.py` だけを `$CELERIS_WU_BASE` の版に戻し、新しい rubric と新しい試験で流すと、21 件のうち 14 件が落ちた（errors 13・failures 1）。そのあと変更後の版に戻した。
  - 落ちた試験: `test_check_human_accepts_four_axes_with_blank_machine_rows`、`test_check_human_rejects_bad_fields`、`test_check_human_rubric_procedure_presence_only_and_pass_line_exact`、`test_machine_axes_point_boundaries`、`test_machine_axes_fills_only_the_two_rows_and_is_idempotent`、`test_machine_axes_points_on_the_synthetic_runs`、`test_machine_axes_takes_the_minimum_over_iterations_and_wrong_pivots_two_gives_zero`、`test_machine_axes_does_not_count_an_invalid_record_as_detection`、`test_machine_axes_refuses_unscored_files_and_other_inputs`、`test_committed_repo_copy_passes_check_blank`（failure: 手順 3 を変えた rubric で、写しの「採点の手順」節が一致しない）、合わせて直した `test_check_scored_fails_on_blank_and_bad_fields`・`test_filled_or_changed_rubric_fails_check_blank`・`test_tally_opens_and_judges_the_pass_line`・`test_tally_refuses_a_key_with_another_sha256`。
- 試験が守る不変条件と、既存の試験で検出できない理由:
  - 点の境界（差 −5・0・1 で 2、2・3 で 1、4・9・検知無しで 0、wrong_pivots 2・3 で 0。比 0・25 % で 2、25 % を少し超えると 1、75 % で 1、75 % を少し超えると 0）。既存の試験には機械の規則が無かった。
  - machine-axes が変えるのは id ごとに 2 行だけ（空の行 → `機械:` の行）で、2 回目は同じ byte。`--out` の json も同じ byte。
  - 反復の最小: 2 反復の合成の metrics で、1 反復の wrong_pivots 2 → pivot 0、1 反復の比 47.45 % → コスト 1、1 反復の検知無し → pivot 0。
  - invalid の記録は検知に数えない: D-done の記録を invalid にすると、数える検知は F-done（差 4）で 0 点。
  - 変えた合格線・変えた軸と基準は check-human と machine-axes で落ちる。変えた採点の手順は通り、無い採点の手順は落ちる。
  - 空の採点・別の key・別の metrics・別の answer key は machine-axes が断り、file を変えない。
  - tally は `--metrics` を受けない（exit 2）。表に「pivot の時点（機械） | コスト（機械）」が出る。

## 受け入れ条件の確かめ（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- rubric の「軸と基準」から末尾が base と同じ: `diff <(git show $CELERIS_WU_BASE:agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md | sed -n '/^## 軸と基準$/,$p') <(sed -n '/^## 軸と基準$/,$p' agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md)` → exit 0（差分なし）。rubric の差分は「採点の手順」の 3 の 1 行が 3 行に替わっただけ。
- repo の写しの check-blank: `python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-blank agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md` → exit 0。写しは `$CELERIS_WU_BASE` と同じ byte（`git diff --quiet $CELERIS_WU_BASE -- agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md` → exit 0）。
- `pilot-key.json` の sha256: `sha256sum $ART/pilot-key.json` → `36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace`（変わっていない）。`$ART/pilot-scoring.md` と `pilot-key.json` の mtime は 2026-11-25 03:44:53Z のまま。
- 実の材料に読むだけの検査を当てた: `pilot_score.py check-blank $ART/pilot-scoring.md` → exit 0（埋め込んだ「採点の手順」節は材料を作った時点の文のままで、有無だけを見る規則で通る）。machine-axes と tally は当てていない。
- 規則の数値（25 %・75 %・137 run・264.2525 USD・E-mid・11.20・反復の最小）は ADR の D7 付記 1 とこの file の「機械が付ける 2 軸の規則」にある（`grep -cF` でどれも 1 件以上）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo_health_gate: all checks passed）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `python3 -I scripts/dev/parent-eval-blind/test_pilot_score.py` → exit 0（21 件 OK）。同じ dir の `test_driver.py`・`test_pilot_compare.py`・`test_research.py` → どれも exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。`sh scripts/dev/check-doc-links.sh` → exit 0（ok）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は ADR・rubric.md・この進捗・`pilot_score.py`・`test_pilot_score.py` の 5 file。

## LoC（`git diff --numstat $CELERIS_WU_BASE`、この進捗の commit の前）
| file | 区分 | 追加 | 削除 |
|---|---|---|---|
| `scripts/dev/parent-eval-blind/pilot_score.py` | script（dev の道具。production の crates ではない） | 213 | 13 |
| `scripts/dev/parent-eval-blind/test_pilot_score.py` | 試験 | 237 | 14 |
| `agent-docs/adr/2026-11-24-strategy-pilot-ablation.md` | 文書 | 51 | 0 |
| `agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md` | 文書 | 3 | 1 |
| この進捗 | 文書 | 87 | 0 |
- crates・web・評価器の prompt・testdata・compare は変えていない。production の Rust・TS の LoC は 0 行の差。script の純増 200 行は、人の決定で増えた 2 つの口（check-human・machine-axes）と規則の根拠の文の分。

## 未確認
- 人の 4 軸の採点はまだ無い。machine-axes と tally は実データに当てていないので、実データでの機械の 2 軸の点と合格線の判定は未確認（close-audit の仕事）。
- 研究ケースの誤った pivot は数えていない（answer key が無く、metrics に wrong_pivots が無い）。研究ケースのコストは史実の基準が無いので全方式 0 で、未確認。
- 材料に埋め込んだ「採点の手順」節は材料を作った時点の手順（6 軸を人が付ける）のままで、rubric.md の今の手順 3 と違う。人の決定で材料を作り直さないため。人が Artifact の上で新しい分担で採点することは operator の報告によるもので、この WU では確かめていない。
- この pilot の時点では pivot の時点の 1 点が出ない（差が −4・−3・0・+1・+4 のどれか）。rubric の 2〜3 時点後の区分は、この pilot では使われない。

## 提案
- 子 9 で同じ分担（人は 4 軸とレビュー時間、pivot の時点とコストは開封の後に機械）を使うかは人の確認待ち（rubric の手順 3）。close-audit が子 8 の task の進捗の「提案」に写し、人に聞く。
- 研究ケースのコストに史実の基準を作るか（例: 11.20 の後に流した HPC の job の数）を、子 9 の前に人に聞く。今は全方式 0 で、方式の差に効かない。
