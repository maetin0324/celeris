---
title: アーキ v4 H R6 adr — ADR release-record-summary-log と R6 の進捗ファイルを書く
tasks: [01M7VJY8Q1MMAH87CSPQZWC7VY]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# adr（WorkUnit）

完了日: 2026-11-20

## 結論

- [ADR 2026-11-20 release-record-summary-log](../../adr/2026-11-20-release-record-summary-log.md) を書いた（設計 brief の 7 項目と D1〜D5）。
- task の進捗ファイル [2026-11-19-arch-v4-h-r6.md](../2026-11-19-arch-v4-h-r6.md) を `status: running` で作った。
- `crates/` と `scripts/` は変えていない。実装はしていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 根本原因の再現（ADR 1.1）: `python3 -I -c` で test-parallel.sh の regex `(\d+) ([a-z ]+?)(?:,|$)` を
  `8525 passed (3 slow), 22 skipped` に当てると `[('22', 'skipped')]`、`7 passed, 2 skipped` に当てると
  `[('7', 'passed'), ('2', 'skipped')]`。
- 既存の gate の試験は base で落ちる（ADR D1）:
  - `bash scripts/selfdeploy/tests/release_parallel_test_gate.sh` → exit 1。release.out に
    「scripts/dev/lib/check-slot.sh: No such file or directory」（cargo-test の段で gate が落ちる）。
  - 同じ試験の本文を、fixture に `check-slot.sh` を写す形に `sed` で変えて `bash -c` で走らせた（repo の file は変えていない）→ exit 1。
    「could not bundle the CoS skills (cos-skills.txt)」。`cos-skills.txt` も写すと、skill の dir（`config/skills/<name>/SKILL.md`）が無く同じ行で落ちる。
    `release_web_required.sh` は空の `cos-skills.txt` を置いてこれを避けている（92 行）。
- release dir の掃除を見る試験は無い: `grep -ln "pruning release\|prune_releases" scripts/selfdeploy/tests/*.sh` は 0 件。
- 範囲・文書の検査は下の「検査」に書く。

## 検査（提出時の HEAD で実行）

- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (468 files)`）。`--refs` も exit 0
- `git diff --name-only "$CELERIS_WU_BASE" HEAD -- crates/ scripts/` → 0 行。`sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は ADR・進捗ファイル・この file の 3 つだけ

## 未解決

- 実装は WorkUnit `counts`（D1）・`summary`（D2〜D5）、全体の検査は `close` が行う。
