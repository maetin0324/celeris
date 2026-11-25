---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: pilot-materials
status: done
updated: 2026-11-25
completed: 2026-11-25
---
# 戦略層 v1 子 8 pilot-materials: operator の実 LLM の出力から盲検の採点材料 pilot-scoring.md を作る

> **採点の前に読まないでください。** 下の「run の一覧」は方式の名と run ごとの USD を書くので、`pilot-scoring.md` の記録（id・反復・時点・USD）と突き合わせると id と方式の対応が分かります。人は採点を書き終えてからこの file を開いてください（rubric の手順 2）。同じ理由で、先行 WU の進捗（`compare-reads.md` の「方式ごとの検知」など）も採点の前に読まないでください。

## 結論
- 人が採点する材料を root の artifacts に作った: `pilot-scoring.md`（801 行）と `pilot-key.json`（sha256 `36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace`）。`check-blank` は exit 0（採点欄は空、方式の名は無い）。`pilot-key.json` は commit していない。
- repo 用の写し `agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md`（784 行）を置いた。研究ケースの理由・主張・版の文を除いてあり、`check-blank` は exit 0。
- 材料の元の指標は `$ART/pilot-metrics/metrics.json`（compare `--pilot` の exit 0、`pass: true`、漏れ 0、`excluded` 0、`failed` 0）。`pilot-compare-v4/metrics.json` と同じ byte。
- 材料は 36 点（2 反復 × 2 事例 × 3 時点 × 3 方式）。評価の記録の status は checked 20・invalid 4・記録無し 12（親の途中評価を持たない方式の 12 点）。invalid の 4 点には、材料に「判定と次の手は gate の既定値で、評価器の答えではない」の行が出る（4 件）。
- この WU は採点していない（点を書いていない）。実 LLM の run は起こしていない。合格線は rubric.md のまま、材料の「合格線（rubric.md から）」節にそのまま載っている。

## 手順（再現 command）
`ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`、repo の root で実行した。

```
python3 -I scripts/dev/parent-eval-blind-compare.py --pilot --runs $ART/pilot-runs/iter-1 --runs $ART/pilot-runs/iter-2 \
  --research-src /local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md \
  --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --out $ART/pilot-metrics
# exit 0。cmp $ART/pilot-metrics/metrics.json $ART/pilot-compare-v4/metrics.json → 同じ byte

python3 -I scripts/dev/parent-eval-blind/pilot_score.py blind --runs $ART/pilot-runs/iter-1 --runs $ART/pilot-runs/iter-2 \
  --metrics $ART/pilot-metrics/metrics.json --out $ART/pilot-scoring.md --key-out $ART/pilot-key.json \
  --repo-copy agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md
# exit 0。--seed は渡していない（id と並びは secrets.SystemRandom で決まり、同じ command でも同じ byte にならない）

python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-blank $ART/pilot-scoring.md                                   # exit 0
python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-blank agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md  # exit 0
python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-scored $ART/pilot-scoring.md                                  # exit 1（採点前なので期待どおり）
```

- 人の採点の後: close-audit が `pilot_score.py check-scored` と `tally --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --metrics $ART/pilot-metrics/metrics.json` で開封する（`tally` は `pilot-key.json` の sha256 が材料の値と一致することを先に確かめる）。

## 承認と上限
- 承認: `$ART/pilot-approval.json` の `approval_id` `strategy-v1-8-pilot-2026-11-24`、`approved_by` `human:rmaeda`、`approved_at` 2026-11-24T08:59:00Z、`expires_at` 2026-12-01T00:00:00Z。account `claude_max_personal`、model `claude-sonnet-5-5`、harness `claude_code`、`scope.environment` `non-production`、`limits` `max_usd` 25・`max_wall_secs` 600。run の数の上限 48 は ADR D3.1・D3.3 の値（承認の json には欄が無い）。
- 使った量: 34 run・8.6335 USD（上限 48 run・25 USD の内）。内訳は次の 2 つ。
  - `pilot-runs` の中の 32 run・8.300332 USD（下の表と `metrics.json` の `usage` の `llm_runs` 32・`usd` 8.300332 が一致）。
  - 方式 c の版 v1 の段で失敗した 2 run・0.333 USD（operator の報告の値）。出力は `/local/celeris/data/scratch/child8-pilot/failed-arm-c/` に移されていて `run.json` が無く、この WU では USD を確かめていない（log には driver が `strategy/pilot-replay/v1.json` が無いとして止まった記録がある）。
