---
title: K2 ops-r2 の開始前チェック修正 — WU leakdoc（漏洩検査の手順書）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU leakdoc

完了日: 2026-10-31

## 内容
`docs/ops/ops-r1-leak-check.md` を `scripts/campaign/check-ops-r1-leak.py` の規則に合わせた。前 run の patch は無く、checker を読んで直した。
- 呼び方を `--deny-readable-file <list>`（1 行 1 絶対 path）に変え、旧 `--deny-readable`（廃止、exit 2）の記述を消した。`--eval-root` を足した。
- same_uid の規則を ops-r1 と ops-r2 に分けた。ops-r2 は runtime と L1 の probe の 4 条件で外し、canary は要らない（D5）。
- L1 の `mounts_outside_allow` は hit にせず `checked.l1_mounts_outside_allow` に残す（G6）。
- 試験の起動を `sh scripts/campaign/run-ops-r1-leak-tests.sh` に直した（`python3 -I` の例を消した）。

## 検査
- `grep -n "deny-readable[^-]\|python3 -I" docs/ops/ops-r1-leak-check.md`: 該当なし
- 文書だけの変更。checker の実装は変えていない。
