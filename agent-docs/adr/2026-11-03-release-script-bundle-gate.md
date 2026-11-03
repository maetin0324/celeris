---
title: release の gate で control-ts の bundle と実 bundle 試験を必須にする
status: accepted
date: 2026-11-03
tasks: [01M6HGFR1T4M08TJM7KD39D443]
---
# release の gate で control-ts の bundle と実 bundle 試験を必須にする

## 背景
- 境界 ADR（2026-11-02-ts-control-layer-boundary）の (d)(e) で、daemon は control-ts の `dist/bundle.mjs` と `dist/manifest.json` だけを読む。S5 までに ScriptRuntime・Host API v1・generation・compare→canary→ts・guard は入ったが、`scripts/selfdeploy/release.sh` は control-ts を入れず bundle も作らなかった。bundle を読む試験（`crates/celeris/tests/script_*`）は node_modules が無いと SKIP し、release の `cargo-test` 段でも SKIP のまま通りえた。
- S4 の review で、node_modules を symlink した写しで `pnpm run bundle` を走らせると、元の木の `node_modules/.bin/{esbuild,tsc,vitest}` の shim が写しの path に書き換わると分かった（KB `control-ts-bundle-node-bin-side-effect`）。原因は pnpm 12 の `verifyDepsBeforeRun`（`pnpm run` の前に依存を確かめ、古ければ install する）で、install が symlink 越しに元の `.bin` を書き直す。

## 決定
1. release の gate に 4 段を足す。順は `cargo-fmt-check` → `control-ts-install` → `control-ts-bundle` → `script-bundle-verify` → `cargo-test` → `script-bundle-tests` → `cargo-clippy` …。段の本体は `scripts/selfdeploy/script-bundle-gate.sh`（release に同梱される）。
   - `control-ts-install`: `corepack pnpm@<packageManager> install --offline --frozen-lockfile`。`.modules.yaml` と 3 つの shim が揃わなければ失敗。
   - `control-ts-bundle`: `node scripts/build-bundle.mjs`（`pnpm run` を通さない）。前後で shim の sha256 と control-ts の追跡 file の状態を比べ、変われば失敗。
   - `script-bundle-verify`: 2 file が在る、`bundle_sha256` = bundle の sha256、`schema_sha256` = 木の `host-v1.schema.json` の sha256、`host_api` = `^<HOST_API_VERSION の major>`、`domains` = `bundle.config.json` の key（Rust の `KNOWN_DOMAINS` の内、`host_selftest` を含む）、`capabilities` = 宣言の和、`source_commit` = ビルドする sha（`-dirty`・別 commit は古い bundle）、`node --check` が通り static import は `celeris:host` だけ。
   - `script-bundle-tests`: `CELERIS_SCRIPT_REQUIRE_BUNDLE=1`・`CELERIS_SCRIPT_BUNDLE_DIR=<dist>` で `script-load-check`（登録と同じ `generation::validate_manifest` と selftest の 1 回の呼び出し）と、`cargo nextest run -p celeris -p celeris-script -E '(package(celeris) & binary(/^script_/)) | (package(celeris-script) & binary(=generation))'` を回す。SKIP が 1 件でもあれば失敗。
   - `cargo-test` にも同じ 2 つの env を渡す（全体の試験でも bundle を読む試験は SKIP できない）。
2. どの段も SKIP にしない。落ちたら既存の `run_step` のとおり `gate.json` の `failed_step` に段の名前、`.gate-<段>.log` に理由を残し、release の dir を作らない。
3. 検証した 2 file を release の `script-bundle/` に写し、`gate.json` に `script_bundle: {bundle_sha256, dir}` を足す。`verify.sh` は `script-bundle-gate.sh verify-release` で、4 段が skip されずに exit 0 で載ること、`script-bundle/` の bundle の sha256 が manifest・`gate.json` と一致することを確かめ、合わなければ検証を拒む。この gate を持たない古い release は verify できない（破壊的変更を是とする ADR）。
4. `control-ts/pnpm-workspace.yaml` に `verifyDepsBeforeRun: error` を置く。依存が古ければ `pnpm run` は install せずに失敗する。
5. TS-only の高速検証（`scripts/dev/ts-verify.sh`、ADR 2026-11-02-ts-fast-verify）は変えない。cargo 0 回・edit→verify→activate の warm p50 7.5 秒の経路はそのまま残し、release の全体検証だけが上の段を持つ。
6. 偽の木で release.sh を回す既存の試験は、control-ts を持たないので `SD_SCRIPT_BUNDLE_GATE` で偽の段（`scripts/selfdeploy/tests/fixtures/fake-script-bundle-gate.sh`）に差し替える。本物の段の挙動は `scripts/dev/test-release-script-gate.sh` が確かめる。

## 結果
- release の作成・promote は人が行う（この ADR の実装は release を作っていない）。次の release の `gate.json` で 4 段の秒を読む。
- gate に足した秒の手元の測定は進捗 `agent-docs/progress/2026-11-03-ts-control-ops/p0a-release-gate.md` にある。
