---
title: P0-A release/verify gate で control-ts の bundle と実 bundle 試験を必須にする
tasks: [01M6HGFR1T4M08TJM7KD39D443]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# P0-A release/verify gate で control-ts の bundle と実 bundle 試験を必須にする

完了日: 2026-11-03。親の分割は親 task 01M6HG7T6ZKTX239KGH0QY4HFQ の進捗（ts-control-ops。この branch の基準の main にはまだ無い）。設計は [ADR 2026-11-03-release-script-bundle-gate](../../adr/2026-11-03-release-script-bundle-gate.md)。

## 結論
- `scripts/selfdeploy/release.sh` の gate に 4 段（`control-ts-install`・`control-ts-bundle`・`script-bundle-verify`・`script-bundle-tests`）を足した。bundle が欠けた・作れない・古い・hash が合わない・shim を書き換えた・実 bundle 試験が SKIP した場合は、SKIP せず gate を落とし、`gate.json` の `failed_step` に段の名前を残す（試験で 11 通りを確認）。
- 実 bundle 試験は `CELERIS_SCRIPT_REQUIRE_BUNDLE=1`・`CELERIS_SCRIPT_BUNDLE_DIR=<dist>` で走る。`cargo-test` 段にも同じ env を渡す。
- `scripts/selfdeploy/verify.sh` は、release の `gate.json` に 4 段が通った記録と `script-bundle/` の hash の一致が無ければ検証を拒む。
- S4 の shim 書き換えは、pnpm 12 の `verifyDepsBeforeRun`（`pnpm run` の前の install）が symlink 越しに元の `.bin` を書き直すことが原因だった。`control-ts/pnpm-workspace.yaml` に `verifyDepsBeforeRun: error` を置いて解消し、試験で防ぐ。
- gate に足した秒は、手元の測定で cold p50 29.350 秒・warm p50 26.156 秒（各 3 回）。S6 の gate 4 件の p50 648.145 秒の 4.5%・4.0% に当たる。
- release の作成・promote はしていない（人の手順は下）。TS-only の高速経路（`scripts/dev/ts-verify.sh`）は変えていない。

## 変更（最新 main の file・関数）
- `scripts/selfdeploy/script-bundle-gate.sh`（新規。release に同梱される `*.sh`）
  - `cmd_install`: `corepack pnpm@<packageManager> install --offline --frozen-lockfile`。`.modules.yaml` と shim 3 つ（esbuild・tsc・vitest）が無ければ失敗。
  - `cmd_bundle`: `node scripts/build-bundle.mjs` を直に呼ぶ（`pnpm run` を通さない）。前後で `shim_state`（shim の sha256）と control-ts の追跡 file の `git status` を比べ、変われば失敗。
  - `cmd_verify`: 2 file の有無、`schema`、`bundle_sha256` = bundle の sha256、`schema_sha256` = 木の `host-v1.schema.json` の sha256、`host_api` = `^<crates/script-runtime/src/dto.rs の HOST_API_VERSION の major>`、`domains` = `control-ts/bundle.config.json` の key（`crates/script-runtime/src/generation.rs` の `KNOWN_DOMAINS` の内、`host_selftest` を含む）、`capabilities` = 宣言の和、`source_commit` = ビルドする sha、static import が `celeris:host` だけ、`require(` が無い、`node --check` が通る。
  - `cmd_test`: env の 2 つが無ければ失敗。`cargo run -p celeris-script --bin script-load-check -- --bundle-dir <dist>` と `cargo nextest run -p celeris -p celeris-script -E '(package(celeris) & binary(/^script_/)) | (package(celeris-script) & binary(=generation))'`。`CELERIS_SCRIPT_SKIP_LOG` に 1 行でもあれば失敗。
  - `cmd_verify_release`: verify.sh 用。`gate.json` の 4 段が skip されず exit 0、`script-bundle/` の bundle の sha256 = manifest = `gate.json` の `script_bundle.bundle_sha256`。
