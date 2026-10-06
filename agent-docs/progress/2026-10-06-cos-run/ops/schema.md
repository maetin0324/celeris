---
tasks: [01M47997XM2GYH4E5J2QNAJZ7R]
unit: schema
status: running
---
# CoS operations schema と生成型

## 実施

- `ApiV1Schema` に CoS operation・checkpoint 型と各 route の説明を追加。`cos_operation` event も API schema と gui 型へ反映。
- `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema` で `docs/api/v1/api-v1.schema.json` を再生成。
- web の生成型・schema copy を更新し、CoS route 名を生成型 header に出すよう generator を更新。
- `gui/app/celeris/types.ts` に CoS 操作/checkpoint 型と event variant を反映。gui の offline install と `gen:types` は pnpm store SQLite error、代替 store は `@codemirror/lang-json@6.0.2` 不足で完了できなかった。
- `docs/api/v1/gui-api.md` に3 route・credential identity・422/409・監査制約を追記し、`gui/docs/celeris-api-v1.md` を同期。

## 検証

- `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema`: pass。
- `node web/scripts/gen-types.mjs --check`: pass。
- `sh scripts/sync-gui-docs.sh --check`: pass。
- `git diff --check`: pass。
- schema/web/gui の `cos/operations` grep: pass。
- 計画の scope check は fail。先行する cred・ops-store の差分 `crates/task-core/src/chat/credential.rs` と `crates/task-core/src/chat/operations.rs` が allowlist 外と報告される。今回の成果と無関係な先行 unit のファイルであり、この check の許可範囲へ `crates/task-core/src/chat/` を加える必要がある。
- GUI `corepack pnpm@11.27.0 -C gui install --offline`: fail（既定 store SQLite open error）。代替 `/tmp` store: fail（offline tarball 不足）。GUI `gen:types` も pnpm install 起動時に同じ SQLite error。
- Web `corepack pnpm@12.6.0 -C web install --offline`: fail（is-promise@4.0.0 offline tarball 不足）。web generator は node 直実行で完了。

## 残り

- scope check allowlist に先行 core 差分を許可した計画で再実行し、範囲を確認する。
- GUI dependency store が利用可能な run で指定の install と `gen:types` を実行し、手動反映型との差分が無いことを確認する。
