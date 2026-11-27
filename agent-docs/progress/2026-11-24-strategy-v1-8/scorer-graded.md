---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: scorer-graded
status: done
updated: 2026-11-27
completed: 2026-11-27
---
# 戦略層 v1 子 8 scorer-graded: check-human を check-graded に改め、人の決定の記録がある model の採点者を受ける

## 結論
- 人 rmaeda の決定（2026-11-27 10:3xZ。operator〈`external_operator:claude-code-ops-session`〉の対話 session）を道具と文書に入れた。
  人は 4 軸を採点しない。Fable の model（`model:claude-fable-5-1`）が盲検で付けた。pivot の時点とコストの 2 軸は、今までどおり開封の後に機械が付ける。
- `pilot_score.py check-human` を `check-graded` に改名した（別名は残していない）。採点者は `human:<名>` なら決定無しで通る。
  `model:<id>` は、`--decision` の json の `scorer` が同じで、`decided_by` が `human:<名>`、`decided_at` が YYYY-MM-DD、`axes` が採点者の 4 軸と同じときだけ通る。
  `operator:` などのほかの採点者は通らない。
- 今の `$ART/pilot-scoring.md` は `check-graded --decision agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json` で exit 0、決定無しでは exit 1（6 表とも「決定の記録が無い」）。
- この WU は道具と文書だけを変えた。`$ART`（`/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`）の file は書き換えていない。
  machine-axes と tally は実データに当てていない。Fable の採点も実 LLM の評価器の run も流していない。repo の写しの `pilot-scoring.md` と `replay/` は変えていない。

## 変更（commit の件名）
- `strategy-v1 8 scorer-graded: check-human を check-graded に改め、人の決定の記録がある model の採点者を受ける（rubric の手順 1・3）`
  - `scripts/dev/parent-eval-blind/pilot_score.py`
    - `check-human` → `check-graded`、関数も改名した（コメント・docstring にも旧い名を残していない）。採点者の規則だけを変え、4 軸の点と根拠・機械の 2 行・レビュー時間の規則は同じ。
    - `--decision <json>` を `check-graded`・`check-scored`・`machine-axes`・`tally` に足した。決定の欄の検査は `decision_problems`（scorer が `model:<id>`、decided_by が `human:<名>`、decided_at が実在の日付の YYYY-MM-DD、axes が 4 軸）。
    - `tally` は model の採点者の行のレビュー時間（分）に「（model の wall・人の時間ではない）」と添え、「採点者 model:…: human:… が YYYY-MM-DD に決めた model の採点。分は model の wall・人の時間ではない」の行を出す。json の採点者ごとに `kind`（human・model）、model なら `minutes_note`・`decided_by`・`decided_at` を書く。
    - `blind` が作る材料の「この文書について」の「採点者は人」の文を、新しい手順（人か人の決定の記録がある model、4 軸だけ、機械の 2 行は空のまま、model のレビュー時間は人の時間ではない）に替えた。今の材料は作り直していない。
  - `scripts/dev/parent-eval-blind/test_pilot_score.py`: `test_check_human_*` 3 件を同じ assertion のまま `test_check_graded_*` に改名し、`test_check_graded_*` を 5 件足した。
  - `agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md`: 「採点の手順」の 1・3 だけを直した（採点者は人か、人の決定の記録がある model。子 8 は 2026-11-27 に人 rmaeda が決め、`model:claude-fable-5-1` が採点した。子 9 で使うかは人の確認待ち。model のレビュー時間は人の時間ではない）。
  - `agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json`: 人の決定の記録（scorer・axes・decided_by `human:rmaeda`・decided_at `2026-11-27`・relayed_by・input の名と sha256・書き写しの前後の sha256・scorer の wall と tool の数・review_minutes_note）。
- `strategy-v1 8 scorer-graded: ADR に D5 付記 2（4 軸の採点者は人が決めた model、検査は check-graded）を足す`
  - `agent-docs/adr/2026-11-24-strategy-pilot-ablation.md`: D5 付記 2（人の決定・採点者の規則・検査の口・close-audit の新しい手順・限界・試験）。D5 本文と付記 1 の「人が採点する」「check-human」は付記 2 で置き換えると書いた。
