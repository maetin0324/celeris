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
片付けた。enum `AccountCheckResult` の定義は workspace で `task-core/src/accounts.rs` の 1 つだけになり、写しの関数
（`map_check_result`・`account_check_result_name`）は 0 件。API の wire（`docs/api/v1`・`web/api/generated`）は作り直して
差分 0。credential の受け渡しと認可の経路は変えていない。WU の範囲（unit の check の正規表現）の外にある dispatcher の
文字列の match は残し、下の「未解決」と「提案」に書いた。

## 前回の run の check の不合格と直し方

前回の run は `Dispatcher::record_account_check` の引数と `AccountAdminEvent::Checked.result` を enum にし、
task-worker の `AccountCheckResult` の re-export を消した。そのため WU の範囲の外の 7 file（`celeris/src/daemon/tick_loop.rs`・
`task-dispatch/src/dispatcher.rs`・`task-dispatch/src/dispatcher/tests/account_auth_failure.rs`・
`task-worker/examples/codex_usage.rs`・`task-worker/src/codex_account.rs`・`task-worker/src/lib.rs`・
`task-worker/src/opencode_account.rs`）を変え、範囲の check と slices-check（`examples/` は R6 の write_set の外）が
exit 1 になった。この run ではこの 7 file を WU の base に戻し、範囲の中だけで型を 1 つにした。

## 実装前 brief

1. 根本原因: 確認結果の 4 値（ok・auth_failed・throttled・spawn_failed）の enum が task-api（`admin.rs`）と
   task-worker（`claude_account.rs`）に別々にあり、celeris の `accounts_admin.rs` が `map_check_result`（型の写し）と
   `account_check_result_name`（文字列の写し）で橋渡ししていた。
2. 責務 owner: 語彙の正本は task-api と task-worker の両方が既に依存する task-core。置き場所は account の観測値の型が
   既にある `crates/task-core/src/accounts.rs`（`AccountAdapter` と同じ理由。Cargo.toml は変えない）。
3. 既存の再利用先: task-core の `accounts` module と `pub use`、`committed_schema_matches_generated`（schema の差分の検出）、
   `web/scripts/gen-types.mjs`、dispatcher の `account_auth_failure` 試験、celeris の `accounts_admin` 試験。
4. 選択肢: (a) task-api の enum を正本にして task-worker から使う — task-worker は task-api に通常依存しない（dev のみ）ので
   Cargo.toml の変更が要り、不可。(b) task-core に置き、event と dispatcher も型で渡し、task-worker の re-export も消す —
   前回の run の形。WU の範囲の外の 7 file を変えるので不可。(c) task-core に置き、task-worker の `claude_account` は
   `pub use task_core::AccountCheckResult` で同じ型を見せ、event は `as_str` の文字列のまま — 採用（範囲の中で型は 1 つになる）。
5. 新しい状態・分岐・fallback・port: 無し。`AccountCheckResult::as_str`（serde と同じ名前）を owner に足したが、消した
   `account_check_result_name` の代わりで、`AccountBook` に保存する文字列は前と同じ。
6. 撤去対象: task-api の enum と `task_api::AccountCheckResult` の re-export、task-worker の enum の定義、`map_check_result`、
   `account_check_result_name`、`AccountLimitPatchBody`、`AccountPoolSetting::is_on`、`codex::from_responses_body`、
   `SourceError::retryable_before_any_bytes`。
7. 不変条件: (i) API の wire は変わらない（schema の再生成で差分 0。`committed_schema_matches_generated`）。
   (ii) `AccountBook` の `last_check.result` は wire と同じ名前（新しい試験
   `account_check_result_as_str_matches_the_wire_name`。`as_str` は `account_check_result_name` を置き換えた唯一の文字列の写しで、
   既存の試験は serde の名前との一致を 4 値すべてでは見ていなかった）。(iii) auth_failed・throttled は cooldown、ok は解除
   （既存の `auth_failure_excludes_the_account_reroutes_notifies_and_recovers` と `accounts_admin` の試験。dispatcher は変えていない）。

## 項目ごとの判定と根拠の検索

検索は base（`$CELERIS_WU_BASE`）で `git grep -nP <名前> -- crates web control-ts scripts docs config/celeris.example.toml`
（web/node_modules を除く）。agent-docs の hit は B0 の候補一覧（`2026-11-07-health-v2-b0/dup-candidates*.json`）だけで、
参照ではない。どの定義にも `#[cfg]`・feature の gate は無い。

