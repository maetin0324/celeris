---
title: G7 2 回目（n1-g7-20261107-1）の raw の取り込みと G7 の再判定
tasks: [01M6VMSFP1R7DSH9T8PB4GND0Q]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# G7 2 回目の取り込みと再判定

完了日: 2026-11-07。基準は `$CELERIS_WU_BASE`。

## 結論
- **G7 は pass**。`cos_usage_null` 0・`cos_not_launched` 0・`cos_drain_timeout` 0、CoS run 11 本の usage がすべてある。G7 の定義（null を 1 本も許さない）は変えていない。
- 実 LLM の pilot は operator が worker の外で流した。この WU は実 LLM・本番 preflight・本番の操作をしていない。
- 本番の限定導入の判定は、G7 が pass になっても G2・G2s・G2b・G6 が未了なので **no-go のまま**。
- C08 は oracle fail。落ちた述語は `cos_invocations_after_achieved == 0`（実測 3）だけ。原因の切り分けは次の WU（c08-cause）が行う。

## credential の grep（取り込み前）
`raw-run2` 全体と `run2.log`・`approval-run2b.json` に対し、Anthropic key の接頭辞・OAuth の 2 種の token の欄名・Authorization の値の語の 4 語を `grep -rc` で検索した。結果は 4 語とも **0 件**。取り込み後の [g7-run2/](.) にも同じ 4 語は 0 件（`grep -rlE` が 0 file）。judge の `leak_hits` も 2 試行とも 0。語そのものはこの file に書かない。

## 取り込み
- 原本 `/local/celeris/data/scratch/n1-g7/raw-run2` を [raw/](raw/) に 1 file ずつコピーした（24 file、`manifest.json` に sha256）。`run2.log` は `raw/run.log`、`approval-run2b.json` は `raw/approval.json` として置いた（g7-run1 と同じ配置）。
- g7-run1 との違い: 試行の repo に `README.md`・`minichunk.py` も入っている（run 1 は `out/` だけ）。C08 の `out/` は o1〜o4。

## 実行と承認（pilot-gate.json と一致）
| 項目 | 値 |
|---|---|
| 承認 | `n1-g7-20261107-1`、approved_by `human:rmaeda`、2026-11-07T18:28:32Z、期限 2026-11-08T12:00:00Z |
| 範囲 | cases C06・C08、arms new、non-production。account claude_max_personal、model claude-sonnet-5-5、harness claude_code |
| 上限 | max_usd 2.7、max_wall_secs 900（2 試行の総量。driver に `--max-wall-secs 450`） |
| 実測 | USD 1.717436（cost_cap pass）、wall 327 秒（wall_cap pass）、cache creation 358,731 token、CoS run 11 本（C06 5・C08 6） |
| 累計 | 1 回目 0.9802 + 2 回目 1.717436 = 2.6976 USD（人の上限 4 以内） |
| 終了 | exit 0。judge は verdict pass・g7 pass・approval_state approved |
| drain | C06 14.0 秒、C08 40.1 秒。timed_out 0・wall_limit_hit false |
| 守りの検査 | events_append_only（135・195 件で変更 0）、leak_hits 0、double_effect 0、cos_launch_rejected 0、approvals_decided 全件。`cos_ops_rejected` は info（C06 2・C08 4） |
| quota | not_measured（0 と書かない） |

## G7 の判定（出力と結果）
`pc_rollout.criteria` に `g7-run2/raw/grade/pilot-facts.json` を渡して判定した（本番 API・本番 token は使わない）。

```
python3 -I - <<'PY'
import sys, json
sys.path.insert(0, "scripts/dev/pc-rollout")
import pc_rollout as p
pf = json.load(open("agent-docs/progress/2026-11-06-phase5/g7-run2/raw/grade/pilot-facts.json"))
print([r for r in p.criteria("dry_run", {"pilot_facts": pf}, "production") if r["id"] == "G7"])
PY
```