- 承認の残り（14 run・約 16.4 USD）は使わない（人の決定 2026-11-25）。

## run の一覧（`pilot-runs` の `run.json`。USD は `cost_usd` を 6 桁に丸めた値）
「段」は build の時期。段 harness の後 = 2026-11-24 14:4xZ（operator の報告では段 harness の統合の後の commit の git worktree）、段 recheck の後 = 2026-11-25 02:1xZ〜02:5xZ（operator の報告では integrate wu/stub-undeclared の後の branch）。どちらも出力の file の時刻と operator の報告で、build の commit はこの WU では確かめていない。方式 a は LLM を呼ばない（`run.json` の `llm_runs` 0）。

| 反復 | 事例 | 時点 | 方式 | model | USD | run の status | 評価の記録 | 段 |
|---|---|---|---|---|---|---|---|---|
| iter-1 | replay | 版 v1 の段（worker） | c | claude-sonnet-5-5 | 0.307576 | completed | — | 段 recheck の後 |
| iter-1 | replay | 版 v1 の段（Critic） | c | claude-sonnet-5-5 | 0.31957 | completed | — | 段 recheck の後 |
| iter-1 | replay | C-mid | a | — | 0 | run 無し | 記録無し | 段 harness の後 |
| iter-1 | replay | C-mid | b | claude-sonnet-5-5 | 0.11134 | completed | checked | 段 harness の後 |
| iter-1 | replay | C-mid | c | claude-sonnet-5-5 | 0.347255 | completed | checked | 段 recheck の後 |
| iter-1 | replay | D-done | a | — | 0 | run 無し | 記録無し | 段 harness の後 |
| iter-1 | replay | D-done | b | claude-sonnet-5-5 | 0.122459 | completed | checked | 段 harness の後 |
| iter-1 | replay | D-done | c | claude-sonnet-5-5 | 0.205153 | completed | checked | 段 recheck の後 |
| iter-1 | replay | F-done | a | — | 0 | run 無し | 記録無し | 段 harness の後 |
| iter-1 | replay | F-done | b | claude-sonnet-5-5 | 0.128982 | completed | checked | 段 harness の後 |
| iter-1 | replay | F-done | c | claude-sonnet-5-5 | 0.294978 | budget_exhausted | **invalid** | 段 recheck の後 |
| iter-1 | research | 版 v1 の段（worker） | c | claude-sonnet-5-5 | 0.267704 | completed | — | 段 recheck の後 |
| iter-1 | research | 版 v1 の段（Critic） | c | claude-sonnet-5-5 | 0.275967 | completed | — | 段 recheck の後 |
| iter-1 | research | 11.16 | a | — | 0 | run 無し | 記録無し | 段 recheck の後 |
| iter-1 | research | 11.16 | b | claude-sonnet-5-5 | 0.260167 | completed | checked | 段 recheck の後 |
| iter-1 | research | 11.16 | c | claude-sonnet-5-5 | 0.330199 | budget_exhausted | **invalid** | 段 recheck の後 |
| iter-1 | research | 11.17 | a | — | 0 | run 無し | 記録無し | 段 recheck の後 |
| iter-1 | research | 11.17 | b | claude-sonnet-5-5 | 0.392292 | completed | checked | 段 recheck の後 |
| iter-1 | research | 11.17 | c | claude-sonnet-5-5 | 0.391539 | completed | checked | 段 recheck の後 |
| iter-1 | research | 11.20 | a | — | 0 | run 無し | 記録無し | 段 recheck の後 |
| iter-1 | research | 11.20 | b | claude-sonnet-5-5 | 0.153369 | completed | checked | 段 recheck の後 |
| iter-1 | research | 11.20 | c | claude-sonnet-5-5 | 0.264775 | completed | checked | 段 recheck の後 |
| iter-2 | replay | 版 v1 の段（worker） | c | claude-sonnet-5-5 | 0.241697 | completed | — | 段 recheck の後 |
| iter-2 | replay | 版 v1 の段（Critic） | c | claude-sonnet-5-5 | 0.219272 | completed | — | 段 recheck の後 |
| iter-2 | replay | C-mid | a | — | 0 | run 無し | 記録無し | 段 harness の後 |
| iter-2 | replay | C-mid | b | claude-sonnet-5-5 | 0.110532 | completed | checked | 段 harness の後 |
| iter-2 | replay | C-mid | c | claude-sonnet-5-5 | 0.172584 | completed | checked | 段 recheck の後 |
| iter-2 | replay | D-done | a | — | 0 | run 無し | 記録無し | 段 harness の後 |
| iter-2 | replay | D-done | b | claude-sonnet-5-5 | 0.137557 | completed | checked | 段 harness の後 |
| iter-2 | replay | D-done | c | claude-sonnet-5-5 | 0.352743 | budget_exhausted | **invalid** | 段 recheck の後 |
| iter-2 | replay | F-done | a | — | 0 | run 無し | 記録無し | 段 harness の後 |
| iter-2 | replay | F-done | b | claude-sonnet-5-5 | 0.143106 | completed | checked | 段 harness の後 |
| iter-2 | replay | F-done | c | claude-sonnet-5-5 | 0.371369 | budget_exhausted | **invalid** | 段 recheck の後 |
| iter-2 | research | 版 v1 の段（worker） | c | claude-sonnet-5-5 | 0.350148 | completed | — | 段 recheck の後 |
| iter-2 | research | 版 v1 の段（Critic） | c | claude-sonnet-5-5 | 0.366132 | completed | — | 段 recheck の後 |
| iter-2 | research | 11.16 | a | — | 0 | run 無し | 記録無し | 段 recheck の後 |
| iter-2 | research | 11.16 | b | claude-sonnet-5-5 | 0.212655 | completed | checked | 段 recheck の後 |
| iter-2 | research | 11.16 | c | claude-sonnet-5-5 | 0.265711 | completed | checked | 段 recheck の後 |
| iter-2 | research | 11.17 | a | — | 0 | run 無し | 記録無し | 段 recheck の後 |
| iter-2 | research | 11.17 | b | claude-sonnet-5-5 | 0.30057 | completed | checked | 段 recheck の後 |
| iter-2 | research | 11.17 | c | claude-sonnet-5-5 | 0.294974 | completed | checked | 段 recheck の後 |
| iter-2 | research | 11.20 | a | — | 0 | run 無し | 記録無し | 段 recheck の後 |
| iter-2 | research | 11.20 | b | claude-sonnet-5-5 | 0.29331 | completed | checked | 段 recheck の後 |
| iter-2 | research | 11.20 | c | claude-sonnet-5-5 | 0.294647 | completed | checked | 段 recheck の後 |

