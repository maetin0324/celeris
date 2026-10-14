---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: eval-run pilot — codex quota 実測 pilot（gpt-6-luna 10 試行）
status: done
completed: 2026-10-14
---
# pilot: gpt-6-luna 10 試行を本番に投入した

- **batch_id: `01M4W35QEMQGVV5ZSD0JJ61ZDD`**（10 試行、投入時はすべて `queued`）
- 全試行が終端（passed / failed / error / cancelled）になったかを人が確かめる command:

```
curl -sf -H "Authorization: Bearer $(cat $HOME/.config/celeris/api.token)" \
  'http://127.0.0.1:7710/api/v1/routing-eval/trials?batch_id=01M4W35QEMQGVV5ZSD0JJ61ZDD' \
  | python3 -c 'import json,sys;t=json.load(sys.stdin)["trials"];print(len(t),sorted({x["status"] for x in t}))'
```

10 件で、status がすべて passed / failed / error / cancelled のどれかになったら continue する。

## 前提の確認（2026-10-14 02:16 UTC、読み取りのみ）

| 項目 | 結果 |
|---|---|
| `grep -n -A1 '^\[routing_eval\]' ~/.config/celeris/config.toml` | `reserved_runs = 2`（決定 eval-reserved-runs、人が 02:15Z に追記・再起動） |
| `GET /api/v1/routing-eval/trials` | 200、`{"trials":[]}`（投入前は 0 件） |
| 本番 DB `SELECT MAX(version) FROM schema_migrations`（mode=ro） | 77 |

## やったこと

1. quota の前値: `GET /api/v1/llm/accounts` の `chatgpt_plus_personal` を `E/pilot-before.json` に保存。`usage.seven_day.utilization = 0.02`、`observed_at = 2026-10-14T02:15:43Z`、`resets_at = 2026-10-20T04:58:23Z`、`five_hour: null`。
2. 試行の投入: runner の `run` に問題を絞る option が無いので、コードは変えず、`E/manifest-v2/manifest.json` から `E/pilot-plan.json` の 5 問だけを残した `E/pilot-manifest.json` を作った（`sha256` は元の manifest の値 `118b413e…21e7e7` のまま。後の本投入でも `eval_trials` の UNIQUE が同じ試行の重複を防ぐ）。
   ```
   $CARGO_TARGET_DIR/debug/celeris-routing-eval run --api http://127.0.0.1:7710 \
     --manifest E/pilot-manifest.json --candidates codex-oauth:gpt-6-luna \
     --token-file $HOME/.config/celeris/api.token \
     --repo /home/rmaeda/workspace/agent-platform --out E/pilot-batch.json
   ```
   exit 0。欄は `make_trials` のまま（harness codex・source codex-oauth・model gpt-6-luna・objective は問題文入り・check は manifest の command と timeout 600）。標準出力は捨て、応答は `E/pilot-batch.json` にだけ書いた。5 問の base_commit はすべて `/home/rmaeda/workspace/agent-platform` にあることを `git cat-file -e` で確かめた。
3. 投入直後の `GET ?batch_id=01M4W35QEMQGVV5ZSD0JJ61ZDD` は 10 件（`queued`・`running`）。

（E = `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/`）

## 次の unit（estimate）へ

- 後値は全試行が終端になった後に同じ `GET /api/v1/llm/accounts` で取り、`observed_at` が最後の試行の `finished_at` より後であることを確かめる（dryrun.md「quota の取り方」）。
- 前値の `utilization` は 0〜1 の割合で返っている（0.02 = 2%）。Δ は同じ単位で引く。
- 試行中に評価以外の codex run が同じ account で走れば Δ から除く（dryrun.md「混入を除く」）。

## 提案

- runner の `run` に `--problems <id,...>` を足すと、部分集合 manifest を作らずに済む（dryrun.md 未解決 3）。この unit ではコードを変えていない。

## 検査

この run はコードを変えていない（進捗ファイルのみ）。WU の check 2 本（pilot-batch.json の batch_id と 10 試行、`GET ?batch_id=` で 10 件）はどちらも exit 0。
