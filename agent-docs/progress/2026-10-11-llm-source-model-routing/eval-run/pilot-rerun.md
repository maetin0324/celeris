---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: eval-run pilot-rerun — codex quota 実測 pilot のやり直し（gpt-6-luna 10 試行）
status: done
completed: 2026-10-14
---
# pilot-rerun: 新しい batch で gpt-6-luna 10 試行を走らせ、全部終端になった

- **batch_id: `01M4W7JDSJVGS8R14D9QA5TEPZ`**（旧 pilot `01M4W35QEMQGVV5ZSD0JJ61ZDD` とは別。10 試行、trial_no 3・4）
- 終端の確認 command:

```
curl -sf -H "Authorization: Bearer $(cat $HOME/.config/celeris/api.token)" \
  'http://127.0.0.1:7710/api/v1/routing-eval/trials?batch_id=01M4W7JDSJVGS8R14D9QA5TEPZ' \
  | python3 -c 'import json,sys,collections;t=json.load(sys.stdin)["trials"];print(len(t),dict(collections.Counter(x["status"] for x in t)))'
```

03:46Z の出力: `10 {'passed': 6, 'failed': 4}`。error・cancelled は 0 件。

## 前提の確認（2026-10-14 03:33Z、読み取りのみ）

| 項目 | 結果 |
|---|---|
| 本番 daemon の release（`ps`） | `/local/celeris/state/releases/267ed2d37887/bin/celeris`。main `267ed2d3` は修正 `387ee141`（result.json の置き場）と `1d6c76c5`（`--trial-offset`）を含む（`git merge-base --is-ancestor`） |
| `GET /api/v1/routing-eval/trials` | 200 |
| `~/.config/celeris/config.toml` の `[routing_eval]` | `reserved_runs = 2` |
| 本番 DB `MAX(version)`（mode=ro） | 77 |
| 旧 batch | 10 件、最大 trial_no 2、全部 `error`（消していない） |

## やったこと

1. runner: この WU の base（`7bb697d8`）には `--trial-offset` が無いので、`git archive 267ed2d3` を `$TMPDIR` に展開し、そこで `cargo build -p routing-eval`（渡された `CARGO_TARGET_DIR`）。repo のコードは変えていない。
2. quota の前値: `GET /api/v1/llm/accounts` の `chatgpt_plus_personal` を `E/pilot2-before.json`。`seven_day.utilization = 0.02`、`observed_at = 03:32:05Z`。
3. 投入（旧 pilot と同じ manifest・候補・repo、offset は旧の最大 trial_no 2）:
   ```
   $CARGO_TARGET_DIR/debug/celeris-routing-eval run --api http://127.0.0.1:7710 \
     --manifest E/pilot-manifest.json --candidates codex-oauth:gpt-6-luna \
     --token-file $HOME/.config/celeris/api.token \
     --repo /home/rmaeda/workspace/agent-platform --trial-offset 2 --out E/pilot2-batch.json
   ```
   exit 0、stderr 空。標準出力は捨てた。`pilot2-batch.json` は 10 試行、trial_no {3,4}、全部 `queued`。旧 `pilot-batch.json`・`pilot-before.json` は上書きしていない。
4. 待ち: `GET ?batch_id=` を 60 秒間隔（sleep）で確かめ、03:46Z に全部終端。最終の応答を `E/pilot2-final.json`。

（E = `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/`）

## 終端の内訳

| stratum（split） | trial 3 | trial 4 | 備考 |
|---|---|---|---|
| coding:repair（dev） | passed | passed | |
| coding:design（dev） | passed | passed | |
| coding:investigate（dev） | passed | passed | |
| coding:test（dev） | failed | failed | check_exit 101、timeout なし |
| coding:implement（held-out） | failed | failed | check_exit 101、timeout なし |

- 計 passed 6・failed 4・error 0・cancelled 0。runs は 10 件とも `completed`（adapter codex、account chatgpt_plus_personal、harness_version codex-cli 0.162.1）。failed は worker が終わった後の決定的 check の不合格で、評価の仕組みの不具合ではない。
- 壁時計: 1 試行 55 秒〜5 分 45 秒、batch 全体 03:33:51Z〜03:46:28Z（約 13 分、reserved_runs 2 で 2 本並行）。
- token（runs.usage_json の合計、10 run）: input 10,032,374（うち cache_read 9,401,856）、output 86,272。

## 旧 batch が集計から外れることの確認

`celeris-routing-eval aggregate --db /local/celeris/data/db/celeris.sqlite3 --manifest E/pilot-manifest.json --out E/pilot2-aggregate`（DB は `mode=ro` で開く。exit 0）:

- coding / codex / codex-oauth / gpt-6-luna: n=5、pass 3、Wilson 95% 0.231–0.882、wall 中央値 72.9 秒、Excluded (error/cancelled) 10/0。
- 旧 batch の error 10 件は除外数に出るだけで n・合格率に入らない。問題ごとに最小の trial_no（=3）が数えられている（ADR 2026-10-14-eval-trial-result-placement D4）。

## quota（参考）

| | observed_at | seven_day.utilization |
|---|---|---|
| 前（`pilot2-before.json`） | 03:32:05Z | 0.02 |
| 後（`pilot2-after.json`） | 03:47:11Z（最後の finished_at 03:46:28Z より後） | 0.02 |

- Δ は 0（表示の刻みが 1% 単位で、10 試行では動かなかった）。窓の間に評価以外の codex run は無い（runs を account・adapter で引いて 0 件）。
- 旧 pilot の前後差も参考値にとどめる（旧 batch は全 error で根拠に使わない）。

## 次の unit（estimate）へ

- quota の 1% 刻みでは 10 試行の消費が読めない。見積りは token 実測（10 run で output 8.6 万・input 1,000 万、うち cache 94%）からの換算か、より多い試行の後の前後差で引く必要がある。
- 1 試行あたり平均: input 約 100 万 token（cache 込み）、output 約 8,600、壁時計約 2.5 分。

## 未解決事項

- 本 WU の base には `--trial-offset` が無い（main 267ed2d3 にはある）。以降の unit も main 追従後の runner か、同じく 267ed2d3 以降の build を使う。

## 検査

この run は repo のコードを変えていない（この進捗ファイルのみ）。`bash scripts/dev/test-parallel.sh`・clippy はコード変更が無いため回していない。
