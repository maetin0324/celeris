---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: LLM source・model routing 評価結果（coding、7 model 中 6 model）
status: done
updated: 2026-10-14
---
# LLM source・model routing 評価結果

- manifest sha256: `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7`（manifest-v2、問題 30 件、cutoff 2026-10-05）
- evidence_version: `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14`
- 集計: `celeris-routing-eval aggregate`（Wilson 95%、問題単位 bootstrap 95% seed 42・2000 回）。n は有効な結果のある問題数、pass は各問題の最初の有効試行（pass^1）の合格数。
- 中間物（repo の外）: `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/`（aggregate/・export.jsonl・batches.json・retry-batches-codex.json・retry-batches-1.json・retry-batches-2.json・pilot2-batch.json）

## 試行の状態

| 区分 | batch | 群 | 状態 |
|---|---|---|---|
| 本評価 | `01M4W9H8WDQG…` | claude | error 2・passed 46 |
| 本評価 | `01M4W9H8WNXE…` | claude | error 9・passed 39 |
| 本評価 | `01M4W9H8WW9W…` | claude | error 2・failed 2・passed 44 |
| 本評価 | `01M4W9H8Z511…` | codex | cancelled 31・error 1・failed 3・passed 25 |
| 本評価 | `01M4W9H8ZDWY…` | codex | cancelled 50 |
| 本評価 | `01M4W9H8ZKD0…` | codex | cancelled 12 |
| 本評価 | `01M4W7JDSJVG…` | pilot | failed 4・passed 6 |
| codex cancel 分の再投入（上限 21%） | `01M4WZH402D7…` | retry-codex | error 2・failed 8・passed 40 |
| codex cancel 分の再投入（上限 21%） | `01M4WZH409YW…` | retry-codex | failed 1・passed 11 |
| codex cancel 分の再投入（上限 21%） | `01M4X08HKH9B…` | retry-codex | error 2・failed 2・passed 27 |
| 仕組み側 error の再投入 1 回目 | `01M4XBEQC26Y…` | retry-1 | error 2 |
| 仕組み側 error の再投入 1 回目 | `01M4XBEQC38Y…` | retry-1 | error 1・passed 1 |
| 仕組み側 error の再投入 1 回目 | `01M4XBEQC5TC…` | retry-1 | passed 9 |
| 仕組み側 error の再投入 1 回目 | `01M4XBEQC65H…` | retry-1 | error 2 |
| 仕組み側 error の再投入 1 回目 | `01M4XBEQC79J…` | retry-1 | passed 3 |
| 仕組み側 error の再投入 2 回目 | `01M4XDW25VZW…` | retry-2 | error 2 |
| 仕組み側 error の再投入 2 回目 | `01M4XDW25WKJ…` | retry-2 | error 1 |
| 旧 pilot（集計除外） | `01M4W35QEMQG…` |  | error 10 |
| pilot やり直し（本評価の試行と同じ。合計に数えない） | `01M4W7JDSJVG…` |  | failed 4・passed 6 |

合計: cancelled 93・error 36・failed 20・passed 251。非終端 0。
error・cancelled は n・pass・区間に数えない（除外）。除外数は cell 表の『除外』列。

## cell ごとの結果（genre × harness × source × model）

| genre | harness | 版 | source | model | 計画 問題×k | n | pass | Wilson 95% | bootstrap 95% | 定価換算 USD 中央値（合計） | token 中央値 in/out/cache-read | 壁時計 中央値 s | timeout 率 | low_confidence | 除外 error/cancelled | yielded 連続 |
|---|---|---|---|---|---|---:|---:|---|---|---|---|---:|---:|---|---|---:|
| coding | claude-code | 2 | claude-oauth | claude-opus-5-5 | 24×2 | 24 | 24 | 0.862–1.000 | 1.000–1.000 | 0.757（41.42） | 38/15767/788143 | 236.226 | 0.000 | true | 4/0 | 1 |
| coding | claude-code | 2 | claude-oauth | claude-sonnet-5-5 | 24×2 | 24 | 24 | 0.862–1.000 | 1.000–1.000 | 0.268（20.46） | 18/8248/344786 | 103.160 | 0.000 | true | 9/0 | 0 |
| coding | claude-code | 2 | claude-oauth | claude-haiku-5-5 | 24×2 | 23 | 22 | 0.790–0.992 | 0.870–1.000 | 0.349（17.46） | 46/36589/1419094 | 263.867 | 0.000 | true | 6/0 | 2 |
| coding | codex | 0 | codex-oauth | gpt-6.1-sol | 30×2 | 30 | 27 | 0.744–0.965 | 0.800–1.000 | — | 901011/10054/819968 | 354.333 | 0.000 | true | 3/31 | 0 |
| coding | codex | 0 | codex-oauth | gpt-6-astra | 12×1 | 12 | 11 | 0.646–0.985 | 0.750–1.000 | — | 427092/6430/380224 | 209.367 | 0.000 | true | 0/12 | 0 |
| coding | codex | 0 | codex-oauth | gpt-6-luna | 30×2 | 29 | 21 | 0.543–0.853 | 0.552–0.862 | — | 534188/6272/478080 | 76.212 | 0.000 | true | 14/50 | 2 |