- 方式ごとの計（`pilot-runs` の中）: 方式 a 0 run・0 USD。方式 b 12 run・2.366339 USD（replay 6 run・0.753976、研究 6 run・1.612363）。方式 c 20 run・5.933993 USD（評価 12 run・3.585927〈replay 1.744082・研究 1.841845〉、版 v1 の段 8 run・2.348065〈replay 1.088115・研究 1.259951〉）。計 32 run・8.300332 USD。
- iter-2 の研究ケースは、3 時点を 1 回で流すと上限の検査を通らない（operator の報告: 6.26 + 3.0 × 7 = 27.26 > 25）ので、時点ごとに分けて流した。流し直した点は無い。

## invalid の 4 点（方式 c）
| 反復 | 事例・時点 | run の status（daemon.log） | 記録の violations | 材料に出る判定・次の手 |
|---|---|---|---|---|
| iter-1 | replay F-done | budget_exhausted(Turns) | `shape parent_evaluation: 節が無い` | unknown・ask_human（gate の既定値） |
| iter-1 | research 11.16 | budget_exhausted(Turns) | `shape parent_evaluation: 節が無い` | unknown・ask_human（gate の既定値） |
| iter-2 | replay D-done | budget_exhausted(Turns) | `shape parent_evaluation: 節が無い` | unknown・ask_human（gate の既定値） |
| iter-2 | replay F-done | budget_exhausted(Turns) | `shape parent_evaluation: 節が無い` | unknown・ask_human（gate の既定値） |

