---
title: "sizing-adr: seed_reflink の有効化条件と targets_max_gb の既定・共有を重ねない測り方"
tasks: [01M4HWZMWSTH3HYPCRRPE6PXB0]
status: done
updated: 2026-10-10
---

# sizing-adr: seed_reflink の有効化条件と targets_max_gb の既定・共有を重ねない測り方

人のコメント（2026-10-10）(2)(3) への決定。コードは変えていない（crates/ の差分なし）。

## 変更

- `agent-docs/adr/2026-10-10-local-disk-growth-paths.md`: D5 の下に「付記 2026-10-10: scratch の上限と測り方」を追加。S1（seed_reflink は有効にできる・条件 3 つ）、S2（`targets_max_gb` 既定 160・`total_max_gb` 200 とその根拠）、S3（FIEMAP の物理 extent を pool 全体で重複除去し、statvfs used で上限を検算する。置き場所・上限・fallback）。
- `docs/ops/local-disk-growth.md`: §7（seed_reflink の有効化・起動 log での probe 確認・戻し方）、§8（targets_max_gb の既定と暫定 150 の撤去）。

## 証拠（読み取りと自分の一時 file だけ）

- `df -T` / `findmnt`: `/local` は btrfs（`compress=zstd:1`、subvol=/）。scratch・作業場所・`$TMPDIR` は同じ fs（`stat -f` id 72c118bd24a3e9a4）。
- `$TMPDIR` で 4 MiB file: `cp --reflink=always` → exit 1（`failed to clone: Operation not permitted`）。ioctl FICLONE → EPERM（worker は seccomp 下）。`cp --reflink=auto` と `copy_file_range` → exit 0、FIEMAP で全 extent が SHARED（0x2000）・物理位置が元 file と一致。`--reflink=never` は SHARED なし。試した file は消した。
- lease の読み取り: 生きている target は 4 個で、`size_bytes` の合計は 106.4 GiB（release-build 68.0）。release-build の hardlink の重複は 2.4 GiB。

## 未解決事項

- 実装は後続葉 `sizing-impl`（試験接頭辞 `scratch_shared_`）。本番での `seed_reflink = true` と暫定 150 の撤去は、配送後に人が §7・§8 の手順で行う。
- daemon の process（seccomp 外）で FICLONE が通るかは未確認。probe は `--reflink=auto` なので、結果は変わらない。

## 提案

- 測り方 S3 が入ったら、`celerisctl scratch status --json` に owner ごとの `exclusive_bytes`・`shared_bytes`・`method` と pool の重複除去後の合計を出すと、人が `du` に頼らずに済む。
