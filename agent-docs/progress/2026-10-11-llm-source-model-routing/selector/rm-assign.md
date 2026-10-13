---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---

# rm-assign: 手動 model 割り当ての撤去

## 変更

- 手動割り当て API、CoS operation、store の読み書き経路と role/tier slot の API 型は先行統合時点で削除済み。API schema にも割り当て endpoint/type は残っていない。
- migration `0072_drop_model_role_assignments.sql` は `model_role_assignments` と `model_role_scopes` を drop する。イベント表は変更しない。旧形式の `ModelRoleAssignmentChanged` event の読み取り試験は残す。
- `task_core::model_catalog::assignments` から旧 `AssignmentState` / `EffectiveAssignment` / `AssignmentView` と選択関数を削除した。隣接する wire prefix/source 名 helper は route 設定と並行する legacy 撤去が使うため維持。

## 本番割り当ての読み取り結果と route_defaults 移行案

本番 DB は `mode=ro` で読み取った。読み取り時点の `schema_migrations` 最大値は 70 で、旧割り当て表には以下の行がある。優先順位は小さい値が先。下表は source×lane の現在の優先順を全て転記したもの。

| source | lane | 優先順 model_id |
| --- | --- | --- |
| claude-oauth | cheap | claude-haiku-5-5 → claude-sonnet-5-5 |
| claude-oauth | frontier | claude-fable-5-1 |
| claude-oauth | standard | claude-opus-5-5 |
| codex-oauth | cheap | gpt-6-luna |
| codex-oauth | frontier | gpt-6-astra |
| codex-oauth | standard | gpt-6-luna → gpt-6.1-sol |
| openai-compatible:qwen | cheap | qwen3.8-27b |
| opencode-go | cheap | gpt-5.6-luna → glm-5.3-flash → qwen3.8-flash → mimo-v2.5-pro → qwen3.7-max → deepseek-v4.1-flash → deepseek-v4-flash → mimo-v2.5 → minimax-m2.7 → qwen3.7-plus → qwen3.6-plus → longcat-2.0 → hy3 |
| opencode-go | standard | qwen3.8-max → minimax-m3 → deepseek-v4-pro → glm-5.3-flash → kimi-k2.7-code → mimo-v2.5-pro → qwen3.7-max → glm-5.2 → glm-5.1 → kimi-k2.6 |

移行案: `[route_defaults.<harness>.lanes.<lane>]` の `candidates` に、上表の優先順を保った `{ source, model }` を並べる。旧表は harness を持たないため、各候補は現行の `[llm_routes]` と harness の source kind/API format が適合する harness のみへ展開し、lane の列が未設定の harness は既存 harness 共通 `candidates` を維持する。config の source ID と旧 source 名の対応は実 config で確定してから適用する。候補を設定できない行は黙って別 source に移さず、人が route_defaults の候補として明示する。

## 検査・未解決

- schema/generated 型に割り当て endpoint/type の残存なし（`rg`）。画面撤去は web-routing の担当。
- DB 読み取りは `sqlite3 file:…?mode=ro` のみ。現在の production DB は migration 0072 より前（version 70）のため、この run から本番表の drop は行っていない。アプリの migration 適用時に 0072 が drop する。
-  route_defaults への本番 config 書き換えは、対応する harness ごとに候補適格性を確認して人が行う。

### 検査結果（rm-assign）

- migration 番号は全ローカル・remote branch head を走査し、最大 0075 を確認。既存の 0072 を割当表削除 migration として使用し、新規番号は追加していない。
- `cargo test -p task-core` は 921 passed、doc tests 3 passed。
- `TMPDIR=/tmp cargo test -p task-api` は成功（schema 同期試験を含む）。`/local/x/sel` はこの run から書き込み不可だったため、Unix socket path の短縮には短い `/tmp` を使った。
- `cargo clippy -p task-core -p task-api --all-targets -- -D warnings` 成功。
- task-api の `schema::tests::committed_schema_matches_generated` と `schema_uses_defs_once_for_shared_types` が成功。schema・web generated に割当 API/type の差分はなく、履歴 event の公開型は維持されている。
- 本番 DB の現在の schema は 70。割当行 32 件、管理 scope 12 件を `mode=ro` で確認し、version 70 のため migration 0072 は未適用。表の drop はアプリの migration 適用時に行われる。
