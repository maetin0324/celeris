# CoS operations: celerisctl

`CELERIS_COS_RUN_CREDENTIAL` があると `celerisctl add` は DB に触れず、
`POST /api/v1/cos/operations` に `POST /api/v1/tasks` を包んで送る。
`celerisctl api-request METHOD /api/v1/... --body JSON` は API の許可表にある
コメント・decision・approval・execution・project・knowledge などの変更に使う
（run credential が必須）。
許可表は API 側が唯一の判定元で、未登録 path は API が拒否し監査する。

`--reason` または `CELERIS_COS_REASON` は必須。`--idempotency-key` を省けば
新しい key を作り、`--expected-revision` は省略時 `null` で送る。
`CELERIS_COS_POLICY_VERSION` の既定は `1`。
`--api-url` は設定の `[api] listen` に代わる API base URL（`/api/v1` を含む）。
CoS credential が無ければ `add` は従来の DB / followups 経路のまま。
CoS credential があると、未対応の既存変更コマンドは監査を迂回しないよう送信前に拒否する。

検証: `cos_chat_ops_ctl_` 9 件が通過。偽 HTTP server で envelope・bearer・旧 domain 経路・
環境変数・送信前の reason 検査を確認した。`cargo test -p celerisctl` と
`cargo clippy --workspace -- -D warnings` も通過。
