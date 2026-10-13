# accounts-rekey api-e2e progress

Task: 01M4QTJR2MQYPSZC8KM3ZPRM2J (run 01M4QXT2YZC54V2A9MJPGYNC9M)

- `tests/e2e/tests/account_pool_scenarios.rs`: 旧 `/accounts` 路を `/llm/accounts`（GET）と `/llm/sources/claude_oauth/accounts…`（POST・login・login/code・DELETE）に直した。一覧の `id` は `account_id` に読み替えた。doc comment の旧路も直した。
- `tests/e2e/tests/codex_account_pool_scenarios.rs`: 旧 `/accounts` と `?adapter=codex` を `/llm/sources/codex_oauth/accounts…` に直した。`roots["codex"]` は `roots["codex_oauth"]`、一覧の照合は `source_id` と `account_id` で行う。作成応答の `adapter` は `source_id` に読み替えた。
- 試験の意図（headroom 選択・restart 後も残る・throttle で a だけ cooldown・login 流れ・token 必須・secret を log に出さない）は変えていない。`crates/` は変更なし。

## 証拠

- 旧路の残り: `grep -n 'GET /accounts\|POST /accounts\|"/accounts' tests/e2e/tests/{account_pool,codex_account_pool}_scenarios.rs` → 0 件。
- `cargo build --workspace --bins`（celeris・celerisctl をビルド。e2e は bin を作らないため必要）→ Finished。
- `TMPDIR=/local/x/ae cargo test -p e2e --test account_pool_scenarios` → 3 passed; 0 failed。
- `TMPDIR=/local/x/ae cargo test -p e2e --test codex_account_pool_scenarios` → 2 passed; 0 failed。
- `cargo clippy -p e2e --all-targets -- -D warnings` → exit 0。
- `cargo fmt -p e2e -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更は両 test file だけ。

## 未解決事項

- なし。`docs/api/v1` の再生成・`api_account_limits_tests.rs`・`accounts_admin.rs` などは他の葉（api-tests・api-celeris・api-schema）の担当で、この葉では触れていない。

## 提案

- e2e の試験は `cargo test -p e2e` だけでは celeris の bin が無く失敗する。検査は `cargo build --workspace --bins` を先に流すか、`cargo test --workspace` を使う。
