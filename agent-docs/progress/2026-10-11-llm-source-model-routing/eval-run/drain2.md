---
unit: drain2
task: 01M4VAAKFJHRSVKZ3996NFKSKS
status: done
completed: 2026-10-14
---
# eval-run drain2: 仕組み側 error の再投入（r=2）と全試行の終端

`E` = `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval`。本番 DB は読み取り専用（`file:...?mode=ro`）で読んだ。

## (1) 開始時の状態件数（14:42Z、`python3 -I $E/count_states.py $E`）

| 群 | 件数 | passed | failed | error | cancelled | 未終端 |
|---|---|---|---|---|---|---|
| batches.json（本評価 266 + pilot やり直し 10） | 276 | 160 | 9 | 14 | 93 | 0 |
| pilot-batch.json（旧 pilot、集計除外） | 10 | 0 | 0 | 10 | 0 | 0 |
| retry-batches-codex.json（luna・astra 62 + sol 31） | 93 | 78 | 11 | 4 | 0 | 0 |
| retry-batches-1.json（r=1） | 18 | 13 | 0 | 5 | 0 | 0 |

全試行が終端。retry-batches-1.json（14:00Z 作成、18 件 = batches.json の error 14 件と retry-batches-codex.json の error 4 件、全部仕組み側）があるので r=1 は実施済み、この unit は r=2 を行った。

## (2) 再投入 r=2（14:43Z）

- 対象: retry-batches-1.json の error 5 件。判定関数（`system_error`）で 5/5 が仕組み側（全部 `run ended as yielded`）。
- 投入前の 7d 使用率（`/api/v1/llm/accounts`）: claude_max_lab 18%・claude_max_personal 18%（上限は投入前 9% + 15% = 24%）、chatgpt_plus_personal 20%（上限 21%、止め線 20.5%）。
- 投入: `python3 -I $E/resubmit_errors.py $E/ 2 --skip-model=gpt-6-luna`（先に `--dry` で件数確認）。`--skip-model` はこの unit で script に足した（旧版は `resubmit_errors.v3.py`）。
  - claude-haiku-5-5: batch `01M4XDW25VZW6HH7MJKQJ44YKB` 2 件
  - claude-opus-5-5: batch `01M4XDW25WKJH2Z28YT877ZV76` 1 件
  - trial_id 3 件は `E/retry-batches-2.json`（`round: 2`、`retry_of` 3 件）に残っていることを確認。
- **投入しなかった 2 件（gpt-6-luna）**: codex 7d が 20%（整数丸めなので実値は 19.5〜20.5% 未満）で、止め線 20.5% までの余裕が 0.5 点を切りうる。luna の yielded 試行は予算を使い切る長い run で 1 件 0.17% 前後（drain.md の実測）かそれ以上を見込むため、2 件で止め線を越えうると判断して投入しなかった。除外として collect で数える。

## (3) 終端（14:58Z）

| 群 | 件数 | passed | failed | error | 未終端 |
|---|---|---|---|---|---|
| retry-batches-2.json（r=2） | 3 | 0 | 0 | 3 | 0 |

ほかの群は (1) と同じ。未終端の合計 0。終端後の 7d: claude lab 18%・personal 18%、codex 20%（どれも上限内）。

## (4) error の内訳と再投入しない理由

- r=2 の error 3 件（haiku 2・opus 1）はまた `run ended as yielded`。試行ごと最大 2 回の再投入を使い切ったので再投入しない。同じ問題で 3 回続けて予算切れなので、仕組みの一過性ではなく「その model がその問題を予算内に終えられない」可能性が高い。collect では除外として数え、除外数と、yielded 3 連続である事実を eval-results.md に書くのがよい。
- retry-batches-1.json の luna error 2 件（yielded）: 上の quota 理由で r=2 を見送り。除外。
- batches.json の error 14・retry-batches-codex.json の error 4: r=1 で再投入済み（retry-batches-1.json）。元の試行は集計で除外、再投入分の結果を使う。
- pilot-batch.json の error 10: 旧 pilot（評価の仕組みの不具合）で集計除外のまま。
- failed は再投入していない。

## 未解決事項・提案

- collect は全群の終端を前提に進めてよい。除外は r=2 の error 3 件と見送った luna 2 件（どちらも yielded 系）。