| 項目 | 判定 | 根拠 |
|---|---|---|
| account-check-result-vocabulary | 寄せた | enum の定義は base で 2 か所（`task-api/src/admin.rs:161`・`task-worker/src/claude_account.rs:32`）、写しの関数は `celeris/src/accounts_admin.rs` の 2 つ。今は定義が `task-core/src/accounts.rs` の 1 か所だけで、写しの関数は 0 件。task-worker の `claude_account.rs` は同じ型を `pub use` で再公開する（範囲の外の `codex_account.rs`・`opencode_account.rs`・`lib.rs`・`examples/codex_usage.rs` がこの path を使う）。schema の description（`Account connectivity check result.`）は doc comment をそのまま移して保った |
| unref-accountlimitpatchbody | 消した | 定義の行（`task-api/src/types.rs:817`）の外に hit 0。schema の登録にも無い（再生成で差分 0） |
| unref-is-on | 消した | `AccountPoolSetting::is_on`（`task-core/src/accounts.rs:91`）の外に `is_on` の呼び出し 0（`is_on_nfs` は別の関数） |
| unref-from-responses-body | 消した | 定義の行（`llm-proxy/src/sources/codex.rs:257`）の外に hit 0。非 stream の応答は `aggregate_stream` が組み立てる |
| unref-retryable-before-any-bytes | 消した | 定義の行（`llm-proxy/src/sources/mod.rs:46`）の外に hit 0。常に `true` を返すだけだった。`send` は bytes の前にしか `Err` を返さないという契約の説明は `SourceError` の doc comment に移した |

## 変更（commit の件名）

- `accounts: AccountCheckResult を task-core の 1 つの型にし、写しの関数と未参照の pub 4 件を消す`
- `accounts: 進捗に項目ごとの判定・根拠の検索・検査の結果を書く`
- `accounts: WU の範囲の外の 7 file を戻し、task-worker は task-core の AccountCheckResult を再公開する` — 上の
  「前回の run の check の不合格と直し方」のとおり。`AccountAdminEvent::Checked.result` は `String` のまま
  `result.as_str()` で作る。この file も書き直した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`・`git merge-base HEAD main`）

- 範囲の check（unit の check と同じ command、`sh "$CELERIS_WU_SCOPE_PATHS"`）→ exit 0。変更は 13 file（この file を含む）。
- `git diff --quiet $(git merge-base HEAD main) -- crates/campaign crates/routing-eval … crates/celeris-credentiald` と
  `slices-check.py --write-set R6` → exit 0（`write-set R6: ok`）。K2/K4/IEEE Access の評価器・fixture の path は差分 0。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-api --lib -E 'test(committed_schema_matches_generated) | test(schema_uses_defs_once_for_shared_types)'`
  → 2 passed。`node web/scripts/gen-types.mjs` → exit 0。`git diff $(git merge-base HEAD main) -- docs/api web/api/generated | wc -l` → 0。
- `git grep -nP 'enum AccountCheckResult' -- crates web control-ts scripts docs` → `crates/task-core/src/accounts.rs` の 1 件だけ。
  `map_check_result`・`account_check_result_name`・`AccountLimitPatchBody`・`from_responses_body`・`retryable_before_any_bytes`・
  `.is_on()` → 各 0 件。
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health は verdict=pass（new=0、worsened=0）。Rust は 8528 passed・23 skipped、
  web は typecheck の後 103 file・714 passed、ts-verify は 20 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- `Dispatcher::record_account_check`（`task-dispatch/src/dispatcher.rs`）は結果を `&str` で受け、`"ok"`・`"auth_failed"`・
  `"throttled"` の文字列で match している（未知の文字列は `_ => {}`）。enum の値の名前の写しがここに残る。この file と
  呼び出し側の `celeris/src/daemon/tick_loop.rs`・試験 `account_auth_failure.rs` は WU の範囲の外なので変えていない。
- task-worker の `claude_account.rs` の `pub use task_core::AccountCheckResult` は写しの型ではないが、利用者の path を
  `task_core` に替えれば要らなくなる。替える先（`codex_account.rs`・`opencode_account.rs`・`task-worker/src/lib.rs`・
  `examples/codex_usage.rs`）は WU の範囲の外で、`examples/` は slices.json の R6 の write_set の外でもある。

## 提案

- 次の健全性の slice で、`record_account_check` の引数と `AccountAdminEvent::Checked.result` を `AccountCheckResult` にし、
  dispatcher の match を網羅にする。同じ変更で task-worker の利用者を `task_core::AccountCheckResult` に替え、
  `claude_account.rs` の `pub use` と `task-worker/src/lib.rs` の re-export を消す。write set に
  `crates/task-dispatch/src/dispatcher.rs`・`crates/celeris/src/daemon/tick_loop.rs`・`crates/task-worker/src/`・
  `crates/task-worker/examples/` が要る。前回の run で同じ変更が test-affected・clippy・fmt を通ることは確かめた
  （dispatcher.rs は source-size の上限 2036 行を超えないよう書き直しが要った）。
- task-dispatch の `route_failure.rs` にも cooldown の理由の文字列の写し（`account_cooldown_reason_name`・
  `account_cooldown_reason_from_failure`・`cooldown_reason_name`）が残っている。今回の割り当ての外なので触っていない。
  `AccountCooldownReason` を正本にした寄せを候補にできる。
