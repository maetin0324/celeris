---
title: "docs-fix: 運用文書の修正（build-tmp-hygiene への新節と重複段落の解消）"
tasks: [01M4HWZMWSTH3HYPCRRPE6PXB0]
status: done
updated: 2026-10-10
---

# docs-fix: 運用文書の修正（build-tmp-hygiene への新節と重複段落の解消）

final review（2026-10-10）の差し戻し: `docs/ops/build-tmp-hygiene.md` が更新されず、`docs/ops/local-disk-growth.md` §2 の
「repo target の自動回収は daemon の常設 tick…」の段落が 2 回続く（約10分間隔版と 600 秒間隔版）ことへの修正。

## 変更

- `docs/ops/local-disk-growth.md` §2: 重複した 2 段落を 1 段落に統合（600 秒間隔、保護理由 `running_run`・`active_descendant`・`grace`、
  cargo lock、木の最後の終端基準、log の message 名 `removing the target of a finished task tree` を残す）。
- `docs/ops/build-tmp-hygiene.md`: §2 の config 注記を新しい刈り込みに合わせた（release-build の target を sweep の roots に入れないのは
  `release.sh` が test binary の prune と `SD_RELEASE_TARGET_MAX_BYTES`（既定 64 GiB）超過時の作り直しをするため。大きさ判定は
  `btrfs filesystem du` の Exclusive + Set shared、非 btrfs は inode ごとに `st_blocks`）。
- `docs/ops/build-tmp-hygiene.md` §8（新規）: `/local` 容量の 3 経路（repo target GC・release-build の刈り込み・DB backup の保持）を短く書き、
  詳細は `docs/ops/local-disk-growth.md` §2〜§4 への相対リンクで指す。

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| §2 の段落が 1 つになった | `grep -c 'repo target の自動回収は daemon の常設 tick' docs/ops/local-disk-growth.md` | `1`、exit 0 |
| 約10分間隔の旧表現が消えた | `grep -c '約10分間隔' docs/ops/local-disk-growth.md` | `0`（grep 非該当、exit 1） |
| build-tmp-hygiene に新節とリンクがある | `grep -n 'local-disk-growth.md' docs/ops/build-tmp-hygiene.md` | §2 注記・§8 の 3 経路それぞれ §2/§3/§4 への相対リンクを確認 |
| §2 注記が新しい刈り込みに揃った | `grep -n 'SD_RELEASE_TARGET_MAX_BYTES' docs/ops/build-tmp-hygiene.md` | 新注記の行を指す |
| 文書リンク検査 | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| progress 索引 | `sh scripts/dev/progress-index.sh --check` | `progress-index --check: ok`、exit 0 |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok`、exit 0 |
| 製品コード・scripts 差分なし | `git status --porcelain -- crates/ scripts/` | 出力なし（未変更） |

`crates/` と `scripts/` は変更していない。
