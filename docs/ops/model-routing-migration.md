# LLM routing 設定の移行

## 旧設定を削除する

新しい Celeris は `[model_routing]` を含む設定を読み込みエラーにします。起動前に TOML から旧 routing section を削除し、source と model の優先順を `[route_defaults.<harness>]` に移してください。

```toml
[llm_sources.claude_oauth]
kind = "claude_oauth"
models = ["claude-opus-5-5"]

[route_defaults.claude_code]
candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }]
```

lane ごとに候補を変える場合は `[route_defaults.<harness>.lanes.<lane>]` に `candidates` を書きます。詳細と設定例は [LLM source と harness の設定](../guides/providers.md) を参照してください。

## 本番設定の手順

1. 本番 config の控えを安全な場所へ保存します。
2. `[model_routing]` と旧 routing 専用設定を削除します。
3. 実際に使う source/model の組を確認し、各 harness の `route_defaults` に優先順で記述します。
4. config 検査を通してから daemon を再起動します。
5. `GET /api/v1/harnesses` と `GET /api/v1/llm/models` で候補宣言と発見状態を確認します。

旧 shadow の記録 event は監査読み取りのため保持されます。予約表は migration で削除されます。
