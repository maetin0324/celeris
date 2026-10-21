# preflight に env-lock の照合を入れ、offline-lock のモデル一覧は測定 daemon から取る

- 状態: 承認（実装中）
- 日付: 2026-10-21
- task: 01M5EKSPKAKTBEYD5RHGZ0S3AQ

## 背景
2026-10-21 の実 pilot mlsys-main-3 で、preflight（無課金）は ok true だったが、watch が試行投入前に
`env-lock mismatch (refusing to run)` で止まった（消費 0）。offline-lock に本番 daemon の
`GET /api/v1/llm/models` 保存（4 source・72 件）を渡したため、env-lock の B 腕モデル一覧が測定 daemon
（claude-oauth のみ 14 件）と 58 件食い違った。preflight はこの照合をしないので無課金段階で見逃した。

## 決定
- D1: 照合は `env_lock` の 1 関数（既存 `env_lock::establish` の verify 経路）に置き、watch 直前の
  `env-lock` と preflight の両方がそれを呼ぶ。preflight は接続先（`--api-url`、launcher.env の測定 daemon）の
  一覧から `Observed` を作り、`--out` に env-lock が在れば照合する。不一致は `missing`（check `env_lock`）と
  `failure_class: env_lock_mismatch` で失敗にする。env-lock がまだ無い（初回）ときは照合項目を飛ばす（watch が作る）。
- D2: `Observed` の組み立て（A 実行ファイル・worker・catalog・cap_env）は env-lock と preflight で共有し、二重実装しない。
- D3: offline-lock の入力のモデル一覧は launcher の測定 daemon からだけ取る。launcher に lock 生成の段
  （測定 daemon の `GET /api/v1/llm/models` を保存し sha256 を付けて offline-lock に渡す）を置き、
  runbook（docs/ops/measure-run-final.md）から本番 daemon の一覧を渡す手順を消す。
- D4: 試験は fake Host で無 LLM・無 network に行う（本番相当一覧で作った env-lock vs 測定一覧 → 失敗、一致 → 通る）。

## 影響
- 本番 DB・config は変えない。既存の pinned env-lock が測定 daemon と合わなければ preflight で止まる（意図どおり）。