- 評価の run が turn の上限（30）で終わり、`parent_evaluation` の節を書けなかった。人の決定（2026-11-25）どおり流し直さず、埋めず、そのまま材料に入れた。
- 材料（`$ART/pilot-scoring.md` と repo 用の写し）には、この 4 点の記録に「評価の記録の状態: invalid（run: budget_exhausted、shape parent_evaluation: 節が無い）。判定と次の手は gate の既定値で、評価器の答えではない」の行が出る（`grep -c '評価の記録の状態: invalid'` が両 file とも 4）。この行は方式の名を出さない。

## 漏れ・secret・mutation の検出数（36 点）
| 項目 | v3（compare-reads の前の script、`pilot-compare-v3`） | 今（compare-reads の後、`pilot-metrics`＝`pilot-compare-v4`） |
|---|---|---|
| compare の exit・`pass` | 1・false | 0・true |
| 漏れ（`leaks`） | 16 件（8 点） | 0 |
| 採点から外す点（`excluded`） | 8 | 0 |
| 失敗した点（`failed`） | 0 | 0 |
| secret の形（入力・出力） | 0・0 | 0・0 |
| 記録の入力の変更（`mutation.input_changed`） | 0 点 | 0 点 |
| `mutation.mutated`（入力の変更 または review.json の外への書き込み） | 15 点 | 15 点 |

- 漏れの 16 件は、どれも実際の読みではなかった（6 件は書き込みの heredoc の本文の `c-results.md`、10 件は版の path `pilot-replay/` と禁じた名 `replay/` の部分一致）。分類は `compare-reads.md` の表。
- `mutated` が真の 15 点は、記録の入力（`input/files.json` と `input/tree/`）を変えていない。中身は次の 3 種類（`reads.json` の command と `mutation()` の `REDIRECT_RE` で 1 件ずつ見た）。
  - 11 点: 評価の run が自分の作業 tree に `result.json`（worker の結果の file）を書いた（`cat > result.json <<'EOF'` など）。
  - 2 点: 評価の run が `$TMPDIR` に一時 file を書いた（`> $TMPDIR/ev.txt`・`> $TMPDIR/rv.txt`・`cat > $TMPDIR/gen.py`）。
  - 2 点: Python の比較の `>`（`e['seq']>1000:`）とコメントの `P(X>=a)` を `REDIRECT_RE` が redirect と読んだ誤検出だけ。
  - 上の 11 点のうち 2 点と `$TMPDIR` の 1 点は、同じ種類の誤検出（heredoc の本文の `>` の後の字句 `の`・`.csv.gz`・`.jsonl`、Python の式 `last[e['task']]`）も `writes` に含む。
  - 本番の DB・daemon・config への書き込みは、どの点の書き先にも無い（書き先は run の作業 tree・`$TMPDIR`・誤検出の字句だけ）。
