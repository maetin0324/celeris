# 実 LLM pilot の raw（A9、2026-11-05 operator が縮小版を実測）

2026-11-05 に operator が A9 の縮小版を実測した。`scripts/dev/long-horizon-pilot-llm.sh --harness config` で C06 と C08 を new 腕・seed 1 で 1 回ずつ走らせた（2 試行）。
この directory は `/local/celeris/data/scratch/a9-pilot/raw-20261105/` を cp -a でそのまま写したもので、raw の中身は書き換えていない（この README だけ書き直した）。
stub harness の raw はここに書かない（`../stub/`）。計画と未解決は `../../a9.md`。

## 結果の要約

2 試行とも、controller は achieved に達した。採点は 2 試行とも fail で、到達率は 0。
- controller 側: 2 試行とも rules が `pc/achieved` で終わり、root はすべて done（C06 は 2 本、C08 は 4 本）。
- 採点側: grade/summary.json の `reach_rate` `case_reach_rate` `autonomous_reach` はいずれも 0.0、`verdict_pass` は 0、`trials` は 2。

| case | 試行 | controller | 採点 | token | wall（秒） | LLM 起動（worker / reviewer / cos） | fail した predicate |
|---|---|---|---|---|---|---|---|
| C06 | 49a77b52eb2fcefa | achieved | fail | 3864 | 98 | 8（2 / 2 / 4） | `outcome_reached(o1)`、`effect_count(C06:o1:publish) == 1`（effects は 0 件） |
| C08 | 7420dfe789f99af7 | achieved | fail | 14557 | 282 | 18（5 / 5 / 8） | `outcome_reached(o1)`〜`outcome_reached(o4)` の 4 件 |
| 計 | | | | 18421 | 380 | 26 | |

数値の出典は grade/summary.json（tokens 18421、wall_secs 380）、grade/grades.jsonl の `llm_invocations`、ops/ の各試行 json（試行ごとの wall_secs、rules、root_statuses）。tokens は 2 試行の合計が summary と一致する（3864 + 14557）。LLM 起動数は grades.jsonl の合計を足したもので、summary.json には無い。

効果の数（`effects`）は C06 の facts で空だった。原因は未確認で、2026-11-05 の fixes task（`../../../2026-11-05-a9-llm-pilot-fixes/investigate.md`）で調べる。

## raw の構成

- `arm-map/<trial>.json` — 試行 id から腕・case・seed への対応（2 件）
- `facts/<trial>.json` — 試行の事実（roots、effects、human_ops、faults、invocations と usage）
- `grade/` — `grades.jsonl`（試行ごとの predicate と metrics）と `summary.json`（腕ごとの集計）
- `ops/` — 試行ごとの操作記録 `<trial>.json`（controller の rules・state 遷移）と pilot のログ `C06-new.log`、`C08-new.log`
- `trials/<trial>/repo/` — 試行が終わった時点の課題 repo（`README.md`、`minichunk.py`、`out/`）。C06 は `out/o1.txt`〜`o2.txt`、C08 は `out/o1.txt`〜`o4.txt`
