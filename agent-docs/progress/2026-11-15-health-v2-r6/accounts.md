---
title: "健全性v2 R6 accounts: account の確認結果の語彙を 1 つの型にし、未参照の pub を消す"
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 accounts

完了日: 2026-11-15

結論: R6 の dup 割り当てのうち account・llm-proxy の 5 件（account-check-result-vocabulary の merge、
unref-accountlimitpatchbody・unref-is-on・unref-from-responses-body・unref-retryable-before-any-bytes の delete）を
すべて片付けた。残した項目は無い。API の wire（`docs/api/v1`・`web/api/generated`）は作り直して差分 0。
credential の受け渡しと認可の経路は変えていない。

## 実装前 brief

1. 根本原因: 確認結果の 4 値（ok・auth_failed・throttled・spawn_failed）の enum が task-api（`admin.rs`）と
   task-worker（`claude_account.rs`）に別々にあり、celeris の `accounts_admin.rs` が `map_check_result`（型の写し）と
   `account_check_result_name`（文字列の写し）で橋渡ししていた。さらに `AccountAdminEvent::Checked.result` が
   `String` で、`Dispatcher::record_account_check` がその文字列を `"ok"`・`"auth_failed"`・`"throttled"` で読み直していた
   （4 つ目の写し。未知の文字列は `_ => {}` に落ちる）。
2. 責務 owner: 語彙の正本は task-api と task-worker の両方が既に依存する task-core。置き場所は account の観測値の型が
   既にある `crates/task-core/src/accounts.rs`（`AccountAdapter` と同じ理由。Cargo.toml は変えない）。
3. 既存の再利用先: task-core の `accounts` module と `pub use`、`committed_schema_matches_generated`（schema の差分の検出）、
   `web/scripts/gen-types.mjs`、dispatcher の `account_auth_failure` 試験、celeris の `accounts_admin` 試験。
4. 選択肢: (a) task-api の enum を正本にして task-worker から使う — task-worker は task-api に通常依存しない（dev のみ）ので
   Cargo.toml の変更が要り、不可。(b) task-core に置き、写しの関数を消し、event と dispatcher も型で渡す — 採用。
   (c) task-core に置くが event は `String` のまま — 写しの文字列 match が dispatcher に残るので不採用。
5. 新しい状態・分岐・fallback・port: 無し。`AccountCheckResult::as_str`（serde と同じ名前）を足したが、消した
   `account_check_result_name` と dispatcher の文字列 match の代わりで、`AccountBook` に保存する文字列は前と同じ。
6. 撤去対象: task-api と task-worker の enum、`task_api::AccountCheckResult`・`task_worker::AccountCheckResult` の
   re-export、`map_check_result`、`account_check_result_name`、dispatcher の文字列 match、`AccountLimitPatchBody`、
   `AccountPoolSetting::is_on`、`codex::from_responses_body`、`SourceError::retryable_before_any_bytes`。
7. 不変条件: (i) API の wire は変わらない（schema の再生成で差分 0。`committed_schema_matches_generated`）。
   (ii) `AccountBook` の `last_check.result` は wire と同じ名前（新しい試験
   `account_check_result_as_str_matches_the_wire_name`）。(iii) auth_failed・throttled は cooldown、ok は解除、spawn_failed は
   観測を残す（既存の `auth_failure_excludes_the_account_reroutes_notifies_and_recovers` と `accounts_admin` の試験）。

## 項目ごとの判定と根拠の検索

検索は base（`$CELERIS_WU_BASE`）で `git grep -nP <名前> -- crates web control-ts scripts docs config/celeris.example.toml`
（web/node_modules を除く）。agent-docs の hit は B0 の候補一覧（`2026-11-07-health-v2-b0/dup-candidates*.json`）だけで、
参照ではない。どの定義にも `#[cfg]`・feature の gate は無い。

