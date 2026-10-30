---
title: K4 追試の証拠束の確定写し（human-ops・operator transcript・DB）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 追試の証拠束の確定写し（WU snapshot）

完了日: 2026-10-30

状態: 完了。親 K4 task の R（01M62S8XWCZDCY2ZBVQ35176MH の artifacts/ops-r1）と追試 task の P（01M6617H9V0TZGQKFA9J82141A の artifacts/ops-r1）を読むだけで、T/evidence に確定写しを作った。LLM・daemon・実試行・本番配備はしていない。repo の code は変えていない。

成果物（repo の外）: `/local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1/evidence/`（README.md に中身と再現手順）

## 結果
- human-ops: R/raw/human-ops.jsonl（54 行）を写した。旧 hash（R/raw/SHA256SUMS の行）は `87e2d61b91cd14dc463a2cb4b25c025e58ed7faa9e603d43f59e1b828ecd274e`、現 hash は `13fddd9e0cee9de97914341cb663a6426c0e744d5c0e0dd352d1f8cda61f4d3e`。現 file の先頭 26 行が旧 hash と一致したので、旧版を `human-ops.pre-append-26lines.jsonl` として残し、追記専用だったことを manifest.json の `append_only: true` に書いた。行の manifest_sha256 は 2 種（26 行と 28 行）。R/raw/SHA256SUMS は書き換えず、写しを置いた（`sha256sum -c` は human-ops.jsonl だけが FAILED になる。追記の結果で想定どおり）。
- operator transcript: P の post-b2-2026-10-30T093938Z を確定版として写した。main.jsonl は 84,997 行、最終行の時刻は 2026-10-30T09:14:19.291Z、copied_at は 09:39:48Z。session は続いていたが、新しい行は待たずに確定した。subagent は 96 件。finished=true は 29 件、finished 欄の無いものが 67 件（終了の有無は記録が無い）。B-t002 を回した subagent（description「Rerun K4 trial B」）は finished=true。
- snapshot 内の subagent の jsonl 1 本は別 session の 2026-09-30 の file を指す symlink で、snapshot の SHA256SUMS にも無いため写さず、manifest.json の `operator.excluded` に path を記録した。
- DB: R/raw/B・R/raw/B-t002 の db-copy.sqlite 本体だけを cp で写した。どちらも db-copy.sha256 と一致した。0 byte の -wal と -shm は写していない。sqlite では開いていない。
- R 全 92,197 file と P/operator 全 file の sha256 一覧を作業前後で取り、`cmp` で一致を確かめた（inputs-guard/）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 -I /local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1/evidence/check-evidence.py /local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1/evidence` → exit 0（checked files=212 errors=0）
- 負の確認: db/B に 0 byte の -wal を置くと exit 1（2 件 NG）、manifest.json から coverage を消すと NG 7 件。どちらも元に戻した
- `cmp inputs-guard/R-before.sha256 inputs-guard/R-after.sha256` → 一致。P-operator も一致
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし。docs のみ）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok

## 未解決
- -wal・-shm を R/raw に作った process は未確認（mtime は B が 09:00、B-t002 が 09:08）。
- finished 欄の無い subagent 67 件が終了していたかは未確認。

## 提案
- 写しを読むときは `file:db-copy.sqlite?immutable=1` で開く。普通に開くと -wal が作られ、SHA256SUMS の照合が落ちる。
