---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: close-audit
status: done
updated: 2026-11-27
completed: 2026-11-27
---
# 戦略層 v1 子 8 close-audit: model の 4 軸の採点を開封し、機械の 2 軸を付けて集計し、指標・LoC・限界・未確認・寄与を task の進捗と ADR に書く

## 結論
- `check-graded --decision` が通ったので開封した。機械の 2 軸を `machine-axes` で付け、`check-scored --decision` と `tally --decision` を流した。どれも exit 0。
- 方式 b・c とも rubric.md の合格線を満たさない（合計は a 5・b 10・c 11。両事例の条件は replay で偽、研究ケースで真）。合格を実証できなかったので rollout しない。本番の controller の checkpoint は mode shadow のまま。
- 4 軸の点は書き換えていない。代わりに採点していない。Fable の採点も実 LLM の評価器の run も流し直していない。採点者は `model:claude-fable-5-1` のままで、`human:` と偽っていない。
- task の進捗 `agent-docs/progress/2026-11-24-strategy-v1-8.md` に採点の結果・限界・機械の軸・指標・再現コマンド・変更前に落ちる試験・LoC・回帰の条件・評価器の入力の修正・人の確認で直したこと・評価器の質・未確認・寄与・提案を書いた。
- `pilot-key.json` の写しを [pilot-key.json](pilot-key.json) に置いた（元と同じ byte）。ADR に D12 付記（結果と rollout の判断）を足した。

## 変更（commit の件名）
- `strategy-v1 8 close-audit: 開封・集計し、採点の結果・指標・LoC・限界・未確認・寄与を task の進捗と ADR に書く`
  - `agent-docs/progress/2026-11-24-strategy-v1-8.md`: 節を書いた（上の結論）。front matter の status は running のまま（close-verify が残る）。
  - `agent-docs/progress/2026-11-24-strategy-v1-8/pilot-key.json`: `$ART/pilot-key.json` の写し。
  - `agent-docs/progress/2026-11-24-strategy-v1-8/close-audit.md`: この file。
  - `agent-docs/adr/2026-11-24-strategy-pilot-ablation.md`: D12 付記。
- crates・web・control-ts・script・試験・testdata・rubric.md は変えていない。

## root の artifacts に書いたもの（`ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`）
- `$ART/pilot-scoring.md`: machine-axes が機械の 2 行 × 6 表の 12 行だけを埋めた。前の sha256 `02f9b38b27c9e1ef9d349cc3a34fadc4ff224be86fb094d65cdc577a872a5d30`、後 `17b5d734fd764f29979bb108719dbd69a7ff9314d3cd9bfe7b50b4af402bf7ee`。
  前の file の写しはこの run の artifacts の `before/pilot-scoring.md` に置いた（run の中間物）。
- `$ART/pilot-machine-axes.json`（sha256 `aa45c883de3017289964e4358180b3399a9428368a10e17d28f88f9d522417b3`）、`$ART/pilot-tally.json`（sha256 `d13c3a8fc8810c760ec89498ee00bd65d6ded71018974736c9f1899113fdcd77`）。
- `$ART/pilot-key.json` は変えていない（sha256 `36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace`）。

## 流した command と結果（repo の root。`S=scripts/dev/parent-eval-blind/pilot_score.py`、`D=agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json`）
- `python3 -I $S check-graded $ART/pilot-scoring.md --decision $D` → exit 0。`grep -c '^- 採点者: model:claude-fable-5-1$'` は 6、`^- 採点者: human:` は 0。
- `python3 -I $S machine-axes --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --metrics $ART/pilot-metrics/metrics.json --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --decision $D --out $ART/pilot-machine-axes.json` → exit 0。
  出力: replay の 3 id は pivot の時点 0・コスト 0、研究ケースの方式 b・c は pivot の時点 2・コスト 0、方式 a は 0・0。
- `python3 -I $S check-scored $ART/pilot-scoring.md --decision $D` → exit 0。
- `python3 -I $S tally --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --decision $D --out $ART/pilot-tally.json` → exit 0。「合格線 model:claude-fable-5-1 方式 b: 満たさない（両事例の条件 False、a との差 5、失格 False）」「方式 c: 満たさない（両事例の条件 False、a との差 6、失格 False）」。
- 冪等の確認（受け入れ条件 1）: `$ART/pilot-scoring.md` を `mktemp -d` に写して machine-axes を当て直し、`cmp` で同じ byte（exit 0）。`--out` の json も同じ byte。
- LoC: base（`git merge-base HEAD main`）の `source-size-report.py` と `.toml` を `git show` で取り出し、`--exclusive-classes --rev <rev> --format json` を base と HEAD に当てた（どちらも exit 0）。
  `views.product_only.lines` 393,338 → 393,415（+77）、`views.product_only.rust_lines` 346,168 → 346,245（+77）、`matrix.production.typescript.lines` 36,035 → 36,035（0）、`views.own_total_with_tests.lines` 1,082,712 → 1,087,451（+4,739）。
- 指標: `$ART/pilot-metrics/metrics.json`（`pass: true`、`leaks` 0、`excluded` 0、`failed` 0、`usage` 32 run・8.300332 USD）と各点の `output.json`・`reads.json` を読んで、task の進捗の「指標」「評価器の質」の表を作った。

## 受け入れ条件の確かめ（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- 0: 受け入れ条件 0 の command（sha256 の一致、check-graded、check-scored、採点者の行が 6、`human:` が 0、tally、2 つの json が空でない）→ exit 0。
- 1: 受け入れ条件 1 の command（写しに machine-axes を当て直して `cmp`）→ exit 0。
- 2: 受け入れ条件 2 の command（task の進捗の節と語、close-audit.md と pilot-key.json の有無）→ exit 0。
- 3: 受け入れ条件 3 の command（change_rules・control-ts・web の差分 0、crates の production の差分が eval-input と artifact-name の範囲だけ）→ exit 0。
- 4: `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0。

## 検査（提出時の HEAD で実行）
- 結果は task の報告（result.json の evidence）に書く。この file を書いた後に流した。
- 同じ dir の script の試験: `test_pilot_score.py` 26 件・`test_driver.py` 19 件・`test_pilot_compare.py` 20 件・`test_research.py` 16 件、どれも exit 0。

## 未確認・限界
- 4 軸は model の採点で、人の独立の採点は無い。評価器（claude-sonnet-5-5）を同じ provider の model（Fable）が採点した。レビュー時間 0.93 分は model の wall で人の時間ではない。
- 採点者が data.json だけを読んだことは operator の申告。close-audit は transcript を見ていない。
- そのほかは task の進捗の「未確認の事項」。

## 提案
- task の進捗の「提案」に書いた（子 9 の採点の分担は人の確認待ち）。
