---
title: Launcher admission stutter 台本の signal 競合（ESRCH）修正
tasks: [01M4GCAW4MXANFEFFD72965PKN]
status: in_progress
updated: 2026-10-09
---

# Launcher admission stutter 台本の signal 競合（ESRCH）修正

## 症状

`launcher-admission-evidence-stutter.sh` の reclose 段の check が 2 回中 1 回、`success dummy command unexpectedly failed`（`EXIT[stutter-1]: 1`）で落ちた。原因は `launcher-admission-evidence.sh` の stutter ループで、試験 process group が終わる瞬間に `kill -STOP -"$pgid"` / `kill -CONT -"$pgid"` が ESRCH で失敗すると `code=1` にしていたこと。

## 修正

- signal の失敗は loop を抜けるだけにし、失敗にしない。合否は `stops>0` と `wait` の終了コードで決める。抜けた後の `kill -CONT -"$pgid" || true` は残す。
- pgid が取れる前に子が終わった場合は失敗のまま（`code=1`、`stops=0` を `STUTTER` 行に記録）。停止が一度も live な試験に届いていないので、証跡にならない。偽の失敗ではなく停止が届かなかった回として扱う。コメントに理由を書いた。
- dash で動く形（`kill -STOP -"$pgid"`、`--` は使わない）と file mode 100755 を保った。

## 試験

- `crates/task-worker/scripts/tests/launcher-admission-evidence-stutter.sh` に、`LAUNCHER_EVIDENCE_TEST_CMD='sleep 0.3'` の `--stutter 3` を 10 回繰り返す段を足した（全回 exit 0・各回 `stops>0`・末尾 `EXIT: 0`）。sleep だけで CPU を使わず、launcher・userns・実 process・外部ネットワークは使わない。

## 証拠

- `sh -n crates/task-worker/scripts/launcher-admission-evidence.sh` と `…/tests/launcher-admission-evidence-stutter.sh` — exit 0（`/bin/sh` は dash）。
- `sh crates/task-worker/scripts/tests/launcher-admission-evidence-stutter.sh` — rc=0、実時間 10.8 秒。1 回目・2 回目とも rc=0（最初の 2 回の実行で偽の失敗は出なかった）。
- `git ls-files -s` — 両台本とも `100755`。
- `sh scripts/dev/check-doc-links.sh` — exit 0。`sh scripts/dev/progress-index.sh --check` — exit 0。`git diff --check` — exit 0。

## 文書

- ADR `agent-docs/adr/2026-10-09-browser-launcher-credential-release.md` に『付記 2026-10-09: stutter 台本の signal 競合（ESRCH）』を追加。「試験 group の終了と競合した signal の失敗（ESRCH）は失敗にしない」を明記。
- `docs/ops/browser-launcher-admission-evidence-run.md` と `docs/ops/browser-launcher-credential-release.md` に同じ趣旨の 1 文を追加。

## 未解決事項

- 台本は、zombie の test leader がまだ `kill -0` で見える間は `kill -STOP` が成功して `stops` が増えうる。失敗は起きないが、停止数はその分だけ過大になりうる。今回の修正の範囲外。
- pgid を取れないうちに子が終わる回（`stops=0`）は、負荷が高いと偶に起きうる。その場合は失敗として記録されるので、人は再実行する。
- 本番 host での必須モード stutter 3 回と HEAD での `ADMISSION[real-session]` 再取得は、従来どおり運用セッションの作業。ここでは実施していない。

## 提案

- reverify 段で、統合後 HEAD の `test-parallel.sh` と `cargo clippy --workspace -- -D warnings` を流し、その結果をこの進捗に追記する。