除外 error/cancelled は cell の全試行（再投入で置き換えた元の試行と旧 pilot を含む）の数。yielded 連続は、仕組み側 error の再投入（試行ごと最大 2 回）の最終世代でもまた `run ended as yielded` で終わった試行の数。

### yielded の扱い（人の回答 2026-10-14）

2 回以上続けて yielded の試行は除外とせず『yielded 連続』列で cell ごとに数える。runner の n・pass・区間（Wilson・bootstrap）は passed/failed だけから作るので、これらは n に入っていない。全部が同じ 1 問（予算内に終えられない問題）で、claude は 3 回連続（初回・再投入 1 回目・2 回目）、gpt-6-luna は 2 回連続（cancel 分の再投入・再投入 1 回目。codex の上限 21% に近いので 2 回目は見送り）。
- `claude-oauth:claude-haiku-5-5:claude-code`: yielded 連続 2 試行（有効な結果の無い問題 1 問）。yielded を不合格として n に足すと 22/24、Wilson 下限 0.742（現行 0.790）。下限 0.5 に対する成否は変わらない。
- `claude-oauth:claude-opus-5-5:claude-code`: yielded 連続 1 試行（有効な結果の無い問題 0 問）。yielded を不合格として n に足すと 24/24、Wilson 下限 0.862（現行 0.862）。下限 0.5 に対する成否は変わらない。
- `codex-oauth:gpt-6-luna:codex`: yielded 連続 2 試行（有効な結果の無い問題 1 問）。yielded を不合格として n に足すと 21/30、Wilson 下限 0.521（現行 0.543）。下限 0.5 に対する成否は変わらない。

### runner の集計の修正（この collect で実施）

- `celeris-routing-eval aggregate`・`export` は評価の route_decisions を route_outcomes と LEFT JOIN して読み、outcome の無い試行（error・cancelled。run は yielded・cancelled・question・旧 pilot の completed）を **不合格として n に数えていた**。ADR D1 の『有効な結果』に反するので、outcome のある試行（passed/failed）だけを読むよう `crates/routing-eval/src/aggregate.rs` を直した runner（LEFT JOIN → JOIN、試験 `collect_reads_eval_decision_outcome_and_run_from_fixture_db` に outcome の無い error 試行を追加）を手元で build して aggregate・export をやり直した。この collect の範囲は agent-docs/progress/ だけなので、修正はリポジトリに入れず差分を `E/aggregate-error-exclude.patch` に残し、取り込みは follow-up task に出した。取り込まれるまで main の runner で aggregate を流すと error 試行が不合格に数えられる（人の手順の `--write-evidence` は修正の取り込み後に流すこと）。
- 直す前の値（error を不合格に数えた。参考、根拠には使わない）: haiku 22/24・opus 23/24・sonnet 20/24・astra 11/12・luna 18/30・sol 25/30、export 307 行。中間物は `aggregate.with-errors/`・`export.with-errors.jsonl`。

### 承認上限の変更と再投入

- 費用・quota の承認（2026-10-14、estimate.md）: claude 144 試行・7d 15%、codex 132 試行・7d 13.2%（pilot 10 を含む）。fable は 0。
- codex: 7d が見積りより速く増えたため queued の codex 93 試行を cancel（10:26Z）。人の指示『上限を21%まで上げていいのでsolも回してください』で codex（chatgpt_plus_personal）の 7d 承認上限を **13.2% から 21%** に上げ、cancel した gpt-6.1-sol 31・gpt-6-luna 50・gpt-6-astra 12 の計 93 試行を新しい batch（trial_no +50、retry-batches-codex.json）で再投入した。sol も luna・astra と同じく再投入分として集計に含める。
- claude: 承認上限 15% は評価で消費する分（投入前 9% + 15% = 24%）という人の回答で、仕組み側 error の claude 13 試行も再投入した。
- 再投入の規則: 仕組み側の error（worktree 消失・dispatcher stopped・daemon recovery・yielded）に限り試行ごとに最大 2 回（1 回目 retry-batches-1.json 18 試行・trial_no +100、2 回目 retry-batches-2.json 3 試行・+200）。failed は再投入しない。元の error 試行は除外し、再投入分の結果を使う。
- cancelled（codex 93）は再投入で置き換えたので、cancel が根拠不足の原因になった cell は無い（cancel 分の有効問題数の不足は 0。下の『根拠の立たない cell』は全部それ以外の理由）。

