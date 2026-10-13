---
tasks: [01M4MRE949FE9DWVT26NDF0T8V]
---
# close: 取り込み状態・操作 API と celerisctl の close-out

設計は [ADR: done task の変更を取り残さない](../../adr/2026-10-11-stranded-done-branches.md)（付記に実装突き合わせ）。この WorkUnit はコードを変えず、文書の更新と検査だけを行った。

## 受け入れ条件ごとの証拠

### 0. 進捗の未解決節が更新され close.md がある

- 条件: 進捗の未解決節の更新、close.md の存在。
- 実行: `agent-docs/progress/2026-10-11-stranded-done-branches.md` に「web-status の前提（API・CLI）の解消」節を追加。`…/api-cli.md` の Status と未解決を解消済みに更新。`…/close.md`（本ファイル）を作成。
- 注: 前回記録の「web-status API 確認」「web-status attempt 3 の前提確認」という見出しは、進捗ファイルに存在しなかった。同じ内容の前提（API・CLI）を新しい節に記録した。

### 1. 文書検査 3 本が通る

- `sh scripts/dev/check-doc-links.sh` — exit 0、`check-doc-links: ok`
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0、`check-doc-layout: ok`（引数に manifest の path が要る。引数無しでは usage で exit 1）
- `sh scripts/dev/check-adr-numbers.sh` — exit 0、`check-adr-numbers: ok (182 files)`

### 参考: 前提の確認（コードは変えていない）

- `node web/scripts/gen-types.mjs --check` — exit 0（web の生成型と docs schema が一致）。
- 記録済みの実装の検査（この WorkUnit では再実行していない。各記録を参照）:
  - routes: `cargo test -p task-api --test integration_api` 3 passed、`UPDATE_SCHEMA=1 cargo test -p task-api schema` passed。
  - routes-wire: `cargo test -p task-api --test integration_api integration_api_` 4 passed、`cargo test -p task-api cos_ops_registry` 2 passed、`cargo test -p task-api schema` 3 passed。
  - cli: `cargo test -p celerisctl integration_api_` 6 passed、`cargo clippy -p celerisctl --all-targets -- -D warnings` exit 0。
  - e2e: `cargo test -p celeris --test integration_api integration_api_` 1 passed、`cargo clippy --workspace --all-targets -- -D warnings` exit 0。

## 未解決

1. **受信箱の discard/hold に操作先が無い**（ADR D4、task の検査 4）。`Unintegrated` item の `answer.native` は merge だけ（`crates/task-ops/src/human_inbox.rs:1068`）。discard/hold は `operation_path` に操作名を足して呼ぶ必要があり、item からは操作名が分からない。説明文だけの URL は消えているので 409 で止まる経路は作らないが、「受信箱の取り残し item から route で merge/discard/hold」は完全には満たしていない。
2. **merge の意味**（ADR D2・D4）。API の merge は既に integrated なら記録、未取り込みなら統合依頼の記録（`merge_requested`）。取り込みそのものは行わない。決定の「git 到達確認を通す merge」と違う。
3. **final workspace 試験の結果が記録間で食い違う**。routes-wire の記録は `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` が 5,144 passed で通過。e2e の記録は同じ検査で nextest 5145 passed・failed 3 で、3 件は `task-api::browser_e2e`（`503 browser_unavailable: credential broker is not configured`）。e2e は base commit での比較を行っていない。この WorkUnit では再実行していないので、merge 後の HEAD で `bash scripts/dev/test-parallel.sh` を流して確かめる必要がある。
4. **discard に確認が無い**。実装は reason の必須だけ（ADR は確認を求めていない）。
5. **promotion の表示**。discarded・held の行にも promotion 欄が付く（e2e の変更の帰結）。ADR の文言どおりだが、表示方針は人の確認が要る。
6. **web の操作画面**。記録上、views・routes・cli・e2e のいずれも web の操作画面を作っていない。web は生成型を持つだけ。
7. **CLI の利用文書**。`docs/` に celerisctl integration の説明ページは無い（cli の記録より）。
8. **gui/ の生成型**。この WorkUnit では `gui` の `gen:types`（json2ts 必要）を実行していない。views の記録では gui/web の生成型を再生成したとあり、web 側は `gen-types --check` で確認済み。

## 提案

- 受信箱の `Unintegrated` item に discard/hold の操作先を載せる（`human_inbox.rs` の item に操作ごとの path を持たせる）。次の WorkUnit として切り出す。
- merge の意味を人が決める。(a) 現状の統合依頼のまま、(b) API が git merge まで行い到達確認する、のどちらか。決定を ADR に書いてから実装する。
- merge 後の HEAD で `bash scripts/dev/test-parallel.sh` を流し、`task-api::browser_e2e` 3 件が base でも落ちるか確かめる。
- discard に `confirm` を足すかは人が決める（ADR に無い追加）。
- `cron::request` を共有の daemon API モジュールへ移す（cli の記録の提案）。