- 材料の「決定的な指標」の「記録の mutation 有/無」と表の mutation の列は、この `mutated` の値をそのまま出す。15 点の「有」は記録の改変ではない。rubric の 6 軸に mutation は無い。

## 方式 b・c の入力に「（Outcome の記録は無い）」が無いこと
- `grep -rlF '（Outcome の記録は無い）' $ART/pilot-runs` は一致無し（exit 1）。対象は `pilot-runs` の全体（方式 b・c の `input/prompt.md` 24 件を含む）。
- 方式 b・c の `input/prompt.md` 24 件はどれも `### 親の Outcome` の節を持つ（`grep -lF '### 親の Outcome' … | wc -l` が 24）。評価器の入力の修正（段 prep の eval-input・eval-seed）の後の入力で、ADR D8 の「修正の前の入力」に当たる点は無い（`metrics.json` の `pre_fix_input` も全点 false で、`excluded` 0）。

## この材料が段 fix の 3 件と compare-reads の後であること
- 段 fix（統合済み）の 3 件:
  - research-goal: 研究ケースの goal と Outcome を 11.15 節より前の task C の問いから作る。研究ケースの 18 点はどれも段 recheck の後の run（この修正の後の driver）。
  - compare-leak: 時点以前にも同じ本文がある後の event を漏れに数えない。compare の規則で、この WU の compare（`pilot-metrics`）は修正の後の script。
  - artifact-name: 方式 c の版の名を artifacts からの相対 path にする。方式 c の版 v1 の段と評価の 12 点は段 recheck の後の run。
  - replay の方式 a・b の 12 点は段 harness の後の run で、段 fix の 3 件はどれも replay の方式 a・b の入力に関わらない（research-goal は研究ケース、artifact-name は方式 c、compare-leak は compare の側）。
- compare-reads（段 reads、統合済み）: `check_reads` の誤検出の修正と、`pilot_score.py` の invalid の行。この WU の compare と blind はこの修正の後の script（WU の base の commit）で流した。

## repo 用の写しの確認
- 研究ケースの記録の「理由」「主張」「版の文」は「（研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）」に替わっている。研究ケースの記録の根拠の欄は task id と成果物の名・sha256（`name=strategy/pilot-research/v1.json sha256=…`）だけ。
- `c-results.md` との 20 字の一致は、Markdown の表の区切り（`|---|---|…`）だけ（11 個）。研究ケースの入力の tree（`input/tree/` の .md・.json・.jsonl）との 30 字の一致は、表の区切りと成果物の sha256 の 64 桁だけ。
- `pilot-key.json` は repo に無い（`git ls-files 'agent-docs/progress/2026-11-24-strategy-v1-8/pilot-key*'` が空）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 結果は下の「提出時の検査」節。

## LoC
- 変更は文書だけ（この進捗 file と repo 用の写し `pilot-scoring.md`）。script・試験・crates・web・評価器の prompt・testdata は変えていない。

## 未確認
- 盲検の強さ: 方式の名は伏せたが、記録の内容（親の途中評価の本文の有無・戦略の版の有無・invalid の行）から方式を推測できる（ADR D5 の既知の限界）。invalid の 4 点が方式 c であることは、この WU の objective と operator の報告にも書かれている。採点の前に人がどこまで知っていたかは、採点の後に close-audit で人に聞く必要がある。
- 方式 c の版 v1 の段の失敗の 2 run・0.333 USD は operator の報告の値で、この WU では確かめていない（`run.json` が無い）。
- build の commit（段 harness の後 = 段 harness の統合の後の worktree、段 recheck の後 = integrate wu/stub-undeclared の後）は operator の報告で、この WU では確かめていない。
- `REDIRECT_RE` は Python の比較の `>` やコメントの `>=` を redirect と読む（`mutated` の誤検出 2 点）。漏れと採点には効かないので、この WU では直していない（範囲の外）。
- 採点・合格線の判定・指標の表は、人の採点の後の close-audit の仕事で、この WU ではしていない。
