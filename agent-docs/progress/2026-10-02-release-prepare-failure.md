---
title: release 準備失敗の切り分け（0d438ec1）
tasks: [01M3Y4AV7801NSXB6FD698QZHW]
status: done
updated: 2026-10-02
---
# release 準備失敗の切り分け（0d438ec1）

> 旧 `docs/PROGRESS.md` の節を ADR-0128 D6 に従い land-verify（task 01M3YBGM64RYPEY9NZANF79A0M）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

- 2026-10-02 の prepare.log（release SHA `0d438ec19d9a474c5b82507cefd0d9e63846d0d6`）を確認。`cargo-fmt-check`、`cargo-test`、`cargo-clippy`、`cargo-build`、GUI の `pnpm-install` / `pnpm-typecheck` / `pnpm-test` / `pnpm-build` / `pnpm-mobile-audit` / `pnpm-e2e-mock` と web の各 pnpm step はすべて exit 0。`source-size-report` も exit 0。
- release.log は、SHA `95ac1644` の `release.sh` が子プロセスへ lock の fd を漏らし、親スクリプト終了後も `tar` とともに lock を保持したため、新しい `release.sh` が stale lock を `.lock-release.leaked-20261002T120143Z` へ退避して新 lock を取得したことを記録している。今回の `web-bundle` tar は non-blocking step として失敗したが release 自体は ready になった。その release dir に `bin/celeris` が無く、続く verify は `missing .../0d438ec19d9a/bin/celeris` で失敗した。
- この task の変更（`CLAUDE.md`、`docs/testing.md`、stress 台本削除、`prompt.rs` とその試験）は検査規則・文書・指示の変更で、release の binary build/package 経路を変更していない。release log でも `cargo-build` は exit 0 で、欠落の前に独立した web tar failure が記録されている。よって今回の bin 欠落はこのブランチ変更と無関係な release 準備上の問題と判断し、コード修正は不要。
- 人が再実行する手順: まず `ps` で並走中の `release.sh` が無いことを確認し、その後、新しい SHA を指定して `scripts/selfdeploy/release.sh <新しい SHA>` と `scripts/selfdeploy/verify.sh <新しい SHA>` を実行する。release dir に `bin/celeris` が存在すること、および verify が `ok` になることを確認する。本番 host の `~/.local/celeris/releases`、`systemctl --user` 等は読み取り確認だけとし、この記録作成時には変更・再実行していない。
- 本記録作成時の検証: `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
