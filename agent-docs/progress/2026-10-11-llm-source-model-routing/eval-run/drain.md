---
task: 01M4VAAKFJHRSVKZ3996NFKSKS
unit: drain
status: done
updated: 2026-10-14
---
# eval-run / drain: 仕組み側 error の再投入と codex 超過決定の実施

`E` = `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval`。本番 DB は読み取りのみ。問題文・check の command はここに書かない。

## 人が continue の前に確かめること

全試行の終端（本番 DB 読み取りのみ。最後の行が `未終端の合計 0 → 全終端` なら終端）:

```
E=/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval
python3 -I $E/count_states.py $E
```

codex の 7d（承認上限: 投入前 2% + 13.2% = 15.2%）:

```
T=$(cat $HOME/.config/celeris/api.token)
curl -sf -H "Authorization: Bearer $T" http://127.0.0.1:7710/api/v1/llm/accounts \
| python3 -c 'import json,sys;[print(a["account_id"],(a.get("usage") or {}).get("seven_day")) for a in json.load(sys.stdin)["items"]]'
```

continue の目安:

1. `count_states.py` が `未終端の合計 0`（batches.json・retry-batches-codex.json とも queued・running・scoring が 0）。
2. 次に仕組み側 error の再投入 1 回目: `python3 -I $E/resubmit_errors.py $E/ 1`。投入した試行が終端になったら `… 2`（retry-batches-1.json の error が対象）。各回の後に 1. をやり直す。対象群に未終端があると script は `AssertionError: N trials not terminal` で何もしない。
3. 再投入の全回が終端になったら continue（collect へ）。
4. 途中で codex 7d が **14% 以上** になり astra の batch `01M4WZH409YW7HPT5KCN1QGGXK` に queued が残っていたら、astra から cancel する（下の command）。15.2% を越えそうなら luna の batch `01M4WZH402D7SR5N5TX6G144CQ` の queued も cancel し、受信箱で報告する。
   ```
   for b in 01M4WZH409YW7HPT5KCN1QGGXK; do curl -sf -H "Authorization: Bearer $T" "http://127.0.0.1:7710/api/v1/routing-eval/trials?batch_id=$b" \
   | python3 -c 'import json,sys;[print(x["trial_id"]) for x in json.load(sys.stdin)["trials"] if x["status"]=="queued"]' \
   | while read id; do curl -sf -X POST -H "Authorization: Bearer $T" "http://127.0.0.1:7710/api/v1/routing-eval/trials/$id/cancel" >/dev/null && echo "cancelled $id"; done; done
   ```

## (1) 決定 codex-overrun = luna-astra の実施

- 対象: `E/codex-cancel.json`（10:26Z に cancel した codex 93 件）のうち gpt-6-luna 50・gpt-6-astra 12 の 62 件（全件 `cancelled` を DB で確認）。gpt-6.1-sol 31 件は再投入しない。
- 投入（10:32Z）: `python3 -I $E/retry_codex.py $E`（先に `--dry` で件数を確認）。同じ問題・model・source・harness、trial_no は元 + 50（error の再投入の +100・+200 と重ならない）。
  - luna: batch `01M4WZH402D7SR5N5TX6G144CQ` 50 件
  - astra: batch `01M4WZH409YW7HPT5KCN1QGGXK` 12 件（luna の後ろに並ぶので、上限に近づいたら astra から止められる）
  - 応答は `E/retry-batches-codex.json`（`retry_of` に元の trial_id）。
- 投入前の quota（10:31Z、`/api/v1/llm/accounts`）: chatgpt_plus_personal 7d 7%、claude_max_lab 15%・claude_max_personal 15%（claude の上限は投入前 9% + 15% = 24%）。
- 見込み: 投入以後の codex run は sol 29 件で 2% → 7%（約 0.17%/run、見積り 0.0625% の約 2.8 倍）。luna は pilot の実測で 0.1%/run 以下 → 50 件で 5% 以下、astra は本番実測 0.136%/run → 12 件で約 1.6%。合計 7 + 5 + 1.6 = 13.6% ≤ 15.2% なので投入した。astra が sol と同じく 2.8 倍だと約 16.6% になりうるので、上の 4. の止め線（14%）を置いた。
- sol は根拠不足として collect で扱う（不足の試行数は collect が cell ごとに書く。次の週窓で別に見積る）。

## (2) 仕組み側 error の再投入

- 可否: **まだ投入できない**（10:32Z）。`python3 -I $E/resubmit_errors.py $E/ 1` → `AssertionError: 15 trials not terminal`（batches.json に queued 13・running 2）。retry-batches-1.json は作られていない。
- 対象の見込み: batches.json の error 12 件はいま全部が仕組み側（dispatcher stopped 4・daemon recovery 3・yielded 2・worktree 消失の question 3。script の判定関数で 12/12）。model 別: sonnet 9・opus 2・sol 1。終端後に 1 回目で 12 件前後、2 回目は 1 回目の error のうち仕組み側だけ。
- sol の error 1 件は error の再投入の規則（試行ごと最大 2 回）に従い再投入対象になる（codex-overrun の「sol は再投入しない」は cancel した 31 件の話）。
- retry-batches-codex.json の試行が仕組み側 error になった場合は resubmit_errors.py の対象外（batches.json と retry-batches-1.json だけを見る）。その分は collect で除外として数える。
- failed と問題を解いた上での error は再投入しない。旧 pilot batch（pilot-batch.json の error 10）は除外のまま。

## (3) 状態ごとの件数（10:32Z、`python3 -I $E/count_states.py $E`）

| 群 | 件数 | passed | failed | error | cancelled | queued | running |
|---|---|---|---|---|---|---|---|
| batches.json（本評価 266 + pilot やり直し 10） | 276 | 147 | 9 | 12 | 93 | 13 | 2 |
| pilot-batch.json（旧 pilot、集計除外） | 10 | 0 | 0 | 10 | 0 | 0 | 0 |
| retry-batches-codex.json | 62 | 0 | 0 | 0 | 0 | 60 | 2 |
| retry-batches-1.json / -2.json | 未作成 | | | | | | |

未終端の合計 77。queued・running は haiku（batches.json）と luna（retry-codex）。

## 未解決事項

- 全試行の終端と error の再投入 2 回は人の確認の後（この unit の run は終端を待たない）。
- codex 7d の伸びの監視は上の止め線で人が行う。
