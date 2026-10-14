---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: eval-run pilot — codex quota 実測 pilot（gpt-6-luna 10 試行）
status: blocked
---
# pilot: 投入せず止めた（config の予約枠待ち）

**人が見る要点**: 本番の `~/.config/celeris/config.toml` に `[routing_eval] reserved_runs = 2` がまだ無い。人の決定 eval-deploy の順序 (3)「config に reserved_runs = 2 を足した後まで評価 run は待つ」に従い、10 試行は投入していない。batch_id はまだ無い。下の手順 1〜3 を行い、受信箱の決定 `eval-reserved-runs` に答えると、次の run が投入する。

## 2026-10-14 に読み取りで確かめたこと

| 項目 | 結果 | 要件 |
|---|---|---|
| `GET /api/v1/routing-eval/trials` | 200 | 200（○） |
| 本番 DB `SELECT MAX(version) FROM schema_migrations`（mode=ro） | 77 | 77 以上（○） |
| `grep -n routing_eval ~/.config/celeris/config.toml` | 該当なし（既定 `reserved_runs = 0`） | `reserved_runs = 2`（×） |

`reserved_runs = 0` のままでは評価 run は dispatch されず、試行は queued のまま残る。その間に一般の codex run が週 quota を動かすので、pilot の前値（`pilot-before.json`）と後値の差が評価以外の消費で汚れる。このため前値の取得も投入もしていない。

## 人の手順（dryrun.md「本番導入の人の手順」と同じ）

1. `~/.config/celeris/config.toml` に足す:
   ```toml
   [routing_eval]
   reserved_runs = 2
   ```
2. `systemctl --user list-units 'celeris@*'` で稼働中の unit を確かめ、`systemctl --user restart celeris@<sha12>.service`。journal に config の読み込みエラーが無いことを見る。
3. 確認: `grep -n -A1 '^\[routing_eval\]' ~/.config/celeris/config.toml` が `reserved_runs = 2` を返す。`curl -sf -H "Authorization: Bearer $(cat $HOME/.config/celeris/api.token)" http://127.0.0.1:7710/api/v1/routing-eval/trials` が exit 0。

## 投入後に人が確かめる command（次の run が batch_id を書き込む）

```
curl -sf -H "Authorization: Bearer $(cat $HOME/.config/celeris/api.token)" \
  'http://127.0.0.1:7710/api/v1/routing-eval/trials?batch_id=<batch_id>' \
  | python3 -c 'import json,sys;d=json.load(sys.stdin);it=d.get("items",d);print(len(it),sorted({t["status"] for t in it}))'
```

10 件すべての status が passed / failed / error / cancelled のどれかになったら continue する。

## 次の run の手順

1. `GET /api/v1/llm/accounts` の `chatgpt_plus_personal` の `usage.seven_day`（utilization・observed_at）を `E/pilot-before.json` に保存。
2. `E/pilot-plan.json` の 5 問 × k=2 を `make_trials` と同じ欄（harness codex・source codex-oauth・model gpt-6-luna・objective は make_trials の組み立て・check は manifest の command と timeout 600）で `POST /api/v1/routing-eval/trials` に投入し、応答を `E/pilot-batch.json` に保存（command は標準出力に出さない）。
3. この file の先頭に batch_id を書く。

（E = `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/`）

## 検査

この run は進捗ファイルだけを変えた（コード不変）。全体検査は投入する run で行う。