- `scripts/selfdeploy/release.sh`: `cargo-fmt-check` の後に install・bundle・verify の 3 段、`run_cargo_test_step` の後に `script-bundle-tests`。`export CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_BUNDLE_DIR`。`write_gate_json` に `script_bundle`。梱包で `script-bundle/` に 2 file を写し、hash を再確認。`SD_SCRIPT_BUNDLE_GATE` は試験の偽の木だけが使う差し替え口。
- `scripts/selfdeploy/verify.sh`: `gate.json` の ok の確認の直後に `script-bundle-gate.sh verify-release "$REL"`。
- `crates/script-runtime/src/bin/script-load-check.rs`: `run` で `generation::validate_manifest`（登録と同じ manifest の検証）も呼び、出力に `domains` を足した。
- `control-ts/pnpm-workspace.yaml`: `verifyDepsBeforeRun: error`。
- 試験: `scripts/dev/test-release-script-gate.sh`（新規）、`scripts/selfdeploy/tests/fixtures/fake-script-bundle-gate.sh`（新規）。偽の木で release.sh を回す 6 本（`release_web_required.sh`・`release_parallel_test_gate.sh`・`release_uses_scratch_lease.sh`・`release_web_bundle_requires_deps.sh`・`release_prune_stale_test_binaries.sh`・`browser_ledger_release_stages.sh`）は `SD_SCRIPT_BUNDLE_GATE` で偽の段を使う。`verify_web_app_start.sh` の偽の release に 4 段と `script-bundle/` を足した。
- 測定: `scripts/dev/script-gate-measure.sh`（新規）。

