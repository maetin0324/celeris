---
tasks: [01M4QTJR2MQYPSZC8KM3ZPRM2J]
status: done
title: accounts の HTTP 路を (source_id, account_id) に作り直し旧 adapter key 路を消す
updated: 2026-10-12
---

# accounts-rekey: accounts の HTTP 路を (source_id, account_id) に作り直す

完了日: 2026-10-12。設計は `artifacts/u-accounts-rekey.md`（run 外の brief）に従った。
task 木の位置は ADR 2026-10-11-llm-source-harness-selection の段階 impl → purge → unit accounts-rekey。

## 結果

- 旧 `/api/v1/accounts…`（adapter key を id に取る路）・`?adapter=`・衝突回避の `PATCH …/check`・`…/login` 路を消した（互換路・警告・転送は残していない）。
- 新路: `GET /api/v1/llm/accounts`、`POST /api/v1/llm/sources/{source_id}/accounts`、
  `DELETE|PATCH /api/v1/llm/sources/{source_id}/accounts/{account_id}`、
  `POST …/{account_id}/check`、`POST|DELETE …/{account_id}/login`、`POST …/{account_id}/login/code`。
- source → 内部 adapter の対応は固定（`claude_oauth`→ClaudeCode、`codex_oauth`→Codex、`opencode_go`→OpencodeGo）。
  内部 effect・store・dispatcher の key（`claude-code`・`implicit`）は変えていない。
- 暗黙 account の公開 `account_id` は `source_id` そのもの。暗黙 account への作成・削除・check・login は 409 `implicit_account`。
- CoS op（`cos/ops/admin.rs` の ALLOWED/EXCLUDED）、ADR 2026-10-09-cos-operations-all-mutations の D2/D3 表、
  `config/skills/cos-operator/operations.md` を新路に合わせた。
- 2026-10-11 ADR に 2026-10-12 付記（accounts-rekey）を追加し、api-v2 の付記（一部実装）の「旧 adapter key 路は残存」を実装済みに直した。

## 各 unit の要点

- `api-route`（task-api の route・handler・型・CoS op・ADR 表・schema）: 新 route と source 解決を実装。
  `AccountView` は `source_id`・`account_id`、`AccountList` は `roots` の key を source_id に。
  base の未整形 4 file を `cargo fmt` で整形（brief で許可された範囲）。
- `api-tests`（task-api の結合試験）: `tests/accounts_admin.rs` を消し `tests/llm_accounts_admin.rs`（15 試験）に新路で書き直し。
  `standby.rs`・`concurrency.rs`・`cos_ops_admin_config.rs` の URL を合わせた。
- `api-celeris`（celeris の accounts HTTP 試験・docs/api/v1/gui-api.md §3.29〜3.35）: 新路・新欄に書き換え。
- `api-e2e`（tests/e2e の account pool 試験 2 本）: 旧 `/accounts` 路を新路へ。試験の意図は変えていない。
- `types`（web/gui の生成型と呼び出し）: `pnpm gen:types` で `web/api/generated`・`gui/app/celeris/types.ts` を再生成。
  web の accounts 画面・gui の accounts 画面と試験を新路に合わせた（画面の構成は変えていない）。

## 本close（この unit）で直したこと

- task-api の doc comment に残っていた旧路（`GET /accounts`・`POST /accounts/{id}/login`・`?adapter=`）を新路に直した
  （`handlers/accounts.rs`・`admin.rs`・`lib.rs`・`stats.rs`）。コードの振る舞いは変えていない。
- ADR 2026-10-11 の api-v2 付記、`agent-docs/progress/2026-10-11-llm-source-harness-api.md` の未解決 1 と提案 accounts-rekey に実装済みの参照を追記。
- この進捗ファイルを作成。

## 証拠（統合後 HEAD `a4f96237` + 本 unit の変更）

- `TMPDIR=/local/x/ar bash scripts/dev/test-parallel.sh`（書込可能な最初の候補 `/local/x/ar`）: exit 0。
  `CELERIS_TEST_SUMMARY`: nextest passed 4898、failed 0、ignored 14、nextest_exit 0、doctest_exit 0、tmp_leftovers 0。
  コメント修正後の再実行（最終）: exit 0、passed 4898、failed 0、ignored 14、doctest_exit 0、`tmp_leftovers: 1`（下の未解決事項）。
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0（初回 `Finished` 1m09s、最終も exit 0）。
- `cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/check-doc-links.sh`: `check-doc-links: ok`（exit 0）。
- `sh scripts/dev/check-adr-numbers.sh`: `check-adr-numbers: ok (182 files)`（exit 0）。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: `check-doc-layout: ok`（exit 0）。
- 旧路の残存 grep（`crates tests docs web gui config agent-docs scripts`）: `/api/v1/accounts`・`?adapter=` は live コードに 0 件。
  残るのは歴史的な ADR（0017・0024・0025 等）、gui 旧 ADR 0012、`gui-api.md` の「`?adapter=` は無い」の否定文、
  路ではないファイルシステムの path（`/accounts/claude` 等のテスト fixture）だけ。

## 未解決事項

- `scripts/dev/check-architecture-map.py` が NG（6 件: `<workspace>/runs/…/tmp/`・`check-test-tmp-leftovers.sh`・
  `crates/task-api/src/model_assignments.rs`・`crates/task-dispatch/src/dispatcher/provider_select.rs`・
  `crates/task-worker/src/tiered.rs`・`test-parallel.sh`）。HEAD `a4f96237` の時点で既に NG（この unit の変更前）。
  本 unit の受け入れ条件の文書検査 3 本には入らない。`docs/architecture-map.md` の索引の陳腐化なので別 unit で直す。
- 範囲外の doc comment に旧 path が残る（この unit の範囲外なので触らなかった）:
  `crates/task-core/src/accounts.rs:29`（API の `?adapter=` に言及）、`crates/task-dispatch/src/capacity.rs:72`（`GET /accounts`）。
- 最終再実行で test-parallel が `tests left 1 entries in TMPDIR (removed on exit)` を出した（exit は 0、試験は全件 pass）。
  初回は 0 件。script の終了時掃除で消えている。どの試験が残したかは未確認。flaky の追跡は別に行う。
- web / gui の単体試験は `types` unit の証拠のみ。この close では再実行していない（test-parallel は cargo の範囲）。
- 旧 `/api/v1/accounts` を叩く外部 script は、repo 内の scripts・config・tests には無いことを確認した。
  外部から叩く人の script があれば人が新路に直す（下の本番への移行）。

## 提案

- `docs/architecture-map.md` の 6 件の陳腐化を直す unit を切る（model_assignments・provider_select・tiered の行は
  provider 廃止・モデル割り当て撤去の後の残り）。
- 範囲外の 2 件の doc comment は次の触る unit で直す。

## 本番への移行

- 本番 config・本番 DB の書き換えは無い。この変更は HTTP 路と試験・生成型・文書の変更だけで、migration は無い。
- 本番の celeris・celerisctl を新路に合わせて差し替えた後、旧 `/api/v1/accounts…` を叩いている外部の人の script
  （あれば）は人が `/api/v1/llm/sources/{source_id}/accounts/{account_id}` に直す。
  旧 `?adapter=` を付けていた呼び出しは `source_id`（`claude_oauth`・`codex_oauth`・`opencode_go`）に置き換える。
- 人が実行する手順: 差し替え後に `GET /api/v1/llm/accounts` が 200 で、実 account の `source_id` が 3 source に出ることを確認する。
