---
title: K2 ops-r2 の開始前チェック修正 — WU leak（漏洩検査 checker の G6・canary なし規則）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU leak

完了日: 2026-10-31

## 内容
- commit `leak: 漏洩検査を deny list file・ops-r2 の same_uid 規則・L1 mount 非 hit に合わせる`。変更は `scripts/campaign/check-ops-r1-leak.py` と `scripts/campaign/test_check_ops_r1_leak.py`。`docs/ops/ops-r1-leak-check.md` の変更は範囲外のため戻した（leakdoc の担当）。
- deny list: `--deny-readable-file <list>`（UTF-8、1 行 1 絶対 path、空行は飛ばす、複数可）。件数に上限は無い。絶対 path でない行・NUL・UTF-8 でない file は入力の誤り。旧 `--deny-readable` は入力の誤り。結果に file の sha256 と件数を書く。probe の照合は path の索引で行い、大きな list でも 1 path ごとは定数時間。
- L1 の mount: L1 の probe は包まずに走り host の mount が見えるので、`mounts_outside_allow` を hit にしない。結果の `l1_mounts_outside_allow` に件数を残す。L1 の `ok=false` は、readable=true でも mount でも説明できないときだけ hit（ADR 2026-10-31-ops-r2-preflight-fixes D6、付記 G6）。runtime の probe の `mounts_outside_allow` と `ok=false` は従来どおり hit。
- same_uid: `--eval-root` を渡すと ops-r2 の規則（ADR D5）。canary は要らない。次の 4 条件がすべて揃うとき外す。
  1. runtime の probe の全部がその path を持ち、どれも readable=false・listable=false
  2. runtime の probe の全部が ok=true で mounts_outside_allow が空
  3. L1 の probe が渡され、その全 path が readable=false・listable=false
  4. path が `--eval-root` の下なら、L1 の probe にその path があり owned_by_run_uid が空
- 1 つでも欠ければ same_uid の limit を残し、canary が無ければ `same_uid_without_canary` の hit にする。`--eval-root` が無い入力（ops-r1）は従来の規則のまま。
- 試験を追加した（`test_check_ops_r1_leak.py`）。

## 未解決
- 手順書 `docs/ops/ops-r1-leak-check.md` は未更新（WU leakdoc）。
