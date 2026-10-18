---
title: artifact_exists がディレクトリの成果物で必ず落ちる
tasks: [01M56DYJFG15PWKAYK2PN977G3]
status: blocked
updated: 2026-10-18
---
# artifact_exists のディレクトリ対応

## 実装（attempt 1、commit 316745bc）
- ADR `agent-docs/adr/2026-10-18-artifact-exists-directory.md`: ディレクトリは通常ファイル 1 件以上（再帰、symlink 除外）で合格、reason に件数・並べた path・合成 sha256。空ディレクトリ・存在しない名前は不合格。
- 試験 `artifact_exists_directory_passes_with_files_and_fails_when_empty`（crates/task-dispatch/src/review/tests.rs）。
- worker 側の検査: test-parallel exit 0（5718 pass）、clippy exit 0、fmt exit 0。reviewer 合格。

## 配送の差し戻し（release prepare 失敗）の診断
- prepare.log: cargo-test で 28 件失敗。全件 browser の隔離 runtime 試験で、原因は
  `bwrap: Can't mount proc on /newroot/proc: Operation not permitted`（`isolated_runtime_unavailable`）。
- この変更とは無関係: main の b561567b・783ce141 の release prepare（2026-10-17 14:40・15:00）も同じ 28 件で落ちている。
  最後に通った release は 1f403f06（2026-10-17 13:58）。その後の差分は browser に触れていない。
- 原因は host 環境: host は LXC（/proc に lxcfs の overmount、`/.lxc-boot-id`）で、入れ子の userns から
  新しい proc を mount できない（kernel の mnt_already_visible が overmount で不成立）。
  手元の run でも `bwrap --ro-bind / / --proc /proc --dev /dev --unshare-pid --unshare-user true` が同じ EPERM。
  `unshare -Ur true` は成功するため test-parallel の userns preflight は true になり、
  release.sh の `CELERIS_ISOLATION_TESTS=require` で試験は skip せず落ちる。
  2026-10-17 のホスト再起動（freebind の件）と時期が一致する。

## 未解決事項
- host（LXC）の設定で入れ子の proc mount を許す（例: Proxmox の `features: nesting=1`。/dev/.lxc/proc が現れる）か、
  人が `CELERIS_ISOLATION_TESTS` を変えて release するかは人の判断。コードでは直さない（gate を弱めることになる）。

## 提案
- test-parallel.sh の preflight を `unshare -Ur` だけでなく `bwrap ... --proc /proc --unshare-pid` まで測り、
  gate の診断行に「proc を mount できない」と出すと原因がすぐ分かる。