結果（exit 0）:

```
{'id': 'G7', 'what': '実 LLM pilot の再実行で cos_usage_null 0', 'state': 'pass', 'detail': 'g7=pass cos_runs=11 cos_usage_null=0 missing=[]'}
```

`pilot-facts.json` は `schema` `celeris.long-horizon-pilot-gate/1`、`g7` pass、`cos_runs` 11、`cos_usage_null` 0、`cos_not_launched` 0、`cos_drain_timeout` 0、`missing` 空、`approval_id` `n1-g7-20261107-1`。1 回目は `g7` fail・`cos_usage_null` 3 だった（[../g7-run1/summary.md](../g7-run1/summary.md)）。本番の `pc_rollout.py preflight` は走らせていない。

## 試行ごとの結果
| 試行（id は raw/arm-map の file 名） | case | wall | judge | oracle | controller_achieved |
|---|---|---|---|---|---|
| C06 の試行 | C06 | 133 秒 | pass | pass | true、不一致なし |
| C08 の試行 | C08 | 194 秒 | pass | **fail** | true、`mismatch` true |

wall は 2 試行の和が 327 秒。

## C08 の oracle 不一致（事実だけ。原因は c08-cause が書く）
- 落ちた述語は `cos_invocations_after_achieved == 0`（実測 3）の 1 つだけ。`grades.jsonl` の C08 行で、`outcome_reached(o1〜o4)`・`roots_for_outcome`・`roots_after_achieved == 0`・`human_ops` は pass。
- C08 の facts（`raw/facts/` の 2 file のうち C08 の方） の `achieved_at` は 147、`horizon_secs` は 450。`ops` の rules は最後が `pc/achieved`、`root_statuses` は done 4 本。
- facts の `invocations` で `at` が 147 より後の CoS run は 3 本（at=156・164・173。いずれも launched、`prelaunch_reason` なし、`slot_wait_secs` 0）。`at` の意味（開始か終了か）は未確認。c08-cause が raw の時刻と突き合わせる。
- C06 は `cos_invocations([600,259200]) <= 2` が実測 0 で pass。

## 受け入れ条件ごとの証拠
- credential の grep 0 件: 上記（4 語とも 0、取り込み後も 0 file）。
- G7 の判定: 上記のコマンドと出力。
- 文書の更新: [n6.md](n6.md)（G1〜G7 の表・次の手の表）、[n1.md](n1.md)、`docs/ops/project-controller.md`（4a 節の G7 の説明と 6 節の状態）。
- 本番の 1 案件の選定と `[project_controller]` の値は、n6.md の次の手の表の 1 行目に人の決定として残した。

## 本番への移行
この WU は本番を操作していない。次は人が決める・行うこと（詳細は n6.md の承認の表）。
1. 本番で限定導入する 1 案件の選定（`allowed_project_ids`）。
2. `[project_controller]` の値（`[project_controller.budget]` の `max_usd_per_day`・`max_roots_per_day`・`max_cos_inputs_per_day` など）。
3. 本番の config の書き換えと daemon の再起動、`generations register`、observe の承認の file。

## 未解決
- C08 の `cos_invocations_after_achieved` 3 回の原因（c08-cause）。N6 の G1〜G7 には入らないが、controller の「到達後に CoS を起こさない」振る舞いの評価に関わる。
- G1r・G3・G4・G4m の本番での確認（人が段 0 の preflight で行う）。

## 提案
- C08 の原因が分かるまで、`cos_invocations_after_achieved` の述語は変えない（G7 の定義と同じく fail closed）。

## 検査（提出時の HEAD で実行）
下の結果は `scripts/dev` の文書検査と cargo の検査で、この WU は文書だけを変えた。

- 下に実行結果を書く。
- `sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0（試行 id を件名の位置に書いた指摘を直してから再実行）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし、docs のみ）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
