---
tasks: [01M47M7QCGRA9V72WKP07Z471B]
---

# ブラウザ設定画面の final review 修正

先行葉 `config-ref`（`58b2b673`, `d9bd9eab`）で、設定例の参照先を実在する `docs/ops/browser-department.md` に直し、手順と実装の更新 API を `PATCH /api/v1/org/{id}/browser-settings` に揃えた。本葉 `settings-gate` で `/browser/settings` を画面台帳へ追加した。設定画面の `GET /api/org` が読む `browser-execution` node を rich fake-daemon fixture に加え、4 幅 gate で許可 origin 2 件の描画も確認する。

## 実行結果

| 葉 | コマンド | 結果 |
| --- | --- | --- |
| config-ref | `grep -q 'docs/ops/browser-department.md' config/org.example.toml && grep -q 'PATCH /api/v1/org/browser-execution/browser-settings' config/org.example.toml && ! grep -q 'browser-execution-section' config/org.example.toml && grep -q 'PATCH /api/v1/org/{id}/browser-settings' docs/ops/browser-department.md && grep -q 'patch_browser_settings' crates/task-api/src/handlers/org.rs` | 5 項目照合、exit 0 |
| settings-gate | `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | exit 0（初回は `get-nonce` の tarball が local store に無く exit 1。`corepack pnpm@12.6.0 -C web install --frozen-lockfile` で取得後に再実行） |
| settings-gate | `corepack pnpm@12.6.0 -C web build` | exit 0 |
| settings-gate | `corepack pnpm@12.6.0 -C web e2e:nfr e2e/parity/mobile-gate.spec.ts --retries=0` | 36 件成功、exit 0（設定画面の 4 幅を含む） |
| settings-gate | `corepack pnpm@12.6.0 -C web e2e:nfr --retries=0` | 103 件成功、exit 0 |
| settings-gate | `corepack pnpm@12.6.0 -C web e2e --retries=0` | 264 件成功、8 件 skip、exit 0 |
| settings-gate | `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| settings-gate | `corepack pnpm@12.6.0 -C web lint` | 378 file 検査、既存 warning 4 件、exit 0 |
| settings-gate | `corepack pnpm@12.6.0 -C web test` | Vitest 458 件、Node 57 件成功、exit 0 |

`crates/` と `gui/` は task base `8aecea650746` との差分がないことを確認した。
