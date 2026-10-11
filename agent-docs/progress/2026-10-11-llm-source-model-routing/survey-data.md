---
task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J
unit: survey-data
status: done
completed: 2026-10-11
---
# survey-data: 本番 DB の run・review・routing events の集計

## やったこと
- 本番 DB を `mode=ro`（+ `PRAGMA query_only=ON`）で読み、run ごとの結果（完了・WU check・review 判定・LLM judge）、所要時間、tokens・費用、quota を genre×harness×source×model で集計する script を task の成果物置き場に置いた: `artifacts/survey/prod_data.py`（task workspace の artifacts。repo には置かない）。
- 出力: `artifacts/survey/prod-data.json`、`prod-data-cells.md`（全 84 cell、Wilson 95% 区間つき）、`prod-data.md`（要点と判断）。

## 要点
- worker 3,523 run のうち判定が付くのは 1,391（39%）、LLM reviewer の判定は 432（12%）。
- coding の主要 model の pass 率は Wilson 区間が重なり順位が付かない（分かれるのは opus-5-5 0.72 [0.67,0.77] と gpt-6-luna 0.53 [0.46,0.59] だけ）。
- legacy は決定的（lane → model がほぼ固定）で、optimizer の候補 13,439 行・shadow 1,183 件のどれにも propensity が無い。shadow 候補は実行されず結果が無い → オフライン方策評価には使えない。
- 同じ work unit で別 model が走った組は 54 で、全部「失敗後に model を変えた」逐次の組。独立な対照比較は無い → harness 固定の反復評価で作る必要がある。

## 証拠
- `python3 -I .../artifacts/survey/prod_data.py --db 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' --out .../artifacts/survey/prod-data.json` → exit 0、`runs=5039, cells=84`、約 2 秒。
- コード差分なし（crates/ web/ gui/ を変えていない）。

## 未解決
- genre が空の task が 262 件あり、genre 別の集計で `(none)` にまとめている。
- codex・qwen・opencode-go の費用は USD で出ない（tokens と quota 推定のみ）。

## 提案
- 選択時に候補ごとの選択確率を記録し、一部を確率的な探索として実行しない限り、本番記録はオフライン評価の根拠にならない（survey-report / adr で扱う）。