定価換算 USD は claude-code の run が報告する cost_usd（定価換算）。codex は USD を報告しないので token を併記する。
評価しなかった model: claude-fable-5-1（計画 0 問）。fable は人の承認で今回 0（次の週窓で別承認）、opencode-go は対象外。

## quota（account 単位の実測）

| source | account | 窓 | 投入前（04:07Z） | 集計時 | 差 | 窓の reset |
|---|---|---|---:|---:|---:|---|
| claude_oauth | claude_max_lab | five_hour | 0.170 | 0.330 | — | 2026-10-14T15:30:00Z |
| claude_oauth | claude_max_lab | seven_day | 0.090 | 0.180 | +9.0 pt | 2026-10-18T23:00:00Z |
| claude_oauth | claude_max_personal | five_hour | 0.150 | 0.250 | — | 2026-10-14T19:10:00Z |
| claude_oauth | claude_max_personal | seven_day | 0.090 | 0.190 | +10.0 pt | 2026-10-19T23:00:00Z |
| codex_oauth | chatgpt_plus_personal | seven_day | 0.020 | 0.200 | +18.0 pt | 2026-10-20T04:58:23Z |

7d の差は同じ account を使う評価以外の run も含む上限値（評価だけの消費はこれ以下）。窓の reset をまたいだ行は差を出さない。run ごとの quota（quota_per_run）は試行の run に quota の実測 event が無いので根拠表では空（{}）。model ごとの計画値は estimate.md の表。

## harness ごとの Pareto と同等集合

### claude-code

- Pareto（aggregate、pass 率・費用・壁時計で非劣）: claude-oauth:claude-sonnet-5-5:claude-code
- 根拠の立つ候補: claude-oauth:claude-opus-5-5:claude-code, claude-oauth:claude-sonnet-5-5:claude-code。下限最大は claude-oauth:claude-opus-5-5:claude-code（0.862）。同等集合（区間が重なる）: claude-oauth:claude-opus-5-5:claude-code, claude-oauth:claude-sonnet-5-5:claude-code
  - held-out 検証 claude-oauth:claude-opus-5-5:claude-code − claude-oauth:claude-sonnet-5-5:claude-code（共通の問題 dev 14・held-out 10）: dev 0.000・held-out 0.000 → 反転なし（参考: 非対応の率の差 dev 0.000・held-out 0.000）
  - paired claude-oauth:claude-haiku-5-5:claude-code − claude-oauth:claude-opus-5-5:claude-code: n 14・McNemar p 1.000・差の区間 0.000–0.000
  - paired claude-oauth:claude-haiku-5-5:claude-code − claude-oauth:claude-sonnet-5-5:claude-code: n 14・McNemar p 1.000・差の区間 0.000–0.000
  - paired claude-oauth:claude-opus-5-5:claude-code − claude-oauth:claude-sonnet-5-5:claude-code: n 14・McNemar p 1.000・差の区間 0.000–0.000

### codex

- Pareto（aggregate、pass 率・費用・壁時計で非劣）: codex-oauth:gpt-6-luna:codex
- 根拠の立つ候補: codex-oauth:gpt-6.1-sol:codex, codex-oauth:gpt-6-astra:codex, codex-oauth:gpt-6-luna:codex。下限最大は codex-oauth:gpt-6.1-sol:codex（0.744）。同等集合（区間が重なる）: codex-oauth:gpt-6.1-sol:codex, codex-oauth:gpt-6-astra:codex, codex-oauth:gpt-6-luna:codex
  - held-out 検証 codex-oauth:gpt-6.1-sol:codex − codex-oauth:gpt-6-astra:codex（共通の問題 dev 5・held-out 7）: dev 0.200・held-out 0.000 → 反転なし（参考: 非対応の率の差 dev 0.075・held-out -0.071）
  - held-out 検証 codex-oauth:gpt-6.1-sol:codex − codex-oauth:gpt-6-luna:codex（共通の問題 dev 16・held-out 13）: dev 0.000・held-out 0.385 → 反転なし（参考: 非対応の率の差 dev 0.000・held-out 0.390）
  - paired codex-oauth:gpt-6-astra:codex − codex-oauth:gpt-6-luna:codex: n 5・McNemar p 1.000・差の区間 0.000–0.000
  - paired codex-oauth:gpt-6-astra:codex − codex-oauth:gpt-6.1-sol:codex: n 5・McNemar p 1.000・差の区間 -0.600–0.000
  - paired codex-oauth:gpt-6-luna:codex − codex-oauth:gpt-6.1-sol:codex: n 16・McNemar p 1.000・差の区間 -0.188–0.188

