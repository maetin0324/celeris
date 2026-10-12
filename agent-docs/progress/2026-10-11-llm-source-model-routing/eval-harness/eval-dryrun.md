---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# eval-dryrun: account 別の評価費用・quota dry-run

`celeris-routing-eval dry-run --manifest <file> --db <path> [--measured <usage-delta.json>] [--out <report.json>]` を実装した。DB は `mode=ro` で開く。Markdown 表を標準出力に出し、`--out` を指定した場合は機械可読 JSON を保存する。

7 候補の計画呼び出しを account ごとに集計する（Claude 192、Codex 132）。30 問×2 の候補と層別 12 問×1 の候補を分けて表示し、token 実測中央値と定価表から list USD を出す。Codex の定価換算は価格未設定として欠測にする。

quota は event の `method=measured` か、`--measured` の 10-run account/window 実測値のみを per-run 率に換算する。値がない窓は「未測定」とし、率や週予測を捏造しない。7d 見積りが 15% を超えた account には候補順の model ごとの縮小案を付ける。

`--measured` JSON 形:

```json
{"runs":10,"accounts":{"claude":{"5h":2.0,"7d":1.0,"monthly":0.5},"codex":{"5h":3.0,"7d":1.5}}}
```

確認: `cargo test -p routing-eval dryrun` が通過（3 tests）。試験は一時 SQLite DB を使用し、stub 入力のみ。
