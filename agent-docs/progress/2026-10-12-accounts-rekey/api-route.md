# accounts-rekey api-route progress

Task: 01M4QTJR2MQYPSZC8KM3ZPRM2J

- 固定 brief の source ID ↔ adapter 対応を実装。`GET /api/v1/llm/accounts` と `/api/v1/llm/sources/{source_id}/accounts/{account_id}…` を登録し、旧 `/api/v1/accounts` route と `?adapter=` query を撤去。
- 実 account は source ID から既存 adapter effect に変換し、store/dispatcher の内部 key は維持。暗黙 account は公開 account ID = source ID、内部 key = `implicit` + source ID。暗黙 account の create/delete/check/login は 409、異なる account ID・未知 source・未設定 pool root は 404。
- AccountView / AccountCreateBody / AccountList、CoS ALLOWED/EXCLUDED と op 入力、cos-operator skill、API route 表、ADR D2/D3 を更新。ADR 2026-10-11 に 2026-10-12 accounts-rekey 付記を追加。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib`（TMPDIR は書込可能な最初の短い候補を選択）: 79 passed、0 failed、2 ignored。schema drift、CoS operation registry、skill table を含む。
- `cargo fmt --all -- --check`: pass。前回不合格の原因だった base の未整形 4 file（brief 指定）を `cargo fmt --all` で整形。
- `cargo clippy -p task-api --all-targets -- -D warnings`: pass。
- `git diff --check`: pass。`sh "$CELERIS_WU_SCOPE_PATHS"`: pass、変更 file はこの unit の許可範囲内。
- URL を伴う task-api integration tests は次段 api-tests の担当として route 更新をしていない。`tests/cos_ops_registry.rs` の除外 fixture は新 login route に更新済み。