## 根拠表（ADR D1・付記 2026-10-12 の形、`routing_evidence` の行）

鍵は (genre, harness_id, harness_version, source_id, model_id, evidence_version)。

| genre | harness_id | harness_version | source_id | model_id | evidence_version | n | pass | pass_lower | pass_upper | pass_boot_lower | pass_boot_upper | wall_median_s | usd_list_median | quota_per_run_json | timeout_rate | low_confidence | 根拠 |
|---|---|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|
| coding | claude-code | 2 | claude-oauth | claude-opus-5-5 | `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14` | 24 | 24 | 0.8620 | 1.0000 | 1.0000 | 1.0000 | 236.226 | 0.757 | `{}` | 0.000 | true | 立つ |
| coding | claude-code | 2 | claude-oauth | claude-sonnet-5-5 | `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14` | 24 | 24 | 0.8620 | 1.0000 | 1.0000 | 1.0000 | 103.160 | 0.270 | `{}` | 0.000 | true | 立つ |
| coding | claude-code | 2 | claude-oauth | claude-haiku-5-5 | `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14` | 23 | 22 | 0.7901 | 0.9923 | 0.8696 | 1.0000 | 263.867 | 0.353 | `{}` | 0.000 | true | 立たない |
| coding | codex | 0 | codex-oauth | gpt-6.1-sol | `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14` | 30 | 27 | 0.7438 | 0.9654 | 0.8000 | 1.0000 | 354.333 | — | `{}` | 0.000 | true | 立つ |
| coding | codex | 0 | codex-oauth | gpt-6-astra | `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14` | 12 | 11 | 0.6461 | 0.9851 | 0.7500 | 1.0000 | 209.367 | — | `{}` | 0.000 | true | 立つ |
| coding | codex | 0 | codex-oauth | gpt-6-luna | `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14` | 29 | 21 | 0.5428 | 0.8530 | 0.5517 | 0.8621 | 76.212 | — | `{}` | 0.000 | true | 立つ |

## 根拠の立たない cell と、何を足せば立つか

- `claude-oauth:claude-haiku-5-5:claude-code`: 有効問題数不足（23/24）: あと 1 問の有効な結果（error 率を見込んで 1 試行以上の再投入）
  - 足りない 1 問は yielded が 3 回続いた問題で、同じ予算の再投入では埋まらない見込み。立てる道は (1) 予算（turn・壁時計）を上げたその 1 問の 1 試行（有効な結果が出れば 24 問で条件を満たす）、または (2) yielded を不合格として数える人の判断（22/24、Wilson 下限 0.742 で下限 0.5 を満たす）。
- 評価していない cell（fable・qwen・opencode-go）は根拠表に行が無く、D3 の shadow のまま route_defaults を使う。fable は計画 12 問×1 を次の週窓で別承認のうえ走らせれば立てうる。

## 学習データ export

export 件数: 271

- path: `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/export.jsonl`（repo には置かない。held-out の問題文を含むため）
- schema `celeris.routing-eval-result/1`（1 行 1 試行の JSON）: `schema`・`manifest_sha256`・`decision_id`・`trial_no`・`candidate{harness_id,harness_version,model_id,source_id}`・`problem{features(celeris.task-features/1)…}`・`result{passed,status,timed_out,wall_secs,usd_list,quota}`・`cell_intervals{wilson_95,bootstrap_95}`

## 本番の根拠表に書く人の手順

本番 DB への書き込みは人が行う（worker は読み取りのみ）。**前提: error 試行を除外する runner の修正（`E/aggregate-error-exclude.patch`、follow-up task）が main に入っていること**。入っていなければ `git apply` してから build する。daemon が動いたままでよい（runner は 1 transaction で同じ evidence_version を丸ごと置換する）。
```sh
cd ~/workspace/agent-platform && cargo build -p routing-eval
$CARGO_TARGET_DIR/debug/celeris-routing-eval aggregate --db /local/celeris/data/db/celeris.sqlite3 \
  --manifest /local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/manifest-v2/manifest.json \
  --out /local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/aggregate-prod --write-evidence
# 確認: 行数がこの表と同じ
sqlite3 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' "select count(*) from routing_evidence where evidence_version like '118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-%'"
```

集計日が変わると evidence_version（`<manifest sha>-<集計日 UTC>`）も変わる。書いた版を routing_stage の shadow→enforce 承認に使う。