- `strategy-v1 8 scorer-graded: WU の進捗を書く`（この file）

## close-audit の手順（ADR D5 付記 2。付記 1 の手順を置き換える）
`ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`、`DEC=agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json`、repo の root で実行する。
```
python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-graded $ART/pilot-scoring.md --decision $DEC
python3 -I scripts/dev/parent-eval-blind/pilot_score.py machine-axes --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json \
  --metrics $ART/pilot-metrics/metrics.json --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json \
  --decision $DEC --out $ART/pilot-machine-axes.json
python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-scored $ART/pilot-scoring.md --decision $DEC
python3 -I scripts/dev/parent-eval-blind/pilot_score.py tally --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --decision $DEC --out $ART/pilot-tally.json
```

## 試験（変更前に落ちる負の試験を含む）
- `python3 -I scripts/dev/parent-eval-blind/test_pilot_score.py` → exit 0（26 件 OK）。
- 変更前に落ちること: `pilot_score.py` だけを `$CELERIS_WU_BASE` の版に戻して新しい試験を流すと、26 件中 9 件が落ちた（failures 18〈subTest を含む〉・errors 1）。そのあと HEAD の版に戻した。
  - 落ちた試験: 新しい `test_check_graded_model_scorer_passes_only_with_the_human_decision`・`test_check_graded_other_scorers_fail_and_human_needs_no_decision`・`test_check_graded_decision_reaches_check_scored_machine_axes_and_tally`・`test_check_graded_committed_decision_admits_the_pilot_scorer`（error: `graded_axes` が無い）・`test_check_graded_blind_material_states_the_scorer_rule`、改名した `test_check_graded_accepts_four_axes_with_blank_machine_rows`・`test_check_graded_rejects_bad_fields`・`test_check_graded_rubric_procedure_presence_only_and_pass_line_exact`（base に `check-graded` が無く exit 2）、`test_machine_axes_refuses_unscored_files_and_other_inputs`（断る理由の文が `check-graded`）。
- 試験が守る不変条件と、既存の試験で検出できない理由（既存の試験は採点者を `human:` かどうかだけで見ており、model の採点者と人の決定の記録を見る試験が無かった）:
  - model の採点者は決定があれば通り、決定が無いと落ちる。決定の `decided_by` が `operator:…`・`model:…`・欠け、`scorer` が違う・欠け、`axes` が 3 軸・5 軸・欠け、`decided_at` が `2026/11/27`・`2026-13-27`・欠けのとき落ちる。
  - `human:rmaeda` は決定無しでも決定付きでも通る。`operator:claude-code-ops-session`・`cos:x`・`claude-fable-5-1`・`human:`・`model:`・`Human:rmaeda`・空は、決定があっても落ちる。
  - `machine-axes` は決定の無い model の採点を断り、file を変えない。決定付きなら埋まり、`check-scored --decision` が通る。`tally` は決定が無ければ開封せず、決定付きなら model の行にだけ「（model の wall・人の時間ではない）」を添え、json の `kind` で人と model を分ける。
  - commit した `scorer-decision.json` は `decision_problems` を通り、scorer `model:claude-fable-5-1`・decided_by `human:rmaeda`・decided_at `2026-11-27`・axes が rubric の 4 軸と同じ順。
  - `blind` の材料の「この文書について」に「採点者は人（rubric の手順）」が無く、新しい手順の文があり、check-blank を通る。

## 受け入れ条件の確かめ（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 0（旧い名が無い）: `grep -rn "check-human\|human_problems" scripts/dev/parent-eval-blind/ agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md` → 出力なし（exit 1）。`check-graded` は `pilot_score.py` の subparser にある。
- 1（model は決定があるときだけ）: 上の試験 `test_check_graded_model_scorer_passes_only_with_the_human_decision`・`test_check_graded_other_scorers_fail_and_human_needs_no_decision`。
- 2: 上の「試験」（変更前に 9 件が落ち、変更後に 26 件 OK）。
- 3（今の材料）:
  - `python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-graded $ART/pilot-scoring.md --decision agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json` → exit 0。
  - `python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-graded $ART/pilot-scoring.md` → exit 1（6 表とも「採点者 model:claude-fable-5-1 を認めた人の決定の記録（--decision）が無い」）。
  - `sha256sum $ART/pilot-scoring.md $ART/pilot-key.json` は検査の前後で `02f9b38b27c9e1ef9d349cc3a34fadc4ff224be86fb094d65cdc577a872a5d30`・`36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace` のまま。mtime も 2026-11-27 10:40:25Z・2026-11-25 03:44:53Z のまま。
  - repo の写しの `check-blank agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md` → exit 0。
