---
title: S1 control-ts — 型検査と bundle を分けた TS package・生成型・ESM bundle と manifest
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
updated: 2026-11-02
---
# S1 control-ts — TS package `control-ts/`

完了日: 2026-11-02

## 結論
- `control-ts/` を追加した。`tsc --noEmit`（型検査）と esbuild（変換と bundle）を別の段にし、`dist/bundle.mjs` と `dist/manifest.json` を作る。
- `scripts/dev/test-affected.sh` は `control-ts/` か `docs/api/script-host/` の変更で control-ts の typecheck と test を走らせる。
- 未解決が 1 件ある。Host API の schema が `note` を `string` とだけ書くため、生成型も `note: string` になり、null を表せない（「未解決事項」1）。
  直す場所はこの unit の範囲外なので、直していない。

## 変更
`phase 1: control-ts: 型検査と bundle を分けた TS package・生成型・ESM bundle と manifest`
- `control-ts/package.json`: packageManager pnpm@12.6.0、typescript 7.0.2・esbuild 0.28.2・vitest 5.0.0 を完全一致で固定。scripts は `gen:types`・`typecheck`・`test`・`bundle`。
- `control-ts/pnpm-workspace.yaml`: `web/` と同じ `minimumReleaseAge`・`strictDepBuilds`。esbuild の postinstall は `allowBuilds` で明示して走らせない（実行 file は optional 依存の `@esbuild/<platform>` にある）。
- `control-ts/tsconfig.json`: `lib: ["esnext"]`・`types: []`。`node:fs` の import と `fetch` は型検査で落ちることを手で確かめた（TS2591・TS2304）。
- `control-ts/scripts/gen-host-types.mjs`: schema から `src/host.d.ts` を生成する。op の関数の形は schema に無いので生成 script に書き、`host.js` の export と名前が一致することを試験で確かめる。schema の `HostError` は TS では class で表す。
- `control-ts/scripts/build-bundle.mjs`: `celeris:host` だけを external にする。scheme 付きの import（`node:`・`bun:`・`https:` 等）、`//` で始まる import、Node の組み込み名（`fs` 等）は bundle 作成で落とす。失敗したら `dist` の 2 file を残さない。
- `control-ts/bundle.config.json`: manifest の `host_api`・`domains`（領域 → capability）・`checkpoint_versions` の元。schema に無い capability は bundle 作成で落とす。領域の鍵とコードの一致は vitest で確かめる。
- `control-ts/src/`: 入口 `main.ts`（`globalThis.__celerisDispatch`）、試験用の領域 `domains/host_selftest.ts`（`selftestEcho` を 1 回呼ぶだけ。挙動の移行はしない）、試験で使う `testing/host-stub.ts`。
- `control-ts/scripts/*.test.mjs` と `src/domains/domains.test.ts`: 試験。
- `scripts/dev/test-affected.sh`: `web_step`・`run_web` を `js_step`・`run_js_package` に一般化した。`--list` に `control-ts: typecheck+test (理由: …)` の行を足した。

## manifest の決めごと
- `source_commit` は `git rev-parse HEAD`。`control-ts/` か `docs/api/script-host/` に未 commit の変更があれば末尾に `-dirty` を付ける。未 commit の変更を含む bundle を、その commit の産物として記録しないため。
- `schema_sha256` は `docs/api/script-host/host-v1.schema.json` の byte の sha256。
- 同じ入力からは同じ byte の bundle ができる（試験あり）。`source_commit` は bundle の本文に入らない。

## 依存の取得
store（`/local/.pnpm-store/v11`）に必要な版が揃っておらず、最初の `--offline` install は `ERR_PNPM_NO_OFFLINE_TARBALL`（vite@8.3.2）で落ちた。そこで registry から 1 回だけ `corepack pnpm@12.6.0 install` を実行し（41 package、11 件を download）、`pnpm-lock.yaml` を作って store を満たした。
その後 `node_modules` を消して `corepack pnpm@12.6.0 install --offline --frozen-lockfile` が exit 0 で通ることを確かめた。

## 証拠（提出時の HEAD で実行）
result.json と報告本文に書く。

## 未解決事項
1. schema の nullable の欠け: `crates/script-runtime/src/dto.rs` の `#[schemars(required)]` は、schemars 1 では `Option<T>` の欄を `T` の schema にする。このため `host-v1.schema.json` の `SelftestEchoRequest.note`・`SelftestEchoReply.note` は `{"type":"string"}` で、null が入らない形になっている。実行時の DTO は null を受けて返す（`dto_null_required`）。生成型 `note: string` も同じ誤りを持ち、ADR (c) の `T | null` と合わない。
   生成 script は `["string","null"]` を `string | null` に変換できる（試験あり）ので、schema を直して `gen:types` を再実行すれば型も直る。Rust 側の直し方の案は `#[schemars(required, extend("type" = ["string", "null"]))]`（未検証）。
   この unit の範囲 check は `crates/script-runtime/` と `docs/api/script-host/` を許可しない。`bundle-load` の範囲 check も `docs/api/script-host/` を許可しないので、直す unit には両方の path を許可する必要がある。
2. 式で組み立てる動的 import（`import(x)`）は esbuild から見えず、bundle 作成では落ちない。Rust の loader が実行時に拒否する（`runtime_import_rejected`）。
3. 実 bundle を V8 で load する確認はこの unit ではしていない（`bundle-load` の `script-load-check` で行う）。
4. `tsconfig.json` の `skipLibCheck` は vitest の型定義のために要る。`src/host.d.ts` だけは試験で検査を外して型検査する。

## 提案
- `CLAUDE.md` の `test-affected.sh` の説明と `docs/architecture-map.md` に `control-ts/` を足す（この unit の範囲外。`close` unit で行う）。
- release gate で `control-ts` の bundle を作る段は S1 の範囲に無い。配備の手順は `bundle-load`・`close` で決める。
