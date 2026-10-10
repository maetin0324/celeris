---
tasks: [01M4KKRM8B801THCFKB490QPRB]
tested_sha: f599df43f7c35a7f20c67e596031456c202ced71
status: done
updated: 2026-10-10
---

# 保存済み credential 再利用: 最終 HEAD の全体検査（final-recheck）

- 実装は変えていない。検査対象は最終 HEAD `f599df43`（実装 `aec3cf31` を含む）。
- [attempt 3](retry-3.md) の `tested_sha: 363c8a90` は古い。以後の差分（`aec3cf31`・`f599df43`）は実装と試験を含むため、この記録が最終 SHA の検査結果として正本になる。
- 以後の commit は `agent-docs/progress/` のみ。`git diff f599df43 HEAD -- . ':(exclude)agent-docs/progress/**'` が空であることを確認する。

## 検査結果（すべて `f599df43` の作業ツリー、clean 状態で実行）

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0。nextest 169 binary + doctest 10 binary。`passed: 5113, failed: 0, ignored: 14`、`nextest_exit: 0`、`doctest_exit: 0`、`userns: null`（`CELERIS_USERNS_TESTS` 未設定のため userns 試験は対象外）。stderr の `tests left 3 entries in TMPDIR` は既存の警告で、試験の失敗ではない。
- `cargo fmt --all -- --check`: exit 0。差分なし。
- `cargo clippy --workspace -- -D warnings`: exit 0。`Finished dev profile`。
- `corepack pnpm@12.6.0 -C web lint`: exit 0。`Checked 468 files`、warning 4 件（reduced-motion の `!important`、既存）。
- `corepack pnpm@12.6.0 -C web test`: exit 0。Vitest `92 files / 659 tests passed`、Node `pass 93 / fail 0`。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0。
- `sh scripts/dev/check-doc-links.sh`: exit 0。
- `sh scripts/dev/check-adr-numbers.sh`: exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: exit 0（`check-doc-layout: ok`。引数なしで起動すると usage を出すので manifest を渡した）。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## userns 環境の試験（release gate で実行）

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require` の試験は、worker・reviewer の sandbox では user namespace の unshare が許されない（Operation not permitted）ため、この run では実行していない。
- この試験は **release gate（prepare）で運用セッションが本番 host で実行する**。
- 参考: 前回 attempt 3 の記録では、同じ設定を sandbox 外で実行し `userns=true`、`5109 passed / 0 failed / 14 ignored`（`363c8a90`）。最終 SHA `f599df43` での userns 付き実行は未実施。

## 未解決事項

- userns 付きの最終 SHA 試験は release gate 待ち（上記）。ただし userns-gate（[userns-gate.md](userns-gate.md)、tested_sha `0cbfdc2d`）で、この sandbox でも隔離設定の全体試験（exit 0、failed 0、userns true）を実行済み。
- 前回 attempt 3 の記録で、隔離設定の初回実行に `real_sandbox_launcher_chrome_downloads_inline_pdf_after_login` の 1 件失敗（`Target.setDiscoverTargets: cdp_command_failed`）があった。単独再試験と全体再試験は成功しているが、最初の失敗原因は未確定。release gate で同じ試験が再び落ちるかを見る。