- 4（rubric）: `diff <(git show $CELERIS_WU_BASE:agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md | sed -n '/^## 軸と基準$/,$p') <(sed -n '/^## 軸と基準$/,$p' agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md)` → exit 0（差分なし）。rubric の差分は「採点の手順」の 1（1 行 → 2 行）と 3 の 1 行目・2 行目だけ（`git diff --numstat` で 4 行追加・3 行削除）。
- 5: `scorer-decision.json`・ADR の D5 付記 2・この file がある。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo-health verdict=pass、fail_grade=0。new=18 はどれも warning の `dup.test-fixture` で、前の WU が足した `scripts/dev/parent-eval-blind/testdata/` の重複。この WU は testdata を変えていない）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 同じ dir の script の試験: `test_pilot_score.py` 26 件・`test_driver.py` 19 件・`test_pilot_compare.py` 20 件・`test_research.py` 16 件、どれも exit 0。
- `sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-doc-links.sh` → どちらも exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は ADR・rubric.md・`scorer-decision.json`・`pilot_score.py`・`test_pilot_score.py`・この進捗の 6 file。

## LoC（`git diff --numstat $CELERIS_WU_BASE`、この進捗の commit の前）
| file | 区分 | 追加 | 削除 |
|---|---|---|---|
| `scripts/dev/parent-eval-blind/pilot_score.py` | script（dev の道具。production の crates ではない） | 110 | 30 |
| `scripts/dev/parent-eval-blind/test_pilot_score.py` | 試験 | 147 | 26 |
| `agent-docs/adr/2026-11-24-strategy-pilot-ablation.md` | 文書 | 31 | 0 |
| `agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md` | 文書 | 4 | 3 |
| `agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json` | 記録 | 37 | 0 |
- crates・web・評価器の prompt・testdata・compare・driver は変えていない。production の Rust・TS の LoC は 0 行の差。
- script の純増 80 行は、人の決定の記録を読む口（`load_decision`・`decision_problems`・`graded_axes`）、model の採点者の分岐、tally の model の表示、docstring の規則の文の分。

## 未確認・限界（close-audit が報告に写す）
- 4 軸は人ではなく model（`claude-fable-5-1`）の採点である。人の独立の採点は無い。
- 評価器（`claude-sonnet-5-5`）を同じ provider の model（Fable）が採点した。同じ provider の model の癖を共有している可能性を除けていない。
- 採点欄のレビュー時間（分）0.93 は subagent の wall 335.7 s を 6 表で等分した値で、人のレビュー時間ではない。task の指標「発表に要る人のレビュー時間」は測れていない。
- 採点者が data.json だけを読み、対応表・answer key を開いていないことは、operator が subagent の transcript で確かめたもの。この WU は transcript を見ていない（close-audit が transcript を確かめるかは close-audit の範囲）。
- `scorer-decision.json` の値（wall・tool の数・sha256・時刻）は operator の報告（task の人の回答の欄）から写した。この WU が確かめたのは、書き写しの後の sha256 が今の `$ART/pilot-scoring.md` と一致することだけ。data.json と書き写しの前の file は見ていない。
- machine-axes・tally は実データに当てていない（close-audit の仕事）。

## 提案
- 子 9 で model の採点者（人が決めた model が 4 軸を付ける）を使うかは人の確認待ち（rubric の手順 1）。close-audit が子 8 の task の進捗の「提案」に写し、人に聞く。
- 子 9 で人の独立の採点を 1 事例だけでも取るかを人に聞く。今は model の採点だけで、同じ provider の model で評価器を採点した限界を外せない。