| 項目 | 判定 | 根拠 |
|---|---|---|
| account-check-result-vocabulary | 寄せた | enum の定義は base で 2 か所（`task-api/src/admin.rs:161`・`task-worker/src/claude_account.rs:32`）、写しの関数は `celeris/src/accounts_admin.rs` の 2 つ。今は `task-core/src/accounts.rs` の 1 か所だけで、写しの関数は 0 件。schema の description（`Account connectivity check result.`）は doc comment をそのまま移して保った |
| unref-accountlimitpatchbody | 消した | 定義の行（`task-api/src/types.rs:817`）の外に hit 0。schema の登録にも無い（再生成で差分 0） |
| unref-is-on | 消した | `AccountPoolSetting::is_on`（`task-core/src/accounts.rs:91`）の外に `is_on` の呼び出し 0（`is_on_nfs` は別の関数） |
| unref-from-responses-body | 消した | 定義の行（`llm-proxy/src/sources/codex.rs:257`）の外に hit 0。非 stream の応答は `aggregate_stream` が組み立てる |
| unref-retryable-before-any-bytes | 消した | 定義の行（`llm-proxy/src/sources/mod.rs:46`）の外に hit 0。常に `true` を返すだけだった。`send` は bytes の前にしか `Err` を返さないという契約の説明は `SourceError` の doc comment に移した |

## 変更

- `accounts: AccountCheckResult を task-core の 1 つの型にし、写しの関数と未参照の pub 4 件を消す` — 上の表のとおり。
  `AccountAdminEvent::Checked.result` と `Dispatcher::record_account_check` の引数を `AccountCheckResult` にし、dispatcher の
  match を網羅にした。`dispatcher.rs` は source-size の上限（2036 行）を超えないよう、失敗の種別から cooldown の理由を
  直接選ぶ形に書き直した（挙動は同じ。行数は base より 1 行少ない）。
- `agent-docs/progress/2026-11-15-health-v2-r6/accounts.md`（この file）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `UPDATE_SCHEMA=1 cargo nextest run -p task-api --lib -E 'test(committed_schema_matches_generated) | test(schema_uses_defs_once_for_shared_types)'`
  → 2 passed。`UPDATE_SCHEMA=1 cargo nextest run -p task-core --lib -E 'test(event_row_schema_matches_committed) | …'` → 2 passed。
  `node web/scripts/gen-types.mjs` の後、`git diff $CELERIS_WU_BASE HEAD -- docs/api web/api/generated | wc -l` → 0。
- `grep -rn "enum AccountCheckResult" crates web control-ts scripts` → `crates/task-core/src/accounts.rs` の 1 件だけ。
  `map_check_result`・`account_check_result_name`・`AccountLimitPatchBody`・`from_responses_body`・`retryable_before_any_bytes`・
  `.is_on()` → 0 件。
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health は verdict=pass（new=0、worsened=0）。Rust は 8528 passed・23 skipped
  （13 crate）、web は typecheck の後 103 file・714 passed、ts-verify も通過。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更は上の commit の 19 file とこの file だけ。

## 未解決

- `crates/task-worker/examples/codex_usage.rs` を 1 行変えた（`task_worker::AccountCheckResult` の re-export を消したので
  `task_core::AccountCheckResult` に替えた）。slices.json の R6 の write_set の正規表現（`^crates/[a-z-]+/src/…`・
  `^crates/[a-z-]+/tests/…`）は `examples/` を含まない。unit write-set（決定 r6-write-set）か close で、この path を
  write_set に入れるか確かめてほしい。re-export を残せば example は変えずに済むが、旧い参照の経路が 2 つになるので採らなかった。

## 提案

- task-dispatch の `route_failure.rs` にも cooldown の理由の文字列の写し（`account_cooldown_reason_name`・
  `account_cooldown_reason_from_failure`・`cooldown_reason_name`）が残っている。今回の割り当ての外なので触っていない。
  次の健全性の slice で `AccountCooldownReason` を正本にした寄せを候補にできる。
