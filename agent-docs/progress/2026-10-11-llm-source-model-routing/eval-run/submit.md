---
unit: submit
task: 01M4VAAKFJHRSVKZ3996NFKSKS
status: done
completed: 2026-10-14
---
# submit: 承認された試行計画（276 試行）を本番 Celeris に投入した

## 人が見る要点（全試行が終端になってから continue）

- 2026-10-14 04:08Z に投入。新しい 6 batch に 266 試行（全部 `queued` で受理）、pilot の 10 試行は UNIQUE 鍵で既存の batch `01M4W7JDSJVGS8R14D9QA5TEPZ` が返った。計 276 = `run-plan.json` の全試行（重複・不足 0）。
- continue の前に確かめること:
  1. 下の確認 command で 7 batch の status がすべて終端（passed・failed・error・cancelled）。
  2. `error` が多い batch が無いこと（仕組みの不具合なら集計に使わず受信箱へ）。
  3. account の 7d 使用率が見積りの範囲: claude は投入前 9%（personal・lab とも）+ 見積り 14.96% 以下、codex（chatgpt_plus_personal）は投入前 2% + 13.2% 以下。越える勢いなら下の cancel で止めて受信箱で報告する（人の承認の条件）。
- 見込み時間: 上限で約 12 時間、pilot の実測比なら約 4 時間（`[routing_eval] reserved_runs = 2` で同時 2 本）。claude の 5h 窓は見積り 132% 相当なので、admission（QuotaCap）の待ちが出て伸びることがある（待ちの間、試行は queued のまま）。fable は今回 0 本。

| batch_id | model | 件数 | 投入時の状態 |
|---|---|---:|---|
| `01M4W9H8WDQGPKNCGA73VSVZ4N` | claude-opus-5-5（claude-oauth） | 48 | queued 48（直後に running 2） |
| `01M4W9H8WNXE7MHDTS719NYA20` | claude-sonnet-5-5（claude-oauth） | 48 | queued 48 |
| `01M4W9H8WW9W7D5VXMCSMX98AQ` | claude-haiku-5-5（claude-oauth） | 48 | queued 48 |
| `01M4W9H8Z511M425FFAP5YHE17` | gpt-6.1-sol（codex-oauth） | 60 | queued 60 |
| `01M4W9H8ZDWYRW8EQWKXSJ20QF` | gpt-6-luna（codex-oauth） | 50 | queued 50 |
| `01M4W9H8ZKD0Q4RBXX4AQRVF9Y` | gpt-6-astra（codex-oauth） | 12 | queued 12 |
| `01M4W7JDSJVGS8R14D9QA5TEPZ` | gpt-6-luna（pilot、既存） | 10 | passed 6・failed 4 |

## 状態を確かめる command

```
T=$(cat $HOME/.config/celeris/api.token)
for b in 01M4W9H8WDQGPKNCGA73VSVZ4N 01M4W9H8WNXE7MHDTS719NYA20 01M4W9H8WW9W7D5VXMCSMX98AQ \
         01M4W9H8Z511M425FFAP5YHE17 01M4W9H8ZDWYRW8EQWKXSJ20QF 01M4W9H8ZKD0Q4RBXX4AQRVF9Y 01M4W7JDSJVGS8R14D9QA5TEPZ; do
  curl -sf -H "Authorization: Bearer $T" "http://127.0.0.1:7710/api/v1/routing-eval/trials?batch_id=$b" \
  | python3 -c 'import json,sys,collections;t=json.load(sys.stdin)["trials"];print(sys.argv[1],len(t),dict(collections.Counter(x["status"] for x in t)))' $b
done
```

quota（7d・5h）:

```
curl -sf -H "Authorization: Bearer $T" http://127.0.0.1:7710/api/v1/llm/accounts \
| python3 -c 'import json,sys;[print(a["account_id"],(a.get("usage") or {}).get("seven_day"),(a.get("usage") or {}).get("five_hour")) for a in json.load(sys.stdin)["items"]]'
```

## 止まった試行・止めたいときの cancel

- 1 試行: `curl -sf -X POST -H "Authorization: Bearer $T" http://127.0.0.1:7710/api/v1/routing-eval/trials/<trial_id>/cancel`（`queued`・`running` だけ 200。他は 409）。
- batch の未終端を全部: 上の GET の応答から `status` が `queued`/`running` の `trial_id` を取り出し、同じ POST を 1 件ずつ送る:
  ```
  curl -sf -H "Authorization: Bearer $T" "http://127.0.0.1:7710/api/v1/routing-eval/trials?batch_id=<batch_id>" \
  | python3 -c 'import json,sys;[print(x["trial_id"]) for x in json.load(sys.stdin)["trials"] if x["status"] in ("queued","running")]' \
  | while read id; do curl -sf -X POST -H "Authorization: Bearer $T" "http://127.0.0.1:7710/api/v1/routing-eval/trials/$id/cancel" >/dev/null && echo "cancelled $id"; done
  ```
- cancel した試行は消えない（`cancelled` で残る）。やり直すなら別の trial_no で投入する（`celeris-routing-eval run --trial-offset`）。

## 投入の前の確認（04:07Z、読み取りのみ）

| 項目 | 結果 |
|---|---|
| 本番 daemon（`ps`） | `/local/celeris/state/releases/267ed2d37887/bin/celeris` |
| `[llm_sources.claude_oauth] models` | `claude-haiku-5-5` を含む（人が 04:06Z に追加） |
| `[routing_eval] reserved_runs` | 2 |
| `GET /api/v1/routing-eval/trials?batch_id=01M4W7JDSJ…` | 200 |
| manifest sha | `manifest-v2`・`pilot-manifest`・`run-plan` とも `118b413e…`。run-plan の問題 id は全部 manifest にあり、base_commit も全部ある |
| 投入前の 7d（`E/submit-before.json`） | claude_max_lab 9%・claude_max_personal 9%（personal の 5h は 15%、lab の 5h は 17%） |

## やったこと

- `E/submit_trials.py`（E = `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval`）で `run-plan.json` の試行を POST した。欄は `routing-eval` の `make_trials` と同じ（repo `/home/rmaeda/workspace/agent-platform`、manifest の base_commit、同じ前置きの objective、check timeout 600 秒）。問題文と check の command は標準出力にも記録にも出していない。
- 最初は account ごと 1 POST（144 件）で送り、413 Payload Too Large（body 上限 1 MiB）で 1 件も入らなかった。model ごと・900 KB 以下に分け直して再実行し、exit 0。
- 応答は `E/batches.json`（`{"batches":[{"batch_id","group","trials":[{trial_id,problem_id,trial_no,status,model_id,source_id}]}]}`）。

## 受け入れ条件の確かめ

- 条件 0: `batches.json` の (problem_id, trial_no, model, source) 集合 276 件 = run-plan の 276 件。missing 0・extra 0。
- 条件 1: 7 batch それぞれで `GET ?batch_id=` の件数 = batches.json の件数（48・48・48・60・50・12・10、すべて一致）。
- 条件 2: この file の先頭に batch_id と確認 command。

## 未解決事項

- この unit は投入までで、終端までは待っていない。7d の伸びの監視（承認の条件）は後段の人の確認で上の quota command を使う。
- `celeris-routing-eval run` に試行計画を渡す入口が無いので、投入は script で行った。runner に `--plan` を足すかは collect 以降の提案。