commit（件名）:
- `phase p0a: release-gate: control-ts の bundle・host API 適合・hash 検証・実 bundle 試験を release/verify の gate に必須化し、shim の書き換えを防ぐ`
- `phase p0a: release-gate: gate に足した段の秒を測る script-gate-measure.sh`
- `phase p0a: release-gate: gate の段の cold・warm 各 3 回の raw records`
- `phase p0a: release-gate: 試験の偽の node の schema の path を木の根から組む`
- `phase p0a: release-gate: 進捗を書く`

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-release-script-gate.sh` → exit 0。
  - (A) 偽の木の release.sh で、bundle 欠落（`missing`）→ `script-bundle-verify`、build 失敗 → `control-ts-bundle`、hash 不一致・source_commit 違い・`-dirty`・schema の hash 違い・host_api `^2`・domains 違い → `script-bundle-verify`、shim の書き換え → `control-ts-bundle`、offline install の失敗 → `control-ts-install`、SKIP 1 件 → `script-bundle-tests`。どれも gate.json の `ok=false`・`failed_step`・段のログが残り、release の dir は作られない。
  - 通る場合は release に `script-bundle/` が入り、`script-load-check`・script 試験・`cargo-test` の nextest に `require=1`・`dir=<tree>/control-ts/dist` が渡る。
  - (B) `verify-release` は中身を差し替えた bundle と `script-bundle-tests` の無い gate.json を拒む。
  - (C) 本物の node・pnpm（offline）で写した control-ts の install・bundle・verify が通り、shim の sha256 は前後で同じ。node_modules を symlink した写しで `corepack pnpm@12.6.0 run bundle` しても元の shim は変わらない。
  - 変異の確認: `verifyDepsBeforeRun: error` の行を消して同じ試験を回すと、(C) の最後で「pnpm run bundle in a symlinked copy rewrote the original shims」で exit 1（元に戻して exit 0）。
- 既存の selfdeploy 試験（`release_web_required.sh`・`release_uses_scratch_lease.sh`・`release_web_bundle_requires_deps.sh`・`release_prune_stale_test_binaries.sh`・`release_parallel_test_gate.sh`・`browser_ledger_release_stages.sh`・`release_notes_promote.sh`・`release_lock_bundled_scripts.sh`・`prepare_timeout_test.sh`・`verify_web_app_start.sh`・`local_state_layout.sh`）→ すべて exit 0。
- 本物の段 `script-bundle-gate.sh test`（この worktree、REQUIRE=1）→ exit 0。64 tests across 7 binaries、64 passed、SKIP 0。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 991 passed（celeris・celeris-script・celerisctl）、script bundle SKIP 0 件、control-ts の typecheck・unit、`test-ts-verify.sh` 18 passed、ts-verify は「rust の変更。従来の経路へ」。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。

## 性能（gate に足した秒）
`bash scripts/dev/script-gate-measure.sh cold 3 …` と `… warm 3 …`。raw は [p0a-script-gate-cold.jsonl](p0a-script-gate-cold.jsonl)・[p0a-script-gate-warm.jsonl](p0a-script-gate-warm.jsonl)（1 試行 1 行、段ごとの秒と exit、nextest の秒と件数、開始時の他の cargo・rustc の数）。

| mode | 試行 | 合計 p50（秒） | 最小〜最大（秒） | install | bundle | verify | test（p50） |
|---|---|---|---|---|---|---|---|
| cold | 3 | 29.350 | 26.925〜35.730 | 0.222〜0.325 | 0.117〜0.191 | 0.046〜0.051 | 28.776 |
| warm | 3 | 26.156 | 25.434〜26.305 | 0.161〜0.168 | 0.115〜0.175 | 0.047〜0.058 | 25.750 |

- 定義: cold は試行の前に `control-ts/node_modules`・`control-ts/dist`・`script-load-check` の binary を消す。release の作業ツリーは毎回 `git clean -ffdx` されるので、release の段はいつも cold から始まる。pnpm の store と、`cargo-test` 段が作る試験の binary は消していない（release でも `cargo-test` の後に回る）。warm は何も消さない。
- S6 の gate 4 件の p50 648.145 秒との比: cold +29.350 秒（4.5%）、warm +26.156 秒（4.0%）。gate 4 件の最大 804.889 秒に足すと 834.2 秒。
- 秒のほとんどは `test`（nextest 24.9〜32.4 秒、64 試験）。install・bundle・verify の 3 段は合わせて 0.6 秒以下。
- 全試行で他の cargo・rustc が 3 個動いていた（`busy_cargo_rustc` 3。同じ host の別 task の build と推定。未確認）。cold rep 2 の 35.730 秒はその影響かもしれないが確かめていない。
- TS-only の経路: `ts-verify.sh` は変えていない。test-affected の中の ts-verify は classify で「rust の変更」と判定して従来の経路へ回り、cargo 0 回・edit→verify→activate の warm p50 はこの task では測っていない（S6 の 7.47〜7.52 秒から変える変更は入れていない）。

## 本番への移行（人の手順）
この task は release を作らず、promote もしていない。次は人が行う。
1. 取り込み後の main で `bash scripts/selfdeploy/release.sh main` を実行する（host に node 24・corepack と pnpm@12.6.0 の store が要る。offline install が落ちると `control-ts-install` で gate が落ちる）。
2. 確認: `python3 -c 'import json;g=json.load(open("<state>/releases/<sha12>/gate.json"));print([(s["step"],s["exit"],s["secs"]) for s in g["steps"] if s["step"].startswith(("control-ts","script-"))], g["script_bundle"])'` で 4 段が exit 0、`script_bundle.bundle_sha256` があること。`<release>/script-bundle/` に 2 file があること。
3. 落ちたときは `<state>/releases/.build/<sha12>/gate.json` の `failed_step` と同じ dir の `.gate-<段>.log` を読む。
4. verify・promote は従来どおり。この gate を持たない古い release は verify.sh が拒むので、promote の前に新しい release を作り直す。
5. 本番の config の変更は無い。

## 未解決
1. `cargo-test` 段が bundle を読む試験を実際に回すようになった分の秒は測っていない。これまでの release では control-ts の node_modules が無く、bundle を読む試験は `cargo-test` の中で SKIP していた（推定。古い release の `.gate-cargo-test.log` の SKIP 行は数えていない）。次の release の `gate.json` の `cargo-test` を S6 の 4 件と比べる。
2. release.sh そのものは本物の木で回していない（全体の試験は release gate だけで走らせる規則のため）。段の秒は `script-gate-measure.sh` の値で、release の中の値は次の release の `gate.json` で確かめる。
3. `script-bundle-tests` は `cargo-test` と同じ script 試験を 2 度回す（約 25 秒）。`cargo-test` から script 試験を外すかは決めていない。
4. `verify-release` は release の `script-bundle/` の hash だけを照合し、`schema_sha256` を release の binary と照合しない（daemon が登録のときに `load_bundle_dir` で照合する）。
5. pnpm の store が空の host（初回）での install の秒は測っていない。

## 次の gate
- 人が release を作り、`gate.json` で 4 段と `cargo-test` の秒を確かめる（親の「本番切替に要る人の判断」3.）。
- 子 task p1e-shadow-preflight は、この task と p1c-ops-observe の後に始まる。

## 提案
- `cargo-test` 段の nextest の filter から script の 7 binary を外し、`script-bundle-tests` だけで回すと約 25 秒の重複が消える。外すかは release の `gate.json` の実測を見てから決める。
